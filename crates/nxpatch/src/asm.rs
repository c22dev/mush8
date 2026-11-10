// Mush8 - asm.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Result, ensure};

pub type Reg = u32;
pub const SP: Reg = 31;

fn imm_fits(v: i64, bits: u32) -> bool {
    let lim = 1i64 << (bits - 1);
    (-lim..lim).contains(&v)
}

pub fn stp(a: Reg, b: Reg, n: Reg, off: i32) -> u32 {
    debug_assert!(off % 8 == 0 && imm_fits(off as i64 / 8, 7));
    0xA900_0000 | (((off / 8) as u32 & 0x7F) << 15) | (b << 10) | (n << 5) | a
}
pub fn ldp(a: Reg, b: Reg, n: Reg, off: i32) -> u32 {
    stp(a, b, n, off) | 0x0040_0000
}
pub fn stp_pre(a: Reg, b: Reg, n: Reg, off: i32) -> u32 {
    0xA980_0000 | (((off / 8) as u32 & 0x7F) << 15) | (b << 10) | (n << 5) | a
}
pub fn ldp_post(a: Reg, b: Reg, n: Reg, off: i32) -> u32 {
    0xA8C0_0000 | (((off / 8) as u32 & 0x7F) << 15) | (b << 10) | (n << 5) | a
}
pub fn str(t: Reg, n: Reg, off: u32) -> u32 {
    debug_assert!(off % 8 == 0 && off / 8 < 4096);
    0xF900_0000 | ((off / 8) << 10) | (n << 5) | t
}
pub fn ldr(t: Reg, n: Reg, off: u32) -> u32 {
    str(t, n, off) | 0x0040_0000
}
pub fn stur(t: Reg, n: Reg, off: i32) -> u32 {
    debug_assert!(imm_fits(off as i64, 9));
    0xF800_0000 | ((off as u32 & 0x1FF) << 12) | (n << 5) | t
}
pub fn ldur(t: Reg, n: Reg, off: i32) -> u32 {
    stur(t, n, off) | 0x0040_0000
}
pub fn ldr_reg(t: Reg, n: Reg, m: Reg) -> u32 {
    0xF860_6800 | (m << 16) | (n << 5) | t
}
pub fn mrs_tpidrro(t: Reg) -> u32 {
    0xD53B_D060 | t
}
pub fn mrs_cntvct(t: Reg) -> u32 {
    0xD53B_E040 | t
}
pub fn add_imm(d: Reg, n: Reg, imm: u32) -> u32 {
    debug_assert!(imm < 4096);
    0x9100_0000 | (imm << 10) | (n << 5) | d
}
pub fn sub_reg(d: Reg, n: Reg, m: Reg) -> u32 {
    0xCB00_0000 | (m << 16) | (n << 5) | d
}
pub fn movz(d: Reg, imm16: u32, shift: u32) -> u32 {
    0xD280_0000 | ((shift / 16) << 21) | (imm16 << 5) | d
}
pub fn movk(d: Reg, imm16: u32, shift: u32) -> u32 {
    0xF280_0000 | ((shift / 16) << 21) | (imm16 << 5) | d
}
pub fn umulh(d: Reg, n: Reg, m: Reg) -> u32 {
    0x9BC0_7C00 | (m << 16) | (n << 5) | d
}
pub fn blr(n: Reg) -> u32 {
    0xD63F_0000 | (n << 5)
}
pub fn brk(imm: u16) -> u32 {
    0xD420_0000 | ((imm as u32) << 5)
}
pub fn str_q(t: Reg, n: Reg, off: u32) -> u32 {
    debug_assert!(off % 16 == 0 && off / 16 < 4096);
    0x3D80_0000 | ((off / 16) << 10) | (n << 5) | t
}
pub fn ldr_q(t: Reg, n: Reg, off: u32) -> u32 {
    str_q(t, n, off) | 0x0040_0000
}
pub fn str_s(t: Reg, n: Reg, off: u32) -> u32 {
    debug_assert!(off % 4 == 0 && off / 4 < 4096);
    0xBD00_0000 | ((off / 4) << 10) | (n << 5) | t
}
pub fn ldr_s(t: Reg, n: Reg, off: u32) -> u32 {
    str_s(t, n, off) | 0x0040_0000
}
pub fn is_str_s(w: u32) -> bool {
    w & 0xFFC0_0000 == 0xBD00_0000
}
pub fn fmov_w_s(d: Reg, n: Reg) -> u32 {
    0x1E26_0000 | (n << 5) | d
}
pub fn fmov_s_w(d: Reg, n: Reg) -> u32 {
    0x1E27_0000 | (n << 5) | d
}
pub fn fmul_s(d: Reg, n: Reg, m: Reg) -> u32 {
    0x1E20_0800 | (m << 16) | (n << 5) | d
}
pub fn lsl_w(d: Reg, n: Reg, shift: u32) -> u32 {
    debug_assert!((1..32).contains(&shift));
    0x5300_0000 | ((32 - shift) << 16) | ((31 - shift) << 10) | (n << 5) | d
}
pub fn cbz_w(t: Reg, off: i32) -> u32 {
    debug_assert!(off % 4 == 0 && imm_fits(off as i64 / 4, 19));
    0x3400_0000 | (((off / 4) as u32 & 0x7FFFF) << 5) | t
}

