// Mush8 - sarc.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Result, bail, ensure};

pub fn yaz0(data: &[u8]) -> Result<Vec<u8>> {
    if data.len() < 16 || &data[..4] != b"Yaz0" {
        return Ok(data.to_vec());
    }
    let size = u32::from_be_bytes(data[4..8].try_into()?) as usize;
    let mut out = Vec::with_capacity(size);
    let mut i = 16;
    while out.len() < size {
        let Some(&code) = data.get(i) else { bail!("truncated Yaz0") };
        i += 1;
        for bit in (0..8).rev() {
            if out.len() >= size {
                break;
            }
            if code >> bit & 1 == 1 {
                out.push(*data.get(i).ok_or_else(|| anyhow::anyhow!("truncated Yaz0"))?);
                i += 1;
            } else {
                ensure!(i + 1 < data.len(), "truncated Yaz0");
                let (b1, b2) = (data[i] as usize, data[i + 1] as usize);
                i += 2;
                let dist = ((b1 & 0xF) << 8 | b2) + 1;
                let n = if b1 >> 4 == 0 {
                    let n = data[i] as usize + 0x12;
                    i += 1;
                    n
                } else {
                    (b1 >> 4) + 2
                };
                ensure!(dist <= out.len(), "bad Yaz0 back-reference");
                let start = out.len() - dist;
                for k in 0..n {
                    let b = out[start + k];
                    out.push(b);
                }
            }
        }
    }
    out.truncate(size);
    Ok(out)
}

pub fn yaz0_tail(data: &[u8]) -> [u8; 8] {
    data.get(8..16).filter(|_| data.starts_with(b"Yaz0")).map_or([0; 8], |t| t.try_into().unwrap())
}

pub fn yaz0_compress(data: &[u8], tail: [u8; 8]) -> Vec<u8> {
    const WINDOW: usize = 0x1000;
    const MIN: usize = 3;
    const MAX: usize = 0x111;
    const DEPTH: usize = 48;
    let hash = |i: usize| (u32::from_le_bytes([data[i], data[i + 1], data[i + 2], 0]).wrapping_mul(0x9E37_79B1) >> 17) as usize;
    let mut head = vec![usize::MAX; 1 << 15];
    let mut prev = vec![usize::MAX; WINDOW];
    let insert = |i: usize, head: &mut Vec<usize>, prev: &mut Vec<usize>| {
        if i + MIN <= data.len() {
            let h = hash(i);
            prev[i % WINDOW] = head[h];
            head[h] = i;
        }
    };
    let longest = |i: usize, head: &[usize], prev: &[usize]| -> (usize, usize) {
        if i + MIN > data.len() {
            return (0, 0);
        }
        let limit = MAX.min(data.len() - i);
        let (mut best, mut dist) = (0, 0);
        let mut j = head[hash(i)];
        for _ in 0..DEPTH {
            if j >= i || i - j > WINDOW {
                break;
            }
            if data[j + best.min(limit - 1)] == data[i + best.min(limit - 1)] {
                let n = data[j..j + limit].iter().zip(&data[i..i + limit]).take_while(|(a, b)| a == b).count();
                if n > best {
                    (best, dist) = (n, i - j);
                    if n == limit {
                        break;
                    }
                }
            }
            let p = prev[j % WINDOW];
            if p >= j {
                break;
            }
            j = p;
        }
        (best, dist)
    };

    let mut out = Vec::with_capacity(16 + data.len() + data.len() / 8 + 1);
    out.extend_from_slice(b"Yaz0");
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(&tail);
    let (mut i, mut code, mut bit) = (0, 0, 8);
    while i < data.len() {
        if bit == 8 {
            code = out.len();
            out.push(0);
            bit = 0;
        }
        let (mut n, d) = longest(i, &head, &prev);
        if n >= MIN {
            insert(i, &mut head, &mut prev);
            if longest(i + 1, &head, &prev).0 > n + 1 {
                n = 0;
            }
        } else {
            insert(i, &mut head, &mut prev);
        }
        if n >= MIN {
            let d = d - 1;
            if n >= 0x12 {
                out.extend_from_slice(&[(d >> 8) as u8, d as u8, (n - 0x12) as u8]);
            } else {
                out.extend_from_slice(&[((n - 2) << 4 | d >> 8) as u8, d as u8]);
            }
            for k in i + 1..i + n {
                insert(k, &mut head, &mut prev);
            }
            i += n;
        } else {
            out[code] |= 0x80 >> bit;
            out.push(data[i]);
            i += 1;
        }
        bit += 1;
    }
    out
}

