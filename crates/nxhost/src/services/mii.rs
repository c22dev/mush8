// Mush8 - mii.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub, write_buffer};
use crate::services::account::nickname;
use std::path::Path;
use std::sync::{Arc, OnceLock};

const CHAR_INFO_SIZE: usize = 0x58;
const STORE_DATA_SIZE: usize = 0x44;
const NFDB_SIZE: usize = 0x1A98;
const NFDB_MAX: usize = 100;
pub const DIR: &str = "miis";
const DEFAULT_COUNT: usize = 6;
const SOURCE_DATABASE: u32 = 1;
const SOURCE_DEFAULT: u32 = 2;
const ELEMENT_DATABASE: u32 = 0;
const ELEMENT_DEFAULT: u32 = 1;
const NOT_FOUND: u32 = 126 | 1 << 9;
const NAME_UNITS: usize = 10;

pub static VERIFY: OnceLock<u64> = OnceLock::new();

mod f {
    pub const NAME: usize = 0x10;
    pub const FAVORITE_COLOR: usize = 0x27;
    pub const GENDER: usize = 0x28;
    pub const HEIGHT: usize = 0x29;
    pub const BUILD: usize = 0x2A;
    pub const FACELINE: usize = 0x2D;
    pub const HAIR: usize = 0x31;
    pub const EYE: usize = 0x34;
    pub const EYEBROW: usize = 0x3B;
    pub const NOSE: usize = 0x42;
    pub const MOUTH: usize = 0x45;
    pub const BEARD: usize = 0x4A;
    pub const GLASSES: usize = 0x4F;
    pub const MOLE: usize = 0x53;
}

fn default_mii(index: usize) -> [u8; CHAR_INFO_SIZE] {
    let mut c = [0u8; CHAR_INFO_SIZE];
    let mut id = *b"mk8dx-default-0\0";
    id[14] = b'0' + index as u8;
    id[6] = (id[6] & 0x0F) | 0x40;
    id[8] = (id[8] & 0x3F) | 0x80;
    c[..16].copy_from_slice(&id);
    for (i, ch) in "Mii".encode_utf16().enumerate() {
        c[f::NAME + 2 * i..f::NAME + 2 * i + 2].copy_from_slice(&ch.to_le_bytes());
    }
    let gender = (index % 2) as u8;
    c[f::FAVORITE_COLOR] = (index * 2 % 12) as u8;
    c[f::GENDER] = gender;
    c[f::HEIGHT] = 64;
    c[f::BUILD] = 64;
    c[f::FACELINE..f::FACELINE + 4].copy_from_slice(&[0, 0, 0, 0]);
    let hair = if gender == 0 { 33 } else { 12 };
    c[f::HAIR..f::HAIR + 3].copy_from_slice(&[hair, 1, 0]);
    c[f::EYE..f::EYE + 7].copy_from_slice(&[if gender == 0 { 2 } else { 4 }, 8, 4, 3, 4, 2, 12]);
    c[f::EYEBROW..f::EYEBROW + 7].copy_from_slice(&[if gender == 0 { 6 } else { 0 }, 1, 4, 3, 6, 2, 10]);
    c[f::NOSE..f::NOSE + 3].copy_from_slice(&[1, 4, 9]);
    c[f::MOUTH..f::MOUTH + 5].copy_from_slice(&[if gender == 0 { 23 } else { 1 }, 19, 4, 3, 13]);
    c[f::BEARD..f::BEARD + 5].copy_from_slice(&[1, 0, 0, 4, 10]);
    c[f::GLASSES..f::GLASSES + 4].copy_from_slice(&[0, 8, 4, 10]);
    c[f::MOLE..f::MOLE + 4].copy_from_slice(&[0, 4, 2, 20]);
    c
}

fn verify(c: &[u8; CHAR_INFO_SIZE]) -> u32 {
    let Some(&v) = VERIFY.get() else { return 0 };
    let f: extern "C" fn(*const u8) -> u32 = unsafe { std::mem::transmute(v as usize) };
    f(c.as_ptr())
}

