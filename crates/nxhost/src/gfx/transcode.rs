// Mush8 - transcode.rs
// Constantin Clerc (c22dev) - 0.1
use super::bc::{self, Kind};
use crate::nvn::tex;
use anyhow::{Context as _, Result};
use ctt_astcenc as astc;
use nxfs::bntx;
use nxfs::romfs::RomFs;
use nxfs::sarc;
use nxfs::storage::DynStorage;
use std::collections::{BTreeMap, HashMap};
use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};

pub const ASTC_BLOCKS: [(u8, u8); 14] = [(4, 4), (5, 4), (5, 5), (6, 5), (6, 6), (8, 5), (8, 6), (8, 8), (10, 5), (10, 6), (10, 8), (10, 10), (12, 10), (12, 12)];

pub struct Piece {
    pub kind: Kind,
    pub srgb: bool,
    pub w: u32,
    pub h: u32,
    pub lin: Vec<u8>,
}

pub fn kind(format: u32) -> Option<(Kind, bool)> {
    let snorm = format & 0xFF == 2;
    let k = match format >> 8 {
        0x1A => Kind::Bc1,
        0x1B => Kind::Bc2,
        0x1C => Kind::Bc3,
        0x1D => Kind::Bc4(snorm),
        0x1E => Kind::Bc5(snorm),
        0x1F => Kind::Bc6(format & 0xFF == 5),
        0x20 => Kind::Bc7,
        _ => return None,
    };
    Some((k, format & 0xFF == 6))
}

pub fn astc_format(block: (u8, u8), srgb: bool) -> u32 {
    let i = ASTC_BLOCKS.iter().position(|&b| b == block).unwrap_or(0) as u32;
    (0x2D + i) << 8 | if srgb { 6 } else { 1 }
}

pub fn key(kind: u8, w: u32, h: u32, d: u32, data: &[u8]) -> u128 {
    const P1: u64 = 0x9E37_79B9_7F4A_7C15;
    const P2: u64 = 0xC2B2_AE3D_27D4_EB4F;
    let fmix = |mut x: u64| {
        x ^= x >> 33;
        x = x.wrapping_mul(0xFF51_AFD7_ED55_8CCD);
        x ^= x >> 33;
        x = x.wrapping_mul(0xC4CE_B9FE_1A85_EC53);
        x ^ x >> 33
    };
    let head = (kind as u64) << 56 ^ (w as u64) << 36 ^ (h as u64) << 16 ^ d as u64 ^ (data.len() as u64).rotate_left(23);
    let (mut a, mut b) = (head ^ P1, head.rotate_left(32) ^ P2);
    let mut mix = |c: &[u8]| {
        let x = u64::from_le_bytes(c[..8].try_into().unwrap());
        let y = u64::from_le_bytes(c[8..16].try_into().unwrap());
        a = (a ^ x).wrapping_mul(P1).rotate_left(29) ^ y;
        b = (b ^ y).wrapping_mul(P2).rotate_left(31) ^ x;
    };
    let mut chunks = data.chunks_exact(16);
    for c in &mut chunks {
        mix(c);
    }
    let rest = chunks.remainder();
    if !rest.is_empty() {
        let mut t = [0u8; 16];
        t[..rest.len()].copy_from_slice(rest);
        mix(&t);
    }
    let (a, b) = (fmix(a ^ b.rotate_left(17)), fmix(b ^ a.rotate_left(41)));
    (a as u128) << 64 | b as u128
}

fn layout_of(t: &bntx::Texture, block: (u32, u32), bpb: u32, log2: u32) -> tex::Layout {
    let shape = tex::Shape { width: t.width, height: t.height, depth: 1, levels: t.mips.max(1), layers: t.layers.max(1), block_w: block.0, block_h: block.1, bytes_per_block: bpb };
    tex::block_linear(&shape, log2, 0)
}

fn each_image(t: &bntx::Texture) -> impl Iterator<Item = (u32, u32, u32, u32)> + '_ {
    (0..t.layers.max(1)).flat_map(move |layer| (0..t.mips.max(1)).map(move |level| (layer, level, (t.width >> level).max(1), (t.height >> level).max(1))))
}

pub fn pieces(t: &bntx::Texture, data: &[u8]) -> Result<Vec<Piece>> {
    let (k, srgb) = kind(t.format).context("not a BC texture")?;
    anyhow::ensure!(t.tile_mode == 0 && t.depth <= 1, "not a block-linear 2D texture");
    let bpb = bc::block_bytes(k) as u32;
    let layout = layout_of(t, (4, 4), bpb, t.block_height_log2);
    let base = t.data_offset(data)?;
    Ok(each_image(t)
        .map(|(layer, level, w, h)| {
            let (row, rows) = (w.div_ceil(4) * bpb, h.div_ceil(4));
            let start = base + (layer as u64 * layout.layer_size + layout.level_offsets[level as usize]) as usize;
            let mut lin = vec![0u8; (row * rows) as usize];
            tex::untile_into(data.get(start..).unwrap_or(&[]), row, rows, 0, layout.level_gobs[level as usize], 0..rows, &mut lin);
            Piece { kind: k, srgb, w, h, lin }
        })
        .collect())
}

fn gob_offset(x: u32, y: u32) -> u32 {
    (x % 64 / 32) * 256 + (y % 8 / 2) * 64 + (x % 32 / 16) * 32 + (y % 2) * 16 + x % 16
}

