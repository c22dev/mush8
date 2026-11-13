// Mush8 - romfs.rs
// Constantin Clerc (c22dev) - 0.1
use crate::storage::{DynStorage, Mem, Patched, Spliced};
use std::collections::HashMap;
use anyhow::{Context, Result, ensure};
use std::sync::Arc;

const NONE: u32 = u32::MAX;

pub struct File {
    pub path: String,
    pub offset: u64,
    pub size: u64,
    entry: usize,
}

pub struct RomFs {
    storage: DynStorage,
    data_offset: u64,
    files_offset: u64,
    dirs: Vec<u8>,
    files: Vec<u8>,
}

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn u64_at(b: &[u8], o: usize) -> u64 {
    u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
}

impl RomFs {
    pub fn open(storage: DynStorage) -> Result<Self> {
        let h = storage.read_vec(0, 0x50)?;
        ensure!(u64_at(&h, 0) == 0x50, "bad RomFS header size");
        let dirs = storage.read_vec(u64_at(&h, 0x18), u64_at(&h, 0x20) as usize)?;
        let files = storage.read_vec(u64_at(&h, 0x38), u64_at(&h, 0x40) as usize)?;
        Ok(Self { storage, data_offset: u64_at(&h, 0x48), files_offset: u64_at(&h, 0x38), dirs, files })
    }

    pub fn storage(&self) -> &DynStorage {
        &self.storage
    }

    pub fn files(&self) -> Vec<File> {
        let mut out = Vec::new();
        self.walk(0, String::new(), &mut out);
        out
    }

    pub fn with_files(&self, replacements: Vec<(String, Vec<u8>)>) -> Result<DynStorage> {
        let files = self.files();
        let mut table = self.files.clone();
        let mut end = self.storage.size();
        let mut patches = Vec::new();
        for (path, data) in replacements {
            let f = files.iter().find(|f| f.path == path).with_context(|| format!("{path} is not in the RomFS"))?;
            end = end.next_multiple_of(0x10);
            table[f.entry + 8..f.entry + 0x10].copy_from_slice(&(end - self.data_offset).to_le_bytes());
            table[f.entry + 0x10..f.entry + 0x18].copy_from_slice(&(data.len() as u64).to_le_bytes());
            let len = data.len() as u64;
            patches.push((end, Arc::<[u8]>::from(data)));
            end += len;
        }
        patches.push((self.files_offset, table.into()));
        Ok(Arc::new(Patched::new(self.storage.clone(), end, patches)))
    }

    pub fn file_table(&self) -> (u64, Vec<u8>) {
        (self.files_offset, self.files.clone())
    }

    pub fn set_entry(&self, table: &mut [u8], f: &File, offset: u64, size: u64) {
        table[f.entry + 8..f.entry + 0x10].copy_from_slice(&(offset - self.data_offset).to_le_bytes());
        table[f.entry + 0x10..f.entry + 0x18].copy_from_slice(&size.to_le_bytes());
    }

    pub fn dirs(&self) -> Vec<String> {
        let mut out = Vec::new();
        self.walk_dirs(0, String::new(), &mut out);
        out
    }

    fn walk_dirs(&self, dir: u32, path: String, out: &mut Vec<String>) {
        let mut c = u32_at(&self.dirs[dir as usize..], 8);
        while c != NONE {
            let e = &self.dirs[c as usize..];
            let len = u32_at(e, 0x14) as usize;
            let child = format!("{path}/{}", String::from_utf8_lossy(&e[0x18..0x18 + len]));
            out.push(child.clone());
            self.walk_dirs(c, child, out);
            c = u32_at(e, 4);
        }
    }

