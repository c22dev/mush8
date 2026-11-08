// Mush8 - prepo.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub};
use crate::services::bsd::bytes;
use crate::services::nexweb;

pub struct Prepo(pub String);

impl Service for Prepo {
    fn name(&self) -> String {
        self.0.clone()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            10100..=10107 => {
                let report = bytes(req.a.first().copied().filter(|b| b.addr != 0 && b.size != 0));
                if let Some(detail) = read_map(report).and_then(|m| mk8_detail(&m)) {
                    nexweb::set_detail(&detail);
                }
                0
            }
            _ => Stub(self.0.clone()).call(req, rsp),
        }
    }
}

struct Reader<'a> {
    b: &'a [u8],
    i: usize,
}

impl Reader<'_> {
    fn take(&mut self, n: usize) -> Option<&[u8]> {
        let s = self.b.get(self.i..self.i.checked_add(n)?)?;
        self.i += n;
        Some(s)
    }
    fn uint(&mut self, n: usize) -> Option<usize> {
        Some(self.take(n)?.iter().fold(0usize, |v, &b| v << 8 | b as usize))
    }
    fn string(&mut self) -> Option<Option<String>> {
        let tag = *self.b.get(self.i)?;
        let len = match tag {
            0xa0..=0xbf => {
                self.i += 1;
                (tag & 0x1f) as usize
            }
            0xd9 | 0xda | 0xdb => {
                self.i += 1;
                self.uint(1 << (tag - 0xd9))?
            }
            _ => {
                self.skip(0)?;
                return Some(None);
            }
        };
        Some(Some(String::from_utf8_lossy(self.take(len)?).into_owned()))
    }
    fn skip(&mut self, depth: usize) -> Option<()> {
        if depth > 32 {
            return None;
        }
        let tag = *self.b.get(self.i)?;
        self.i += 1;
        let (fixed, items) = match tag {
            0x00..=0x7f | 0xe0..=0xff | 0xc0 | 0xc2 | 0xc3 => (0, 0),
            0xcc | 0xd0 => (1, 0),
            0xcd | 0xd1 => (2, 0),
            0xce | 0xd2 | 0xca => (4, 0),
            0xcf | 0xd3 | 0xcb => (8, 0),
            0xd4 => (2, 0),
            0xd5 => (3, 0),
            0xd6 => (5, 0),
            0xd7 => (9, 0),
            0xd8 => (17, 0),
            0xa0..=0xbf => ((tag & 0x1f) as usize, 0),
            0xd9 | 0xc4 => (self.uint(1)?, 0),
            0xda | 0xc5 => (self.uint(2)?, 0),
            0xdb | 0xc6 => (self.uint(4)?, 0),
            0xc7 => (self.uint(1)? + 1, 0),
            0xc8 => (self.uint(2)? + 1, 0),
            0xc9 => (self.uint(4)? + 1, 0),
            0x90..=0x9f => (0, (tag & 0x0f) as usize),
            0xdc => (0, self.uint(2)?),
            0xdd => (0, self.uint(4)?),
            0x80..=0x8f => (0, (tag & 0x0f) as usize * 2),
            0xde => (0, self.uint(2)? * 2),
            0xdf => (0, self.uint(4)? * 2),
            0xc1 => return None,
        };
        self.take(fixed)?;
        for _ in 0..items {
            self.skip(depth + 1)?;
        }
        Some(())
    }
}

pub fn read_map(b: &[u8]) -> Option<Vec<(String, String)>> {
    let mut r = Reader { b, i: 0 };
    let tag = *b.first()?;
    r.i = 1;
    let n = match tag {
        0x80..=0x8f => (tag & 0x0f) as usize,
        0xde => r.uint(2)?,
        0xdf => r.uint(4)?,
        _ => return None,
    };
    let mut out = Vec::new();
    for _ in 0..n {
        let key = r.string()?;
        let value = r.string()?;
        if let (Some(k), Some(v)) = (key, value) {
            out.push((k, v));
        }
    }
    Some(out)
}