fn bits(w: &[u32; 7], word: usize, off: u32, len: u32) -> u8 {
    (w[word] >> off & ((1 << len) - 1)) as u8
}

fn from_store_data(s: &[u8]) -> [u8; CHAR_INFO_SIZE] {
    let w: [u32; 7] = std::array::from_fn(|i| u32::from_le_bytes(s[i * 4..i * 4 + 4].try_into().unwrap()));
    let b = |word, off, len| bits(&w, word, off, len);
    let mut c = [0u8; CHAR_INFO_SIZE];
    c[..16].copy_from_slice(&s[0x30..0x40]);
    c[f::NAME..f::NAME + 0x14].copy_from_slice(&s[0x1C..0x30]);
    c[0x26] = b(2, 22, 2);
    c[f::FAVORITE_COLOR] = b(5, 8, 4);
    c[f::GENDER] = b(1, 7, 1);
    c[f::HEIGHT] = b(0, 8, 7);
    c[f::BUILD] = b(0, 16, 7);
    c[0x2B] = b(0, 31, 1);
    c[0x2C] = b(2, 14, 2);
    c[f::FACELINE..f::FACELINE + 4].copy_from_slice(&[b(5, 12, 4), b(5, 16, 4), b(5, 20, 4), b(5, 24, 4)]);
    c[f::HAIR..f::HAIR + 3].copy_from_slice(&[b(0, 0, 8), b(0, 24, 7), b(0, 23, 1)]);
    c[f::EYE..f::EYE + 7].copy_from_slice(&[b(2, 8, 6), b(1, 0, 7), b(4, 21, 3), b(4, 13, 3), b(4, 5, 3), b(5, 28, 4), b(2, 24, 5)]);
    c[f::EYEBROW..f::EYEBROW + 7].copy_from_slice(&[b(3, 0, 5), b(1, 8, 7), b(6, 0, 4), b(3, 29, 3), b(6, 4, 4), b(6, 8, 4), b(6, 12, 4)]);
    c[f::NOSE..f::NOSE + 3].copy_from_slice(&[b(3, 8, 5), b(6, 16, 4), b(3, 16, 5)]);
    c[f::MOUTH..f::MOUTH + 5].copy_from_slice(&[b(2, 16, 6), b(1, 16, 7), b(6, 20, 4), b(3, 21, 3), b(3, 24, 5)]);
    c[f::BEARD..f::BEARD + 5].copy_from_slice(&[b(1, 24, 7), b(3, 13, 3), b(3, 5, 3), b(6, 24, 4), b(4, 0, 5)]);
    c[f::GLASSES..f::GLASSES + 4].copy_from_slice(&[b(5, 0, 5), b(2, 0, 7), b(2, 29, 3), b(4, 8, 5)]);
    c[f::MOLE..f::MOLE + 4].copy_from_slice(&[b(0, 15, 1), b(6, 28, 4), b(4, 16, 5), b(4, 24, 5)]);
    c
}

fn parse(data: &[u8]) -> Vec<[u8; CHAR_INFO_SIZE]> {
    if data.len() >= NFDB_SIZE && data.starts_with(b"NFDB") {
        let count = (data[NFDB_SIZE - 3] as usize).min(NFDB_MAX);
        return data[4..4 + count * STORE_DATA_SIZE].chunks_exact(STORE_DATA_SIZE).map(from_store_data).collect();
    }
    match data.len() {
        CHAR_INFO_SIZE => vec![data.try_into().unwrap()],
        STORE_DATA_SIZE => vec![from_store_data(data)],
        _ => Vec::new(),
    }
}

fn with_valid_id(mut c: [u8; CHAR_INFO_SIZE]) -> [u8; CHAR_INFO_SIZE] {
    if c[6] & 0xF0 == 0x40 && c[8] & 0xC0 == 0x80 {
        return c;
    }
    if c[..16].iter().all(|&b| b == 0) {
        let mut h: u64 = 0xcbf29ce484222325;
        for &b in &c[16..] {
            h = (h ^ b as u64).wrapping_mul(0x100000001b3);
        }
        c[..8].copy_from_slice(&h.to_le_bytes());
        c[8..16].copy_from_slice(&h.rotate_left(32).to_le_bytes());
    }
    c[6] = (c[6] & 0x0F) | 0x40;
    c[8] = (c[8] & 0x3F) | 0x80;
    c
}