pub struct Entry<'a> {
    pub name: String,
    pub hash: u32,
    pub data: &'a [u8],
}

impl Entry<'_> {
    pub fn is(&self, path: &str) -> bool {
        self.name.eq_ignore_ascii_case(path) || self.hash == name_hash(path)
    }
}

pub fn name_hash(name: &str) -> u32 {
    name.bytes().fold(0u32, |h, c| h.wrapping_mul(0x65).wrapping_add(c as u32))
}

struct Header {
    le: bool,
    data_off: usize,
    nodes: usize,
    count: usize,
    names: usize,
}

impl Header {
    fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() >= 0x20 && &data[..4] == b"SARC", "not a SARC");
        let le = u16::from_le_bytes([data[6], data[7]]) == 0xFEFF;
        let h = Self { le, data_off: 0, nodes: 0, count: 0, names: 0 };
        let sfat = h.u16(data, 4) as usize;
        ensure!(data.get(sfat..sfat + 4) == Some(b"SFAT"), "missing SFAT");
        let count = h.u16(data, sfat + 6) as usize;
        let nodes = sfat + 0xC;
        let sfnt = nodes + count * 0x10;
        ensure!(data.get(sfnt..sfnt + 4) == Some(b"SFNT"), "missing SFNT");
        Ok(Self { data_off: h.u32(data, 0xC) as usize, nodes, count, names: sfnt + 8, ..h })
    }
    fn u16(&self, data: &[u8], o: usize) -> u16 {
        let b = [data[o], data[o + 1]];
        if self.le { u16::from_le_bytes(b) } else { u16::from_be_bytes(b) }
    }
    fn u32(&self, data: &[u8], o: usize) -> u32 {
        let b: [u8; 4] = data[o..o + 4].try_into().unwrap();
        if self.le { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }
    }
    fn put_u32(&self, data: &mut [u8], o: usize, v: u32) {
        data[o..o + 4].copy_from_slice(&if self.le { v.to_le_bytes() } else { v.to_be_bytes() });
    }
    fn range(&self, data: &[u8], k: usize) -> (usize, usize) {
        let n = self.nodes + k * 0x10;
        (self.u32(data, n + 8) as usize, self.u32(data, n + 0xC) as usize)
    }
}

pub fn sarc(data: &[u8]) -> Result<Vec<Entry<'_>>> {
    let h = Header::parse(data)?;
    let mut out = Vec::with_capacity(h.count);
    for k in 0..h.count {
        let n = h.nodes + k * 0x10;
        let attr = h.u32(data, n + 4);
        let (start, end) = h.range(data, k);
        let name = if attr >> 24 != 0 {
            let o = h.names + (attr & 0xFFFF) as usize * 4;
            let e = data[o..].iter().position(|&c| c == 0).unwrap_or(0);
            String::from_utf8_lossy(&data[o..o + e]).into_owned()
        } else {
            format!("{:08x}", h.u32(data, n))
        };
        ensure!(h.data_off + end <= data.len() && start <= end, "SARC entry out of range");
        out.push(Entry { name, hash: h.u32(data, n), data: &data[h.data_off + start..h.data_off + end] });
    }
    Ok(out)
}