pub fn tile_into(lin: &[u8], row_bytes: u32, rows: u32, (gy, _): (u32, u32), dst: &mut [u8]) {
    let block_size = gy * 512;
    let block_row = row_bytes.div_ceil(64) * block_size;
    for y in 0..rows {
        let ybase = (y / (8 * gy)) * block_row + (y / 8 % gy) * 512;
        let mut x = 0;
        while x < row_bytes {
            let n = (16 - x % 16).min(row_bytes - x) as usize;
            let o = (ybase + (x / 64) * block_size + gob_offset(x, y)) as usize;
            let s = (y * row_bytes + x) as usize;
            dst[o..o + n].copy_from_slice(&lin[s..s + n]);
            x += n as u32;
        }
    }
}

pub fn bntx_spans(data: &[u8]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some(p) = data.get(i..).and_then(|d| d.windows(4).position(|w| w == b"BNTX")) {
        let at = i + p;
        i = at + 4;
        let Some(h) = data.get(at..at + 0x30) else { break };
        let size = u32::from_le_bytes(h[0x1C..0x20].try_into().unwrap()) as usize;
        if &h[0x20..0x24] == b"NX  " && size >= 0x30 && at + size <= data.len() {
            out.push((at, size));
            i = at + size;
        }
    }
    out
}

pub struct Encoder {
    quality: f32,
    pub bc1: (u8, u8),
    contexts: HashMap<(bool, u8, u8), astc::Context>,
    pub measure: bool,
    pub error: Option<(f64, u64)>,
}

impl Encoder {
    pub fn new(quality: f32, bc1: (u8, u8)) -> Self {
        Self { quality, bc1, contexts: HashMap::new(), measure: false, error: None }
    }

    pub fn target(&self, k: Kind) -> Option<(u8, u8)> {
        match k {
            Kind::Bc1 => Some(self.bc1),
            Kind::Bc2 | Kind::Bc3 | Kind::Bc7 => Some((4, 4)),
            _ => None,
        }
    }

    pub fn encode(&mut self, p: &Piece) -> Result<Vec<u8>> {
        let (bw, bh) = self.target(p.kind).context("format is not transcoded")?;
        let (px, _) = bc::decode(p.kind, &p.lin, (p.w.div_ceil(4) as usize) * bc::block_bytes(p.kind), p.w as usize, p.h as usize, 1);
        let ctx = match self.contexts.entry((p.srgb, bw, bh)) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => {
                let profile = if p.srgb { astc::Profile::LdrSrgb } else { astc::Profile::Ldr };
                let config = astc::config_init(profile, bw as u32, bh as u32, 1, astc::Preset::Custom(self.quality), astc::Flags::empty()).map_err(|e| anyhow::anyhow!("astcenc config: {e:?}"))?;
                e.insert(astc::Context::new(&config).map_err(|e| anyhow::anyhow!("astcenc context: {e:?}"))?)
            }
        };
        let image = |slice: &mut *mut std::ffi::c_void| astc::bindings::astcenc_image { dim_x: p.w, dim_y: p.h, dim_z: 1, data_type: astc::bindings::astcenc_type_ASTCENC_TYPE_U8, data: slice };
        let mut slice = px.as_ptr() as *mut std::ffi::c_void;
        let mut out = vec![0u8; (p.w.div_ceil(bw as u32) * p.h.div_ceil(bh as u32) * 16) as usize];
        ctx.compress(&mut image(&mut slice), astc::Swizzle::IDENTITY, &mut out).map_err(|e| anyhow::anyhow!("astcenc: {e:?}"))?;
        ctx.compress_reset().map_err(|e| anyhow::anyhow!("astcenc: {e:?}"))?;
        let mut error = None;
        if self.measure {
            let mut back = vec![0u8; px.len()];
            let mut slice = back.as_mut_ptr() as *mut std::ffi::c_void;
            ctx.decompress(&out, &mut image(&mut slice), astc::Swizzle::IDENTITY).map_err(|e| anyhow::anyhow!("astcenc: {e:?}"))?;
            ctx.decompress_reset().map_err(|e| anyhow::anyhow!("astcenc: {e:?}"))?;
            error = Some((px.iter().zip(&back).map(|(&a, &b)| (a as f64 - b as f64).powi(2)).sum(), px.len() as u64));
        }
        self.error = error;
        Ok(out)
    }
}

fn quality() -> f32 {
    match std::env::var("NX_ASTC_QUALITY").unwrap_or_default().to_ascii_lowercase().as_str() {
        "fastest" => 0.0,
        "fast" => 10.0,
        "" | "medium" => 60.0,
        "thorough" => 98.0,
        "verythorough" => 99.0,
        "exhaustive" => 100.0,
        n => n.parse().unwrap_or(60.0),
    }
}

fn bc1_block() -> (u8, u8) {
    let v = std::env::var("NX_TEXTURE_BC1_BLOCK").unwrap_or_default();
    v.split_once('x').and_then(|(w, h)| Some((w.parse().ok()?, h.parse().ok()?))).filter(|b| ASTC_BLOCKS.contains(b)).unwrap_or((6, 6))
}

pub fn selected(k: Kind) -> bool {
    static SET: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| std::env::var("NX_TEXTURE_FORMATS").unwrap_or_else(|_| "bc1,bc2,bc3,bc7".into()).to_ascii_lowercase());
    let name = kind_name(k).to_ascii_lowercase();
    matches!(k, Kind::Bc1 | Kind::Bc2 | Kind::Bc3 | Kind::Bc7) && (SET.as_str() == "all" || SET.split(',').any(|f| f.trim() == name))
}

fn kind_id(k: Kind) -> u8 {
    match k {
        Kind::Bc1 => 1,
        Kind::Bc2 => 2,
        Kind::Bc3 => 3,
        Kind::Bc4(s) => 4 | (s as u8) << 4,
        Kind::Bc5(s) => 5 | (s as u8) << 4,
        Kind::Bc6(s) => 6 | (s as u8) << 4,
        Kind::Bc7 => 7,
    }
}

