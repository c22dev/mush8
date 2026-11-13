// Mush8 - equiv.rs
// Constantin Clerc (c22dev) - 0.1
use crate::asm;
use crate::rewrite::{Kind, Output, Patch, ctx, keypage};
use anyhow::{Result, anyhow};
use std::cell::Cell;
use std::rc::Rc;
use unicorn_engine::{Arch, Arm64CpuModel, Mode, Prot, RegisterARM64 as R, Unicorn};

const BASE: u64 = 0x10_0000_0000;
const HEAP: u64 = 0x2000_0000;
const HEAP_SIZE: usize = 0x2_0000;
const STACK: u64 = 0x3000_0000;
const STACK_SIZE: usize = 0x1_0000;
const SP0: u64 = STACK + 0x8000;
const TSD: u64 = 0x4000_0000;
const CTX: u64 = 0x4000_1000;
const SLOT: u64 = 0x100;
const GATE: u64 = 0x5000_0000;
const GARBAGE: u64 = 0xBAD0_0000_0000_0018;
const TRIALS: u64 = 12;
const CNT_MUL: u64 = ((19_200_000u128 << 64) / 1_000_000_000) as u64;
const FALLBACK: u64 = 100;

fn xreg(i: u32) -> R {
    match i {
        0..=28 => unsafe { std::mem::transmute::<i32, R>(R::X0 as i32 + i as i32) },
        29 => R::X29,
        30 => R::X30,
        _ => unreachable!(),
    }
}
fn qreg(i: u32) -> R {
    unsafe { std::mem::transmute::<i32, R>(R::Q0 as i32 + i as i32) }
}

struct Rng(u64);
impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
}

#[derive(Debug, PartialEq)]
struct State {
    x: [u64; 31],
    sp: u64,
    pc: u64,
    nzcv: u64,
    q: Vec<Vec<u8>>,
    tpidr: u64,
    heap: Vec<u8>,
    stack: Vec<u8>,
}

struct Input {
    x: [u64; 31],
    nzcv: u64,
    q: Vec<[u8; 16]>,
    tpidr: u64,
    tpidrro: u64,
    heap: Vec<u8>,
}

fn make_input(trial: u64, seed: u64) -> Input {
    let mut r = Rng(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1);
    let mut x = [0u64; 31];
    for (i, v) in x.iter_mut().enumerate() {
        let ptr = HEAP + 0x1_0000 + (i as u64) * 0x100;
        *v = match trial {
            0 => ptr,
            1 => (i as u64) * 7 + 3,
            2 => HEAP + 0x1_0000,
            t if t >= FALLBACK => if i as u64 == t - FALLBACK { HEAP + 0x1_0000 } else { i as u64 & 3 },
            3 => if i % 2 == 0 { ptr } else { i as u64 },
            4 => if i % 2 == 1 { ptr } else { i as u64 },
            _ => match r.next() % 3 { 0 => ptr, 1 => r.next() & 0xFF, _ => r.next() & 0xF },
        };
    }
    let heap = (0..HEAP_SIZE).map(|_| r.next() as u8).collect();
    Input {
        x,
        nzcv: (r.next() & 0xF) << 28,
        q: (0..32).map(|_| { let mut b = [0u8; 16]; b[..8].copy_from_slice(&r.next().to_le_bytes()); b[8..].copy_from_slice(&r.next().to_le_bytes()); b }).collect(),
        tpidr: HEAP + 0x8000,
        tpidrro: HEAP + 0xC000,
        heap,
    }
}

struct Machine<'a> {
    uc: Unicorn<'a, ()>,
    ranges: Rc<Cell<[(u64, u64); 3]>>,
    patched: bool,
}

