// Mush8 - msbt.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result, ensure};

pub struct Msbt {
    header: Vec<u8>,
    sections: Vec<([u8; 4], Vec<u8>)>,
    labels: Vec<(String, usize)>,
    texts: Vec<Vec<u8>>,
    added: bool,
}

fn u32_at(b: &[u8], o: usize) -> Result<u32> {
    Ok(u32::from_le_bytes(b.get(o..o + 4).context("MSBT cut short")?.try_into()?))
}

fn encode(text: &str) -> Vec<u8> {
    text.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()
}

impl Msbt {
    pub fn parse(data: &[u8]) -> Result<Self> {
        ensure!(data.len() >= 0x20 && &data[..8] == b"MsgStdBn", "not an MSBT");
        ensure!(data[8..10] == [0xFF, 0xFE], "big-endian MSBT is not supported");
        ensure!(data[0xC] == 1, "MSBT text is not UTF-16");
        let count = u16::from_le_bytes([data[0xE], data[0xF]]) as usize;
        let mut sections = Vec::with_capacity(count);
        let mut at = 0x20;
        for _ in 0..count {
            let magic: [u8; 4] = data.get(at..at + 4).context("MSBT cut short")?.try_into()?;
            let size = u32_at(data, at + 4)? as usize;
            let body = data.get(at + 0x10..at + 0x10 + size).context("MSBT section cut short")?.to_vec();
            sections.push((magic, body));
            at = (at + 0x10 + size).next_multiple_of(0x10);
        }
        let mut labels = Vec::new();
        if let Some((_, lbl)) = sections.iter().find(|(m, _)| m == b"LBL1") {
            for b in 0..u32_at(lbl, 0)? as usize {
                let (n, mut o) = (u32_at(lbl, 4 + b * 8)?, u32_at(lbl, 8 + b * 8)? as usize);
                for _ in 0..n {
                    let len = *lbl.get(o).context("MSBT label cut short")? as usize;
                    let name = String::from_utf8_lossy(lbl.get(o + 1..o + 1 + len).context("MSBT label cut short")?).into_owned();
                    labels.push((name, u32_at(lbl, o + 1 + len)? as usize));
                    o += 1 + len + 4;
                }
            }
        }
        let txt = &sections.iter().find(|(m, _)| m == b"TXT2").context("MSBT has no TXT2")?.1;
        let n = u32_at(txt, 0)? as usize;
        let mut texts = Vec::with_capacity(n);
        for i in 0..n {
            let start = u32_at(txt, 4 + i * 4)? as usize;
            let end = if i + 1 < n { u32_at(txt, 8 + i * 4)? as usize } else { txt.len() };
            texts.push(txt.get(start..end).context("MSBT text out of range")?.to_vec());
        }
        labels.sort_by_key(|(_, i)| *i);
        Ok(Self { header: data[..0x20].to_vec(), sections, labels, texts, added: false })
    }

    pub fn labels(&self) -> impl Iterator<Item = &str> {
        self.labels.iter().map(|(l, _)| l.as_str())
    }

    pub fn text(&self, label: &str) -> Option<String> {
        let (_, i) = self.labels.iter().find(|(l, _)| l == label)?;
        let units: Vec<u16> = self.texts.get(*i)?.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        Some(String::from_utf16_lossy(&units).trim_end_matches('\0').to_string())
    }

    pub fn set_text(&mut self, label: &str, text: &str) -> Result<()> {
        let (_, i) = self.labels.iter().find(|(l, _)| l == label).with_context(|| format!("no label {label}"))?;
        self.texts[*i] = encode(text);
        Ok(())
    }

    pub fn add_text(&mut self, label: &str, text: &str) -> Result<()> {
        if self.labels.iter().any(|(l, _)| l == label) {
            return self.set_text(label, text);
        }
        ensure!(!label.is_empty() && label.len() < 256, "label {label} must be 1 to 255 bytes");
        ensure!(self.sections.iter().any(|(m, _)| m == b"LBL1"), "this MSBT has no labels");
        if let Some((m, _)) = self.sections.iter().find(|(m, _)| !matches!(m, b"LBL1" | b"TXT2")) {
            anyhow::bail!("cannot add label {label}: the MSBT has a {} section", String::from_utf8_lossy(m));
        }
        self.labels.push((label.to_string(), self.texts.len()));
        self.texts.push(encode(text));
        self.added = true;
        Ok(())
    }