fn load(dir: &Path) -> Vec<[u8; CHAR_INFO_SIZE]> {
    let mut files: Vec<_> = std::fs::read_dir(dir).into_iter().flatten().flatten().map(|e| e.path()).filter(|p| p.is_file()).collect();
    files.sort_by_key(|p| (!p.file_name().is_some_and(|n| n.eq_ignore_ascii_case("MiiDatabase.dat")), p.clone()));
    let mut out: Vec<[u8; CHAR_INFO_SIZE]> = Vec::new();
    for p in files {
        let Ok(data) = std::fs::read(&p) else { continue };
        let miis = parse(&data);
        if miis.is_empty() {
            eprintln!("[mii] {}: not a MiiDatabase.dat, CharInfo or StoreData file", p.display());
        }
        for c in miis.into_iter().map(with_valid_id) {
            let r = verify(&c);
            if r != 0 {
                eprintln!("[mii] {}: Mii rejected by CharInfoRaw::Verify: code {r:#x}", p.display());
            } else if out.len() < NFDB_MAX && !out.iter().any(|o| o[..16] == c[..16]) {
                out.push(c);
            }
        }
    }
    if !out.is_empty() {
        eprintln!("[mii] {} Miis loaded from {}", out.len(), dir.display());
    }
    out
}

fn database() -> &'static Vec<[u8; CHAR_INFO_SIZE]> {
    static D: OnceLock<Vec<[u8; CHAR_INFO_SIZE]>> = OnceLock::new();
    D.get_or_init(|| load(Path::new(DIR)))
}

fn defaults() -> &'static Vec<[u8; CHAR_INFO_SIZE]> {
    static D: OnceLock<Vec<[u8; CHAR_INFO_SIZE]>> = OnceLock::new();
    D.get_or_init(|| {
        let v: Vec<_> = (0..DEFAULT_COUNT).map(default_mii).collect();
        for (i, c) in v.iter().enumerate() {
            let r = verify(c);
            if r != 0 {
                eprintln!("[mii] default Mii {i} rejected by CharInfoRaw::Verify: code {r:#x}");
            }
        }
        v
    })
}

