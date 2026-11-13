// Mush8 - plugins.rs
// Skyline-style hook API for extra code modules (subsdk1 to subsdk9 in a mod's exefs)
use crate::kernel;
use crate::mem::{Block, PAGE, perm, state};
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Mutex;

const POOL_SIZE: u64 = 0x10_0000;

struct Pool {
    next: u64,
    end: u64,
}

static POOL: Mutex<Pool> = Mutex::new(Pool { next: 0, end: 0 });
static MAIN: Mutex<[u64; 4]> = Mutex::new([0; 4]);

pub fn is_api(name: &str) -> bool {
    table().contains_key(name)
}

pub fn table() -> HashMap<&'static str, u64> {
    HashMap::from([
        ("A64HookFunction", hook_function as *const () as usize as u64),
        ("A64InlineHook", inline_hook as *const () as usize as u64),
        ("sky_memcpy", sky_memcpy as *const () as usize as u64),
        ("_ZN7skyline5utils16getRegionAddressENS0_6regionE", region_address as *const () as usize as u64),
    ])
}

// Reserves the trampoline pool right after the modules, so that a b instruction reaches it from the game code
pub fn init(at: u64, main: &crate::loader::Module) -> Result<()> {
    kernel::aspace().set(at, POOL_SIZE, Block { state: state::CODE, perm: perm::RX, attr: 0 })?;
    *POOL.lock().unwrap() = Pool { next: at, end: at + POOL_SIZE };
    *MAIN.lock().unwrap() = [main.base, main.base + main.ro.0, main.base + main.data.0, main.base + main.bss.0];
    Ok(())
}

// macOS does not allow a page that is writable and executable, so the page loses execute while it is written
fn write_code(addr: u64, bytes: &[u8]) {
    let a = kernel::aspace();
    let (lo, hi) = (addr & !(PAGE - 1), (addr + bytes.len() as u64).next_multiple_of(PAGE));
    let mut page = lo;
    let mut saved = Vec::new();
    while page < hi {
        let (start, size, cur) = a.map.lock().unwrap().query(page);
        let end = (start + size).min(hi);
        saved.push((page, end - page, cur));
        a.set(page, end - page, Block { perm: perm::RW, ..cur }).expect("make code writable");
        page = end;
    }
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), addr as *mut u8, bytes.len()) };
    for (p, len, b) in saved {
        a.set(p, len, b).expect("restore code permissions");
    }
    crate::loader::icache_flush(addr, bytes.len() as u64);
}

fn alloc(words: &[u32]) -> u64 {
    let mut pool = POOL.lock().unwrap();
    let at = pool.next;
    assert!(at + 4 * words.len() as u64 <= pool.end, "plugin trampoline pool is full");
    pool.next += 4 * words.len() as u64;
    drop(pool);
    write_code(at, &words.iter().flat_map(|w| w.to_le_bytes()).collect::<Vec<_>>());
    at
}

fn sext(v: u32, bits: u32) -> i64 {
    ((v as i64) << (64 - bits)) >> (64 - bits)
}

fn b(from: u64, to: u64) -> u32 {
    let d = (to as i64 - from as i64) / 4;
    assert!((-(1 << 25)..1 << 25).contains(&d), "branch out of range {from:#x} -> {to:#x}");
    0x1400_0000 | (d as u32 & 0x03FF_FFFF)
}

fn literal(words: &mut Vec<u32>, v: u64) {
    words.push(v as u32);
    words.push((v >> 32) as u32);
}