    pub fn find(&self, path: &str) -> Option<File> {
        let header = self.storage.read_vec(0, 0x50).ok()?;
        let dir_hash = self.storage.read_vec(u64_at(&header, 0x08), u64_at(&header, 0x10) as usize).ok()?;
        let file_hash = self.storage.read_vec(u64_at(&header, 0x28), u64_at(&header, 0x30) as usize).ok()?;
        let parts: Vec<&str> = path.trim_start_matches('/').split('/').collect();
        let (name, dirs) = parts.split_last()?;
        let mut parent = 0u32;
        for d in dirs {
            let mut e = u32_at(&dir_hash, (path_hash(parent, d) % (dir_hash.len() / 4) as u32) as usize * 4);
            loop {
                if e == NONE {
                    return None;
                }
                let x = &self.dirs[e as usize..];
                let len = u32_at(x, 0x14) as usize;
                if u32_at(x, 0) == parent && &x[0x18..0x18 + len] == d.as_bytes() {
                    break;
                }
                e = u32_at(x, 0x10);
            }
            parent = e;
        }
        let mut e = u32_at(&file_hash, (path_hash(parent, name) % (file_hash.len() / 4) as u32) as usize * 4);
        while e != NONE {
            let x = &self.files[e as usize..];
            let len = u32_at(x, 0x1C) as usize;
            if u32_at(x, 0) == parent && &x[0x20..0x20 + len] == name.as_bytes() {
                return Some(File { path: path.to_string(), offset: self.data_offset + u64_at(x, 8), size: u64_at(x, 0x10), entry: e as usize });
            }
            e = u32_at(x, 0x18);
        }
        None
    }

    pub fn with_overlay(&self, overlay: Vec<(String, DynStorage)>) -> Result<DynStorage> {
        let mut tree = Tree::new();
        for d in self.dirs() {
            tree.dir(&d);
        }
        let mut files: Vec<(String, Source)> = self.files().into_iter().map(|f| (f.path, Source::Base(f.offset - self.data_offset, f.size))).collect();
        let mut index: HashMap<String, usize> = files.iter().enumerate().map(|(i, (p, _))| (p.to_ascii_lowercase(), i)).collect();
        for (path, data) in overlay {
            match index.get(&path.to_ascii_lowercase()) {
                Some(&i) => files[i].1 = Source::Extra(data),
                None => {
                    index.insert(path.to_ascii_lowercase(), files.len());
                    files.push((path, Source::Extra(data)));
                }
            }
        }
        let mut end = self.storage.size().next_multiple_of(0x10);
        let mut parts: Vec<(u64, DynStorage)> = Vec::new();
        for (path, src) in files {
            let (offset, size) = match src {
                Source::Base(o, s) => (o, s),
                Source::Extra(data) => {
                    let size = data.size();
                    let at = end;
                    parts.push((at, data));
                    end = (at + size).next_multiple_of(0x10);
                    (at - self.data_offset, size)
                }
            };
            tree.file(&path, offset, size);
        }
        let (dir_hash, dir_meta, file_hash, file_meta) = tree.tables();
        let mut header = vec![0u8; 0x50];
        let mut put = |slot: usize, v: u64| header[slot..slot + 8].copy_from_slice(&v.to_le_bytes());
        put(0, 0x50);
        let mut at = end;
        for (i, table) in [&dir_hash, &dir_meta, &file_hash, &file_meta].into_iter().enumerate() {
            put(0x08 + i * 0x10, at);
            put(0x10 + i * 0x10, table.len() as u64);
            at = (at + table.len() as u64).next_multiple_of(0x10);
        }
        put(0x48, self.data_offset);
        let mut at = end;
        for table in [dir_hash, dir_meta, file_hash, file_meta] {
            let len = table.len() as u64;
            parts.push((at, Arc::new(Mem(table))));
            at = (at + len).next_multiple_of(0x10);
        }
        parts.push((0, Arc::new(Mem(header))));
        Ok(Arc::new(Spliced::new(self.storage.clone(), at, parts)))
    }

