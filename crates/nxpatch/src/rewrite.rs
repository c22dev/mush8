// Mush8 - rewrite.rs
// Constantin Clerc (c22dev) - 0.1
use crate::asm::{self, Reg, SP};
use crate::classify;
use anyhow::{Context, Result, bail, ensure};
use capstone::prelude::*;
use std::collections::{BTreeMap, BTreeSet};

pub use nxabi::{ctx, keypage};

const CTR_EL0: u32 = 0x8444_C004;

pub struct JumpTable {
    pub module_id: &'static str,
    pub site: u64,
    pub table: u64,
    pub count: usize,
}

pub const JUMP_TABLES: &[JumpTable] = &[
    JumpTable { module_id: "ba520906609c503227871a314a94fb5f6ccbe444", site: 0x759cc, table: 0x7b7578, count: 4 },
    JumpTable { module_id: "d9de423efc1512328c3c05378c7cb03173facde3", site: 0x1ae1f0, table: 0x51a2fc, count: 27 },
    JumpTable { module_id: "d9de423efc1512328c3c05378c7cb03173facde3", site: 0x1b18ec, table: 0x51afa4, count: 27 },
];

pub struct GamePatch {
    pub module_id: &'static str,
    pub site: u64,
    pub from: &'static [u32],
    pub to: &'static [u32],
}

const MK8DX_401: &str = "158c0c7399aabd7bf83795c2ef3484c700000000";
const MK8DX_401_KEYPAGE: u64 = 0x1368000;
const MK8DX_401_ASPECT_SITES: [u64; 2] = [0x3cd258, 0x3cd314];
const ASPECT_INSTRUCTIONS: [u32; 2] = [0x1e201820, 0xbd00b120]; // fdiv s0, s1, s0; str s0, [x9, #176]