pub fn b(from: u64, to: u64) -> Result<u32> {
    let d = (to as i64 - from as i64) / 4;
    ensure!(imm_fits(d, 26), "branch out of range {from:#x} -> {to:#x}");
    Ok(0x1400_0000 | (d as u32 & 0x03FF_FFFF))
}

pub fn adrp(d: Reg, pc: u64, target: u64) -> Result<u32> {
    let pages = ((target & !0xFFF) as i64 - (pc & !0xFFF) as i64) >> 12;
    ensure!(imm_fits(pages, 21), "adrp out of range");
    let p = pages as u32;
    Ok(0x9000_0000 | ((p & 3) << 29) | (((p >> 2) & 0x7FFFF) << 5) | d)
}

pub fn retarget(word: u32, from: u64, to: u64) -> Result<u32> {
    let d = (to as i64 - from as i64) / 4;
    if is_bcond(word) || is_cbz(word) {
        ensure!(imm_fits(d, 19), "conditional branch out of range");
        Ok((word & 0xFF00_001F) | ((d as u32 & 0x7FFFF) << 5))
    } else if is_tbz(word) {
        ensure!(imm_fits(d, 14), "test branch out of range");
        Ok((word & 0xFFF8_001F) | ((d as u32 & 0x3FFF) << 5))
    } else {
        anyhow::bail!("not a conditional branch: {word:#010x}")
    }
}

pub fn is_bcond(w: u32) -> bool {
    w & 0xFF00_0010 == 0x5400_0000
}
pub fn is_cbz(w: u32) -> bool {
    w & 0x7E00_0000 == 0x3400_0000
}
pub fn is_tbz(w: u32) -> bool {
    w & 0x7E00_0000 == 0x3600_0000
}
pub fn is_cond_branch(w: u32) -> bool {
    is_bcond(w) || is_cbz(w) || is_tbz(w)
}
pub fn bl(from: u64, to: u64) -> Result<u32> {
    Ok(b(from, to)? | 0x8000_0000)
}
pub fn ldrb_w(t: Reg, n: Reg, off: u32) -> u32 {
    debug_assert!(off < 4096);
    0x3940_0000 | (off << 10) | (n << 5) | t
}
pub fn sub_imm_w(d: Reg, n: Reg, imm: u32) -> u32 {
    debug_assert!(imm < 4096);
    0x5100_0000 | (imm << 10) | (n << 5) | d
}
pub fn is_b(w: u32) -> bool {
    w & 0xFC00_0000 == 0x1400_0000
}
pub fn is_bl(w: u32) -> bool {
    w & 0xFC00_0000 == 0x9400_0000
}
pub fn branch_target(w: u32, pc: u64) -> u64 {
    (pc as i64 + sext(w & 0x03FF_FFFF, 26) * 4) as u64
}
pub fn cond_target(w: u32, pc: u64) -> u64 {
    let d = if is_tbz(w) { sext((w >> 5) & 0x3FFF, 14) } else { sext((w >> 5) & 0x7FFFF, 19) };
    (pc as i64 + d * 4) as u64
}
pub fn is_uncond_branch(w: u32) -> bool {
    w & 0x7C00_0000 == 0x1400_0000
        || w & 0xFE00_0000 == 0xD600_0000
}
pub fn is_adr(w: u32) -> bool {
    w & 0x9F00_0000 == 0x1000_0000
}
pub fn is_adrp(w: u32) -> bool {
    w & 0x9F00_0000 == 0x9000_0000
}
pub fn adr_target(w: u32, pc: u64) -> u64 {
    let imm = sext((((w >> 5) & 0x7FFFF) << 2) | ((w >> 29) & 3), 21);
    if is_adrp(w) { ((pc & !0xFFF) as i64 + (imm << 12)) as u64 } else { (pc as i64 + imm) as u64 }
}
pub fn is_ldr_literal(w: u32) -> bool {
    w & 0x3B00_0000 == 0x1800_0000
}
pub fn ldr_literal_target(w: u32, pc: u64) -> u64 {
    (pc as i64 + sext((w >> 5) & 0x7FFFF, 19) * 4) as u64
}
pub fn ldr_literal_as_base(w: u32, n: Reg) -> Option<u32> {
    let (opc, v, rt) = (w >> 30, (w >> 26) & 1, w & 31);
    let base = match (v, opc) {
        (0, 0) => 0xB940_0000,
        (0, 1) => 0xF940_0000,
        (0, 2) => 0xB980_0000,
        (1, 0) => 0xBD40_0000,
        (1, 1) => 0xFD40_0000,
        (1, 2) => 0x3DC0_0000,
        _ => return None,
    };
    Some(base | (n << 5) | rt)
}