// Copies the instruction w, which ran at src, so that it runs the same way at dst
fn relocate(w: u32, src: u64, dst: u64) -> Vec<u32> {
    let mut out = Vec::new();
    if w & 0x7C00_0000 == 0x1400_0000 {
        let target = (src as i64 + sext(w & 0x03FF_FFFF, 26) * 4) as u64;
        out.push((w & 0x8000_0000) | b(dst, target));
    } else if w & 0xFF00_0010 == 0x5400_0000 || w & 0x7E00_0000 == 0x3400_0000 || w & 0x7E00_0000 == 0x3600_0000 {
        let tbz = w & 0x7E00_0000 == 0x3600_0000;
        let target = if tbz { src as i64 + sext((w >> 5) & 0x3FFF, 14) * 4 } else { src as i64 + sext((w >> 5) & 0x7FFFF, 19) * 4 } as u64;
        // the condition jumps 2 instructions ahead, to an unconditional branch to the target
        out.push(if tbz { (w & 0xFFF8_001F) | (2 << 5) } else { (w & 0xFF00_001F) | (2 << 5) });
        out.push(b(dst + 4, dst + 12));
        out.push(b(dst + 8, target));
    } else if w & 0x1F00_0000 == 0x1000_0000 {
        let imm = sext((((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 3), 21);
        let v = if w & 0x8000_0000 != 0 { ((src & !0xFFF) as i64 + (imm << 12)) as u64 } else { (src as i64 + imm) as u64 };
        let rd = w & 31;
        out.push(0x5800_0000 | (2 << 5) | rd);
        out.push(b(dst + 4, dst + 16));
        literal(&mut out, v);
    } else if w & 0x3B00_0000 == 0x1800_0000 {
        assert!(w & 0x0400_0000 == 0, "cannot move a SIMD literal load at {src:#x}");
        let addr = (src as i64 + sext((w >> 5) & 0x7FFFF, 19) * 4) as u64;
        let rt = w & 31;
        let load = match w >> 30 {
            0 => 0xB940_0000,
            1 => 0xF940_0000,
            2 => 0xB980_0000,
            _ => panic!("cannot move a prefetch literal at {src:#x}"),
        } | (rt << 5) | rt;
        out.push(0x5800_0000 | (3 << 5) | rt);
        out.push(load);
        out.push(b(dst + 8, dst + 20));
        literal(&mut out, addr);
    } else {
        out.push(w);
    }
    out
}

// Builds code that runs what the instruction at addr did, then goes on at addr + 4
fn moved_original(addr: u64, at: u64) -> Vec<u32> {
    let w = unsafe { *(addr as *const u32) };
    let mut words = relocate(w, addr, at);
    let pc = at + 4 * words.len() as u64;
    words.push(b(pc, addr + 4));
    words
}

fn next_pool_address() -> u64 {
    POOL.lock().unwrap().next
}

// Skyline: replaces the function at target with replace, and gives in *orig a pointer that runs the original
extern "C" fn hook_function(target: u64, replace: u64, orig: *mut u64) {
    if !orig.is_null() {
        let at = next_pool_address();
        let t = alloc(&moved_original(target, at));
        unsafe { *orig = t };
    }
    let mut stub = vec![0x5800_0050, 0xD61F_0200];
    literal(&mut stub, replace);
    let stub = alloc(&stub);
    write_code(target, &b(target, stub).to_le_bytes());
    eprintln!("[plugin] hooked function at main+{:#x}", target.wrapping_sub(MAIN.lock().unwrap()[0]));
}

const CTX_SIZE: u32 = 0x100;

// Skyline: calls callback(ctx) before the instruction at addr. ctx holds x0 to x30, then NZCV at 0xf8.
// The registers in ctx are written back when the callback returns.
extern "C" fn inline_hook(addr: u64, callback: u64) {
    let stp = |a: u32, b: u32, off: u32| 0xA900_0000 | ((off / 8) << 15) | (b << 10) | (31 << 5) | a;
    let ldp = |a: u32, b: u32, off: u32| stp(a, b, off) | 0x0040_0000;
    let mut w = vec![0xD100_0000 | (CTX_SIZE << 10) | (31 << 5) | 31];
    for r in (0..30).step_by(2) {
        w.push(stp(r, r + 1, r * 8));
    }
    w.push(0xF900_0000 | ((0xF0 / 8) << 10) | (31 << 5) | 30);
    w.push(0xD53B_4210);
    w.push(0xF900_0000 | ((0xF8 / 8) << 10) | (31 << 5) | 16);
    w.push(0x9100_03E0);
    let lit_at = w.len();
    w.push(0);
    w.push(0xD63F_0200);
    w.push(0xF940_0000 | ((0xF8 / 8) << 10) | (31 << 5) | 16);
    w.push(0xD51B_4210);
    w.push(0xF940_0000 | ((0xF0 / 8) << 10) | (31 << 5) | 30);
    for r in (0..30).step_by(2) {
        w.push(ldp(r, r + 1, r * 8));
    }
    w.push(0x9100_0000 | (CTX_SIZE << 10) | (31 << 5) | 31);
    let at = next_pool_address();
    let moved = moved_original(addr, at + 4 * w.len() as u64);
    w.extend(moved);
    let lit = w.len();
    literal(&mut w, callback);
    w[lit_at] = 0x5800_0000 | (((lit - lit_at) as u32) << 5) | 16;
    let stub = alloc(&w);
    assert_eq!(stub, at);
    write_code(addr, &b(addr, stub).to_le_bytes());
}

extern "C" fn sky_memcpy(dst: u64, src: *const u8, size: usize) -> u64 {
    write_code(dst, unsafe { std::slice::from_raw_parts(src, size) });
    0
}

// Skyline: region 0 is the main module's text, 1 its read-only data, 2 its data, 3 its bss
extern "C" fn region_address(region: u8) -> u64 {
    MAIN.lock().unwrap().get(region as usize).copied().unwrap_or(0)
}