impl<'a> Machine<'a> {
    fn new(image: &[u8], patched: Option<&Output>) -> Result<Self> {
        let e = |e| anyhow!("unicorn: {e:?}");
        let mut uc = Unicorn::new(Arch::ARM64, Mode::LITTLE_ENDIAN).map_err(e)?;
        uc.ctl_set_cpu_model(Arm64CpuModel::MAX as i32).map_err(e)?;
        let size = (image.len() as u64 + 0xFFF) & !0xFFF;
        uc.mem_map(BASE, size, Prot::ALL).map_err(e)?;
        uc.mem_write(BASE, image).map_err(e)?;
        for (a, s) in [(HEAP, HEAP_SIZE), (STACK, STACK_SIZE), (TSD, 0x2000), (GATE, 0x1000)] {
            uc.mem_map(a, s as u64, Prot::ALL).map_err(e)?;
        }
        uc.mem_write(GATE, &[0xE0, 0x03, 0x10, 0xAA, 0xC0, 0x03, 0x5F, 0xD6]).map_err(e)?;
        if let Some(o) = patched {
            let kp = BASE + o.keypage;
            uc.mem_write(kp + keypage::CTX_SLOT as u64, &SLOT.to_le_bytes()).map_err(e)?;
            uc.mem_write(kp + keypage::SVC_GATE as u64, &GATE.to_le_bytes()).map_err(e)?;
            uc.mem_write(kp + keypage::CNT_MUL as u64, &CNT_MUL.to_le_bytes()).map_err(e)?;
            uc.mem_write(kp + keypage::NET as u64, &[keypage::NET_NEXTENDO]).map_err(e)?;
            uc.mem_write(TSD + SLOT, &CTX.to_le_bytes()).map_err(e)?;
        }
        uc.reg_write(R::CPACR_EL1, 3 << 20).map_err(e)?;
        let ranges = Rc::new(Cell::new([(0, 0); 3]));
        let r2 = ranges.clone();
        uc.add_code_hook(1, 0, move |uc, addr, _| {
            if !r2.get().iter().any(|&(lo, hi)| (lo..hi).contains(&addr)) {
                let _ = uc.emu_stop();
            }
        })
        .map_err(e)?;
        Ok(Self { uc, ranges, patched: patched.is_some() })
    }

    fn run(&mut self, inp: &Input, start: u64, ranges: [(u64, u64); 2]) -> Option<State> {
        let uc = &mut self.uc;
        uc.mem_write(HEAP, &inp.heap).ok()?;
        uc.mem_write(STACK, &vec![0u8; STACK_SIZE]).ok()?;
        for i in 0..31 {
            let v = if i == 18 && self.patched { GARBAGE } else { inp.x[i as usize] };
            uc.reg_write(xreg(i), v).ok()?;
        }
        for (i, q) in inp.q.iter().enumerate() {
            uc.reg_write_long(qreg(i as u32), q).ok()?;
        }
        uc.reg_write(R::SP, SP0).ok()?;
        uc.reg_write(R::NZCV, inp.nzcv).ok()?;
        if self.patched {
            uc.reg_write(R::TPIDR_EL0, GARBAGE).ok()?;
            uc.reg_write(R::TPIDRRO_EL0, TSD).ok()?;
            let mut c = vec![0u8; 0x30];
            c[ctx::X18 as usize..][..8].copy_from_slice(&inp.x[18].to_le_bytes());
            c[ctx::TPIDR as usize..][..8].copy_from_slice(&inp.tpidr.to_le_bytes());
            c[ctx::TPIDRRO as usize..][..8].copy_from_slice(&inp.tpidrro.to_le_bytes());
            uc.mem_write(CTX, &c).ok()?;
        } else {
            uc.reg_write(R::TPIDR_EL0, inp.tpidr).ok()?;
            uc.reg_write(R::TPIDRRO_EL0, inp.tpidrro).ok()?;
        }
        let abs = |(lo, hi): (u64, u64)| (BASE + lo, BASE + hi);
        self.ranges.set([abs(ranges[0]), abs(ranges[1]), (GATE, GATE + 0x1000)]);
        uc.emu_start(BASE + start, 0, 0, 4000).ok()?;

        let mut x = [0u64; 31];
        for i in 0..31 {
            x[i as usize] = uc.reg_read(xreg(i)).ok()?;
        }
        let ctx_read = |off: u32| -> Option<u64> {
            let b = uc.mem_read_as_vec(CTX + off as u64, 8).ok()?;
            Some(u64::from_le_bytes(b.try_into().ok()?))
        };
        let tpidr = if self.patched {
            x[18] = ctx_read(ctx::X18)?;
            ctx_read(ctx::TPIDR)?
        } else {
            uc.reg_read(R::TPIDR_EL0).ok()?
        };
        let sp = uc.reg_read(R::SP).ok()?;
        let mut stack = uc.mem_read_as_vec(STACK, STACK_SIZE).ok()?;
        for s in [SP0, sp] {
            if (STACK + 128..=STACK + STACK_SIZE as u64).contains(&s) {
                let o = (s - STACK) as usize;
                stack[o - 128..o].fill(0);
            }
        }
        Some(State {
            x,
            sp,
            pc: uc.reg_read(R::PC).ok()? - BASE,
            nzcv: uc.reg_read(R::NZCV).ok()? & 0xF000_0000,
            q: (0..32).map(|i| uc.reg_read_long(qreg(i)).map(|b| b.to_vec())).collect::<Result<_, _>>().ok()?,
            tpidr,
            heap: uc.mem_read_as_vec(HEAP, HEAP_SIZE).ok()?,
            stack,
        })
    }
}