fn kind_name(k: Kind) -> &'static str {
    match k {
        Kind::Bc1 => "BC1",
        Kind::Bc2 => "BC2",
        Kind::Bc3 => "BC3",
        Kind::Bc4(_) => "BC4",
        Kind::Bc5(_) => "BC5",
        Kind::Bc6(_) => "BC6H",
        Kind::Bc7 => "BC7",
    }
}

const MAGIC: &[u8; 4] = b"NXAC";

struct Memo {
    quality: f32,
    old: HashMap<u128, (u64, u32)>,
    old_data: Option<std::fs::File>,
    new: Mutex<(std::fs::File, u64, Vec<(u128, u64, u32)>)>,
}

impl Memo {
    fn open(dir: &Path, quality: f32) -> Result<Memo> {
        std::fs::create_dir_all(dir)?;
        let idx = std::fs::read(dir.join("astc.idx")).unwrap_or_default();
        let mut old = HashMap::new();
        if idx.len() >= 12 && &idx[..4] == MAGIC && f32::from_le_bytes(idx[4..8].try_into()?) == quality {
            for e in idx[12..].chunks_exact(28) {
                old.insert(u128::from_le_bytes(e[..16].try_into()?), (u64::from_le_bytes(e[16..24].try_into()?), u32::from_le_bytes(e[24..28].try_into()?)));
            }
        }
        let old_data = std::fs::File::open(dir.join("astc.bin")).ok();
        let old = if old_data.is_some() { old } else { HashMap::new() };
        let new = std::fs::File::create(dir.join("astc.bin.new"))?;
        Ok(Memo { quality, old, old_data, new: Mutex::new((new, 0, Vec::new())) })
    }

    fn get(&self, key: u128) -> Option<Vec<u8>> {
        let &(o, n) = self.old.get(&key)?;
        let mut b = vec![0; n as usize];
        self.old_data.as_ref()?.read_exact_at(&mut b, o).ok()?;
        self.put(key, &b);
        Some(b)
    }

    fn put(&self, key: u128, b: &[u8]) {
        let mut w = self.new.lock().unwrap();
        if w.2.iter().rev().take(64).any(|e| e.0 == key) {
            return;
        }
        let at = w.1;
        if w.0.write_all_at(b, at).is_ok() {
            w.1 += b.len() as u64;
            w.2.push((key, at, b.len() as u32));
        }
    }

    fn finish(self, dir: &Path) -> Result<()> {
        let (file, _, mut entries) = self.new.into_inner().unwrap();
        drop(file);
        drop(self.old_data);
        entries.sort_by_key(|e| e.0);
        entries.dedup_by_key(|e| e.0);
        let mut idx = Vec::with_capacity(12 + entries.len() * 28);
        idx.extend_from_slice(MAGIC);
        idx.extend_from_slice(&self.quality.to_le_bytes());
        idx.extend_from_slice(&(entries.len() as u32).to_le_bytes());
        for (k, o, n) in entries {
            idx.extend_from_slice(&k.to_le_bytes());
            idx.extend_from_slice(&o.to_le_bytes());
            idx.extend_from_slice(&n.to_le_bytes());
        }
        std::fs::rename(dir.join("astc.bin.new"), dir.join("astc.bin"))?;
        std::fs::write(dir.join("astc.idx"), idx)?;
        Ok(())
    }
}

#[derive(Default)]
struct Stats {
    converted: BTreeMap<String, (u64, u64, u64)>,
    skipped: BTreeMap<String, u64>,
    error: BTreeMap<String, (f64, u64)>,
    files: u64,
    before: u64,
    after: u64,
}

struct Candidate {
    block: (u8, u8),
    log2: u32,
    layout: tex::Layout,
}

struct Conv {
    kind: Kind,
    srgb: bool,
    candidates: Vec<Candidate>,
    pick: usize,
}

struct Slot {
    t: bntx::Texture,
    base: usize,
    size: usize,
    align: usize,
    conv: Option<Conv>,
}

impl Slot {
    fn new_size(&self) -> usize {
        self.conv.as_ref().map_or(self.size, |c| c.candidates[c.pick].layout.total_size as usize)
    }
}

struct Job<'a> {
    enc: Encoder,
    memo: &'a Memo,
    stats: &'a Mutex<Stats>,
}