    fn label_table(&self, old: &[u8]) -> Vec<u8> {
        let buckets = u32_at(old, 0).unwrap_or(1).max(1) as usize;
        let mut slots: Vec<Vec<&(String, usize)>> = vec![Vec::new(); buckets];
        for l in &self.labels {
            let h = l.0.bytes().fold(0u32, |h, c| h.wrapping_mul(0x492).wrapping_add(c as u32));
            slots[h as usize % buckets].push(l);
        }
        let mut head = (buckets as u32).to_le_bytes().to_vec();
        let mut names = Vec::new();
        let mut at = 4 + 8 * buckets;
        for s in &slots {
            head.extend_from_slice(&(s.len() as u32).to_le_bytes());
            head.extend_from_slice(&(at as u32).to_le_bytes());
            for (name, i) in s {
                names.push(name.len() as u8);
                names.extend_from_slice(name.as_bytes());
                names.extend_from_slice(&(*i as u32).to_le_bytes());
                at += 1 + name.len() + 4;
            }
        }
        head.extend_from_slice(&names);
        head
    }

    pub fn build(&self) -> Vec<u8> {
        let mut out = self.header.clone();
        for (magic, body) in &self.sections {
            let body = if magic == b"TXT2" {
                let mut t = (self.texts.len() as u32).to_le_bytes().to_vec();
                let mut at = 4 + 4 * self.texts.len();
                for s in &self.texts {
                    t.extend_from_slice(&(at as u32).to_le_bytes());
                    at += s.len();
                }
                self.texts.iter().for_each(|s| t.extend_from_slice(s));
                t
            } else if magic == b"LBL1" && self.added {
                self.label_table(body)
            } else {
                body.clone()
            };
            out.extend_from_slice(magic);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&body);
            out.resize(out.len().next_multiple_of(0x10), 0xAB);
        }
        let size = out.len() as u32;
        out[0x12..0x16].copy_from_slice(&size.to_le_bytes());
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Vec<u8> {
        let mut lbl = 1u32.to_le_bytes().to_vec();
        lbl.extend_from_slice(&2u32.to_le_bytes());
        lbl.extend_from_slice(&12u32.to_le_bytes());
        for (name, i) in [("B_Name", 1u32), ("A_Name", 0)] {
            lbl.push(name.len() as u8);
            lbl.extend_from_slice(name.as_bytes());
            lbl.extend_from_slice(&i.to_le_bytes());
        }
        let strings: Vec<Vec<u8>> = ["Mario", "Luigi"].iter().map(|s| s.encode_utf16().chain([0]).flat_map(u16::to_le_bytes).collect()).collect();
        let mut txt = 2u32.to_le_bytes().to_vec();
        txt.extend_from_slice(&12u32.to_le_bytes());
        txt.extend_from_slice(&(12 + strings[0].len() as u32).to_le_bytes());
        strings.iter().for_each(|s| txt.extend_from_slice(s));
        let mut out = b"MsgStdBn\xFF\xFE\0\0\x01\x03\x02\0\0\0".to_vec();
        out.resize(0x20, 0);
        for (magic, body) in [(b"LBL1", lbl), (b"TXT2", txt)] {
            out.extend_from_slice(magic);
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&body);
            out.resize(out.len().next_multiple_of(0x10), 0xAB);
        }
        let size = out.len() as u32;
        out[0x12..0x16].copy_from_slice(&size.to_le_bytes());
        out
    }

    #[test]
    fn round_trip_is_identical() {
        let data = sample();
        let m = Msbt::parse(&data).unwrap();
        assert_eq!(m.text("A_Name").as_deref(), Some("Mario"));
        assert_eq!(m.text("B_Name").as_deref(), Some("Luigi"));
        assert_eq!(m.build(), data);
    }

    #[test]
    fn set_text_moves_the_following_strings() {
        let mut m = Msbt::parse(&sample()).unwrap();
        m.set_text("A_Name", "Peter Griffin").unwrap();
        let m = Msbt::parse(&m.build()).unwrap();
        assert_eq!(m.text("A_Name").as_deref(), Some("Peter Griffin"));
        assert_eq!(m.text("B_Name").as_deref(), Some("Luigi"));
        assert!(Msbt::parse(&sample()).unwrap().set_text("C", "x").is_err());
    }

    #[test]
    fn add_text_adds_a_label() {
        let mut m = Msbt::parse(&sample()).unwrap();
        m.add_text("1470", "GPDX").unwrap();
        m.add_text("A_Name", "Toad").unwrap();
        let m = Msbt::parse(&m.build()).unwrap();
        assert_eq!(m.text("1470").as_deref(), Some("GPDX"));
        assert_eq!(m.text("A_Name").as_deref(), Some("Toad"));
        assert_eq!(m.text("B_Name").as_deref(), Some("Luigi"));
    }
}