fn screen(to: &str) -> Option<&'static str> {
    Some(match to {
        "Single" => "Single Player",
        "Multi-2players" => "Multiplayer: 2 Players",
        "Multi-3players" => "Multiplayer: 3 Players",
        "Multi-4players" => "Multiplayer: 4 Players",
        "Local-Single" => "Wireless/LAN Play",
        "Local-2players" => "Wireless/LAN Play 2 Players",
        "Wifi-Single" => "Online",
        "Wifi-2players" => "Online: 2 Players",
        "Anybody-World" => "Worldwide",
        "Anybody-Area" => "Regional",
        "Friends" => "Friends' rooms",
        "Tournament" => "Tournament",
        "Roulette" => "Choosing a course",
        "50cc" => "50cc",
        "100cc" => "100cc",
        "150cc" => "150cc",
        "Mirror" => "Mirror (150cc)",
        "200cc" => "200cc",
        "GrandPrix" => "Grand Prix",
        "TimeAttack" => "Time Trials",
        "VS" => "VS Races",
        "Battle" => "Battle Mode",
        "RaceStart" => "Selecting a Course",
        "Race" => "Racing",
        _ => return None,
    })
}

pub fn mk8_detail(report: &[(String, String)]) -> Option<String> {
    let get = |k: &str| report.iter().find(|(key, _)| key == k).map(|(_, v)| v.as_str()).filter(|v| !v.is_empty());
    let mut parts = Vec::new();
    if let Some(mode) = get("Mode") {
        parts.push(if mode == "Internet" { "Online" } else { "Offline" });
    }
    parts.extend(get("Rule"));
    parts.extend(get("Engine"));
    if !parts.is_empty() {
        return Some(parts.join(" · "));
    }
    get("To").and_then(screen).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pack(pairs: &[(&str, &str)]) -> Vec<u8> {
        let mut out = vec![0x80 | pairs.len() as u8];
        for (k, v) in pairs {
            for s in [k, v] {
                out.push(0xa0 | s.len() as u8);
                out.extend_from_slice(s.as_bytes());
            }
        }
        out
    }

    #[test]
    fn screens_map_to_details() {
        let m = read_map(&pack(&[("To", "Anybody-World"), ("From", "Wifi")])).unwrap();
        assert_eq!(mk8_detail(&m).as_deref(), Some("Worldwide"));
        let m = read_map(&pack(&[("To", "SomethingNew")])).unwrap();
        assert_eq!(mk8_detail(&m), None);
    }

    #[test]
    fn race_end_wins_over_screens() {
        let m = read_map(&pack(&[("Mode", "Internet"), ("Rule", "VS"), ("Engine", "100cc"), ("Course", "Gu_FirstCircuit"), ("InternetMode", "Anybody-World")])).unwrap();
        assert_eq!(mk8_detail(&m).as_deref(), Some("Online · VS · 100cc"));
        let m = read_map(&pack(&[("Mode", "Local"), ("Rule", "Battle")])).unwrap();
        assert_eq!(mk8_detail(&m).as_deref(), Some("Offline · Battle"));
    }

    #[test]
    fn other_value_types_are_skipped() {
        let mut b = vec![0x84];
        b.extend_from_slice(b"\xa5Count\xcd\x01\x00");
        b.extend_from_slice(b"\xa4List\x92\x01\xa1x");
        b.extend_from_slice(b"\xa3Map\x81\xa1k\xc3");
        b.extend_from_slice(b"\xa2To\xd9\x04Race");
        assert_eq!(read_map(&b).unwrap(), vec![("To".to_string(), "Race".to_string())]);
        assert_eq!(mk8_detail(&read_map(&b).unwrap()).as_deref(), Some("Racing"));
    }

    #[test]
    fn broken_reports_are_ignored() {
        assert!(read_map(&[]).is_none());
        assert!(read_map(&[0x81, 0xa2, b'T']).is_none());
        assert!(read_map(&[0x91, 0x01]).is_none());
    }
}