    fn walk(&self, dir: u32, path: String, out: &mut Vec<File>) {
        let d = &self.dirs[dir as usize..];
        let mut f = u32_at(d, 0xC);
        while f != NONE {
            let e = &self.files[f as usize..];
            let len = u32_at(e, 0x1C) as usize;
            let name = String::from_utf8_lossy(&e[0x20..0x20 + len]);
            out.push(File { path: format!("{path}/{name}"), offset: self.data_offset + u64_at(e, 8), size: u64_at(e, 0x10), entry: f as usize });
            f = u32_at(e, 4);
        }
        let mut c = u32_at(d, 8);
        while c != NONE {
            let e = &self.dirs[c as usize..];
            let len = u32_at(e, 0x14) as usize;
            let name = String::from_utf8_lossy(&e[0x18..0x18 + len]);
            self.walk(c, format!("{path}/{name}"), out);
            c = u32_at(e, 4);
        }
    }
}

enum Source {
    Base(u64, u64),
    Extra(DynStorage),
}

fn path_hash(parent: u32, name: &str) -> u32 {
    name.bytes().fold(parent ^ 123456789, |h, b| h.rotate_right(5) ^ b as u32)
}

fn bucket_count(entries: usize) -> usize {
    match entries {
        0..3 => 3,
        3..19 => entries | 1,
        _ => (entries..).find(|n| [2, 3, 5, 7, 11, 13, 17].iter().all(|p| n % p != 0)).unwrap(),
    }
}

struct TreeDir {
    name: String,
    parent: usize,
    dirs: Vec<usize>,
    files: Vec<usize>,
}

struct TreeFile {
    name: String,
    parent: usize,
    offset: u64,
    size: u64,
}

struct Tree {
    dirs: Vec<TreeDir>,
    files: Vec<TreeFile>,
    by_path: HashMap<String, usize>,
}

impl Tree {
    fn new() -> Self {
        let root = TreeDir { name: String::new(), parent: 0, dirs: Vec::new(), files: Vec::new() };
        Self { dirs: vec![root], files: Vec::new(), by_path: HashMap::from([(String::new(), 0)]) }
    }

    fn dir(&mut self, path: &str) -> usize {
        if let Some(&d) = self.by_path.get(path) {
            return d;
        }
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = self.dir(parent);
        let d = self.dirs.len();
        self.dirs.push(TreeDir { name: name.to_string(), parent, dirs: Vec::new(), files: Vec::new() });
        self.dirs[parent].dirs.push(d);
        self.by_path.insert(path.to_string(), d);
        d
    }

    fn file(&mut self, path: &str, offset: u64, size: u64) {
        let (parent, name) = path.rsplit_once('/').unwrap_or(("", path));
        let parent = self.dir(parent);
        self.dirs[parent].files.push(self.files.len());
        self.files.push(TreeFile { name: name.to_string(), parent, offset, size });
    }