impl Job<'_> {
    fn skip(&self, why: String) {
        *self.stats.lock().unwrap().skipped.entry(why).or_default() += 1;
    }

    fn encode(&mut self, t: &bntx::Texture, file: &[u8], k: Kind, srgb: bool, block: (u8, u8)) -> Result<Vec<Vec<u8>>> {
        let mut out = Vec::new();
        for p in pieces(t, file)? {
            let id = key(kind_id(k) ^ (srgb as u8) << 7, p.w, p.h, (block.0 as u32) << 8 | block.1 as u32, &p.lin);
            if let Some(b) = self.memo.get(id) {
                out.push(b);
                continue;
            }
            let saved = self.enc.bc1;
            self.enc.bc1 = block;
            let b = self.enc.encode(&p);
            self.enc.bc1 = saved;
            let b = b?;
            if let Some((sq, n)) = self.enc.error {
                let mut s = self.stats.lock().unwrap();
                let e = s.error.entry(format!("{} -> ASTC {}x{}", kind_name(k), block.0, block.1)).or_default();
                e.0 += sq;
                e.1 += n;
            }
            self.memo.put(id, &b);
            out.push(b);
        }
        Ok(out)
    }

    fn candidates(&self, t: &bntx::Texture, k: Kind) -> Vec<Candidate> {
        let Some(first) = self.enc.target(k) else { return Vec::new() };
        let mut blocks = vec![first];
        if k == Kind::Bc1 {
            blocks.extend([(8, 5), (8, 6), (8, 8)].into_iter().filter(|b: &(u8, u8)| b.0 as u32 * b.1 as u32 > first.0 as u32 * first.1 as u32));
        }
        blocks
            .into_iter()
            .map(|block| {
                let bb = (block.0 as u32, block.1 as u32);
                let (log2, layout) = if block == (4, 4) {
                    (t.block_height_log2, layout_of(t, bb, 16, t.block_height_log2))
                } else {
                    let top = tex::default_block_log2(t.height.div_ceil(bb.1).div_ceil(tex::GOB_HEIGHT), 4);
                    (0..=top)
                        .rev()
                        .map(|l| (l, layout_of(t, bb, 16, l)))
                        .fold(None::<(u32, tex::Layout)>, |best, (l, n)| match best {
                            Some(b) if b.1.total_size <= n.total_size => Some(b),
                            _ => Some((l, n)),
                        })
                        .unwrap()
                };
                Candidate { block, log2, layout }
            })
            .collect()
    }

    fn bntx(&mut self, file: &mut [u8]) -> Result<bool> {
        let u32_at = |f: &[u8], o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
        let ptr = |f: &[u8], t: &bntx::Texture, l: usize| u64::from_le_bytes(f[t.mip_offsets + l * 8..t.mip_offsets + l * 8 + 8].try_into().unwrap()) as usize;
        let mut slots = Vec::new();
        for t in bntx::textures(file)? {
            let base = t.data_offset(file)?;
            let size = u32_at(file, t.info + 0x50) as usize;
            let align = (u32_at(file, t.info + 0x54) as usize).max(512);
            anyhow::ensure!(align.is_power_of_two() && base + size <= file.len(), "texture {} has an unexpected data range", t.name);
            anyhow::ensure!((0..t.mips.max(1) as usize).all(|l| (base..base + size.max(1)).contains(&ptr(file, &t, l))), "texture {} has mips outside its data", t.name);
            let mut conv = None;
            if let Some((k, srgb)) = kind(t.format) {
                let bpb = bc::block_bytes(k) as u32;
                let old = layout_of(&t, (4, 4), bpb, t.block_height_log2);
                if !selected(k) {
                    self.skip(format!("{} (left as is, decoded on the CPU when the GPU lacks BC)", kind_name(k)));
                } else if t.tile_mode != 0 || t.depth > 1 {
                    self.skip(format!("{}: not a block-linear 2D texture", kind_name(k)));
                } else if old.total_size as usize != size || (0..t.mips.max(1) as usize).any(|l| ptr(file, &t, l) != base + old.level_offsets[l] as usize) {
                    self.skip(format!("{}: unexpected data layout", kind_name(k)));
                } else {
                    let candidates = self.candidates(&t, k);
                    conv = (!candidates.is_empty()).then_some(Conv { kind: k, srgb, candidates, pick: 0 });
                }
            }
            slots.push(Slot { t, base, size, align, conv });
        }
        if slots.iter().all(|s| s.conv.is_none()) {
            return Ok(false);
        }
        slots.sort_by_key(|s| s.base);
        anyhow::ensure!(slots.windows(2).all(|w| w[0].base + w[0].size <= w[1].base), "overlapping textures");
        let start = slots[0].base;
        let end = slots.iter().map(|s| s.base + s.size).max().unwrap();
        let packed = |slots: &[Slot]| {
            let mut off = start;
            let mut at = Vec::with_capacity(slots.len());
            for s in slots {
                off = off.next_multiple_of(s.align);
                at.push(off);
                off += s.new_size();
            }
            (off, at)
        };
        while packed(&slots).0 > end {
            let Some(s) = slots.iter_mut().filter(|s| s.conv.as_ref().is_some_and(|c| c.pick + 1 < c.candidates.len())).max_by_key(|s| s.new_size()) else {
                for s in slots.iter().filter_map(|s| s.conv.as_ref()) {
                    self.skip(format!("{}: ASTC does not fit in the BNTX", kind_name(s.kind)));
                }
                return Ok(false);
            };
            s.conv.as_mut().unwrap().pick += 1;
        }
        let (_, at) = packed(&slots);
        let mut encoded = Vec::with_capacity(slots.len());
        for s in &slots {
            encoded.push(match &s.conv {
                Some(c) => Some(self.encode(&s.t, file, c.kind, c.srgb, c.candidates[c.pick].block)?),
                None => None,
            });
        }
        let mut region = vec![0u8; end - start];
        for ((s, &new_base), enc) in slots.iter().zip(&at).zip(&encoded) {
            let dst = &mut region[new_base - start..];
            match (&s.conv, enc) {
                (Some(c), Some(images)) => {
                    let cand = &c.candidates[c.pick];
                    let bb = (cand.block.0 as u32, cand.block.1 as u32);
                    for ((layer, level, w, h), b) in each_image(&s.t).zip(images) {
                        let o = (layer as u64 * cand.layout.layer_size + cand.layout.level_offsets[level as usize]) as usize;
                        tile_into(b, w.div_ceil(bb.0) * 16, h.div_ceil(bb.1), cand.layout.level_gobs[level as usize], &mut dst[o..]);
                    }
                }
                _ => dst[..s.size].copy_from_slice(&file[s.base..s.base + s.size]),
            }
        }
        file[start..end].copy_from_slice(&region);
        let put32 = |f: &mut [u8], o: usize, v: u32| f[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let put64 = |f: &mut [u8], o: usize, v: u64| f[o..o + 8].copy_from_slice(&v.to_le_bytes());
        for (s, &new_base) in slots.iter().zip(&at) {
            let t = &s.t;
            match &s.conv {
                Some(c) => {
                    let cand = &c.candidates[c.pick];
                    put32(file, t.info + 0x1C, astc_format(cand.block, c.srgb));
                    let layout = u32_at(file, t.info + 0x34);
                    put32(file, t.info + 0x34, layout & !7 | cand.log2);
                    put32(file, t.info + 0x50, cand.layout.total_size as u32);
                    for l in 0..t.mips.max(1) as usize {
                        put64(file, t.mip_offsets + l * 8, new_base as u64 + cand.layout.level_offsets[l]);
                    }
                    let mut st = self.stats.lock().unwrap();
                    let e = st.converted.entry(format!("{} -> ASTC {}x{}", kind_name(c.kind), cand.block.0, cand.block.1)).or_default();
                    e.0 += 1;
                    e.1 += s.size as u64;
                    e.2 += each_image(t).map(|(_, _, w, h)| w as u64 * h as u64 * 4).sum::<u64>();
                }
                None => {
                    for l in 0..t.mips.max(1) as usize {
                        let p = ptr(file, t, l);
                        put64(file, t.mip_offsets + l * 8, (p - s.base + new_base) as u64);
                    }
                }
            }
        }
        Ok(true)
    }

    fn transform(&mut self, data: &[u8], depth: u32) -> Option<Vec<u8>> {
        if depth > 6 {
            return None;
        }
        if data.starts_with(b"Yaz0") {
            let raw = sarc::yaz0(data).ok()?;
            let new = self.transform(&raw, depth + 1)?;
            return Some(sarc::yaz0_compress(&new, sarc::yaz0_tail(data)));
        }
        if data.starts_with(b"SARC") {
            if let Ok(entries) = sarc::sarc(data) {
                let base = data.as_ptr() as usize;
                let spans: Vec<(usize, usize)> = entries.iter().map(|e| (e.data.as_ptr() as usize - base, e.data.len())).collect();
                let new: Vec<Option<Vec<u8>>> = entries.iter().map(|e| self.transform(e.data, depth + 1)).collect();
                if new.iter().all(Option::is_none) {
                    return None;
                }
                if new.iter().zip(&spans).all(|(n, s)| n.as_ref().is_none_or(|n| n.len() == s.1)) {
                    let mut out = data.to_vec();
                    for (n, &(at, len)) in new.iter().zip(&spans) {
                        if let Some(n) = n {
                            out[at..at + len].copy_from_slice(n);
                        }
                    }
                    return Some(out);
                }
                let contents: Vec<&[u8]> = new.iter().zip(&entries).map(|(n, e)| n.as_deref().unwrap_or(e.data)).collect();
                return sarc::rebuild(data, &contents).ok();
            }
        }
        let mut out: Option<Vec<u8>> = None;
        for (at, size) in bntx_spans(data) {
            let mut file = data[at..at + size].to_vec();
            match self.bntx(&mut file) {
                Ok(true) => out.get_or_insert_with(|| data.to_vec())[at..at + size].copy_from_slice(&file),
                Ok(false) => {}
                Err(e) => self.skip(format!("BNTX left as is: {e:#}")),
            }
        }
        out.filter(|o| o.as_slice() != data)
    }
}