pub fn sext(v: u32, bits: u32) -> i64 {
    let s = 64 - bits;
    ((v as i64) << s) >> s
}

pub fn is_load_exclusive(w: u32) -> bool {
    w & 0x3F80_0000 == 0x0800_0000 && (w >> 22) & 1 == 1
}
pub fn is_store_exclusive(w: u32) -> bool {
    w & 0x3F80_0000 == 0x0800_0000 && (w >> 22) & 1 == 0
}
pub fn is_svc(w: u32) -> bool {
    w & 0xFFE0_001F == 0xD400_0001
}
pub fn sysreg(w: u32) -> u32 {
    (w >> 5) & 0x7FFF
}
pub fn is_mrs(w: u32) -> bool {
    w & 0xFFF0_0000 == 0xD530_0000
}
pub fn is_msr_reg(w: u32) -> bool {
    w & 0xFFF0_0000 == 0xD510_0000
}
pub const SYSREG_TPIDR_EL0: u32 = 0x5E82;
pub const SYSREG_TPIDRRO_EL0: u32 = 0x5E83;
pub const SYSREG_CNTPCT_EL0: u32 = 0x5F01;
pub const SYSREG_CTR_EL0: u32 = 0x5801;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sysreg_constants() {
        assert_eq!(sysreg(0xD53B_D040), SYSREG_TPIDR_EL0);
        assert_eq!(sysreg(0xD53B_D060), SYSREG_TPIDRRO_EL0);
        assert_eq!(sysreg(0xD53B_E020), SYSREG_CNTPCT_EL0);
    }

    #[test]
    fn encodings_match_assembler() {
        assert_eq!(stp(9, 10, SP, -16), 0xA93F_2BE9);
        assert_eq!(ldp(9, 10, SP, -16), 0xA97F_2BE9);
        assert_eq!(ldr(9, 10, 8), 0xF940_0549);
        assert_eq!(str(9, 10, 0), 0xF900_0149);
        assert_eq!(ldr_reg(10, 9, 10), 0xF86A_692A);
        assert_eq!(stur(11, SP, -24), 0xF81E_83EB);
        assert_eq!(ldur(11, SP, -24), 0xF85E_83EB);
        assert_eq!(mrs_tpidrro(9), 0xD53B_D069);
        assert_eq!(mrs_cntvct(9), 0xD53B_E049);
        assert_eq!(umulh(9, 9, 10), 0x9BCA_7D29);
        assert_eq!(sub_reg(9, 9, 10), 0xCB0A_0129);
        assert_eq!(stp_pre(16, 30, SP, -16), 0xA9BF_7BF0);
        assert_eq!(ldp_post(16, 30, SP, 16), 0xA8C1_7BF0);
        assert_eq!(blr(30), 0xD63F_03C0);
        assert_eq!(str_q(1, SP, 16), 0x3D80_07E1);
        assert_eq!(ldr_q(1, SP, 16), 0x3DC0_07E1);
        assert_eq!(ldr_s(1, 16, 0x50), 0xBD40_5201);
        assert_eq!(str_s(0, 9, 0xB0), 0xBD00_B120);
        assert_eq!(fmov_w_s(17, 1), 0x1E26_0031);
        assert_eq!(lsl_w(17, 17, 1), 0x531F_7A31);
        assert_eq!(cbz_w(17, 8), 0x3400_0051);
        assert_eq!(fmul_s(0, 0, 1), 0x1E21_0800);
    }

    #[test]
    fn pc_relative() {
        let w = adrp(9, 0x1000, 0x7B7578).unwrap();
        assert_eq!(adr_target(w, 0x1000), 0x7B7000);
        let cbz = 0x3400_0012;
        let r = retarget(cbz, 0x100, 0x40).unwrap();
        assert_eq!(cond_target(r, 0x100), 0x40);
        let tbnz = 0x3700_0012;
        let r = retarget(tbnz, 0x100, 0x180).unwrap();
        assert_eq!(cond_target(r, 0x100), 0x180);
    }
}