pub const GAME_PATCHES: &[GamePatch] = &[
    GamePatch { module_id: MK8DX_401, site: 0x4d8a04, from: &[0x9400_0038], to: &[0x9400_074E] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8af8, from: &[0x2A01_03F3], to: &[0x2A02_03F3] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8b08, from: &[0x1200_0276], to: &[0x1200_0036] },
    GamePatch { module_id: MK8DX_401, site: 0x4d8bb4, from: &[0x1A88_0513], to: &[0xD503_201F] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4da738,
        from: &[
            0x2A1F_03E0, 0x940F_9287, 0xB400_0640, 0x940F_8E5E, 0xB400_0600, 0xF940_AC08, 0xB400_05C8, 0xB941_150A, 0x528C_1009,
            0x0A2A_012A, 0x3500_054A, 0x7940_1108, 0x0A09_0108, 0x3400_04E8, 0x3200_07E8, 0x9100_63E0, 0xB900_1BE8, 0x940F_924D,
            0x9000_7108, 0xF947_3908, 0x08DF_FD08, 0xAA00_03F4, 0x3700_01C8, 0x9000_7100, 0xF947_3800, 0x9419_E0C9, 0x3400_0140,
            0x9000_7108, 0xF947_3D08, 0x9000_7109, 0xF947_4129, 0x9100_4108, 0xF900_0128, 0x9000_7100, 0xF947_3800, 0x9419_E0C3,
            0xF940_0288, 0x9000_7101, 0xF940_0108, 0xF947_4021, 0xAA14_03E0, 0xD63F_0100, 0x7200_001F, 0xAA13_03E0, 0x9A9F_1288,
            0xF940_1908, 0x5283_F329, 0x2A1F_03E2, 0x3869_6908, 0x7100_011F, 0x1A9F_17E1, 0x97FF_F8B8,
        ],
        to: &[
            0x1400_0034, 0xA95E_2408, 0xF940_2108, 0xF940_0D08, 0xF940_2129, 0xF940_0D29, 0xA95D_2C0A, 0xF940_214A, 0xF940_0D4A,
            0xF940_216B, 0xF940_0D6B, 0x3941_610C, 0x121F_198C, 0x3901_610C, 0xBD40_3541, 0xBD40_3562, 0x1E21_3841, 0xBD40_3500,
            0xD000_746E, 0x3940_61CE, 0x3400_016E, 0x1E2C_1002, 0x1E22_0822, 0x1E22_3800, 0xF940_190D, 0xF900_192D, 0x1E21_2802,
            0xBD00_3522, 0x3941_612C, 0x321C_018C, 0x1400_0003, 0x3941_612C, 0x121F_198C, 0x3901_612C, 0xBD00_3560, 0x1E21_3802,
            0xBD00_3542, 0x3941_616C, 0x321C_018C, 0x3901_616C, 0x3941_614C, 0x321C_018C, 0x3901_614C, 0x5280_0021, 0x5281_07A2,
            0x7100_0DDF, 0x5281_078D, 0x1A82_01A2, 0x7100_09DF, 0x5281_072D, 0x1A82_01A2, 0x17FF_F8B8,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x3fc4a8,
        from: &[
            0x121B_0048, 0x121A_0049, 0x7100_0C3F, 0x5400_0188, 0x5280_014A, 0x1A81_054B, 0x7100_013F, 0x1A8B_0029, 0x5100_042B,
            0x7100_003F, 0x1A8B_014A, 0x7100_011F, 0x1A8A_0121, 0x2A01_03E0, 0xD65F_03C0, 0x3400_0088, 0x3200_07E1, 0x2A01_03E0,
            0xD65F_03C0, 0x3500_0109, 0x3738_0142, 0x3640_0082, 0x7100_283F, 0x321E_03E8, 0x1A81_0501, 0x2A01_03E0, 0xD65F_03C0,
            0x2A1F_03E1, 0x2A01_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x5280_0149, 0x1A88_0121, 0x2A01_03E0, 0xD65F_03C0,
        ],
        to: &[
            0x9000_7B6B, 0x3940_616B, 0x7100_017F, 0x5280_006C, 0x1A9F_158B, 0x5280_014A, 0x7100_0C3F, 0x5400_0168, 0x3728_00A2,
            0x3630_0222, 0x531F_7820, 0x6B0B_003F, 0x1400_0004, 0x5100_0C20, 0x1301_7C00, 0x7100_003F, 0x1A80_0540, 0xD65F_03C0,
            0x3728_0142, 0x3730_0162, 0x3738_0182, 0x3640_00A2, 0x7100_283F, 0x5280_0088, 0x1A81_0500, 0xD65F_03C0, 0x2A01_03E0,
            0xD65F_03C0, 0x2A0B_03E0, 0xD65F_03C0, 0x2A1F_03E0, 0xD65F_03C0, 0x5100_0428, 0x7100_103F, 0x1A88_0140, 0xD65F_03C0,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x438f24,
        from: &[
            0xF941_F6A8, 0x5280_02E9, 0xB900_4909, 0xF941_FAA8, 0x321D_07E9, 0xB900_4909, 0xF941_FEA8, 0x5280_0329, 0xB900_4909,
            0xB000_7609, 0xF947_1529, 0x9100_4137, 0xF942_02A9, 0x5280_0348, 0xB900_4928,
        ],
        to: &[
            0xB000_7609, 0xF947_1529, 0x9100_4137, 0x5280_0069, 0xB904_0AA9, 0xB000_5588, 0x910F_2908, 0xA900_A3F7, 0xAA13_03E0,
            0x9100_23E1, 0x9411_D267, 0x52A8_64C8, 0xB900_3008, 0x52A8_8BC8, 0xB900_5008,
        ],
    },
    GamePatch { module_id: MK8DX_401, site: 0x439600, from: &[0xF000_5568, 0x9135_8508], to: &[0xB000_38C8, 0x9129_D108] },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a3214,
        from: &[
            0x5280_0D68, 0x5281_1B40, 0xB900_4A88, 0x9410_2A5A, 0xAA00_03E1, 0xAA16_03E0, 0x9410_2A69, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xAA14_03F7, 0x1400_000F, 0xAA0D_03E9, 0xEB0C_015F, 0x5400_0080, 0xF840_8549, 0xF940_2129, 0xF940_0D29, 0xB940_352B,
            0xB900_350B, 0x3941_610B, 0x321C_016B, 0x3901_610B, 0xAA09_03E8, 0x54FF_FE81, 0x1400_0032,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a32d8,
        from: &[
            0x5280_0D88, 0x5281_1C80, 0xB900_4A88, 0x9410_2A29, 0xAA00_03E1, 0xAA16_03E0, 0x9410_2A38, 0xB988_5A68, 0xB948_5E69,
            0x6B09_011F, 0x5400_00CA, 0xF944_3269, 0xF828_7934, 0xB948_5A68, 0x1100_0508, 0xB908_5A68,
        ],
        to: &[
            0xF940_2288, 0xF940_0D08, 0x3941_6109, 0x121F_1929, 0x3901_6109, 0xF940_22ED, 0xF940_0DAD, 0x3941_61A9, 0x121F_1929,
            0x3901_61A9, 0xF944_326A, 0x9100_614C, 0xF840_8548, 0xF940_2108, 0xF940_0D08, 0x1400_058B,
        ],
    },
    GamePatch {
        module_id: MK8DX_401,
        site: 0x4a493c,
        from: &[0xB948_5A68, 0x7100_111F, 0x5400_0083, 0xF944_3268, 0xF940_0D00, 0x1400_0002, 0xAA1F_03E0, 0x2A15_03E1],
        to: &[0x1400_0013, 0xB940_8A89, 0x1200_7929, 0xB900_8A89, 0xB940_8AE9, 0x1200_7929, 0xB900_8AE9, 0x17FF_FA31],
    },
    GamePatch { module_id: MK8DX_401, site: 0xb52a74, from: &[0xD503_201F, 0xD503_201F], to: &[0x305F_6E4F, 0x0000_0032] },
    GamePatch { module_id: MK8DX_401, site: 0x546a80, from: &[0x3400_23C8], to: &[0x1400_011E] },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    X18,
    SysReg,
    Svc,
    Exclusive,
    JumpTable,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub kind: Kind,
    pub start: u64,
    pub end: u64,
    pub tramp: u64,
    pub tramp_len: u64,
}

pub struct Output {
    pub image: Vec<u8>,
    pub text_size: u64,
    pub ro: (u64, u64),
    pub data: (u64, u64),
    pub bss: (u64, u64),
    pub keypage: u64,
    pub tramp: (u64, u64),
    pub patches: Vec<Patch>,
}

fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}

fn field_regs(w: u32) -> u32 {
    [0, 5, 10, 16].iter().fold(0, |m, &s| m | 1 << ((w >> s) & 31))
}

fn replace_reg_tokens(text: &str, from: u32, to: u32) -> String {
    let mut out = String::new();
    let mut tok = String::new();
    let flush = |tok: &mut String, out: &mut String| {
        for p in ["x", "w"] {
            if *tok == format!("{p}{from}") {
                *tok = format!("{p}{to}");
            }
        }
        out.push_str(tok);
        tok.clear();
    };
    for c in text.chars() {
        if c.is_ascii_alphanumeric() {
            tok.push(c);
        } else {
            flush(&mut tok, &mut out);
            out.push(c);
        }
    }
    flush(&mut tok, &mut out);
    out
}

struct Ctx<'a> {
    cs: Capstone,
    module_id: String,
    words: &'a [u32],
    image: &'a [u8],
}

