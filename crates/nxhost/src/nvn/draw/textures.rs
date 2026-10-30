// Mush8 - textures.rs
// Constantin Clerc (c22dev) - 0.1
use super::super::cmd::TexBuilder;
use super::super::objects::STATE;
use crate::gfx::{self, Shared};
use nxshader::TexKind;
use objc2_foundation::NSRange;
use objc2_metal::*;
use super::super::fx::FastMap;

pub type Texture = gfx::Texture;
pub type Sampler = objc2::runtime::ProtocolObject<dyn MTLSamplerState>;

fn u32_at(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes(b[o..o + 4].try_into().unwrap())
}
fn f32_at(b: &[u8], o: usize) -> f32 {
    f32::from_bits(u32_at(b, o))
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub struct TexKey {
    pub addr: u64,
    pub format: u32,
    pub target: u32,
    pub width: u32,
    pub height: u32,
    pub depth: u32,
    pub levels: u32,
}

impl TexKey {
    pub fn of(b: &TexBuilder) -> Option<TexKey> {
        Self::of_in(b, &STATE.lock().unwrap())
    }

    pub fn of_in(b: &TexBuilder, s: &super::super::objects::State) -> Option<TexKey> {
        let pool = u64::from_le_bytes(b[8..16].try_into().unwrap());
        let offset = u64::from_le_bytes(b[16..24].try_into().unwrap());
        s.pools.get(&pool)?;
        let addr = s.pool_offset_cpu(pool, offset).unwrap_or((pool | 1 << 63) + offset);
        Some(TexKey {
            addr,
            format: u32_at(b, 0x30),
            target: u32_at(b, 0x1c),
            width: u32_at(b, 0x20).max(1),
            height: u32_at(b, 0x24).max(1),
            depth: u32_at(b, 0x28).max(1),
            levels: u32_at(b, 0x2c).max(1),
        })
    }
}

pub fn is_compressed(nvn: u32) -> bool {
    (65..=80).contains(&nvn) || nvn >= 121
}

#[derive(Default)]
pub struct Textures {
    map: FastMap<TexKey, Shared<Texture>>,
    block_views: Vec<(TexKey, Shared<Texture>)>,
    aliases: FastMap<TexKey, Vec<Shared<Texture>>>,
    storage: FastMap<TexKey, (u32, Shared<Texture>)>,
    views: FastMap<(TexKey, TexKind, [u32; 4]), Shared<Texture>>,
    dummies: FastMap<(TexKind, u8), Shared<Texture>>,
    mismatches: FastMap<(TexKind, u64), ()>,
    samplers: FastMap<([u8; 0x40], bool), Shared<Sampler>>,
    default_sampler: Option<Shared<Sampler>>,
    misses: u64,
    uploaded: FastMap<TexKey, ([u64; WINDOWS], u64, u64)>,
    pub frame: u64,
}

impl Textures {
    pub fn evict(&mut self, gone: &dyn Fn(&TexKey) -> bool) {
        self.map.retain(|k, _| !gone(k));
        self.block_views.retain(|(k, _)| !gone(k));
        self.aliases.retain(|k, _| !gone(k));
        self.storage.retain(|k, _| !gone(k));
        self.views.retain(|(k, _, _), _| !gone(k));
        self.uploaded.retain(|k, _| !gone(k));
    }

    pub fn cached(&self, key: &TexKey) -> Option<(TexKey, Shared<Texture>)> {
        self.map.get(key).map(|t| (*key, t.clone()))
    }

    pub fn aliases_of(&self, key: &TexKey) -> &[Shared<Texture>] {
        self.aliases.get(key).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn get(&mut self, b: &TexBuilder, render_target: bool) -> Option<(TexKey, Shared<Texture>)> {
        self.get_keyed(TexKey::of(b), b, render_target)
    }

    pub fn get_keyed(&mut self, key: Option<TexKey>, b: &TexBuilder, render_target: bool) -> Option<(TexKey, Shared<Texture>)> {
        let Some(key) = key else {
            self.miss("memory pool not found", b);
            return None;
        };
        if let Some(t) = self.map.get(&key) {
            let t = t.clone();
            let frame = self.frame;
            if !render_target {
                if let Some((mut hashes, checked, len)) = self.uploaded.get(&key).copied() {
                    if checked != frame {
                        let w = window(len, frame);
                        let changed = memory_hash(key.addr, len, w) != hashes[w];
                        if changed {
                            let levels = t.0.mipmapLevelCount() as u32;
                            upload(b, &key, &t.0, levels);
                            hashes = memory_hashes(key.addr, len);
                        }
                        self.uploaded.insert(key, (hashes, frame, len));
                    }
                }
            }
            return Some((key, t));
        }
        let Some(format) = gfx::pixel_format(key.format) else {
            self.miss("no Metal format", b);
            return None;
        };
        let base_key = TexKey { format: 0, ..key };
        if is_compressed(key.format) && gfx::bc::soft(key.format).is_none() {
            if let Some((bk, base)) = self.block_views.iter().find(|(bk, _)| bk.addr == key.addr).map(|(k, t)| (*k, t.clone())) {
                let (bw, bh, bpb) = super::super::format_info(key.format);
                let (_, _, base_bpb) = super::super::format_info(bk.format);
                if bpb == base_bpb && bk.width == key.width.div_ceil(bw) && bk.height == key.height.div_ceil(bh) {
                    let alias = self.create(b, key, format, true)?;
                    let cb = gfx::gpu().queue.0.commandBuffer()?;
                    copy_blocks(&cb, &base.0, &alias.0, None);
                    cb.commit();
                    self.aliases.entry(bk).or_default().push(alias.clone());
                    self.map.insert(key, alias.clone());
                    return Some((key, alias));
                }
            }
        }
        if let Some((base_fmt, base)) = self.storage.get(&base_key).cloned() {
            let (_, _, bpb) = super::super::format_info(key.format);
            let (_, _, base_bpb) = super::super::format_info(base_fmt);
            let plain = |f: u32| !is_compressed(f) && !gfx::is_depth_format(f);
            if bpb == base_bpb && plain(key.format) && plain(base_fmt) {
                let v = unsafe {
                    base.0.newTextureViewWithPixelFormat_textureType_levels_slices(
                        format,
                        base.0.textureType(),
                        NSRange { location: 0, length: base.0.mipmapLevelCount() },
                        NSRange { location: 0, length: base.0.arrayLength() * if base.0.textureType() == MTLTextureType::TypeCube { 6 } else { 1 } },
                    )
                };
                if let Some(v) = v {
                    let v = Shared(v);
                    self.map.insert(key, v.clone());
                    return Some((key, v));
                }
            }
        }
        if !render_target && key.addr >> 63 != 0 && self.misses < 40 {
            self.misses += 1;
            let near: Vec<_> = self.map.keys().filter(|k| k.addr == key.addr).collect();
            eprintln!("[gfx] sampled virtual-pool texture not rendered yet: {key:?} same-address keys {near:?}");
        }
        let tex = self.create(b, key, format, render_target)?;
        self.map.insert(key, tex.clone());
        self.storage.entry(TexKey { format: 0, ..key }).or_insert((key.format, tex.clone()));
        Some((key, tex))
    }

    fn miss(&mut self, why: &str, b: &TexBuilder) {
        self.misses += 1;
        if self.misses <= 10 {
            eprintln!(
                "[gfx] texture unavailable ({why}): pool {:#x} offset {:#x} format {} target {} {}x{}",
                u64::from_le_bytes(b[8..16].try_into().unwrap()),
                u64::from_le_bytes(b[16..24].try_into().unwrap()),
                u32_at(b, 0x30),
                u32_at(b, 0x1c),
                u32_at(b, 0x20),
                u32_at(b, 0x24)
            );
        }
    }

    fn create(&mut self, b: &TexBuilder, key: TexKey, format: MTLPixelFormat, render_target: bool) -> Option<Shared<Texture>> {
        let (ty, slices) = match key.target {
            2 => (MTLTextureType::Type3D, 1),
            3 => (MTLTextureType::Type2DArray, key.height),
            4 | 6 => (MTLTextureType::Type2DArray, key.depth),
            8 => (MTLTextureType::TypeCube, 1),
            9 => (MTLTextureType::TypeCubeArray, (key.depth / 6).max(1)),
            10 => return None,
            _ => (MTLTextureType::Type2D, 1),
        };
        let height = if key.target == 3 { 1 } else { key.height };
        let depth_format = gfx::is_depth_format(key.format);
        let max_levels = 32 - key.width.max(height).leading_zeros();
        let levels = key.levels.min(max_levels).max(1);
        let d = MTLTextureDescriptor::new();
        unsafe {
            d.setTextureType(ty);
            d.setPixelFormat(format);
            d.setWidth(key.width as usize);
            d.setHeight(height as usize);
            d.setDepth(if key.target == 2 { key.depth as usize } else { 1 });
            d.setMipmapLevelCount(levels as usize);
            d.setArrayLength(slices.max(1) as usize);
        }
        let mut usage = MTLTextureUsage::ShaderRead;
        if !is_compressed(key.format) {
            usage |= MTLTextureUsage::RenderTarget | MTLTextureUsage::PixelFormatView;
        }
        d.setUsage(usage);
        d.setStorageMode(MTLStorageMode::Private);
        let tex = Shared(gfx::gpu().device.0.newTextureWithDescriptor(&d)?);
        if !render_target && !depth_format {
            upload(b, &key, &tex.0, levels);
        }
        let len = storage_len(b, &key);
        if !render_target && !depth_format && cpu_writable(b) && len > 0 && gfx::heap().is_some_and(|h| h.gpu_addr(key.addr).is_some() && h.gpu_addr(key.addr + len - 1).is_some()) {
            self.uploaded.insert(key, (memory_hashes(key.addr, len), self.frame, len));
        }
        if render_target && super::super::gfx_integer_block_format(key.format) {
            self.block_views.push((key, tex.clone()));
        }
        Some(tex)
    }

    pub fn view(&mut self, key: TexKey, b: &TexBuilder, tex: &Shared<Texture>, kind: TexKind) -> Option<Shared<Texture>> {
        let mut swz = [u32_at(b, 0x38), u32_at(b, 0x3c), u32_at(b, 0x40), u32_at(b, 0x44)];
        if swz == [0; 4] || swz.iter().any(|&c| c > 5) {
            swz = [2, 3, 4, 5];
        }
        if key.format == 60 {
            swz = swz.map(|c| match c {
                2 => 4,
                4 => 2,
                c => c,
            });
        }
        let want = kind_type(kind);
        let have = tex.0.textureType();
        if have == want && swz == [2, 3, 4, 5] {
            return Some(tex.clone());
        }
        if let Some(v) = self.views.get(&(key, kind, swz)) {
            return Some(v.clone());
        }
        let available = tex.0.arrayLength() * if matches!(have, MTLTextureType::TypeCube | MTLTextureType::TypeCubeArray) { 6 } else { 1 };
        let slices = match (have, want) {
            (h, w) if h == w => available,
            (MTLTextureType::Type3D, _) | (_, MTLTextureType::Type3D) => return None,
            (_, MTLTextureType::Type2D) => 1,
            (_, MTLTextureType::Type2DArray) => available,
            (_, MTLTextureType::TypeCube) => 6,
            (_, MTLTextureType::TypeCubeArray) => available / 6 * 6,
            _ => return None,
        };
        if slices == 0 || slices > available {
            return None;
        }
        let s = |c: u32| MTLTextureSwizzle(c as _);
        let v = unsafe {
            tex.0.newTextureViewWithPixelFormat_textureType_levels_slices_swizzle(
                tex.0.pixelFormat(),
                want,
                NSRange { location: 0, length: tex.0.mipmapLevelCount() },
                NSRange { location: 0, length: slices },
                MTLTextureSwizzleChannels { red: s(swz[0]), green: s(swz[1]), blue: s(swz[2]), alpha: s(swz[3]) },
            )
        }?;
        let v = Shared(v);
        self.views.insert((key, kind, swz), v.clone());
        Some(v)
    }

    pub fn dummy(&mut self, kind: TexKind) -> Shared<Texture> {
        self.filled(kind, 0xFF)
    }

    pub fn fallback(&mut self, kind: TexKind, tex: &Shared<Texture>) -> Shared<Texture> {
        let have = tex.0.textureType();
        if have == kind_type(kind) {
            return self.dummy(kind);
        }
        if self.mismatches.insert((kind, have.0 as u64), ()).is_none() {
            eprintln!("[gfx] texture type mismatch: shader declares {kind:?}, texture is {have:?} with {} layers; binding zero", tex.0.arrayLength());
        }
        self.filled(kind, 0)
    }

    fn filled(&mut self, kind: TexKind, fill: u8) -> Shared<Texture> {
        self.dummies
            .entry((kind, fill))
            .or_insert_with(|| {
                let d = MTLTextureDescriptor::new();
                let ty = kind_type(kind);
                let slices = if ty == MTLTextureType::TypeCube || ty == MTLTextureType::TypeCubeArray { 6 } else { 1 };
                unsafe {
                    d.setTextureType(ty);
                    d.setWidth(1);
                    d.setHeight(1);
                }
                if kind.is_depth() {
                    d.setPixelFormat(MTLPixelFormat::Depth32Float);
                    d.setStorageMode(MTLStorageMode::Private);
                } else {
                    d.setPixelFormat(MTLPixelFormat::RGBA8Unorm);
                    d.setStorageMode(MTLStorageMode::Shared);
                }
                let t = gfx::gpu().device.0.newTextureWithDescriptor(&d).expect("dummy texture");
                if !kind.is_depth() {
                    let texel = [fill; 4];
                    for s in 0..slices {
                        unsafe {
                            t.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                                region(1, 1, 1),
                                0,
                                s,
                                std::ptr::NonNull::from(&texel).cast(),
                                4,
                                4,
                            )
                        };
                    }
                }
                Shared(t)
            })
            .clone()
    }

    pub fn sampler(&mut self, b: Option<&[u8; 0x60]>, tex: &Texture) -> Shared<Sampler> {
        let Some(b) = b else {
            return self.default_sampler.get_or_insert_with(|| Shared(make_sampler(None, false))).clone();
        };
        let mut key = [0u8; 0x40];
        key.copy_from_slice(&b[8..0x48]);
        let enhance = ENHANCE.on && tex.mipmapLevelCount() > 1 && !tex.usage().contains(MTLTextureUsage::RenderTarget);
        self.samplers.entry((key, enhance)).or_insert_with(|| Shared(make_sampler(Some(b), enhance))).clone()
    }
}

pub fn kind_type(kind: TexKind) -> MTLTextureType {
    match kind {
        TexKind::T2D | TexKind::Depth2D => MTLTextureType::Type2D,
        TexKind::T2DArray | TexKind::Depth2DArray => MTLTextureType::Type2DArray,
        TexKind::T3D => MTLTextureType::Type3D,
        TexKind::Cube | TexKind::DepthCube => MTLTextureType::TypeCube,
        TexKind::CubeArray => MTLTextureType::TypeCubeArray,
    }
}

fn region(w: u32, h: u32, d: u32) -> MTLRegion {
    MTLRegion { origin: MTLOrigin { x: 0, y: 0, z: 0 }, size: MTLSize { width: w as usize, height: h as usize, depth: d as usize } }
}

struct Piece {
    offset: usize,
    row: usize,
    image: usize,
    size: (u32, u32, u32),
    sub: (u32, u32),
}

fn upload(b: &TexBuilder, key: &TexKey, tex: &Texture, levels: u32) {
    let Some(heap) = gfx::heap() else { return };
    let (bw, bh, bpb) = super::super::format_info(key.format);
    let soft = gfx::bc::soft(key.format);
    let mut pieces = Vec::new();
    let mut total = 0usize;
    let mut piece = |size: (u32, u32, u32), sub: (u32, u32), row: usize, rows: usize| {
        pieces.push(Piece { offset: total, row, image: row * rows, size, sub });
        total = (total + row * rows * size.2 as usize).next_multiple_of(16);
    };
    match super::super::texture_layout(b.as_ptr() as u64) {
        Some((shape, layout)) => {
            if heap.gpu_addr(key.addr).is_none() || heap.gpu_addr(key.addr + layout.total_size - 1).is_none() || shape.width != key.width {
                return;
            }
            let levels = levels.min(layout.level_offsets.len() as u32);
            for layer in 0..shape.layers {
                for level in 0..levels {
                    let (w, h) = ((shape.width >> level).max(1), (shape.height >> level).max(1));
                    let (cols, rows) = (w.div_ceil(bw), h.div_ceil(bh));
                    piece((w, h, (shape.depth >> level).max(1)), (level, layer), (cols * bpb) as usize, rows as usize);
                }
            }
            let src = unsafe { std::slice::from_raw_parts(key.addr as *const u8, layout.total_size as usize) };
            let fill = |dst: &mut [u8]| untile_pieces(src, &layout, &pieces, dst);
            match soft {
                None => stage_with(tex, total, &pieces, fill),
                Some(k) => {
                    let mut data = vec![0u8; total];
                    fill(&mut data);
                    decode_and_stage(tex, k, data, pieces);
                }
            }
        }
        None => {
            let stride = u32_at(b, 0x5c);
            let rows = key.height.div_ceil(bh);
            let row_bytes = key.width.div_ceil(bw) * bpb;
            if stride < row_bytes || heap.gpu_addr(key.addr + (stride * rows) as u64 - 1).is_none() {
                return;
            }
            let src = unsafe { std::slice::from_raw_parts(key.addr as *const u8, (stride * rows) as usize) };
            piece((key.width, key.height, 1), (0, 0), stride as usize, rows as usize);
            match soft {
                None => stage_with(tex, total, &pieces, |dst| dst[..src.len()].copy_from_slice(src)),
                Some(k) => decode_and_stage(tex, k, src.to_vec(), pieces),
            }
        }
    }
}

fn untile_pieces(src: &[u8], layout: &super::super::tex::Layout, pieces: &[Piece], dst: &mut [u8]) {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    const BAND: u32 = 64;
    let mut jobs = Vec::new();
    for (i, p) in pieces.iter().enumerate() {
        let rows = (p.image / p.row.max(1)) as u32;
        for z in 0..p.size.2 {
            let mut y = 0;
            while y < rows {
                jobs.push((i, z, y..(y + BAND).min(rows)));
                y += BAND;
            }
        }
    }
    let layer_of = |p: &Piece| p.sub.1 as u64 * layout.layer_size + layout.level_offsets[p.sub.0 as usize];
    let base = dst.as_mut_ptr() as usize;
    let run = |(i, z, ys): &(usize, u32, std::ops::Range<u32>)| {
        let p = &pieces[*i];
        let rows = (p.image / p.row.max(1)) as u32;
        let start = layer_of(p) as usize;
        let at = p.offset + *z as usize * p.image + ys.start as usize * p.row;
        let out = unsafe { std::slice::from_raw_parts_mut((base + at) as *mut u8, ys.len() * p.row) };
        super::super::tex::untile_into(src.get(start..).unwrap_or(&[]), p.row as u32, rows, *z, layout.level_gobs[p.sub.0 as usize], ys.clone(), out);
    };
    if dst.len() < 1 << 20 || jobs.len() < 4 {
        jobs.iter().for_each(run);
        return;
    }
    let next = AtomicUsize::new(0);
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get()).clamp(2, 6);
    std::thread::scope(|s| {
        for _ in 1..threads {
            s.spawn(|| {
                while let Some(j) = jobs.get(next.fetch_add(1, Relaxed)) {
                    run(j);
                }
            });
        }
        while let Some(j) = jobs.get(next.fetch_add(1, Relaxed)) {
            run(j);
        }
    });
}

