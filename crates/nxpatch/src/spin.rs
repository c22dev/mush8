// Mush8 - spin.rs
// Constantin Clerc (c22dev) - 0.1
use crate::asm;

pub const MAX_LEN: usize = 24;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Loop {
    pub head: usize,
    pub back: usize,
}

#[derive(Default, Clone, Copy)]
struct Use {
    reads: u32,
    writes: u32,
    reads_flags: bool,
    sets_flags: bool,
    load: bool,
    branch: Option<i64>,
}

fn reg(r: u32) -> u32 {
    if r == 31 { 0 } else { 1 << r }
}

fn fields(w: u32) -> (u32, u32, u32) {
    (w & 31, (w >> 5) & 31, (w >> 16) & 31)
}

fn decode(w: u32) -> Option<Use> {
    let (rd, rn, rm) = fields(w);
    let alu = |reads: u32, s: bool, sp_dest: bool| {
        if sp_dest && !s && rd == 31 {
            return None;
        }
        Some(Use { reads, writes: reg(rd), sets_flags: s, ..Default::default() })
    };
    if w & 0x3F00_0000 == 0x3900_0000 {
        let (size, opc) = (w >> 30, (w >> 22) & 3);
        if opc == 0 || (size == 3 && opc >= 2) {
            return None;
        }
        return Some(Use { reads: reg(rn), writes: reg(rd), load: true, ..Default::default() });
    }
    if w & 0x3F00_0000 == 0x3800_0000 {
        let (size, opc) = (w >> 30, (w >> 22) & 3);
        if opc == 0 || (size == 3 && opc >= 2) {
            return None;
        }
        let kind = (w >> 10) & 3;
        return match (w >> 21 & 1, kind) {
            (0, 0) => Some(Use { reads: reg(rn), writes: reg(rd), load: true, ..Default::default() }),
            (1, 2) => Some(Use { reads: reg(rn) | reg(rm), writes: reg(rd), load: true, ..Default::default() }),
            _ => None,
        };
    }
    if w & 0x3FFF_FC00 == 0x08DF_FC00 || w & 0x3FFF_FC00 == 0x38BF_C000 {
        return Some(Use { reads: reg(rn), writes: reg(rd), load: true, ..Default::default() });
    }
    let s = w >> 29 & 1 != 0;
    if w & 0x1F80_0000 == 0x1100_0000 {
        return alu(reg(rn), s, true);
    }
    if w & 0x1F20_0000 == 0x0B00_0000 {
        return alu(reg(rn) | reg(rm), s, false);
    }
    if w & 0x1F20_0000 == 0x0B20_0000 {
        return alu(reg(rn) | reg(rm), s, true);
    }
    if w & 0x1F00_0000 == 0x0A00_0000 {
        return alu(reg(rn) | reg(rm), (w >> 29) & 3 == 3, false);
    }
    if w & 0x1F80_0000 == 0x1200_0000 {
        return alu(reg(rn), (w >> 29) & 3 == 3, true);
    }
    if w & 0x1F80_0000 == 0x1280_0000 {
        return match (w >> 29) & 3 {
            0 | 2 => alu(0, false, false),
            3 => alu(reg(rd), false, false),
            _ => None,
        };
    }
    if w & 0x1F80_0000 == 0x1300_0000 {
        return match (w >> 29) & 3 {
            0 | 2 => alu(reg(rn), false, false),
            1 => alu(reg(rn) | reg(rd), false, false),
            _ => None,
        };
    }
    if w & 0x7FA0_0000 == 0x1380_0000 {
        return alu(reg(rn) | reg(rm), false, false);
    }
    if w & 0x1FE0_0000 == 0x1A40_0000 {
        let reads = reg(rn) | if w >> 11 & 1 == 0 { reg(rm) } else { 0 };
        return Some(Use { reads, reads_flags: true, sets_flags: true, ..Default::default() });
    }
    if w & 0x1FE0_0000 == 0x1A80_0000 {
        return alu(reg(rn) | reg(rm), false, false).map(|u| Use { reads_flags: true, ..u });
    }
    if w & 0x5FE0_0000 == 0x1AC0_0000 || w & 0x5FE0_0000 == 0x5AC0_0000 {
        return alu(reg(rn) | reg(rm), false, false);
    }
    if asm::is_bcond(w) {
        return Some(Use { reads_flags: true, branch: Some(asm::sext((w >> 5) & 0x7FFFF, 19)), ..Default::default() });
    }
    if asm::is_cbz(w) {
        return Some(Use { reads: reg(rd), branch: Some(asm::sext((w >> 5) & 0x7FFFF, 19)), ..Default::default() });
    }
    if asm::is_tbz(w) {
        return Some(Use { reads: reg(rd), branch: Some(asm::sext((w >> 5) & 0x3FFF, 14)), ..Default::default() });
    }
    if asm::is_b(w) {
        return Some(Use { branch: Some(asm::sext(w & 0x03FF_FFFF, 26)), ..Default::default() });
    }
    if matches!(w, 0xD503_201F | 0xD503_203F) || [0xD503_30BF, 0xD503_309F, 0xD503_30DF].contains(&(w & 0xFFFF_F0FF)) {
        return Some(Use::default());
    }
    None
}