pub struct Summary {
    pub tested: usize,
    pub untested: Vec<u64>,
    pub failures: Vec<String>,
}

pub fn verify(original: &[u8], out: &Output) -> Result<Summary> {
    let mut orig = Machine::new(original, None)?;
    let mut pat = Machine::new(&out.image, Some(out))?;
    let tramp = out.tramp;
    let mut s = Summary { tested: 0, untested: Vec::new(), failures: Vec::new() };

    for p in out.patches.iter().filter(|p| p.kind != Kind::Spin) {
        let words: Vec<u32> = (p.start..p.end)
            .map(|a| u32::from_le_bytes(original[a as usize..a as usize + 4].try_into().unwrap()))
            .collect();
        let cntpct_rt = words
            .iter()
            .find(|&&w| asm::is_mrs(w) && matches!(asm::sysreg(w), asm::SYSREG_CNTPCT_EL0 | asm::SYSREG_CTR_EL0))
            .map(|w| (w & 31) as usize);
        let mut ok_trials = 0;
        let mut failed = false;
        for trial in (0..TRIALS).chain(FALLBACK..FALLBACK + 31) {
            if trial >= FALLBACK && ok_trials > 0 {
                break;
            }
            let mut inp = make_input(trial, p.start ^ (trial << 40));
            if p.kind == Kind::JumpTable {
                let jt = crate::rewrite::JUMP_TABLES.iter().find(|j| j.site == p.start).unwrap();
                let k = trial as usize % jt.count;
                let e = i32::from_le_bytes(original[(jt.table as usize + 4 * k)..][..4].try_into().unwrap());
                inp.x[18] = BASE + (jt.table as i64 + e as i64) as u64;
            }
            let expect = if p.kind == Kind::Svc {
                let mut st = orig.run(&inp, p.start + 4, [(0, 0), (0, 0)]);
                if let Some(st) = st.as_mut() {
                    st.x[0] = ((words[0] >> 5) & 0xFFFF) as u64;
                }
                st
            } else {
                orig.run(&inp, p.start, [(p.start, p.end), (0, 0)])
            };
            let Some(mut expect) = expect else { continue };
            let got = pat.run(&inp, p.start, [(p.start, p.end), tramp]);
            let Some(mut got) = got else {
                s.failures.push(format!("{:?} {:#x}: rewritten code faulted (trial {trial})", p.kind, p.start));
                failed = true;
                break;
            };
            if let Some(rt) = cntpct_rt {
                expect.x[rt] = 0;
                got.x[rt] = 0;
            }
            if expect != got {
                s.failures.push(describe(p, trial, &expect, &got));
                failed = true;
                break;
            }
            ok_trials += 1;
        }
        if failed {
            continue;
        }
        if ok_trials == 0 {
            s.untested.push(p.start);
        } else {
            s.tested += 1;
        }
    }
    Ok(s)
}

fn describe(p: &Patch, trial: u64, a: &State, b: &State) -> String {
    let mut d = Vec::new();
    for i in 0..31 {
        if a.x[i] != b.x[i] {
            d.push(format!("x{i} {:#x}!={:#x}", a.x[i], b.x[i]));
        }
    }
    for (n, x, y) in [("sp", a.sp, b.sp), ("pc", a.pc, b.pc), ("nzcv", a.nzcv, b.nzcv), ("tpidr", a.tpidr, b.tpidr)] {
        if x != y {
            d.push(format!("{n} {x:#x}!={y:#x}"));
        }
    }
    if a.q != b.q {
        d.push("simd".into());
    }
    if a.heap != b.heap {
        d.push("heap".into());
    }
    if a.stack != b.stack {
        d.push("stack".into());
    }
    format!("{:?} {:#x} (trial {trial}): {}", p.kind, p.start, d.join(", "))
}