fn renamed(c: &[u8; CHAR_INFO_SIZE], name: &str) -> [u8; CHAR_INFO_SIZE] {
    let mut c = *c;
    let mut units = Vec::new();
    for ch in name.chars() {
        let mut buf = [0u16; 2];
        let encoded = ch.encode_utf16(&mut buf);
        if units.len() + encoded.len() > NAME_UNITS {
            break;
        }
        units.extend_from_slice(encoded);
    }
    if units.is_empty() {
        return c;
    }
    c[f::NAME..f::NAME + 2 * (NAME_UNITS + 1)].fill(0);
    for (i, u) in units.iter().enumerate() {
        c[f::NAME + 2 * i..f::NAME + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
    }
    c
}

pub struct MiiService;

impl Service for MiiService {
    fn name(&self) -> String {
        "mii".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.objects.push(Arc::new(Database)),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct Database;

impl Database {
    fn stored() -> &'static [[u8; CHAR_INFO_SIZE]] {
        if database().is_empty() { defaults() } else { database() }
    }

    fn listed(flag: u32) -> Vec<(&'static [u8; CHAR_INFO_SIZE], u32)> {
        let mut v = Vec::new();
        let from_database = flag & SOURCE_DATABASE != 0;
        if from_database {
            v.extend(Self::stored().iter().map(|c| (c, ELEMENT_DATABASE)));
        }
        if flag & SOURCE_DEFAULT != 0 && !(from_database && database().is_empty()) {
            v.extend(defaults().iter().map(|c| (c, ELEMENT_DEFAULT)));
        }
        v
    }

    fn find(id: &[u8]) -> Option<usize> {
        Self::stored().iter().position(|c| id.len() >= 16 && c[..16] == id[..16])
    }
}

impl Service for Database {
    fn name(&self) -> String {
        "mii::IDatabaseService".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 | 1 => rsp.data.push(0),
            2 => rsp.push_u32(Self::listed(req.u32(0)).len() as u32),
            3 | 4 => {
                let list = Self::listed(req.u32(0));
                let name = nickname();
                let mut out = Vec::new();
                for &(c, source) in &list {
                    out.extend_from_slice(&renamed(c, &name));
                    if req.cmd == 3 {
                        out.extend_from_slice(&source.to_le_bytes());
                    }
                }
                let n = req.out_buffer().map_or(0, |b| write_buffer(&b, &out)) / if req.cmd == 3 { 0x5C } else { CHAR_INFO_SIZE };
                rsp.push_u32(n.min(list.len()) as u32);
            }
            6 => {
                let gender = req.u32(4);
                let c = &defaults()[if gender == 1 { 1 } else { 0 }];
                rsp.data.extend_from_slice(&renamed(c, &nickname()));
            }
            7 => {
                let i = (req.u32(0) as usize).min(DEFAULT_COUNT - 1);
                rsp.data.extend_from_slice(&renamed(&defaults()[i], &nickname()));
            }
            5 => match Self::find(&req.data) {
                Some(i) if req.u32(CHAR_INFO_SIZE) & SOURCE_DATABASE != 0 => rsp.data.extend_from_slice(&renamed(&Self::stored()[i], &nickname())),
                _ => rsp.data.extend_from_slice(&req.data[..CHAR_INFO_SIZE.min(req.data.len())]),
            },
            11 => rsp.push_u32(Self::find(&req.data).map_or(-1, |i| i as i32) as u32),
            21 => match Self::find(&req.data) {
                Some(i) => rsp.push_u32(i as u32),
                None => return NOT_FOUND,
            },
            22 => {}
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_database_falls_back_to_the_defaults_once() {
        assert!(database().is_empty());
        let listed = Database::listed(SOURCE_DATABASE);
        assert_eq!(listed.len(), DEFAULT_COUNT);
        assert!(listed.iter().all(|&(_, s)| s == ELEMENT_DATABASE));
        assert!(Database::listed(SOURCE_DEFAULT).iter().all(|&(_, s)| s == ELEMENT_DEFAULT));
        assert_eq!(Database::listed(SOURCE_DATABASE | SOURCE_DEFAULT), listed);
        assert!(Database::listed(0).is_empty());
        let ids: std::collections::HashSet<_> = listed.iter().map(|(c, _)| &c[..16]).collect();
        assert_eq!(ids.len(), DEFAULT_COUNT);
    }

    fn name_of(c: &[u8; CHAR_INFO_SIZE]) -> String {
        let units: Vec<u16> = c[f::NAME..f::NAME + 2 * NAME_UNITS].chunks(2).map(|b| u16::from_le_bytes([b[0], b[1]])).take_while(|&u| u != 0).collect();
        String::from_utf16(&units).unwrap()
    }

    #[test]
    fn player_name_replaces_the_mii_name() {
        let mut mii = defaults()[0];
        for (i, ch) in "Mii Studio".encode_utf16().enumerate() {
            mii[f::NAME + 2 * i..f::NAME + 2 * i + 2].copy_from_slice(&ch.to_le_bytes());
        }
        let short = renamed(&mii, "c22");
        assert_eq!(name_of(&short), "c22");
        assert_eq!(short[..f::NAME], mii[..f::NAME]);
        assert_eq!(short[f::NAME + 2 * (NAME_UNITS + 1)..], mii[f::NAME + 2 * (NAME_UNITS + 1)..]);
        assert_eq!(name_of(&renamed(&mii, "abcdefghijkl")), "abcdefghij");
        assert_eq!(name_of(&renamed(&mii, "abcdefghi🇨🇭")), "abcdefghi");
        assert_eq!(name_of(&renamed(&mii, "")), "Mii Studio");
    }

    #[test]
    fn listed_miis_can_be_found_by_id() {
        for (i, (c, _)) in Database::listed(SOURCE_DATABASE).iter().enumerate() {
            assert_eq!(Database::find(&c[..]), Some(i));
        }
        assert_eq!(Database::find(&[0xFF; CHAR_INFO_SIZE]), None);
        assert_eq!(Database::find(&[]), None);
    }

    fn store_data(id: u8, words: [u32; 7]) -> [u8; STORE_DATA_SIZE] {
        let mut s = [0u8; STORE_DATA_SIZE];
        for (i, w) in words.iter().enumerate() {
            s[i * 4..i * 4 + 4].copy_from_slice(&w.to_le_bytes());
        }
        for (i, ch) in "Lara".encode_utf16().enumerate() {
            s[0x1C + 2 * i..0x1E + 2 * i].copy_from_slice(&ch.to_le_bytes());
        }
        s[0x30] = id;
        s
    }

    #[test]
    fn store_data_fields() {
        let w0 = 33 | 64 << 8 | 1 << 15 | 70 << 16 | 1 << 23 | 5 << 24;
        let w1 = 8 | 1 << 7 | 3 << 8 | 19 << 16 | 2 << 24;
        let w2 = 4 | 2 << 8 | 23 << 16 | 12 << 24 | 6 << 29;
        let w6 = 4 | 6 << 4 | 2 << 8 | 10 << 12 | 4 << 16 | 5 << 20 | 6 << 24 | 7 << 28;
        let c = from_store_data(&store_data(9, [w0, w1, w2, 0, 0, 3 << 8, w6]));
        assert_eq!(c[0], 9);
        assert_eq!(&c[f::NAME..f::NAME + 4], &[b'L', 0, b'a', 0]);
        assert_eq!((c[f::GENDER], c[f::HEIGHT], c[f::BUILD], c[f::FAVORITE_COLOR]), (1, 64, 70, 3));
        assert_eq!(&c[f::HAIR..f::HAIR + 3], &[33, 5, 1]);
        assert_eq!(&c[f::EYE..f::EYE + 2], &[2, 8]);
        assert_eq!(c[f::EYE + 6], 12);
        assert_eq!(&c[f::MOUTH..f::MOUTH + 3], &[23, 19, 5]);
        assert_eq!((c[f::GLASSES + 1], c[f::GLASSES + 2]), (4, 6));
        assert_eq!((c[f::MOLE], c[f::MOLE + 1]), (1, 7));
    }

    #[test]
    fn nfdb() {
        let mut db = vec![0u8; NFDB_SIZE];
        db[..4].copy_from_slice(b"NFDB");
        for i in 0..3 {
            db[4 + i * STORE_DATA_SIZE..4 + (i + 1) * STORE_DATA_SIZE].copy_from_slice(&store_data(i as u8 + 1, [0; 7]));
        }
        db[NFDB_SIZE - 4] = 1;
        db[NFDB_SIZE - 3] = 2;
        let miis = parse(&db);
        assert_eq!(miis.iter().map(|c| c[0]).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(parse(&[0u8; CHAR_INFO_SIZE]).len(), 1);
        assert!(parse(&[0u8; 0x20]).is_empty());
    }

    #[test]
    fn invalid_create_ids_are_repaired_deterministically() {
        let mut c = defaults()[0];
        c[..16].copy_from_slice(&[0x80, 0xdf, 0x19, 0x9a, 0x95, 0x0c, 0x7b, 0xb9, 0x36, 0x14, 0, 0, 0, 0, 0, 0]);
        let fixed = with_valid_id(c);
        assert_eq!((fixed[6], fixed[8]), (0x4b, 0xb6));
        assert_eq!(fixed[16..], c[16..]);
        assert_eq!(with_valid_id(defaults()[1]), defaults()[1]);
        c[..16].fill(0);
        let a = with_valid_id(c);
        assert!(a[..16].iter().any(|&b| b != 0));
        assert_eq!(a, with_valid_id(c));
        assert_eq!((a[6] & 0xF0, a[8] & 0xC0), (0x40, 0x80));
    }
}