fn copy_range(storage: &DynStorage, from: u64, len: u64, out: &std::fs::File, to: u64, buf: &mut [u8]) -> Result<()> {
    let mut done = 0;
    while done < len {
        let n = (len - done).min(buf.len() as u64) as usize;
        storage.read_at(from + done, &mut buf[..n])?;
        out.write_all_at(&buf[..n], to + done)?;
        done += n as u64;
    }
    Ok(())
}

fn prepare_image(storage: DynStorage, path: &Path, memo: &Memo, stats: &Mutex<Stats>, quality: f32, bc1: (u8, u8)) -> Result<()> {
    let t0 = std::time::Instant::now();
    let fs = RomFs::open(storage.clone())?;
    let mut files = fs.files();
    files.sort_by_key(|f| std::cmp::Reverse(f.size));
    let spill_path = path.with_extension("spill");
    let spill_file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&spill_path)?;
    let spill = Mutex::new((spill_file, 0u64, HashMap::<String, (u64, u64)>::new()));
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let failed = Mutex::new(None);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| {
                let mut enc = Encoder::new(quality, bc1);
                enc.measure = std::env::var_os("NX_TEXTURE_PSNR").is_some();
                let mut job = Job { enc, memo, stats };
                while let Some(f) = files.get(next.fetch_add(1, Relaxed)) {
                    let n = done.fetch_add(1, Relaxed) + 1;
                    if n % 1000 == 0 {
                        eprintln!("[prep] {n}/{} files ({:.0} s)", files.len(), t0.elapsed().as_secs_f32());
                    }
                    let Ok(data) = fs.storage().read_vec(f.offset, f.size as usize) else { continue };
                    let Some(new) = job.transform(&data, 0) else { continue };
                    let mut sp = spill.lock().unwrap();
                    let at = sp.1;
                    if let Err(e) = sp.0.write_all_at(&new, at) {
                        failed.lock().unwrap().get_or_insert(e);
                        continue;
                    }
                    sp.1 += new.len() as u64;
                    sp.2.insert(f.path.clone(), (at, new.len() as u64));
                    drop(sp);
                    let mut s = stats.lock().unwrap();
                    s.files += 1;
                    s.before += f.size;
                    s.after += new.len() as u64;
                }
            });
        }
    });
    if let Some(e) = failed.into_inner().unwrap() {
        return Err(e.into());
    }
    let (spill_file, _, changed) = spill.into_inner().unwrap();
    let spilled: DynStorage = std::sync::Arc::new(nxfs::storage::FileRange::new(std::sync::Arc::new(spill_file), 0, u64::MAX / 2));
    let header = storage.read_vec(0, 0x50)?;
    let field = |o: usize| u64::from_le_bytes(header[o..o + 8].try_into().unwrap());
    let data_off = field(0x48);
    let (_, mut table) = fs.file_table();
    let tmp = path.with_extension("bin.new");
    let out = std::fs::File::create(&tmp)?;
    let mut buf = vec![0u8; 16 << 20];
    copy_range(&storage, 0, data_off, &out, 0, &mut buf)?;
    files.sort_by_key(|f| (f.offset, f.size));
    let mut cur = data_off;
    for f in &files {
        cur = cur.next_multiple_of(16);
        let len = match changed.get(&f.path) {
            Some(&(at, len)) => {
                copy_range(&spilled, at, len, &out, cur, &mut buf)?;
                len
            }
            None => {
                copy_range(&storage, f.offset, f.size, &out, cur, &mut buf)?;
                f.size
            }
        };
        fs.set_entry(&mut table, f, cur, len);
        cur += len;
    }
    let mut header = header.clone();
    for (i, o) in [0x08usize, 0x18, 0x28, 0x38].into_iter().enumerate() {
        cur = cur.next_multiple_of(16);
        let (at, len) = (field(o), field(o + 8));
        if i == 3 {
            out.write_all_at(&table, cur)?;
        } else {
            copy_range(&storage, at, len, &out, cur, &mut buf)?;
        }
        header[o..o + 8].copy_from_slice(&cur.to_le_bytes());
        cur += len;
    }
    out.write_all_at(&header, 0)?;
    out.set_len(cur)?;
    out.sync_all()?;
    drop(out);
    drop(spilled);
    let _ = std::fs::remove_file(&spill_path);
    std::fs::rename(&tmp, path)?;
    eprintln!("[prep] {}: {:.2} GB (source {:.2} GB), done in {:.0} s", path.display(), cur as f64 / 1e9, nxfs::storage::Storage::size(&*storage) as f64 / 1e9, t0.elapsed().as_secs_f32());
    Ok(())
}