fn decode_and_stage(tex: &Texture, k: gfx::bc::Kind, data: Vec<u8>, mut pieces: Vec<Piece>) {
    let mut decoded = Vec::with_capacity(data.len() * 4);
    for p in &mut pieces {
        let (w, h, d) = p.size;
        let rows = (h as usize).div_ceil(4);
        let offset = decoded.len();
        let mut bpr = 0;
        for i in 0..d as usize {
            let Some(img) = data.get(p.offset + i * p.image..) else { break };
            let (px, r) = gfx::bc::decode(k, &img[..img.len().min(p.row * rows)], p.row, w as usize, h as usize, 1);
            decoded.extend_from_slice(&px);
            bpr = r;
        }
        (p.offset, p.row, p.image) = (offset, bpr, bpr * h as usize);
        decoded.resize(decoded.len().next_multiple_of(16), 0);
    }
    stage_with(tex, decoded.len(), &pieces, |dst| dst.copy_from_slice(&decoded));
}

fn stage_with(tex: &Texture, total: usize, pieces: &[Piece], fill: impl FnOnce(&mut [u8])) {
    if pieces.is_empty() || total == 0 {
        return;
    }
    let g = gfx::gpu();
    let Some(buf) = g.device.0.newBufferWithLength_options(total, MTLResourceOptions::StorageModeShared | MTLResourceOptions::CPUCacheModeWriteCombined) else { return };
    let dst = unsafe { std::slice::from_raw_parts_mut(buf.contents().as_ptr() as *mut u8, total) };
    fill(dst);
    let Some(cb) = g.queue.0.commandBuffer() else { return };
    let Some(enc) = cb.blitCommandEncoder() else { return };
    for p in pieces {
        let (w, h, d) = p.size;
        unsafe {
            enc.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                &buf,
                p.offset,
                p.row,
                p.image,
                MTLSize { width: w as usize, height: h as usize, depth: d as usize },
                tex,
                p.sub.1 as usize,
                p.sub.0 as usize,
                MTLOrigin { x: 0, y: 0, z: 0 },
            )
        };
    }
    enc.endEncoding();
    cb.commit();
}