pub fn rebuild(data: &[u8], contents: &[&[u8]]) -> Result<Vec<u8>> {
    let h = Header::parse(data)?;
    ensure!(contents.len() == h.count, "SARC has {} entries, got {}", h.count, contents.len());
    let mut order: Vec<usize> = (0..h.count).collect();
    order.sort_by_key(|&k| h.range(data, k).0);
    let mut out = data[..h.data_off].to_vec();
    for k in order {
        let align = 1usize << (h.data_off + h.range(data, k).0).trailing_zeros().min(13);
        out.resize(out.len().next_multiple_of(align), 0);
        let start = out.len() - h.data_off;
        out.extend_from_slice(contents[k]);
        let n = h.nodes + k * 0x10;
        h.put_u32(&mut out, n + 8, start as u32);
        h.put_u32(&mut out, n + 0xC, (start + contents[k].len()) as u32);
    }
    let size = out.len() as u32;
    h.put_u32(&mut out, 8, size);
    Ok(out)
}

// Rebuilds the archive with new contents for its entries, and adds the entries in `added`
pub fn rebuild_adding(data: &[u8], contents: &[&[u8]], added: &[(&str, &[u8])]) -> Result<Vec<u8>> {
    if added.is_empty() {
        return rebuild(data, contents);
    }
    let h = Header::parse(data)?;
    let entries = sarc(data)?;
    ensure!(contents.len() == h.count, "SARC has {} entries, got {}", h.count, contents.len());
    let mut all: Vec<(String, u32, &[u8], usize)> = Vec::new();
    for (k, e) in entries.iter().enumerate() {
        let attr = h.u32(data, h.nodes + k * 0x10 + 4);
        ensure!(attr >> 24 != 0, "cannot add files to a SARC with unnamed entries");
        let align = 1usize << (h.data_off + h.range(data, k).0).trailing_zeros().min(13);
        all.push((e.name.clone(), e.hash, contents[k], align));
    }
    for &(name, d) in added {
        let hash = name_hash(name);
        ensure!(!all.iter().any(|a| a.1 == hash), "{name} has the same hash as a file in the archive");
        let align = if d.starts_with(b"BNTX") || d.starts_with(b"FRES") { 0x1000 } else { 0x80 };
        all.push((name.to_string(), hash, d, align));
    }
    all.sort_by_key(|a| a.1);
    let put16 = |o: &mut Vec<u8>, v: u16| o.extend_from_slice(&if h.le { v.to_le_bytes() } else { v.to_be_bytes() });
    let put32 = |o: &mut Vec<u8>, v: u32| o.extend_from_slice(&if h.le { v.to_le_bytes() } else { v.to_be_bytes() });
    let mut names = Vec::new();
    let mut name_at = Vec::new();
    for a in &all {
        name_at.push(names.len() / 4);
        names.extend_from_slice(a.0.as_bytes());
        names.push(0);
        names.resize(names.len().next_multiple_of(4), 0);
    }
    let mut out = data[..0x14].to_vec();
    out.extend_from_slice(b"SFAT");
    put16(&mut out, 0xC);
    put16(&mut out, all.len() as u16);
    put32(&mut out, 0x65);
    let nodes = out.len();
    out.resize(nodes + all.len() * 0x10, 0);
    out.extend_from_slice(b"SFNT");
    put16(&mut out, 8);
    put16(&mut out, 0);
    out.extend_from_slice(&names);
    let data_align = all.iter().map(|a| a.3).max().unwrap_or(4).max(1usize << h.data_off.trailing_zeros().min(13));
    out.resize(out.len().next_multiple_of(data_align), 0);
    let data_off = out.len();
    for (i, a) in all.iter().enumerate() {
        out.resize(out.len().next_multiple_of(a.3), 0);
        let start = out.len() - data_off;
        out.extend_from_slice(a.2);
        let mut node = Vec::new();
        put32(&mut node, a.1);
        put32(&mut node, 0x0100_0000 | name_at[i] as u32);
        put32(&mut node, start as u32);
        put32(&mut node, (start + a.2.len()) as u32);
        out[nodes + i * 0x10..nodes + i * 0x10 + 0x10].copy_from_slice(&node);
    }
    let size = out.len() as u32;
    h.put_u32(&mut out, 8, size);
    h.put_u32(&mut out, 0xC, data_off as u32);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn name_hash_matches_the_game() {
        assert_eq!(name_hash("timg/__Combined.bntx"), 0xc352_5d09);
    }

    #[test]
    fn yaz0_literals_and_backrefs() {
        let mut d = b"Yaz0".to_vec();
        d.extend_from_slice(&9u32.to_be_bytes());
        d.extend_from_slice(&[0; 8]);
        d.push(0b1110_0000);
        d.extend_from_slice(b"abc");
        d.extend_from_slice(&[0x40, 0x02]);
        assert_eq!(yaz0(&d).unwrap(), b"abcabcabc");
    }

    #[test]
    fn yaz0_round_trip() {
        let mut seed = 0x1234_5678u32;
        let mut noise = || {
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            seed
        };
        let mut data = Vec::new();
        for _ in 0..4000 {
            match noise() % 3 {
                0 => data.extend((0..noise() % 40).map(|_| noise() as u8)),
                1 => data.extend(std::iter::repeat_n(noise() as u8, (noise() % 600) as usize)),
                _ if data.len() > 10 => {
                    let start = noise() as usize % data.len();
                    let n = (noise() as usize % 300).min(data.len() - start);
                    data.extend_from_within(start..start + n);
                }
                _ => {}
            }
        }
        for d in [&data[..], b"", b"a", b"abcabcabcabc"] {
            let packed = yaz0_compress(d, [1, 2, 3, 4, 5, 6, 7, 8]);
            assert_eq!(yaz0_tail(&packed), [1, 2, 3, 4, 5, 6, 7, 8]);
            assert_eq!(yaz0(&packed).unwrap(), d);
        }
        assert!(yaz0_compress(&data, [0; 8]).len() < data.len() / 2);
    }

    #[test]
    fn sarc_rebuild_moves_entries() {
        let mut d = b"SARC\x14\x00\xFF\xFE\0\0\0\0\0\0\0\0\0\x01\0\0".to_vec();
        d.extend_from_slice(b"SFAT\x0C\x00\x02\x00\x65\0\0\0");
        for (hash, start, end) in [(1u32, 0x80u32, 0x83u32), (2, 0, 2)] {
            for v in [hash, 0, start, end] {
                d.extend_from_slice(&v.to_le_bytes());
            }
        }
        d.extend_from_slice(b"SFNT\x08\x00\x00\x00");
        d.resize(0x80, 0);
        d[0xC..0x10].copy_from_slice(&0x80u32.to_le_bytes());
        d.extend_from_slice(b"hi");
        d.resize(0x100, 0);
        d.extend_from_slice(b"abc");
        let out = rebuild(&d, &[b"longer than before", b"x"]).unwrap();
        let e = sarc(&out).unwrap();
        assert_eq!((e[0].data, e[1].data), (&b"longer than before"[..], &b"x"[..]));
        assert_eq!(u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize, out.len());
        assert_eq!((e[0].data.as_ptr() as usize - out.as_ptr() as usize) % 0x80, 0);
    }

    #[test]
    fn sarc_rebuild_adds_entries() {
        let mut d = b"SARC\x14\x00\xFF\xFE\0\0\0\0\0\0\0\0\0\x01\0\0".to_vec();
        d.extend_from_slice(b"SFAT\x0C\x00\x01\x00\x65\0\0\0");
        for v in [name_hash("a.szs"), 0x0100_0000, 0, 2] {
            d.extend_from_slice(&v.to_le_bytes());
        }
        d.extend_from_slice(b"SFNT\x08\x00\x00\x00a.szs\0\0\0");
        d.resize(0x80, 0);
        d[0xC..0x10].copy_from_slice(&0x80u32.to_le_bytes());
        d.extend_from_slice(b"hi");
        let out = rebuild_adding(&d, &[b"hello"], &[("new.msbt", b"MsgStdBn")]).unwrap();
        let e = sarc(&out).unwrap();
        let get = |n: &str| e.iter().find(|x| x.name == n).map(|x| x.data);
        assert_eq!((get("a.szs"), get("new.msbt")), (Some(&b"hello"[..]), Some(&b"MsgStdBn"[..])));
        assert!(e.windows(2).all(|w| w[0].hash < w[1].hash));
        assert_eq!(u32::from_le_bytes(out[8..12].try_into().unwrap()) as usize, out.len());
    }
}