fn check(words: &[u32], head: usize, back: usize) -> bool {
    let mut uses = Vec::with_capacity(back - head + 1);
    for (k, &w) in words[head..=back].iter().enumerate() {
        let Some(u) = decode(w) else { return false };
        if let Some(d) = u.branch {
            let at = (head + k) as i64;
            let to = at + d;
            if head + k == back {
                if to != head as i64 {
                    return false;
                }
            } else if !asm::is_cond_branch(w) || (head as i64..=back as i64).contains(&to) {
                return false;
            }
        }
        uses.push(u);
    }
    let all_writes = uses.iter().fold(0, |m, u| m | u.writes);
    let any_flags = uses.iter().any(|u| u.sets_flags);
    let (mut written, mut flags) = (0u32, false);
    for u in &uses {
        if u.reads & all_writes & !written != 0 {
            return false;
        }
        if u.reads_flags && !flags && any_flags {
            return false;
        }
        written |= u.writes;
        flags |= u.sets_flags;
    }
    uses.iter().any(|u| u.load)
}

pub fn find(words: &[u32], plain: impl Fn(usize) -> bool) -> Vec<Loop> {
    let mut out = Vec::new();
    for back in 0..words.len() {
        let w = words[back];
        let d = if asm::is_b(w) {
            asm::sext(w & 0x03FF_FFFF, 26)
        } else if asm::is_cond_branch(w) {
            if asm::is_tbz(w) { asm::sext((w >> 5) & 0x3FFF, 14) } else { asm::sext((w >> 5) & 0x7FFFF, 19) }
        } else {
            continue;
        };
        if d >= 0 || -d as usize >= MAX_LEN {
            continue;
        }
        let head = back - (-d as usize);
        if (head..=back).all(&plain) && check(words, head, back) {
            out.push(Loop { head, back });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(words: &[u32]) -> Vec<Loop> {
        find(words, |_| true)
    }

    #[test]
    fn flag_poll_is_a_spin() {
        let w = [0xB940_0268, 0x7100_051F, 0x54FF_FFC1];
        assert_eq!(found(&w), vec![Loop { head: 0, back: 2 }]);
    }

    #[test]
    fn ldar_cbz_with_exit_is_a_spin() {
        let w = [0x88DF_FE68, 0x3500_0048, 0x17FF_FFFE];
        assert_eq!(found(&w), vec![Loop { head: 0, back: 2 }]);
    }

    #[test]
    fn counter_is_not_a_spin() {
        let w = [0xB940_0268, 0x1100_0529, 0x7100_011F, 0x54FF_FFA0];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn pointer_chase_is_not_a_spin() {
        let w = [0xF940_0108, 0xB5FF_FFE8];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn store_is_not_a_spin() {
        let w = [0xB940_0268, 0xB900_0669, 0x34FF_FFC8];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn exclusive_load_is_not_a_spin() {
        let w = [0x885F_7E68, 0x35FF_FFE8];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn writeback_load_is_not_a_spin() {
        let w = [0xB840_4668, 0x34FF_FFE8];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn loop_carried_flags_are_not_a_spin() {
        let w = [0xB940_0268, 0x5400_0060, 0x7100_011F, 0x17FF_FFFD];
        assert!(found(&w).is_empty());
    }

    #[test]
    fn rewritten_sites_are_left_alone() {
        let w = [0x88DF_FE68, 0x3500_0048, 0x17FF_FFFE];
        assert!(find(&w, |i| i != 1).is_empty());
    }
}