pub fn prepare(romfs: DynStorage, dlc: Vec<(u64, DynStorage)>, main_id: &str, dir: &Path) -> Result<()> {
    let t0 = std::time::Instant::now();
    let (quality, bc1) = (quality(), bc1_block());
    let out = dir.join("prepared");
    std::fs::create_dir_all(&out)?;
    for e in std::fs::read_dir(&out)?.flatten() {
        if e.file_name().to_string_lossy().starts_with("dlc-") {
            std::fs::remove_file(e.path())?;
        }
    }
    let _ = std::fs::remove_file(out.join("main_id"));
    let memo = Memo::open(dir, quality)?;
    eprintln!("[prep] BC1 -> ASTC {}x{}, BC2/BC3/BC7 -> ASTC 4x4, quality {quality}, {} encoded images reusable", bc1.0, bc1.1, memo.old.len());
    let stats = Mutex::new(Stats::default());
    prepare_image(romfs, &out.join("romfs.bin"), &memo, &stats, quality, bc1)?;
    for (id, s) in dlc {
        prepare_image(s, &out.join(format!("dlc-{id:016x}.bin")), &memo, &stats, quality, bc1)?;
    }
    memo.finish(dir)?;
    std::fs::write(out.join("main_id"), format!("{main_id}\n"))?;
    let s = stats.into_inner().unwrap();
    let mb = |b: u64| b as f64 / (1 << 20) as f64;
    for (k, (n, bytes, rgba)) in &s.converted {
        let psnr = s.error.get(k).filter(|e| e.1 > 0).map_or(String::new(), |e| format!(", PSNR vs BC {:.2} dB", 10.0 * (255.0f64 * 255.0 / (e.0 / e.1 as f64)).log10()));
        eprintln!("[prep] {k}: {n} textures converted in place ({:.1} MB of data, {:.1} MB when decoded on the CPU){psnr}", mb(*bytes), mb(*rgba));
    }
    for (why, n) in &s.skipped {
        eprintln!("[prep] {n} textures skipped: {why}");
    }
    eprintln!(
        "[prep] {} files rewritten: {:.1} MB -> {:.1} MB, {:.0} s total",
        s.files,
        mb(s.before),
        mb(s.after),
        t0.elapsed().as_secs_f32()
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Rng(u64);
    impl Rng {
        fn next(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (self.0 >> 33) as u32
        }
    }

    struct Tex {
        format: u32,
        w: u32,
        h: u32,
        mips: u32,
        layers: u32,
        log2: u32,
        images: Vec<Vec<u8>>,
    }

    fn bpb(format: u32) -> u32 {
        if matches!(format >> 8, 0x1A | 0x1D) { 8 } else { 16 }
    }

    fn bntx(texs: &[Tex]) -> Vec<u8> {
        let mut b = vec![0u8; 0x1000];
        let put32 = |b: &mut Vec<u8>, o: usize, v: u32| b[o..o + 4].copy_from_slice(&v.to_le_bytes());
        let put64 = |b: &mut Vec<u8>, o: usize, v: u64| b[o..o + 8].copy_from_slice(&v.to_le_bytes());
        b[..4].copy_from_slice(b"BNTX");
        b[0x20..0x24].copy_from_slice(b"NX  ");
        put32(&mut b, 0x24, texs.len() as u32);
        put64(&mut b, 0x28, 0x40);
        for (i, t) in texs.iter().enumerate() {
            let shape = tex::Shape { width: t.w, height: t.h, depth: 1, levels: t.mips, layers: t.layers, block_w: 4, block_h: 4, bytes_per_block: bpb(t.format) };
            let layout = tex::block_linear(&shape, t.log2, 0);
            let base = b.len().next_multiple_of(512);
            b.resize(base + layout.total_size as usize, 0);
            let info = 0x100 + i * 0x200;
            put64(&mut b, 0x40 + i * 8, info as u64);
            b[info..info + 4].copy_from_slice(b"BRTI");
            b[info + 0x16..info + 0x18].copy_from_slice(&(t.mips as u16).to_le_bytes());
            put32(&mut b, info + 0x1C, t.format);
            put32(&mut b, info + 0x24, t.w);
            put32(&mut b, info + 0x28, t.h);
            put32(&mut b, info + 0x2C, 1);
            put32(&mut b, info + 0x30, t.layers);
            put32(&mut b, info + 0x34, 0x10000 | t.log2);
            put32(&mut b, info + 0x50, layout.total_size as u32);
            put32(&mut b, info + 0x54, 512);
            put64(&mut b, info + 0x60, (info + 0x100) as u64);
            b[info + 0x100..info + 0x103].copy_from_slice(&[1, 0, b't']);
            put64(&mut b, info + 0x70, (info + 0x110) as u64);
            for l in 0..t.mips as usize {
                put64(&mut b, info + 0x110 + l * 8, base as u64 + layout.level_offsets[l]);
            }
            let mut i = 0;
            for layer in 0..t.layers {
                for level in 0..t.mips {
                    let (lw, lh) = ((t.w >> level).max(1), (t.h >> level).max(1));
                    let start = base + (layer as u64 * layout.layer_size + layout.level_offsets[level as usize]) as usize;
                    tile_into(&t.images[i], lw.div_ceil(4) * bpb(t.format), lh.div_ceil(4), layout.level_gobs[level as usize], &mut b[start..]);
                    i += 1;
                }
            }
        }
        let size = b.len() as u32;
        put32(&mut b, 0x1C, size);
        b
    }

    fn bc1_block(rng: &mut Rng) -> Vec<u8> {
        let (a, b) = (rng.next() as u16, rng.next() as u16);
        let (c0, c1) = (a.max(b), a.min(b).min(a.max(b).saturating_sub(1)));
        let dir = rng.next() % 2;
        let idx = (0..16u32).fold(0u32, |acc, i| acc | [0, 2, 3, 1][(if dir == 0 { i % 4 } else { i / 4 }) as usize] << (i * 2));
        [c0.to_le_bytes(), c1.to_le_bytes()].concat().into_iter().chain(idx.to_le_bytes()).collect()
    }

    struct Bits(u128, u32);
    impl Bits {
        fn put(&mut self, v: u32, n: u32) {
            self.0 |= (v as u128 & ((1 << n) - 1)) << self.1;
            self.1 += n;
        }
    }

    fn bc7_mode6_block(rng: &mut Rng) -> Vec<u8> {
        let mut b = Bits(0, 0);
        b.put(1 << 6, 7);
        for _ in 0..8 {
            b.put(rng.next(), 7);
        }
        b.put(rng.next(), 2);
        for i in 0..16u32 {
            let t = ((i % 4 + i / 4) * 15 / 6).min(15);
            b.put(if i == 0 { t.min(7) } else { t }, if i == 0 { 3 } else { 4 });
        }
        b.0.to_le_bytes().to_vec()
    }

    fn make(rng: &mut Rng, format: u32, w: u32, h: u32, mips: u32, layers: u32, log2: u32) -> Tex {
        let mut images = Vec::new();
        for _ in 0..layers {
            for level in 0..mips {
                let n = (w >> level).max(1).div_ceil(4) * (h >> level).max(1).div_ceil(4);
                images.push(
                    (0..n)
                        .flat_map(|_| match format >> 8 {
                            0x1A => bc1_block(rng),
                            0x20 | 0x1C => bc7_mode6_block(rng),
                            _ => (0..bpb(format)).map(|_| rng.next() as u8).collect(),
                        })
                        .collect(),
                );
            }
        }
        Tex { format, w, h, mips, layers, log2, images }
    }

    fn untile_level(file: &[u8], t: &bntx::Texture, block: (u32, u32), bpb: u32, layer: u32, level: u32) -> Vec<u8> {
        let l = layout_of(t, block, bpb, t.block_height_log2);
        let (w, h) = ((t.width >> level).max(1), (t.height >> level).max(1));
        let (row, rows) = (w.div_ceil(block.0) * bpb, h.div_ceil(block.1));
        let mut out = vec![0u8; (row * rows) as usize];
        let ptr = u64::from_le_bytes(file[t.mip_offsets + level as usize * 8..][..8].try_into().unwrap()) as usize;
        assert_eq!(ptr, t.data_offset(file).unwrap() + l.level_offsets[level as usize] as usize);
        let start = ptr + (layer as u64 * l.layer_size) as usize;
        tex::untile_into(&file[start..], row, rows, 0, l.level_gobs[level as usize], 0..rows, &mut out);
        out
    }

    fn rgba_rms(astc_data: &[u8], block: (u32, u32), w: u32, h: u32, want: &[u8]) -> f64 {
        let mut img = vec![0u32; (w * h) as usize];
        texture2ddecoder::decode_astc(astc_data, w as usize, h as usize, block.0 as usize, block.1 as usize, &mut img).unwrap();
        let mut sq = 0.0;
        for (i, c) in img.iter().enumerate() {
            let [b, g, r, a] = c.to_le_bytes();
            for (got, want) in [r, g, b, a].into_iter().zip(&want[i * 4..i * 4 + 4]) {
                sq += (got as f64 - *want as f64).powi(2);
            }
        }
        (sq / (img.len() * 4) as f64).sqrt()
    }

    fn with_job<R>(name: &str, f: impl FnOnce(&mut Job) -> R) -> R {
        let dir = std::env::temp_dir().join(format!("mush8-memo-{name}-{}", std::process::id()));
        let memo = Memo::open(&dir, 60.0).unwrap();
        let stats = Mutex::new(Stats::default());
        let r = f(&mut Job { enc: Encoder::new(60.0, (6, 6)), memo: &memo, stats: &stats });
        let _ = std::fs::remove_dir_all(&dir);
        r
    }

    fn check(texs: &[Tex], name: &str) {
        let file = bntx(texs);
        let mut wrapped = vec![0x55u8; 0x1000];
        wrapped.extend_from_slice(&file);
        let packed = sarc::yaz0_compress(&wrapped, [0; 8]);
        let changed = with_job(name, |job| job.transform(&packed, 0).unwrap_or_else(|| panic!("unchanged: {:?}", job.stats.lock().unwrap().skipped)));
        let new = sarc::yaz0(&changed).unwrap();
        assert_eq!(new.len(), wrapped.len());
        assert_eq!(&new[..0x1000], &wrapped[..0x1000]);
        let new = &new[0x1000..];
        let after = bntx::textures(new).unwrap();
        let before = bntx::textures(&file).unwrap();
        let mut ranges = Vec::new();
        for ((src, t), old) in texs.iter().zip(&after).zip(&before) {
            let (k, srgb) = kind(src.format).unwrap();
            let image_size = u32::from_le_bytes(new[t.info + 0x50..t.info + 0x54].try_into().unwrap()) as usize;
            let base = t.data_offset(new).unwrap();
            assert_eq!(base % 512, 0);
            ranges.push((base, base + image_size));
            let mut i = 0;
            if !selected(k) {
                assert_eq!(t.format, src.format);
                for layer in 0..src.layers {
                    for level in 0..src.mips {
                        assert_eq!(untile_level(new, t, (4, 4), bpb(src.format), layer, level), src.images[i]);
                        i += 1;
                    }
                }
                continue;
            }
            let block = ASTC_BLOCKS[(t.format >> 8) as usize - 0x2D];
            assert_eq!(t.format, astc_format(block, srgb));
            assert_eq!(old.name, t.name);
            let bb = (block.0 as u32, block.1 as u32);
            for layer in 0..src.layers {
                for level in 0..src.mips {
                    let (lw, lh) = ((src.w >> level).max(1), (src.h >> level).max(1));
                    let data = untile_level(new, t, bb, 16, layer, level);
                    let (px, _) = bc::decode(k, &src.images[i], (lw.div_ceil(4) as usize) * bc::block_bytes(k), lw as usize, lh as usize, 1);
                    let rms = rgba_rms(&data, bb, lw, lh, &px);
                    assert!(rms < if block == (4, 4) { 8.0 } else { 40.0 }, "{:#x} {block:?} level {level} layer {layer}: rms {rms}", src.format);
                    i += 1;
                }
            }
        }
        ranges.sort();
        assert!(ranges.windows(2).all(|w| w[0].1 <= w[1].0), "{ranges:x?}");
        assert!(ranges.last().unwrap().1 <= file.len());
    }

    #[test]
    fn bc1_bc3_bc7_rewritten_in_place() {
        let mut rng = Rng(5);
        check(&[make(&mut rng, 0x1A01, 512, 256, 3, 1, 4)], "big-bc1");
        check(&[make(&mut rng, 0x2001, 64, 64, 7, 1, 3)], "bc7");
        check(&[make(&mut rng, 0x1C06, 256, 8, 3, 2, 4)], "bc3-array");
        check(&[make(&mut rng, 0x1A06, 100, 60, 4, 6, 3)], "bc1-cube");
    }

    #[test]
    fn small_bc1_borrows_room_and_neighbours_move() {
        let mut rng = Rng(8);
        let texs = [make(&mut rng, 0x1A01, 256, 128, 6, 1, 4), make(&mut rng, 0x1D01, 64, 64, 4, 1, 3), make(&mut rng, 0x1A01, 512, 512, 1, 1, 4), make(&mut rng, 0x1E02, 32, 16, 2, 1, 1)];
        check(&texs, "mixed");
    }

    #[test]
    fn untouched_formats_stay_identical() {
        let mut rng = Rng(9);
        let file = bntx(&[make(&mut rng, 0x1D01, 64, 64, 4, 1, 3)]);
        assert!(with_job("bc4", |job| job.transform(&file, 0)).is_none());
    }

    #[test]
    fn tile_round_trips() {
        let mut rng = Rng(2);
        for &(row, rows, gy) in &[(256u32, 64u32, 4u32), (100, 37, 2), (16, 3, 1), (520, 90, 16), (48, 11, 2)] {
            let lin: Vec<u8> = (0..row * rows).map(|_| rng.next() as u8).collect();
            let size = (row.div_ceil(64) * 64 * rows.div_ceil(8 * gy) * 8 * gy) as usize;
            let mut tiled = vec![0u8; size];
            tile_into(&lin, row, rows, (gy, 1), &mut tiled);
            let mut back = vec![0u8; lin.len()];
            tex::untile_into(&tiled, row, rows, 0, (gy, 1), 0..rows, &mut back);
            assert_eq!(back, lin, "{row}x{rows} gy {gy}");
        }
    }

    #[test]
    fn bntx_found_inside_yaz0() {
        let mut rng = Rng(1);
        let file = bntx(&[make(&mut rng, 0x1A01, 32, 32, 1, 1, 2)]);
        let mut wrapped = vec![7u8; 333];
        wrapped.extend_from_slice(&file);
        wrapped.extend_from_slice(b"BNTX junk");
        assert_eq!(bntx_spans(&wrapped), vec![(333, file.len())]);
        assert_eq!(sarc::yaz0(&sarc::yaz0_compress(&wrapped, [0; 8])).unwrap(), wrapped);
    }
}
