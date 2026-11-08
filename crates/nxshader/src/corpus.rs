// Mush8 - corpus.rs
// Constantin Clerc (c22dev) - 0.1
use std::collections::{BTreeMap, BTreeSet};

pub fn key(blob: &[u8]) -> Option<u64> {
    Some(fnv1a(&blob[0x30..code_len(blob)?]))
}

pub fn scan(data: &[u8], found: &mut BTreeMap<u64, Vec<u8>>) {
    scan_at(data, found, 0, &mut None);
}

pub fn scan_pairs(data: &[u8], found: &mut BTreeMap<u64, Vec<u8>>, pairs: &mut BTreeSet<(u64, u64)>) {
    scan_at(data, found, 0, &mut Some(pairs));
}

fn u32_at(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], o: usize) -> Option<usize> {
    usize::try_from(u64::from_le_bytes(b.get(o..o + 8)?.try_into().ok()?)).ok()
}

fn bnsh_stage(b: &[u8], stage: usize) -> Option<u64> {
    if stage == 0 {
        return None;
    }
    let code = u64_at(b, stage + 0x10)?;
    (u32_at(b, code)? == 0x1234_5678).then_some(())?;
    key(b.get(code..)?)
}

pub fn bnsh_pairs(b: &[u8], pairs: &mut BTreeSet<(u64, u64)>) {
    let Some(grsc) = u32_at(b, 0x14).map(|v| (v >> 16) as usize) else { return };
    if b.get(grsc..grsc + 4) != Some(b"grsc") {
        return;
    }
    let (Some(n), Some(arr)) = (u32_at(b, grsc + 0x1C), u64_at(b, grsc + 0x20)) else { return };
    for i in 0..n.min(0x10000) as usize {
        let Some(prog) = u64_at(b, arr + 0x40 * i + 0x10) else { continue };
        let (Some(vs), Some(fs)) = (u64_at(b, prog + 0x08), u64_at(b, prog + 0x28)) else { continue };
        if let (Some(v), Some(f)) = (bnsh_stage(b, vs), bnsh_stage(b, fs)) {
            pairs.insert((v, f));
        }
    }
}

fn scan_bnsh(data: &[u8], pairs: &mut BTreeSet<(u64, u64)>) {
    let mut i = 0;
    while let Some(p) = data[i..].windows(4).position(|w| w == b"BNSH") {
        let at = i + p;
        i = at + 4;
        let size = u32_at(data, at + 0x1C).unwrap_or(0) as usize;
        if at % 8 == 0 && size >= 0x40 && at + size <= data.len() {
            bnsh_pairs(&data[at..at + size], pairs);
        }
    }
}

const NVN_CODE_MAGIC: [u8; 4] = 0x1234_5678u32.to_le_bytes();

fn code_len(blob: &[u8]) -> Option<usize> {
    let code = blob.get(0x80..)?;
    for (i, c) in code.chunks_exact(8).enumerate().take(0x8000) {
        if i % 4 == 0 {
            continue;
        }
        let w = u64::from_le_bytes(c.try_into().unwrap());
        let rel = ((((w >> 20) as i32) << 8) >> 8) + 8;
        if crate::table::lookup(w).0 == crate::table::InstName::Bra && (-32..=0).contains(&rel) {
            return Some(0x80 + (i / 4 + 1) * 32);
        }
    }
    None
}

fn valid_sph(blob: &[u8]) -> bool {
    blob.len() >= 0x80 && {
        let w0 = u32::from_le_bytes(blob[0x30..0x34].try_into().unwrap());
        (1..=5).contains(&(w0 >> 10 & 0xF)) || (1..=5).contains(&(w0 & 0x1F))
    }
}

fn scan_at(data: &[u8], found: &mut BTreeMap<u64, Vec<u8>>, depth: usize, pairs: &mut Option<&mut BTreeSet<(u64, u64)>>) {
    let data = match nxfs::sarc::yaz0(data) {
        Ok(d) => d,
        Err(_) => return,
    };
    if depth < 4 {
        if let Ok(entries) = nxfs::sarc::sarc(&data) {
            for e in entries {
                scan_at(e.data, found, depth + 1, pairs);
            }
            return;
        }
    }
    if let Some(p) = pairs {
        scan_bnsh(&data, p);
    }
    let mut i = 0;
    while let Some(p) = data[i..].windows(4).position(|w| w == NVN_CODE_MAGIC) {
        let at = i + p;
        i = at + 4;
        if at % 0x10 != 0 || data.get(at + 4..at + 0x30).is_none_or(|z| z.iter().any(|&b| b != 0)) {
            continue;
        }
        let blob = &data[at..];
        if !valid_sph(blob) {
            continue;
        }
        if let Some(len) = code_len(blob) {
            let code = blob[..len].to_vec();
            let h = fnv1a(&code[0x30..]);
            found.entry(h).or_insert(code);
        }
    }
}

pub fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100_0000_01b3))
}