pub fn compare_fn(nvn: u32) -> MTLCompareFunction {
    use MTLCompareFunction as C;
    match nvn {
        1 => C::Never,
        2 => C::Less,
        3 => C::Equal,
        4 => C::LessEqual,
        5 => C::Greater,
        6 => C::NotEqual,
        7 => C::GreaterEqual,
        _ => C::Always,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum MipBlend {
    Game,
    Bilinear,
    Trilinear,
}

struct Enhance {
    on: bool,
    aniso: usize,
    lod_bias: f32,
    mips: MipBlend,
}

const PLAIN: Enhance = Enhance { on: false, aniso: 0, lod_bias: 0.0, mips: MipBlend::Game };

static ENHANCE: std::sync::LazyLock<Enhance> = std::sync::LazyLock::new(|| {
    let mut e = Enhance {
        on: false,
        aniso: std::env::var("NX_ANISO").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(0).min(16),
        lod_bias: std::env::var("NX_LOD_BIAS").ok().and_then(|v| v.parse::<f32>().ok()).filter(|b| b.is_finite()).unwrap_or(0.0).clamp(-4.0, 4.0),
        mips: match std::env::var("NX_TEXTURE_FILTER").as_deref() {
            Ok("bilinear") => MipBlend::Bilinear,
            Ok("trilinear") => MipBlend::Trilinear,
            _ => MipBlend::Game,
        },
    };
    e.on = e.aniso > 1 || e.lod_bias != 0.0 || e.mips != MipBlend::Game;
    if e.on {
        eprintln!("[gfx] samplers: anisotropy {}x, LOD bias {:+}, mip filter {:?}", e.aniso.max(1), e.lod_bias, e.mips);
    }
    e
});

fn make_sampler(b: Option<&[u8; 0x60]>, enhance: bool) -> objc2::rc::Retained<Sampler> {
    let d = MTLSamplerDescriptor::new();
    if let Some(b) = b {
        let min = u32_at(b, 8);
        let linear = matches!(min, 1 | 3 | 5);
        let mipmapped = matches!(min, 2..=5);
        let compare = u32_at(b, 0x28) == 1;
        let fx = if enhance { &*ENHANCE } else { &PLAIN };
        d.setMinFilter(if linear { MTLSamplerMinMagFilter::Linear } else { MTLSamplerMinMagFilter::Nearest });
        d.setMipFilter(match min {
            3 if fx.mips == MipBlend::Trilinear => MTLSamplerMipFilter::Linear,
            5 if fx.mips == MipBlend::Bilinear => MTLSamplerMipFilter::Nearest,
            2 | 3 => MTLSamplerMipFilter::Nearest,
            4 | 5 => MTLSamplerMipFilter::Linear,
            _ => MTLSamplerMipFilter::NotMipmapped,
        });
        d.setMagFilter(if u32_at(b, 0xc) == 1 { MTLSamplerMinMagFilter::Linear } else { MTLSamplerMinMagFilter::Nearest });
        let wrap = |w: u32| match w {
            1 => MTLSamplerAddressMode::Repeat,
            2..=4 => MTLSamplerAddressMode::MirrorClampToEdge,
            5 => MTLSamplerAddressMode::ClampToBorderColor,
            6 => MTLSamplerAddressMode::MirrorRepeat,
            _ => MTLSamplerAddressMode::ClampToEdge,
        };
        d.setSAddressMode(wrap(u32_at(b, 0x10)));
        d.setTAddressMode(wrap(u32_at(b, 0x14)));
        d.setRAddressMode(wrap(u32_at(b, 0x18)));
        let (lo, hi) = (f32_at(b, 0x1c), f32_at(b, 0x20));
        if lo.is_finite() && hi.is_finite() && lo <= hi {
            d.setLodMinClamp(lo.max(0.0));
            d.setLodMaxClamp(hi);
        }
        d.setCompareFunction(if compare { compare_fn(u32_at(b, 0x2c)) } else { MTLCompareFunction::LessEqual });
        let border = [f32_at(b, 0x30), f32_at(b, 0x34), f32_at(b, 0x38), f32_at(b, 0x3c)];
        d.setBorderColor(if border == [1.0; 4] {
            MTLSamplerBorderColor::OpaqueWhite
        } else if border[3] == 1.0 {
            MTLSamplerBorderColor::OpaqueBlack
        } else {
            MTLSamplerBorderColor::TransparentBlack
        });
        let mut bias = f32_at(b, 0x24);
        if !bias.is_finite() {
            bias = 0.0;
        }
        if mipmapped {
            bias += fx.lod_bias;
        }
        if bias != 0.0 && objc2::runtime::NSObjectProtocol::respondsToSelector(&*d, objc2::sel!(setLodBias:)) {
            d.setLodBias(bias.clamp(-16.0, 15.99));
        }
        let aniso = f32_at(b, 0x40);
        let mut aniso = if aniso.is_finite() { (aniso as usize).clamp(1, 16) } else { 1 };
        if mipmapped && linear && !compare {
            aniso = aniso.max(fx.aniso);
        }
        d.setMaxAnisotropy(aniso);
    } else {
        d.setMinFilter(MTLSamplerMinMagFilter::Linear);
        d.setMagFilter(MTLSamplerMinMagFilter::Linear);
        d.setSAddressMode(MTLSamplerAddressMode::Repeat);
        d.setTAddressMode(MTLSamplerAddressMode::Repeat);
    }
    gfx::gpu().device.0.newSamplerStateWithDescriptor(&d).expect("sampler")
}

fn storage_len(b: &TexBuilder, key: &TexKey) -> u64 {
    match super::super::texture_layout(b.as_ptr() as u64) {
        Some((_, l)) => l.total_size,
        None => {
            let (_, bh, _) = super::super::format_info(key.format);
            u32_at(b, 0x5c) as u64 * key.height.div_ceil(bh) as u64
        }
    }
}

const FULL_HASH: u64 = 64 << 10;
const WINDOWS: usize = 16;

fn window(len: u64, frame: u64) -> usize {
    if len <= FULL_HASH { 0 } else { (frame % WINDOWS as u64) as usize }
}

fn memory_hash(addr: u64, len: u64, w: usize) -> u64 {
    let mix = |h: u64, w: u64| (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    if len <= FULL_HASH {
        return mix(len, checksum_at(addr, len & !7));
    }
    let mut h = len ^ w as u64;
    let mut o = 256 * w as u64;
    while o + 256 <= len {
        h = mix(h, checksum_at(addr + o, 256));
        o += 4096;
    }
    h
}

fn checksum_at(addr: u64, len: u64) -> u64 {
    if len == 0 {
        return 0;
    }
    let data = unsafe { std::slice::from_raw_parts(addr as *const u8, len as usize) };
    #[cfg(target_arch = "aarch64")]
    if std::arch::is_aarch64_feature_detected!("crc") {
        return unsafe { crc32_slice(data) };
    }
    let mut h = 0u64;
    for chunk in data.chunks_exact(8) {
        let w = u64::from_le_bytes(chunk.try_into().unwrap());
        h = (h ^ w).wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(29);
    }
    h
}

#[cfg(target_arch = "aarch64")]
#[target_feature(enable = "crc")]
unsafe fn crc32_slice(data: &[u8]) -> u64 {
    use std::arch::aarch64::{__crc32cb, __crc32cd};
    unsafe {
        let (mut a, mut b, mut c, mut d) = (0u32, 1u32, 2u32, 3u32);
        let mut p = data.as_ptr();
        let mut n = data.len();
        while n >= 32 {
            let q = p.cast::<u64>();
            a = __crc32cd(a, q.read_unaligned());
            b = __crc32cd(b, q.add(1).read_unaligned());
            c = __crc32cd(c, q.add(2).read_unaligned());
            d = __crc32cd(d, q.add(3).read_unaligned());
            p = p.add(32);
            n -= 32;
        }
        while n >= 8 {
            a = __crc32cd(a, p.cast::<u64>().read_unaligned());
            p = p.add(8);
            n -= 8;
        }
        while n > 0 {
            b = __crc32cb(b, *p);
            p = p.add(1);
            n -= 1;
        }
        ((a as u64) << 32 | b as u64) ^ ((c as u64) << 32 | d as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
    }
}

fn cpu_writable(b: &TexBuilder) -> bool {
    let pool = u64::from_le_bytes(b[8..16].try_into().unwrap());
    STATE.lock().unwrap().pools.get(&pool).is_some_and(|p| p.flags & 1 == 0)
}

fn memory_hashes(addr: u64, len: u64) -> [u64; WINDOWS] {
    let mut hashes = [0; WINDOWS];
    let n = if len <= FULL_HASH { 1 } else { WINDOWS };
    for (w, h) in hashes.iter_mut().enumerate().take(n) {
        *h = memory_hash(addr, len, w);
    }
    hashes
}

pub fn copy_blocks(cb: &gfx::CommandBuffer, src: &Texture, dst: &Texture, only: Option<(u32, u32)>) {
    let Some(enc) = cb.blitCommandEncoder() else { return };
    let cube = |t: &Texture| if matches!(t.textureType(), MTLTextureType::TypeCube | MTLTextureType::TypeCubeArray) { 6 } else { 1 };
    let slices = (src.arrayLength() * cube(src)).min(dst.arrayLength() * cube(dst));
    let bpb = match src.pixelFormat() {
        MTLPixelFormat::RGBA32Uint | MTLPixelFormat::RGBA32Sint | MTLPixelFormat::RGBA32Float => 16,
        _ => 8,
    };
    let levels = src.mipmapLevelCount().min(dst.mipmapLevelCount());
    let level_size = |level: usize| (src.width() >> level).max(1) * (src.height() >> level).max(1) * bpb * slices;
    let total: usize = (0..levels).map(level_size).sum();
    static SCRATCH: std::sync::Mutex<Option<Shared<gfx::Buffer>>> = std::sync::Mutex::new(None);
    let mut scratch = SCRATCH.lock().unwrap();
    if scratch.as_ref().is_none_or(|b| b.0.length() < total) {
        let Some(b) = gfx::gpu().device.0.newBufferWithLength_options(total.next_power_of_two(), MTLResourceOptions::StorageModePrivate) else {
            enc.endEncoding();
            return;
        };
        *scratch = Some(Shared(b));
    }
    let buf = scratch.as_ref().unwrap().0.clone();
    drop(scratch);
    let mut base = 0;
    for level in 0..levels {
        let (w, h) = ((src.width() >> level).max(1), (src.height() >> level).max(1));
        let (dw, dh) = ((dst.width() >> level).max(1), (dst.height() >> level).max(1));
        let level_base = base;
        base += level_size(level);
        if only.is_some_and(|(_, l)| l as usize != level) {
            continue;
        }
        for slice in 0..slices {
            if only.is_some_and(|(s, _)| s as usize != slice) {
                continue;
            }
            let off = level_base + slice * w * h * bpb;
            unsafe {
                enc.copyFromTexture_sourceSlice_sourceLevel_sourceOrigin_sourceSize_toBuffer_destinationOffset_destinationBytesPerRow_destinationBytesPerImage(
                    src, slice, level, MTLOrigin { x: 0, y: 0, z: 0 }, MTLSize { width: w, height: h, depth: 1 }, &buf, off, w * bpb, w * h * bpb,
                );
                enc.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &buf, off, w * bpb, w * h * bpb, MTLSize { width: dw, height: dh, depth: 1 }, dst, slice, level, MTLOrigin { x: 0, y: 0, z: 0 },
                );
            }
        }
    }
    enc.endEncoding();
}