impl Ctx<'_> {
    fn text(&self, w: u32, pc: u64) -> Result<String> {
        let b = w.to_le_bytes();
        let insns = self.cs.disasm_count(&b, pc, 1)?;
        let i = insns.iter().next().with_context(|| format!("undecodable word {w:#010x} at {pc:#x}"))?;
        Ok(format!("{} {}", i.mnemonic().unwrap_or(""), i.op_str().unwrap_or("")))
    }

    fn regs(&self, w: u32, pc: u64) -> Result<u32> {
        let b = w.to_le_bytes();
        let insns = self.cs.disasm_count(&b, pc, 1)?;
        let mut m = field_regs(w);
        if let Some(i) = insns.iter().next() {
            let d = self.cs.insn_detail(i)?;
            for r in d.regs_read().iter().chain(d.regs_write()) {
                if let Some(g) = self.cs.reg_name(*r).as_deref().and_then(classify::gpr_index) {
                    m |= 1 << g;
                }
            }
        }
        Ok(m)
    }

    fn subst18(&self, w: u32, pc: u64, s: Reg) -> Result<u32> {
        let want = replace_reg_tokens(&self.text(w, pc)?, 18, s);
        let fields: Vec<u32> = [0, 5, 10, 16].into_iter().filter(|&f| (w >> f) & 31 == 18).collect();
        for mask in 1..(1u32 << fields.len()) {
            let mut n = w;
            for (i, f) in fields.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    n = (n & !(31 << f)) | (s << f);
                }
            }
            if self.text(n, pc).ok().as_deref() == Some(want.as_str()) {
                return Ok(n);
            }
        }
        bail!("cannot rename x18 in `{}` at {pc:#x}", self.text(w, pc)?)
    }

    fn writes_sp(&self, w: u32, pc: u64) -> Result<bool> {
        let t = self.text(w, pc)?;
        let has_sp = t.split(|c: char| !c.is_ascii_alphanumeric()).any(|x| x == "sp" || x == "wsp");
        let first_is_sp = t.split_whitespace().nth(1).is_some_and(|o| o.starts_with("sp,") || o.starts_with("wsp,"));
        Ok(has_sp && (t.contains('!') || t.contains("],") || first_is_sp))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Need {
    Plain,
    X18,
    SysReg,
}

struct Emitter {
    base: u64,
    words: Vec<u32>,
    fixups: Vec<(usize, usize)>,
    labels: Vec<Option<usize>>,
}

impl Emitter {
    fn pc(&self) -> u64 {
        self.base + self.words.len() as u64 * 4
    }
    fn emit(&mut self, w: u32) {
        self.words.push(w);
    }
    fn label(&mut self) -> usize {
        self.labels.push(None);
        self.labels.len() - 1
    }
    fn bind(&mut self, l: usize) {
        self.labels[l] = Some(self.words.len());
    }
    fn cond_to(&mut self, w: u32, l: usize) {
        self.fixups.push((self.words.len(), l));
        self.words.push(w);
    }
    fn resolve(&mut self) -> Result<()> {
        for &(at, l) in &self.fixups {
            let to = self.labels[l].context("unbound label")?;
            let from = self.base + at as u64 * 4;
            self.words[at] = asm::retarget(self.words[at], from, self.base + to as u64 * 4)?;
        }
        self.fixups.clear();
        Ok(())
    }
}

struct Scratch {
    s: Reg,
    t: Reg,
    u: Option<Reg>,
    ctx_spill: bool,
}

const SCRATCH_ORDER: [Reg; 28] = [9, 10, 11, 12, 13, 14, 15, 16, 17, 0, 1, 2, 3, 4, 5, 6, 7, 8, 19, 20, 21, 22, 23, 24, 25, 26, 27, 28];

impl Scratch {
    fn prologue(&self, e: &mut Emitter, keypage: u64) -> Result<()> {
        let (s, t) = (self.s, self.t);
        e.emit(asm::stp(s, t, SP, -16));
        if let Some(u) = self.u {
            e.emit(asm::stur(u, SP, -24));
        }
        e.emit(asm::mrs_tpidrro(s));
        let pc = e.pc();
        e.emit(asm::adrp(t, pc, keypage)?);
        e.emit(asm::ldr(t, t, keypage::CTX_SLOT));
        e.emit(asm::ldr_reg(t, s, t));
        if self.ctx_spill {
            e.emit(asm::ldur(s, SP, -16));
            e.emit(asm::str(s, t, ctx::SPILL0));
            e.emit(asm::ldur(s, SP, -8));
            e.emit(asm::str(s, t, ctx::SPILL1));
            if self.u.is_some() {
                e.emit(asm::ldur(s, SP, -24));
                e.emit(asm::str(s, t, ctx::SPILL2));
            }
        }
        e.emit(asm::ldr(s, t, ctx::X18));
        Ok(())
    }

    fn epilogue(&self, e: &mut Emitter, store: bool, dest: u64) -> Result<()> {
        let (s, t) = (self.s, self.t);
        if store {
            e.emit(asm::str(s, t, ctx::X18));
        }
        if self.ctx_spill {
            if let Some(u) = self.u {
                e.emit(asm::ldr(u, t, ctx::SPILL2));
            }
            e.emit(asm::ldr(s, t, ctx::SPILL0));
            e.emit(asm::ldr(t, t, ctx::SPILL1));
        } else {
            e.emit(asm::ldp(s, t, SP, -16));
            if let Some(u) = self.u {
                e.emit(asm::ldur(u, SP, -24));
            }
        }
        let pc = e.pc();
        e.emit(asm::b(pc, dest)?);
        Ok(())
    }
}

pub fn rewrite(nso: &nxfs::nso::Nso) -> Result<Output> {
    let text = nso.text_bytes();
    ensure!(nso.text.mem_offset == 0, "text must start at 0");
    let mut words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let module_id = hex::encode(&nso.module_id[..20]);
    for p in GAME_PATCHES.iter().filter(|p| p.module_id == module_id) {
        let w = &mut words[p.site as usize / 4..][..p.from.len()];
        ensure!(w == p.from, "game patch at {:#x}: code differs from the expected original", p.site);
        w.copy_from_slice(p.to);
    }
    let cx = Ctx { cs: classify::capstone()?, module_id, words: &words, image: &nso.image };

    let mod0 = u32::from_le_bytes(nso.image[4..8].try_into()?) as usize;
    let is_header = |i: usize| i < 2 || (mod0 / 4..mod0 / 4 + 7).contains(&i);
    let mut need = vec![Need::Plain; words.len()];
    for s in classify::find_sites(text)? {
        if !is_header(s.offset / 4) {
            need[s.offset / 4] = Need::X18;
        }
    }
    let mut svcs = Vec::new();
    for (i, &w) in words.iter().enumerate() {
        if is_header(i) {
            continue;
        }
        if asm::is_svc(w) {
            svcs.push(i);
        } else if asm::is_mrs(w) || asm::is_msr_reg(w) {
            match asm::sysreg(w) {
                asm::SYSREG_TPIDR_EL0 => need[i] = Need::SysReg,
                asm::SYSREG_TPIDRRO_EL0 | asm::SYSREG_CNTPCT_EL0 | asm::SYSREG_CTR_EL0 if asm::is_mrs(w) => need[i] = Need::SysReg,
                0x5807 | 0x5A10 | 0x5A20 | 0x5A21 => {}
                r => bail!("unhandled system register {r:#x} at {:#x}", i * 4),
            }
        }
    }

    let mut regions: Vec<(usize, usize, Kind)> = Vec::new();
    for i in 0..words.len() {
        if !asm::is_load_exclusive(words[i]) {
            continue;
        }
        let end = (i + 1..(i + 16).min(words.len())).find(|&j| asm::is_store_exclusive(words[j]));
        if let Some(j) = end {
            if (i..=j).any(|k| need[k] != Need::Plain) {
                regions.push((i, j + 1, Kind::Exclusive));
            }
        }
    }
    let window_starts: BTreeSet<usize> = regions.iter().map(|r| r.0).collect();
    for i in 0..words.len() {
        if need[i] != Need::Plain && !window_starts.contains(&i) {
            let kind = if need[i] == Need::SysReg { Kind::SysReg } else { Kind::X18 };
            regions.push((i, i + 1, kind));
        }
    }
    regions.sort();

    let bss_start = (nso.data.mem_offset + nso.data.size) as u64;
    let bss_end = bss_start + nso.bss_size as u64;
    let keypage = align_up(bss_end, keypage::SIZE);
    ensure!(cx.module_id != MK8DX_401 || keypage == MK8DX_401_KEYPAGE, "game patches expect the keypage at {MK8DX_401_KEYPAGE:#x}");
    let tramp_base = keypage + keypage::SIZE;
    let mut e = Emitter { base: tramp_base, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
    let mut patched = words.clone();
    let mut patches = Vec::new();

    for &(start, end, kind) in &regions {
        let tramp = e.pc();
        let kind = emit_region(&cx, &mut e, &need, start, end, kind, keypage)
            .with_context(|| format!("region at {:#x}", start * 4))?;
        e.resolve()?;
        patched[start] = asm::b(start as u64 * 4, tramp)?;
        patches.push(Patch { kind, start: start as u64 * 4, end: end as u64 * 4, tramp, tramp_len: e.pc() - tramp });
    }
    for &i in &svcs {
        let tramp = e.pc();
        let n = (words[i] >> 5) & 0xFFFF;
        e.emit(asm::stp_pre(16, 30, SP, -16));
        e.emit(asm::movz(16, n, 0));
        let pc = e.pc();
        e.emit(asm::adrp(30, pc, keypage)?);
        e.emit(asm::ldr(30, 30, keypage::SVC_GATE));
        e.emit(asm::blr(30));
        e.emit(asm::ldp_post(16, 30, SP, 16));
        let pc = e.pc();
        e.emit(asm::b(pc, (i as u64 + 1) * 4)?);
        patched[i] = asm::b(i as u64 * 4, tramp)?;
        patches.push(Patch { kind: Kind::Svc, start: i as u64 * 4, end: i as u64 * 4 + 4, tramp, tramp_len: e.pc() - tramp });
    }

    if cx.module_id == MK8DX_401 {
        for site in MK8DX_401_ASPECT_SITES {
            let i = site as usize / 4;
            ensure!(words.get(i..i + ASPECT_INSTRUCTIONS.len()) == Some(ASPECT_INSTRUCTIONS.as_slice()), "ultrawide projection code differs at {site:#x}");
            ensure!(patched[i] == words[i], "ultrawide overlaps another rewrite at {site:#x}");
            patched[i] = asm::b(site, e.pc())?;
            emit_aspect_scale(&mut e, keypage, site + 4)?;
        }
    }

    let tramp_end = e.pc();
    let mut image = vec![0u8; tramp_end as usize];
    image[..nso.image.len()].copy_from_slice(&nso.image);
    for (i, w) in patched.iter().enumerate() {
        image[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
    }
    for (i, w) in e.words.iter().enumerate() {
        let o = tramp_base as usize + i * 4;
        image[o..o + 4].copy_from_slice(&w.to_le_bytes());
    }
    Ok(Output {
        image,
        text_size: nso.text.size as u64,
        ro: (nso.ro.mem_offset as u64, nso.ro.size as u64),
        data: (nso.data.mem_offset as u64, nso.data.size as u64),
        bss: (bss_start, nso.bss_size as u64),
        keypage,
        tramp: (tramp_base, tramp_end),
        patches,
    })
}

fn emit_aspect_scale(e: &mut Emitter, keypage: u64, dest: u64) -> Result<()> {
    e.emit(ASPECT_INSTRUCTIONS[0]);
    e.emit(asm::stp_pre(16, 17, SP, -32));
    e.emit(0x3d8007e2); // str q2, [sp, #16]
    e.emit(asm::adrp(16, e.pc(), keypage)?);
    e.emit(0xbd400202 | ((keypage::ASPECT_SCALE / 4) << 10)); // ldr s2, [x16, #ASPECT_SCALE]
    e.emit(0x1e260051); // fmov w17, s2
    e.emit(0x531f7a31); // lsl w17, w17, #1 (both signed zeros become zero)
    e.emit(0x34000051); // cbz w17, +8 (zero scale leaves the original aspect unchanged)
    e.emit(0x1e220800); // fmul s0, s0, s2
    e.emit(0x3dc007e2); // ldr q2, [sp, #16]
    e.emit(asm::ldp_post(16, 17, SP, 32));
    e.emit(asm::b(e.pc(), dest)?);
    Ok(())
}

#[test]
fn aspect_trampoline_preserves_registers() {
    use unicorn_engine::{Arch, Mode, Prot, RegisterARM64 as R, Unicorn};
    for factor in [0.0f32, -0.0, 1.0, 1.3, 2.0] {
        let mut e = Emitter { base: 0x1000, words: Vec::new(), fixups: Vec::new(), labels: Vec::new() };
        emit_aspect_scale(&mut e, 0x4000, 0x2000).unwrap();
        let code: Vec<u8> = e.words.iter().flat_map(|w| w.to_le_bytes()).collect();
        let mut uc = Unicorn::new(Arch::ARM64, Mode::LITTLE_ENDIAN).unwrap();
        uc.mem_map(0x1000, 0x8000, Prot::ALL).unwrap();
        uc.mem_write(0x1000, &code).unwrap();
        uc.mem_write(0x4000 + keypage::ASPECT_SCALE as u64, &factor.to_le_bytes()).unwrap();
        uc.reg_write(R::CPACR_EL1, 3 << 20).unwrap();
        uc.reg_write(R::SP, 0x8000).unwrap();
        uc.reg_write(R::X16, 0x12345678).unwrap();
        uc.reg_write(R::X17, 0xabcdef12).unwrap();
        uc.reg_write(R::NZCV, 0xa0000000).unwrap();
        uc.reg_write(R::S0, 9.0f32.to_bits() as u64).unwrap();
        uc.reg_write(R::S1, 16.0f32.to_bits() as u64).unwrap();
        uc.reg_write_long(R::Q2, &[0x5a; 16]).unwrap();
        uc.emu_start(0x1000, 0x2000, 0, 20).unwrap();
        let result = f32::from_bits(uc.reg_read(R::S0).unwrap() as u32);
        let expected_factor = if factor == 0.0 { 1.0 } else { factor };
        assert_eq!(result, (16.0f32 / 9.0) * expected_factor);
        assert_eq!(uc.reg_read(R::S1).unwrap(), 16.0f32.to_bits() as u64);
        assert_eq!(&*uc.reg_read_long(R::Q2).unwrap(), &[0x5a; 16]);
        assert_eq!(uc.reg_read(R::X16).unwrap(), 0x12345678);
        assert_eq!(uc.reg_read(R::X17).unwrap(), 0xabcdef12);
        assert_eq!(uc.reg_read(R::SP).unwrap(), 0x8000);
        assert_eq!(uc.reg_read(R::NZCV).unwrap(), 0xa0000000);
    }
}

fn emit_region(cx: &Ctx, e: &mut Emitter, need: &[Need], start: usize, end: usize, kind: Kind, keypage: u64) -> Result<Kind> {
    let words = cx.words;
    let pc_of = |i: usize| i as u64 * 4;

    let mut avoid = (1u32 << 18) | (1 << 29) | (1 << 30) | (1 << 31);
    let mut ctx_spill = false;
    let mut needs_u = false;
    for i in start..end {
        avoid |= cx.regs(words[i], pc_of(i))?;
        ctx_spill |= cx.writes_sp(words[i], pc_of(i))?;
        needs_u |= need[i] == Need::SysReg && asm::sysreg(words[i]) == asm::SYSREG_CNTPCT_EL0;
        ensure!(i + 1 == end || !asm::is_uncond_branch(words[i]), "branch inside a multi-instruction region");
    }
    let mut free = SCRATCH_ORDER.iter().copied().filter(|r| avoid & (1 << r) == 0);
    let sc = Scratch {
        s: free.next().context("no scratch register")?,
        t: free.next().context("no scratch register")?,
        u: if needs_u { Some(free.next().context("no scratch register")?) } else { None },
        ctx_spill,
    };

    if end - start == 1 && words[start] == 0xD61F_0240 {
        let jt = JUMP_TABLES
            .iter()
            .find(|j| j.module_id == cx.module_id && j.site == pc_of(start))
            .with_context(|| format!("br x18 at {:#x} is not a known jump table", pc_of(start)))?;
        return emit_jump_table(cx, e, &sc, jt, keypage).map(|_| Kind::JumpTable);
    }

    sc.prologue(e, keypage)?;
    let labels: Vec<usize> = (start..end).map(|_| e.label()).collect();
    let mut exits: BTreeMap<u64, usize> = BTreeMap::new();
    for i in start..end {
        e.bind(labels[i - start]);
        let (w, pc) = (words[i], pc_of(i));
        let d18 = |r: u32| if r == 18 { sc.s } else { r };
        match need[i] {
            Need::SysReg => {
                let rt = d18(w & 31);
                match asm::sysreg(w) {
                    asm::SYSREG_TPIDR_EL0 if asm::is_mrs(w) => e.emit(asm::ldr(rt, sc.t, ctx::TPIDR)),
                    asm::SYSREG_TPIDR_EL0 => e.emit(asm::str(rt, sc.t, ctx::TPIDR)),
                    asm::SYSREG_TPIDRRO_EL0 => e.emit(asm::ldr(rt, sc.t, ctx::TPIDRRO)),
                    asm::SYSREG_CTR_EL0 => {
                        e.emit(asm::movz(rt, CTR_EL0 & 0xFFFF, 0));
                        e.emit(asm::movk(rt, CTR_EL0 >> 16, 16));
                    }
                    asm::SYSREG_CNTPCT_EL0 => {
                        let u = sc.u.unwrap();
                        e.emit(asm::mrs_cntvct(rt));
                        let here = e.pc();
                        e.emit(asm::adrp(u, here, keypage)?);
                        e.emit(asm::ldr(u, u, keypage::CNT_MUL));
                        e.emit(asm::umulh(rt, rt, u));
                    }
                    r => bail!("sysreg {r:#x}"),
                }
            }
            n => {
                let renamed = if n == Need::X18 && !asm::is_adr(w) && !asm::is_adrp(w) && !asm::is_ldr_literal(w) {
                    cx.subst18(w, pc, sc.s)?
                } else {
                    w
                };
                if asm::is_adrp(w) || asm::is_adr(w) {
                    let (rd, target) = (d18(w & 31), asm::adr_target(w, pc));
                    let here = e.pc();
                    e.emit(asm::adrp(rd, here, target)?);
                    if target & 0xFFF != 0 {
                        e.emit(asm::add_imm(rd, rd, (target & 0xFFF) as u32));
                    }
                } else if asm::is_ldr_literal(w) {
                    let rt = w & 31;
                    ensure!((w >> 26) & 1 == 0, "SIMD literal load in region");
                    let rd = d18(rt);
                    let target = asm::ldr_literal_target(w, pc);
                    let here = e.pc();
                    e.emit(asm::adrp(rd, here, target)?);
                    e.emit(asm::add_imm(rd, rd, (target & 0xFFF) as u32));
                    e.emit(asm::ldr_literal_as_base((w & !31) | rd, rd).context("prfm literal")?);
                } else if asm::is_cond_branch(w) {
                    let target = asm::cond_target(w, pc);
                    let ti = target / 4;
                    let l = if (start as u64..end as u64).contains(&ti) {
                        labels[ti as usize - start]
                    } else {
                        *exits.entry(target).or_insert_with(|| e.label())
                    };
                    e.cond_to(renamed, l);
                } else {
                    ensure!(!asm::is_uncond_branch(w), "unsupported branch `{}` at {pc:#x}", cx.text(w, pc)?);
                    e.emit(renamed);
                }
            }
        }
    }
    sc.epilogue(e, true, pc_of(end))?;
    for (target, l) in exits {
        e.bind(l);
        sc.epilogue(e, true, target)?;
    }
    Ok(kind)
}

fn emit_jump_table(cx: &Ctx, e: &mut Emitter, sc: &Scratch, jt: &JumpTable, keypage: u64) -> Result<()> {
    ensure!(!sc.ctx_spill && sc.u.is_none());
    let read_i32 = |addr: u64| -> Result<i32> {
        let b = cx.image.get(addr as usize..addr as usize + 4).context("jump table outside image")?;
        Ok(i32::from_le_bytes(b.try_into()?))
    };
    let mut targets = BTreeSet::new();
    for k in 0..jt.count {
        let t = (jt.table as i64 + read_i32(jt.table + 4 * k as u64)? as i64) as u64;
        ensure!((t as usize) < cx.words.len() * 4, "jump table target {t:#x} outside text");
        targets.insert(t);
    }
    let (s, t) = (sc.s, sc.t);
    sc.prologue(e, keypage)?;
    e.emit(asm::str(s, t, ctx::X18));
    let mut pads = Vec::new();
    for &target in &targets {
        let here = e.pc();
        e.emit(asm::adrp(t, here, target)?);
        e.emit(asm::add_imm(t, t, (target & 0xFFF) as u32));
        e.emit(asm::sub_reg(t, s, t));
        let l = e.label();
        e.cond_to(0xB400_0000 | t, l);
        pads.push((l, target));
    }
    e.emit(asm::brk(0x18));
    for (l, target) in pads {
        e.bind(l);
        sc.epilogue(e, false, target)?;
    }
    Ok(())
}