    fn tables(&self) -> (Vec<u8>, Vec<u8>, Vec<u8>, Vec<u8>) {
        let mut dir_at = Vec::with_capacity(self.dirs.len());
        let mut at = 0u32;
        for d in &self.dirs {
            dir_at.push(at);
            at += 0x18 + d.name.len().next_multiple_of(4) as u32;
        }
        let mut file_at = Vec::with_capacity(self.files.len());
        let mut at = 0u32;
        for f in &self.files {
            file_at.push(at);
            at += 0x20 + f.name.len().next_multiple_of(4) as u32;
        }
        let link = |list: &[usize], at: &[u32]| list.first().map_or(NONE, |&i| at[i]);
        let mut dir_next = vec![NONE; self.dirs.len()];
        let mut file_next = vec![NONE; self.files.len()];
        for d in &self.dirs {
            for w in d.dirs.windows(2) {
                dir_next[w[0]] = dir_at[w[1]];
            }
            for w in d.files.windows(2) {
                file_next[w[0]] = file_at[w[1]];
            }
        }
        let mut dir_buckets = vec![NONE; bucket_count(self.dirs.len())];
        let mut dir_meta = Vec::new();
        for (i, d) in self.dirs.iter().enumerate() {
            let parent = dir_at[d.parent];
            let b = path_hash(parent, &d.name) as usize % dir_buckets.len();
            let next = std::mem::replace(&mut dir_buckets[b], dir_at[i]);
            for v in [parent, dir_next[i], link(&d.dirs, &dir_at), link(&d.files, &file_at), next, d.name.len() as u32] {
                dir_meta.extend_from_slice(&v.to_le_bytes());
            }
            dir_meta.extend_from_slice(d.name.as_bytes());
            dir_meta.resize(dir_meta.len().next_multiple_of(4), 0);
        }
        let mut file_buckets = vec![NONE; bucket_count(self.files.len())];
        let mut file_meta = Vec::new();
        for (i, f) in self.files.iter().enumerate() {
            let parent = dir_at[f.parent];
            let b = path_hash(parent, &f.name) as usize % file_buckets.len();
            let next = std::mem::replace(&mut file_buckets[b], file_at[i]);
            file_meta.extend_from_slice(&parent.to_le_bytes());
            file_meta.extend_from_slice(&file_next[i].to_le_bytes());
            file_meta.extend_from_slice(&f.offset.to_le_bytes());
            file_meta.extend_from_slice(&f.size.to_le_bytes());
            file_meta.extend_from_slice(&next.to_le_bytes());
            file_meta.extend_from_slice(&(f.name.len() as u32).to_le_bytes());
            file_meta.extend_from_slice(f.name.as_bytes());
            file_meta.resize(file_meta.len().next_multiple_of(4), 0);
        }
        let flat = |b: Vec<u32>| b.into_iter().flat_map(u32::to_le_bytes).collect();
        (flat(dir_buckets), dir_meta, flat(file_buckets), file_meta)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mem(b: &[u8]) -> DynStorage {
        Arc::new(Mem(b.to_vec()))
    }

    fn read(fs: &RomFs, path: &str) -> Vec<u8> {
        let f = fs.find(path).unwrap_or_else(|| panic!("{path} not found"));
        fs.storage().read_vec(f.offset, f.size as usize).unwrap()
    }

    fn build(files: &[(&str, &[u8])]) -> RomFs {
        let empty = RomFs { storage: mem(&[0; 0x50]), data_offset: 0x50, files_offset: 0, dirs: vec![0; 0x18], files: Vec::new() };
        let mut root = empty.dirs.clone();
        root[0x8..0x10].copy_from_slice(&[0xFF; 8]);
        let empty = RomFs { dirs: root, ..empty };
        RomFs::open(empty.with_overlay(files.iter().map(|(p, d)| (p.to_string(), mem(d))).collect()).unwrap()).unwrap()
    }

    #[test]
    fn overlay_replaces_and_adds_files() {
        let fs = build(&[("/UI/cmn/common.szs", b"old"), ("/Kart/a.bfres", b"kart"), ("/top.bin", b"t")]);
        let fs = RomFs::open(fs.with_overlay(vec![("/ui/CMN/common.szs".into(), mem(b"new common")), ("/Driver/new/x.bfres".into(), mem(b"x"))]).unwrap()).unwrap();
        assert_eq!(read(&fs, "/UI/cmn/common.szs"), b"new common");
        assert_eq!(read(&fs, "/Kart/a.bfres"), b"kart");
        assert_eq!(read(&fs, "/Driver/new/x.bfres"), b"x");
        assert_eq!(read(&fs, "/top.bin"), b"t");
        assert!(fs.find("/ui/CMN/common.szs").is_none());
        let mut paths: Vec<String> = fs.files().into_iter().map(|f| f.path).collect();
        paths.sort();
        assert_eq!(paths, ["/Driver/new/x.bfres", "/Kart/a.bfres", "/UI/cmn/common.szs", "/top.bin"]);
        assert_eq!(fs.dirs(), ["/UI", "/UI/cmn", "/Kart", "/Driver", "/Driver/new"]);
    }

    #[test]
    fn many_files_share_buckets() {
        let names: Vec<String> = (0..500).map(|i| format!("/d{}/f{i}.bin", i % 7)).collect();
        let files: Vec<(&str, &[u8])> = names.iter().map(|n| (n.as_str(), n.as_bytes())).collect();
        let fs = build(&files);
        for n in &names {
            assert_eq!(read(&fs, n), n.as_bytes());
        }
    }
}
