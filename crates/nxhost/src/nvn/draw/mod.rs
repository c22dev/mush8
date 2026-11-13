// Mush8 - mod.rs
// Constantin Clerc (c22dev) - 0.1
mod copy;
mod exec;
mod pso4;
mod shaders;
mod state;
mod textures;

use super::cmd::{self, Cmd, Draw, TexBuilder};
use super::objects::STATE;
use crate::gfx::{self, Attachments, Load, RenderEncoder, Shared};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_metal::*;
use state::{Arena, GState, States, Targets};
use std::collections::{BTreeMap, HashMap};
use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
use std::sync::{LazyLock, Mutex};

pub(crate) static TRACE_DRAWS: AtomicBool = AtomicBool::new(false);
static DUMP_NEXT: AtomicBool = AtomicBool::new(false);
static UPPER_LEFT: AtomicBool = AtomicBool::new(false);
static ZERO_TO_ONE: AtomicBool = AtomicBool::new(false);

pub fn set_origin_upper_left(v: bool) {
    UPPER_LEFT.store(v, Relaxed);
}
pub fn set_depth_zero_to_one(v: bool) {
    ZERO_TO_ONE.store(v, Relaxed);
}

#[derive(Default)]
struct Engine {
    st: GState,
    textures: textures::Textures,
    shaders: shaders::Shaders,
    states: States,
    arena: Arena,
    syncs: HashMap<u64, Shared<gfx::CommandBuffer>>,
    last_submit: Option<Shared<gfx::CommandBuffer>>,
    colors: Vec<Option<Box<TexBuilder>>>,
    depth: Option<Box<TexBuilder>>,
    overlays: BTreeMap<u64, (u64, u64, usize)>,
    target_first_frame: HashMap<textures::TexKey, u64>,
    copier: copy::Copier,
    kernels: HashMap<u64, Shared<ProtocolObject<dyn MTLComputePipelineState>>>,
    bank_tex: [[u64; 32]; 3],
    bank_gpu: [u64; 3],
    bank_ok: [bool; 3],
    sampled: HashMap<textures::TexKey, Shared<gfx::Texture>>,
    touched: Vec<(textures::TexKey, Shared<gfx::Texture>)>,
    clears: Vec<PendingClear>,
    draws: u32,
    skipped: u32,
    held: Held,
}

#[derive(Default)]
struct Held {
    last: bool,
}

impl Held {
    fn present(&mut self, skipped: u32, draws: u32) -> bool {
        self.last = !self.last && skipped * 50 >= draws.max(1);
        !self.last
    }
}

struct PendingClear {
    key: textures::TexKey,
    sub: (u32, u32),
    tex: Shared<gfx::Texture>,
    load: Load,
    stencil: bool,
}

impl PendingClear {
    fn is_depth(&self) -> bool {
        matches!(self.load, Load::ClearDepthStencil(..))
    }
}

impl Engine {
    fn forget_dead(&mut self) {
        let dead = std::mem::take(&mut *DEAD.lock().unwrap());
        if !dead.is_empty() {
            let gone = |k: &textures::TexKey| dead.iter().any(|&(lo, hi)| k.addr >= lo && k.addr < hi);
            self.textures.evict(&gone);
            self.target_first_frame.retain(|k, _| !gone(k));
        }
    }
}

static ENGINE: LazyLock<Mutex<Engine>> = LazyLock::new(Default::default);

struct Pass {
    enc: Retained<RenderEncoder>,
    targets: Targets,
    size: (u32, u32),
    heap: u64,
    chunks: usize,
    keys: Vec<(textures::TexKey, (u32, u32))>,
    fresh: bool,
    fixed: Option<EncState>,
}

#[derive(Clone, Copy)]
struct EncState {
    pipeline: usize,
    depth: usize,
    cull: u32,
    winding: u32,
    fill: u32,
    bias: [u32; 3],
    depth_clamp: bool,
    origin: [u32; 4],
    zrange: [u32; 2],
    scissor: [u32; 4],
    stencil: [u32; 2],
    blend: [u32; 4],
}

struct Replay<'a> {
    e: &'a mut Engine,
    cb: Retained<gfx::CommandBuffer>,
    pass: Option<Pass>,
    heap: Option<&'static gfx::HeapBuffer>,
    passes: u32,
    draws_since_commit: u32,
    copies: HashMap<(u64, u64), u64>,
    storage_touched: [[(u64, u64); 16]; 3],
}

impl Replay<'_> {
    fn run(&mut self, cmds: &[Cmd], depth: usize) {
        for c in cmds {
            let st = &mut self.e.st;
            match c {
                Cmd::SetRenderTargets { colors, depth } => {
                    if self.pass.is_some() && self.e.colors == *colors && self.e.depth == *depth {
                        continue;
                    }
                    self.end_pass();
                    self.e.colors = colors.clone();
                    self.e.depth = depth.clone();
                }
                Cmd::ClearColor { index, rgba } => {
                    if !st.render_enable {
                        continue;
                    }
                    self.end_pass();
                    let Some(Some(b)) = self.e.colors.get(*index as usize).cloned() else { continue };
                    if textures::is_compressed(u32::from_le_bytes(b[0x30..0x34].try_into().unwrap())) {
                        continue;
                    }
                    let Some((key, t)) = self.e.textures.get(&b, true) else { continue };
                    if TRACE_DRAWS.load(Relaxed) {
                        eprintln!("[frame] clear color rt{index} {key:?} = {rgba:?}");
                    }
                    let c = rgba.map(|x| x as f64);
                    let sub = super::objects::view_subresource(&b);
                    self.defer_clear(PendingClear { key, sub, tex: t, load: Load::ClearColor(c), stencil: false });
                }
                Cmd::ClearDepthStencil { depth: z, stencil } => {
                    if !st.render_enable {
                        continue;
                    }
                    self.end_pass();
                    let Some(b) = self.e.depth.clone() else { continue };
                    let fmt = u32::from_le_bytes(b[0x30..0x34].try_into().unwrap());
                    let Some((key, t)) = self.e.textures.get(&b, true) else { continue };
                    let sub = super::objects::view_subresource(&b);
                    self.defer_clear(PendingClear { key, sub, tex: t, load: Load::ClearDepthStencil(*z as f64, *stencil), stencil: gfx::has_stencil(fmt) });
                }
                Cmd::Call(handles) => {
                    if depth < 8 {
                        for h in handles {
                            if let Some(sub) = lookup(*h) {
                                self.run(&sub, depth + 1);
                            }
                        }
                    }
                }
                Cmd::Program(p) => st.set_program(p),
                Cmd::VertexAttribs(v) => {
                    for (i, w) in v.iter().take(16).enumerate() {
                        st.attribs[i] = *w;
                    }
                }
                Cmd::VertexStreams(v) => {
                    for (i, s) in v.iter().take(16).enumerate() {
                        st.streams[i] = *s;
                    }
                }
                Cmd::VertexBuffer { index, addr, size } => {
                    if let Some(v) = st.vbufs.get_mut(*index as usize) {
                        *v = (*addr, *size);
                    }
                }
                Cmd::Uniform { stage, index, addr, size } => {
                    if let Some(u) = stage_index(*stage).and_then(|s| st.ubos[s].get_mut(*index as usize)) {
                        *u = (*addr, *size);
                    }
                }
                Cmd::Texture { stage, index, handle } => {
                    if let Some(t) = stage_index(*stage).and_then(|s| st.textures[s].get_mut(*index as usize)) {
                        *t = *handle;
                    }
                }
                Cmd::Storage { stage, index, addr, size } => {
                    if let Some(u) = stage_index(*stage).and_then(|s| st.storage[s].get_mut(*index as usize)) {
                        *u = (*addr, *size);
                    }
                }
                Cmd::UpdateUniform { addr, size, offset, data } => self.update_uniform(*addr, *size, *offset, data),
                Cmd::ColorState(v) => st.color_state = *v,
                Cmd::ChannelMask(v) => st.channel_mask = *v,
                Cmd::Blend(v) => st.blend[(*v & 7) as usize] = *v,
                Cmd::DepthStencil(v) => st.depth_stencil = *v,
                Cmd::Polygon(v) => st.polygon = *v,
                Cmd::Viewport(v) => st.viewport = Some(*v),
                Cmd::Scissor(v) => st.scissor = Some(*v),
                Cmd::BlendColor(v) => st.blend_color = *v,
                Cmd::StencilRef(f, v) => set_faces(&mut st.stencil_ref, *f, *v),
                Cmd::StencilValueMask(f, v) => set_faces(&mut st.stencil_read, *f, *v),
                Cmd::StencilMask(f, v) => set_faces(&mut st.stencil_write, *f, *v),
                Cmd::TexturePool(p) => st.texture_pool = *p,
                Cmd::SamplerPool(p) => st.sampler_pool = *p,
                Cmd::DepthClamp(v) => st.depth_clamp = *v,
                Cmd::RasterizerDiscard(v) => st.rasterizer_discard = *v,
                Cmd::ReportCounter { counter, addr } => {
                    static T0: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
                    let ns = T0.elapsed().as_nanos() as u64;
                    let value = if matches!(*counter, 0 | 16) { ns } else { 0 };
                    if *addr != 0 {
                        unsafe {
                            (*addr as *mut u64).write_unaligned(value);
                            ((*addr + 8) as *mut u64).write_unaligned(ns);
                        }
                    }
                }
                Cmd::DepthRange(n, f) => st.depth_range = (*n, *f),
                Cmd::PolygonOffset(v) => st.polygon_offset = *v,
                Cmd::CopyBufferToTexture { src, dst, region } => {
                    self.end_pass();
                    self.flush_clears();
                    self.copy_buffer_to_texture(*src, dst, *region);
                }
                Cmd::CopyTextureToTexture { src, src_region, dst, dst_region, flags } => {
                    self.end_pass();
                    self.flush_clears();
                    let r = match (self.e.textures.get(src, false), self.e.textures.get(dst, true)) {
                        (Some((_, s)), Some((_, d))) => {
                            let (ss, ds) = (super::objects::view_subresource(src), super::objects::view_subresource(dst));
                            self.e.copier.copy(&self.cb, &s, ss, *src_region, &d, ds, *dst_region, *flags)
                        }
                        _ => Err("copy of an unavailable texture"),
                    };
                    let _ = r;
                }
                Cmd::CopyTextureToBuffer { src, region, dst } => {
                    self.end_pass();
                    self.flush_clears();
                    let format = u32::from_le_bytes(src[0x30..0x34].try_into().unwrap());
                    let r = match self.e.textures.get(src, false) {
                        Some((_, s)) => copy::copy_to_buffer(&self.cb, &s.0, super::objects::view_subresource(src), *region, format, *dst),
                        None => Err("copy of an unavailable texture"),
                    };
                    let _ = r;
                }
                Cmd::ClearTexture { dst, region, color, mask } => {
                    if !st.render_enable {
                        continue;
                    }
                    self.end_pass();
                    self.flush_clears();
                    let r = match self.e.textures.get(dst, true) {
                        Some((_, d)) => self.e.copier.clear(&self.cb, &d, super::objects::view_subresource(dst), *region, *color, *mask),
                        None => Err("clear of an unavailable texture"),
                    };
                    let _ = r;
                }
                Cmd::RenderEnable(v) => st.render_enable = *v,
                Cmd::Dispatch(groups) => {
                    self.end_pass();
                    self.flush_clears();
                    let r = self.dispatch(*groups);
                    let _ = r;
                }
                Cmd::Draw(d) => {
                    self.e.draws += 1;
                    self.draws_since_commit += 1;
                    if self.draw(d).is_err_and(|e| e.ends_with("compiling")) {
                        self.e.skipped += 1;
                    }
                }
            }
        }
    }

    fn copy_buffer_to_texture(&mut self, src: u64, b: &TexBuilder, region: [i32; 6]) {
        let Some((key, tex)) = self.e.textures.get(b, true) else { return };
        let (bw, bh, bpb) = super::format_info(key.format);
        let [x, y, z, w, h, d] = region.map(|v| v.max(0) as usize);
        if w == 0 || h == 0 {
            return;
        }
        let (layer, level) = super::objects::view_subresource(b);
        let row = w.div_ceil(bw as usize) * bpb as usize;
        let image = row * h.div_ceil(bh as usize);
        let layered = matches!(tex.0.textureType(), MTLTextureType::Type2DArray | MTLTextureType::TypeCube | MTLTextureType::TypeCubeArray);
        let len = image * d.max(1);
        let (buf, off) = match gfx::heap().and_then(|h| h.buffer_range(src, len as u64)) {
            Some((chunk, off)) => (chunk.buffer.clone(), off),
            None => {
                let data = unsafe { std::slice::from_raw_parts(src as *const u8, len) };
                let a = self.e.arena.alloc_bytes(data);
                (a.buffer, a.offset)
            }
        };
        let Some(enc) = self.cb.blitCommandEncoder() else { return };
        for i in 0..d.max(1) {
            let (slice, oz, depth) = if layered { (layer as usize + z + i, 0, 1) } else if i == 0 { (0, z, d.max(1)) } else { break };
            unsafe {
                enc.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                    &buf.0,
                    off + i * image,
                    row,
                    image,
                    MTLSize { width: w, height: h, depth },
                    &tex.0,
                    slice,
                    level as usize,
                    MTLOrigin { x, y, z: oz },
                )
            };
        }
        enc.endEncoding();
    }

    fn end_pass(&mut self) {
        if let Some(p) = self.pass.take() {
            p.enc.endEncoding();
            for (k, sub) in &p.keys {
                if let Some((_, src)) = self.e.textures.cached(k) {
                    for (dst, soft) in self.e.textures.aliases_of(k) {
                        textures::refresh_alias(&self.cb, &src.0, &dst.0, *soft, Some(*sub));
                    }
                }
            }
            self.passes += 1;
            if self.passes % PASSES_PER_COMMIT == 0 || (*SPLIT_DRAWS > 0 && self.draws_since_commit >= *SPLIT_DRAWS) {
                self.draws_since_commit = 0;
                if let Some(next) = gfx::gpu().queue.0.commandBuffer() {
                    commit(&std::mem::replace::<Retained<gfx::CommandBuffer>>(&mut self.cb, next));
                }
            }
        }
    }

    fn update_uniform(&mut self, addr: u64, size: u64, offset: u64, data: &[u8]) {
        let size = size.clamp(offset + data.len() as u64, 0x10000);
        let a = self.e.arena.alloc(size as usize);
        let src = match self.e.overlays.get(&addr) {
            Some(&(osize, _, ptr)) if osize >= size => ptr as *const u8,
            _ => addr as *const u8,
        };
        unsafe {
            std::ptr::copy_nonoverlapping(src, a.ptr, size as usize);
            std::ptr::copy_nonoverlapping(data.as_ptr(), a.ptr.add(offset as usize), data.len());
        }
        self.e.overlays.insert(addr, (size, a.gpu, a.ptr as usize));
        if a.new_chunk {
            self.use_chunk(&a.buffer);
        }
    }

    fn use_chunk(&mut self, b: &Shared<state::Buffer>) {
        if gfx::residency() {
            return;
        }
        if let Some(p) = &mut self.pass {
            p.enc.useResource_usage_stages(ProtocolObject::from_ref(&*b.0), MTLResourceUsage::Read, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
            p.chunks += 1;
        }
    }

    fn gpu_range(&mut self, addr: u64, size: u64, overlay: bool) -> u64 {
        if overlay {
            if let Some((&base, &(size, gpu, _))) = self.e.overlays.range(..=addr).next_back() {
                if addr < base + size {
                    return gpu + (addr - base);
                }
            }
        }
        if let Some(h) = self.heap {
            let size = size.min((h.base + h.size).saturating_sub(addr)).max(1);
            if let Some((chunk, off)) = h.buffer_range(addr, size) {
                return chunk.gpu + off as u64;
            }
            if let Some(&gpu) = self.copies.get(&(addr, size)) {
                return gpu;
            }
            if h.contains(addr, size) {
                let data = unsafe { std::slice::from_raw_parts(addr as *const u8, size as usize) };
                let a = self.e.arena.alloc_bytes(data);
                if a.new_chunk {
                    self.use_chunk(&a.buffer);
                }
                self.copies.insert((addr, size), a.gpu);
                return a.gpu;
            }
        }
        gfx::gpu().zeros.0.gpuAddress()
    }

    fn defer_clear(&mut self, c: PendingClear) {
        self.e.clears.retain(|p| !(p.key == c.key && p.sub == c.sub && p.is_depth() == c.is_depth()));
        self.e.clears.push(c);
    }

    fn take_clear(&mut self, key: textures::TexKey, sub: (u32, u32), depth: bool) -> Option<PendingClear> {
        let i = self.e.clears.iter().position(|p| p.key == key && p.sub == sub && p.is_depth() == depth)?;
        Some(self.e.clears.remove(i))
    }

    fn flush_clears(&mut self) {
        for c in std::mem::take(&mut self.e.clears) {
            let a = if c.is_depth() {
                Attachments { colors: vec![], depth: Some((c.tex, c.load, c.stencil)), subs: vec![], depth_sub: c.sub }
            } else {
                Attachments { colors: vec![Some((c.tex, c.load))], depth: None, subs: vec![c.sub], depth_sub: (0, 0) }
            };
            gfx::run_pass(&self.cb, &a);
        }
    }

    fn ensure_pass(&mut self) -> Result<(), &'static str> {
        if self.pass.is_none() {
            let mut colors = Vec::new();
            let mut subs = Vec::new();
            let mut fmts = Vec::new();
            let mut size = None;
            let mut bound = Vec::new();
            for b in self.e.colors.clone() {
                let b = b.filter(|b| !textures::is_compressed(u32::from_le_bytes(b[0x30..0x34].try_into().unwrap())));
                let sub = b.as_ref().map_or((0, 0), |b| super::objects::view_subresource(b));
                let t = b.and_then(|b| self.e.textures.get(&b, true));
                let t = t.filter(|(k, _)| {
                    let dup = bound.contains(&(*k, sub));
                    bound.push((*k, sub));
                    !dup
                });
                fmts.push(t.as_ref().map(|(_, t)| t.0.pixelFormat()));
                colors.push(t.map(|(k, t)| {
                    if !self.e.touched.iter().any(|(tk, _)| *tk == k) {
                        self.e.touched.push((k, t.clone()));
                    }
                    size.get_or_insert(((k.width >> sub.1).max(1), (k.height >> sub.1).max(1)));
                    let load = self.take_clear(k, sub, false).map_or(Load::Keep, |c| c.load);
                    (t, load)
                }));
                subs.push(sub);
            }
            let mut depth = None;
            let mut dfmt = None;
            let mut stencil = false;
            let mut depth_sub = (0, 0);
            if let Some(b) = self.e.depth.clone() {
                if let Some((k, t)) = self.e.textures.get(&b, true) {
                    depth_sub = super::objects::view_subresource(&b);
                    stencil = gfx::has_stencil(k.format);
                    dfmt = Some(t.0.pixelFormat());
                    size.get_or_insert(((k.width >> depth_sub.1).max(1), (k.height >> depth_sub.1).max(1)));
                    let load = self.take_clear(k, depth_sub, true).map_or(Load::Keep, |c| c.load);
                    depth = Some((t, load, stencil));
                }
            }
            let Some(size) = size else { return Err("no render target") };
            let keys: Vec<(textures::TexKey, (u32, u32))> =
                self.e.colors.iter().flatten().filter_map(|b| Some((textures::TexKey::of(b)?, super::objects::view_subresource(b)))).collect();
            let frame = self.e.textures.frame;
            let mut fresh = false;
            for b in self.e.colors.iter().flatten().chain(self.e.depth.iter()) {
                if let Some(k) = textures::TexKey::of(b) {
                    let first = *self.e.target_first_frame.entry(k).or_insert(frame);
                    fresh |= first == frame;
                }
            }
            while fmts.last() == Some(&None) {
                fmts.pop();
            }
            self.flush_clears();
            let enc = gfx::begin_pass(&self.cb, &Attachments { colors, depth, subs, depth_sub }).ok_or("no encoder")?;
            let chunks: Vec<_> = self.e.arena.used.clone();
            if !gfx::residency() {
                for c in &chunks {
                    enc.useResource_usage_stages(ProtocolObject::from_ref(&*c.0), MTLResourceUsage::Read, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
                }
                let z = &gfx::gpu().zeros.0;
                enc.useResource_usage_stages(ProtocolObject::from_ref(&**z), MTLResourceUsage::Read, MTLRenderStages::Vertex | MTLRenderStages::Fragment);
            }
            self.pass = Some(Pass { enc, targets: Targets { colors: fmts, depth: dfmt, stencil }, size, heap: 0, chunks: chunks.len(), fresh, keys, fixed: None });
        }
        Ok(())
    }
}

fn stage_index(nvn_stage: u32) -> Option<usize> {
    match nvn_stage {
        0 => Some(0),
        1 => Some(1),
        5 => Some(2),
        _ => None,
    }
}

fn set_faces(v: &mut [u32; 2], faces: u32, value: u32) {
    if faces & 1 != 0 {
        v[0] = value;
    }
    if faces & 2 != 0 {
        v[1] = value;
    }
}

struct Debug {
    trace_frame: Option<u32>,
    trace_at: Option<f32>,
    trace_count: u32,
    dump_rt_at: Option<f32>,
    dump_frames: Vec<u32>,
    capture_file: bool,
}

static DEBUG: LazyLock<Debug> = LazyLock::new(|| {
    let var = |k: &str| std::env::var(k).ok();
    Debug {
        trace_frame: var("NX_TRACE_FRAME").and_then(|v| v.parse().ok()),
        trace_at: var("NX_TRACE_AT").and_then(|v| v.parse().ok()),
        trace_count: var("NX_TRACE_COUNT").and_then(|v| v.parse().ok()).unwrap_or(1),
        dump_rt_at: var("NX_DUMP_RT_AT").and_then(|v| v.parse().ok()),
        dump_frames: var("NX_DUMP_FRAME").map(|v| v.split(',').filter_map(|f| f.parse().ok()).collect()).unwrap_or_default(),
        capture_file: var("NX_NO_CAPTURE_FILE").is_none(),
    }
});

thread_local! {
    static PINNED: std::cell::RefCell<HashMap<u64, std::sync::Arc<Vec<Cmd>>>> = Default::default();
}

fn lookup(h: u64) -> Option<std::sync::Arc<Vec<Cmd>>> {
    PINNED.with(|p| p.borrow().get(&h).cloned()).or_else(|| cmd::recording(h))
}

fn pin_tree(h: u64, depth: usize, out: &mut HashMap<u64, std::sync::Arc<Vec<Cmd>>>) {
    if out.contains_key(&h) {
        return;
    }
    let Some(c) = cmd::recording(h) else { return };
    out.insert(h, c.clone());
    if depth < 8 {
        for x in c.iter() {
            if let Cmd::Call(hs) = x {
                for &sub in hs {
                    pin_tree(sub, depth + 1, out);
                }
            }
        }
    }
}

mod async_gfx {
    use super::*;
    use std::collections::VecDeque;
    use std::sync::Condvar;
    use std::sync::atomic::{AtomicU64, Ordering::SeqCst};

    pub enum Job {
        Submit { handles: Vec<u64>, pinned: HashMap<u64, std::sync::Arc<Vec<Cmd>>> },
        Fence(u64),
        Present(TexBuilder),
        FreePool(u64, super::super::objects::MemoryPool),
    }

    static QUEUE: Mutex<VecDeque<(u64, Job)>> = Mutex::new(VecDeque::new());
    static WORK: Condvar = Condvar::new();
    static DONE_LOCK: Mutex<()> = Mutex::new(());
    static DONE_CV: Condvar = Condvar::new();
    static ENQ: AtomicU64 = AtomicU64::new(0);
    static DONE: AtomicU64 = AtomicU64::new(0);
    static LAST_PRESENT: AtomicU64 = AtomicU64::new(0);
    pub static FENCES: LazyLock<Mutex<HashMap<u64, u64>>> = LazyLock::new(Default::default);

    pub fn on() -> bool {
        static ON: LazyLock<bool> = LazyLock::new(|| std::env::var("NX_ASYNC_GFX").map_or(cfg!(target_os = "ios"), |v| v.trim() != "0"));
        *ON
    }

    pub fn push(job: Job) -> u64 {
        push_then(job, |_| {})
    }

    pub fn push_then(job: Job, before: impl FnOnce(u64)) -> u64 {
        start();
        let present = matches!(job, Job::Present(_));
        let seq = {
            let mut q = QUEUE.lock().unwrap();
            let seq = ENQ.fetch_add(1, SeqCst) + 1;
            before(seq);
            q.push_back((seq, job));
            seq
        };
        WORK.notify_one();
        if present {
            wait_for(LAST_PRESENT.swap(seq, SeqCst));
        } else if seq > DONE.load(SeqCst) + 512 {
            wait_for(seq - 256);
        }
        seq
    }

    pub fn wait_for(seq: u64) {
        if DONE.load(SeqCst) >= seq {
            return;
        }
        crate::kernel::blocking(|| {
            let mut g = DONE_LOCK.lock().unwrap();
            while DONE.load(SeqCst) < seq {
                g = DONE_CV.wait(g).unwrap();
            }
        });
    }

    pub fn drain() {
        if on() {
            wait_for(ENQ.load(SeqCst));
        }
    }

    pub fn submit(handles: &[u64]) {
        let mut pinned = HashMap::new();
        for &h in handles {
            pin_tree(h, 0, &mut pinned);
        }
        push(Job::Submit { handles: handles.to_vec(), pinned });
    }

    fn start() {
        static ONCE: std::sync::Once = std::sync::Once::new();
        ONCE.call_once(|| {
            std::thread::Builder::new()
                .name("gfx-submit".into())
                .stack_size(8 << 20)
                .spawn(|| {
                    unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INTERACTIVE, 0) };
                    eprintln!("[gfx] graphics on a separate thread");
                    loop {
                        let (seq, job) = {
                            let mut q = QUEUE.lock().unwrap();
                            loop {
                                if let Some(j) = q.pop_front() {
                                    break j;
                                }
                                q = WORK.wait(q).unwrap();
                            }
                        };
                        let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match job {
                            Job::Submit { handles, pinned } => {
                                PINNED.with(|p| *p.borrow_mut() = pinned);
                                submit_now(&handles);
                            }
                            Job::Fence(sync) => fence_now(sync),
                            Job::Present(b) => present_now(b),
                            Job::FreePool(obj, p) => free_pool_now(obj, p),
                        }));
                        PINNED.with(|p| p.borrow_mut().clear());
                        if let Err(e) = r {
                            static WARNED: AtomicBool = AtomicBool::new(false);
                            if !WARNED.swap(true, SeqCst) {
                                let msg = e.downcast_ref::<&str>().map(|s| s.to_string()).or_else(|| e.downcast_ref::<String>().cloned()).unwrap_or_default();
                                eprintln!("[gfx] graphics job panicked, skipping it: {msg}");
                            }
                        }
                        {
                            let _g = DONE_LOCK.lock().unwrap();
                            DONE.store(seq, SeqCst);
                        }
                        DONE_CV.notify_all();
                    }
                })
                .expect("graphics thread");
        });
    }
}

pub fn submit(handles: &[u64]) {
    if async_gfx::on() {
        async_gfx::submit(handles);
    } else {
        submit_now(handles);
    }
}

pub fn free_pool(obj: u64, p: super::objects::MemoryPool) {
    if async_gfx::on() {
        async_gfx::push(async_gfx::Job::FreePool(obj, p));
    } else {
        free_pool_now(obj, p);
    }
}

fn free_pool_now(obj: u64, p: super::objects::MemoryPool) {
    {
        let mut s = STATE.lock().unwrap();
        if s.pools.get(&obj).is_some_and(|q| q.generation == p.generation) {
            s.pools.remove(&obj);
        }
    }
    forget_memory(p.cpu, p.size);
    forget_memory(obj | 1 << 63, p.size);
}

fn submit_now(handles: &[u64]) {
    gfx::wait_foreground();
    static FIRST: std::sync::Once = std::sync::Once::new();
    FIRST.call_once(state::wait_prewarm);
    objc2::rc::autoreleasepool(|_| {
        let mut e = ENGINE.lock().unwrap();
        e.arena.recycle();
        e.forget_dead();
        e.bank_ok = [false; 3];
        e.textures.frame += 1;
        e.textures.maintain();
        shaders::reset_wait_budget();
        let Some(cb) = gfx::gpu().queue.0.commandBuffer() else { return };
        let mut r = Replay { e: &mut e, cb, pass: None, heap: None, passes: 0, draws_since_commit: 0, copies: HashMap::new(), storage_touched: [[(0, 0); 16]; 3] };
        let mut updated = BTreeMap::new();
        let mut programs = std::collections::BTreeSet::new();
        for h in handles {
            if let Some(cmds) = lookup(*h) {
                collect_updates(&cmds, 0, &mut updated, &mut programs);
            }
        }
        r.e.shaders.prefetch(&programs);
        for (&addr, &size) in &updated {
            let size = size.min(0x10000) as usize;
            let src = unsafe { std::slice::from_raw_parts(addr as *const u8, size) };
            let a = r.e.arena.alloc_bytes(src);
            r.e.overlays.insert(addr, (size as u64, a.gpu, a.ptr as usize));
        }
        static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = N.fetch_add(1, Relaxed);
        static T0: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
        static TRACED_AT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let dbg = &*DEBUG;
        let at = dbg.trace_at.is_some_and(|t| T0.elapsed().as_secs_f32() >= t) && TRACED_AT.fetch_add(1, Relaxed) < dbg.trace_count;
        static CAPTURE_LEFT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if dbg.capture_file && n % 32 == 0 {
            if let Ok(v) = std::fs::read_to_string("work/capture") {
                let _ = std::fs::remove_file("work/capture");
                CAPTURE_LEFT.store(v.trim().parse::<u32>().unwrap_or(1).max(1), Relaxed);
            }
        }
        let capture = CAPTURE_LEFT.load(Relaxed) > 0;
        if capture {
            CAPTURE_LEFT.fetch_sub(1, Relaxed);
            DUMP_NEXT.store(true, Relaxed);
        }
        TRACE_DRAWS.store(capture || dbg.trace_frame == Some(n) || at, Relaxed);
        for h in handles {
            if let Some(cmds) = lookup(*h) {
                r.run(&cmds, 0);
            }
        }
        r.end_pass();
        r.flush_clears();
        for (&addr, &(size, _, ptr)) in &r.e.overlays {
            unsafe { std::ptr::copy_nonoverlapping(ptr as *const u8, addr as *mut u8, size as usize) };
        }
        let cb = r.cb;
        commit(&cb);
        static DUMPED_AT: AtomicBool = AtomicBool::new(false);
        let dump_at = dbg.dump_rt_at.is_some_and(|t| T0.elapsed().as_secs_f32() >= t) && !DUMPED_AT.swap(true, Relaxed);
        if DUMP_NEXT.swap(false, Relaxed) || dump_at || dbg.dump_frames.contains(&n) {
            cb.waitUntilCompleted();
            let _ = std::fs::create_dir_all("work/frame");
            for (i, (k, t)) in std::mem::take(&mut e.touched).into_iter().enumerate() {
                let path = format!("work/frame/{n:04}_{i:03}_{:x}_f{}_{}x{}.png", k.addr, k.format, k.width, k.height);
                gfx::dump_png(&t.0, &path);
            }
            let _ = std::fs::create_dir_all("work/frame/tex");
            for (k, t) in std::mem::take(&mut e.sampled) {
                gfx::dump_png(&t.0, &format!("work/frame/tex/{n:04}_{:x}_f{}_{}x{}.png", k.addr, k.format, k.width, k.height));
            }
            eprintln!("[gfx] frame {n} dumped to work/frame");
        }
        e.touched.clear();
        e.sampled.clear();
        let cb = Shared(cb);
        e.arena.retire(cb.clone());
        e.overlays.clear();
        e.last_submit = Some(cb);
    });
}

static DEAD: Mutex<Vec<(u64, u64)>> = Mutex::new(Vec::new());

pub fn forget_memory(addr: u64, size: u64) {
    if size > 0 {
        DEAD.lock().unwrap().push((addr, addr + size));
    }
}

pub fn fence_sync(sync: u64) {
    if async_gfx::on() {
        async_gfx::push_then(async_gfx::Job::Fence(sync), |seq| {
            async_gfx::FENCES.lock().unwrap().insert(sync, seq);
        });
        return;
    }
    fence_now(sync);
}

fn fence_now(sync: u64) {
    let mut e = ENGINE.lock().unwrap();
    if let Some(cb) = e.last_submit.clone() {
        e.syncs.insert(sync, cb);
    }
}

pub use shaders::precompile;
pub use state::prewarm;

pub fn finish() {
    async_gfx::drain();
    let cb = ENGINE.lock().unwrap().last_submit.clone();
    if let Some(cb) = cb {
        crate::kernel::blocking(|| cb.0.waitUntilCompleted());
    }
}

pub fn sync_wait(sync: u64) -> u64 {
    if async_gfx::on() {
        let seq = async_gfx::FENCES.lock().unwrap().remove(&sync);
        if let Some(seq) = seq {
            async_gfx::wait_for(seq);
        }
    }
    let cb = ENGINE.lock().unwrap().syncs.remove(&sync);
    match cb {
        Some(cb) => {
            crate::kernel::blocking(|| cb.0.waitUntilCompleted());
            1
        }
        None => 0,
    }
}

const PASSES_PER_COMMIT: u32 = 24;
static SPLIT_DRAWS: LazyLock<u32> = LazyLock::new(|| std::env::var("NX_SPLIT_DRAWS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(80));

fn commit(cb: &gfx::CommandBuffer) {
    if *FPS {
        let h = block2::RcBlock::new(|cb: std::ptr::NonNull<ProtocolObject<dyn MTLCommandBuffer>>| {
            let cb = unsafe { cb.as_ref() };
            static LAST_END: Mutex<f64> = Mutex::new(0.0);
            let (start, end) = (cb.GPUStartTime(), cb.GPUEndTime());
            let mut last = LAST_END.lock().unwrap();
            GPU_NS.fetch_add(((end - start.max(*last)).max(0.0) * 1e9) as u64, Relaxed);
            *last = last.max(end);
        });
        unsafe { cb.addCompletedHandler(block2::RcBlock::as_ptr(&h)) };
    }
    cb.commit();
}

static FPS: LazyLock<bool> = LazyLock::new(|| std::env::var_os("NX_FPS").is_some());
static GPU_NS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn count_fps() {
    static STATS: Mutex<Option<(std::time::Instant, std::time::Instant, u32, f32)>> = Mutex::new(None);
    if !*FPS {
        return;
    }
    let now = std::time::Instant::now();
    let mut s = STATS.lock().unwrap();
    let (start, last, n, worst) = s.get_or_insert((now, now, 0, 0.0));
    *worst = worst.max(now.duration_since(*last).as_secs_f32() * 1000.0);
    *last = now;
    *n += 1;
    if now.duration_since(*start).as_secs() >= 1 {
        let gpu = GPU_NS.swap(0, Relaxed) as f32 / 1e6 / *n as f32;
        let uploads = textures::UPLOAD_NS.swap(0, Relaxed) as f32 / 1e6;
        eprintln!("[fps] {:.1} fps, worst frame {worst:.1} ms, GPU {gpu:.1} ms/frame, texture uploads {uploads:.0} ms", *n as f32 / now.duration_since(*start).as_secs_f32());
        *s = Some((now, now, 0, 0.0));
    }
}

pub fn present(texture: u64) {
    let Some(b) = super::objects::texture(texture).map(|t| t.builder) else { return };
    if async_gfx::on() {
        async_gfx::push(async_gfx::Job::Present(b));
    } else {
        present_now(b);
    }
}

fn present_now(b: TexBuilder) {
    count_fps();
    objc2::rc::autoreleasepool(|_| {
        let (t, show) = {
            let mut e = ENGINE.lock().unwrap();
            e.forget_dead();
            let (skipped, draws) = (std::mem::take(&mut e.skipped), std::mem::take(&mut e.draws));
            (e.textures.get(&b, true), e.held.present(skipped, draws))
        };
        if let Some((_, t)) = t {
            if show {
                dump_at(&t);
            }
            crate::kernel::blocking(|| gfx::present(show.then_some(&*t.0), !UPPER_LEFT.load(Relaxed)));
        }
    });
}

pub fn write_texels(mut b: TexBuilder, view: u64, region: [i32; 6], data: *const u8, strides: Option<(usize, usize)>) {
    async_gfx::drain();
    super::objects::apply_view(&mut b, view, true);
    objc2::rc::autoreleasepool(|_| {
        let mut e = ENGINE.lock().unwrap();
        e.forget_dead();
        let Some((key, tex)) = e.textures.get(&b, false) else { return };
        let (bw, bh, bpb) = super::format_info(key.format);
        let [x, y, z, w, h, d] = region.map(|v| v.max(0) as usize);
        if w == 0 || h == 0 || data.is_null() {
            return;
        }
        let row = w.div_ceil(bw as usize) * bpb as usize;
        let image = row * h.div_ceil(bh as usize);
        let (row, image) = strides.filter(|s| s.0 >= row).unwrap_or((row, image));
        let decoded = gfx::bc::soft(key.format).map(|k| {
            let blocks = row * h.div_ceil(4);
            (0..d.max(1)).flat_map(|i| gfx::bc::decode(k, unsafe { std::slice::from_raw_parts(data.add(i * image), blocks) }, row, w, h, 1).0).collect::<Vec<u8>>()
        });
        let (data, row, image) = match &decoded {
            Some(px) => (px.as_ptr(), px.len() / d.max(1) / h, px.len() / d.max(1)),
            None => (data, row, image),
        };
        let level = if view != 0 && unsafe { *(view as *const u8) } & 1 != 0 { (unsafe { *((view + 1) as *const u8) }) as usize } else { 0 };
        let layered = matches!(tex.0.textureType(), MTLTextureType::Type2DArray | MTLTextureType::TypeCube | MTLTextureType::TypeCubeArray);
        let slices = if layered { z..z + d.max(1) } else { 0..1 };
        if tex.0.storageMode() == MTLStorageMode::Private {
            let count = if layered { d.max(1) } else { 1 };
            let depth = if layered { 1 } else { d.max(1) };
            let len = image * (count * depth - 1) + row * h.div_ceil(bh as usize);
            let g = gfx::gpu();
            let Some(buf) = (unsafe { g.device.0.newBufferWithBytes_length_options(std::ptr::NonNull::new(data as *mut u8).unwrap().cast(), len, MTLResourceOptions::StorageModeShared) }) else { return };
            let Some(cb) = g.queue.0.commandBuffer() else { return };
            let Some(enc) = cb.blitCommandEncoder() else { return };
            for (i, slice) in slices.enumerate() {
                unsafe {
                    enc.copyFromBuffer_sourceOffset_sourceBytesPerRow_sourceBytesPerImage_sourceSize_toTexture_destinationSlice_destinationLevel_destinationOrigin(
                        &buf,
                        i * image,
                        row,
                        image,
                        MTLSize { width: w, height: h, depth },
                        &tex.0,
                        slice,
                        level,
                        MTLOrigin { x, y, z: if layered { 0 } else { z } },
                    )
                };
            }
            enc.endEncoding();
            cb.commit();
            return;
        }
        for (i, slice) in slices.enumerate() {
            let region = MTLRegion {
                origin: MTLOrigin { x, y, z: if layered { 0 } else { z } },
                size: MTLSize { width: w, height: h, depth: if layered { 1 } else { d.max(1) } },
            };
            let p = unsafe { data.add(i * image) } as *mut u8;
            unsafe {
                tex.0.replaceRegion_mipmapLevel_slice_withBytes_bytesPerRow_bytesPerImage(
                    region,
                    level,
                    slice,
                    std::ptr::NonNull::new(p).unwrap().cast(),
                    row,
                    image,
                )
            };
        }
    });
}

pub fn read_texels(mut b: TexBuilder, view: u64, region: [i32; 6], data: *mut u8, strides: Option<(usize, usize)>) {
    super::objects::apply_view(&mut b, view, true);
    let [_, _, _, w, h, d] = region.map(|v| v.max(0) as usize);
    if w == 0 || h == 0 || data.is_null() {
        return;
    }
    finish();
    objc2::rc::autoreleasepool(|_| {
        let mut e = ENGINE.lock().unwrap();
        e.forget_dead();
        let Some((key, tex)) = e.textures.get(&b, true) else { return };
        drop(e);
        let (bw, bh, bpb) = super::format_info(key.format);
        let row = w.div_ceil(bw as usize) * bpb as usize;
        let image = row * h.div_ceil(bh as usize);
        let len = image * d.max(1);
        let g = gfx::gpu();
        let Some(buf) = g.device.0.newBufferWithLength_options(len, MTLResourceOptions::StorageModeShared) else { return };
        let Some(cb) = g.queue.0.commandBuffer() else { return };
        if copy::copy_into(&cb, &tex.0, super::objects::view_subresource(&b), region, key.format, &buf, 0).is_err() {
            return;
        }
        cb.commit();
        crate::kernel::blocking(|| cb.waitUntilCompleted());
        let src = unsafe { std::slice::from_raw_parts(buf.contents().as_ptr() as *const u8, len) };
        let (dst_row, dst_image) = strides.filter(|s| s.0 >= row).unwrap_or((row, image));
        let rows = h.div_ceil(bh as usize);
        for z in 0..d.max(1) {
            for y in 0..rows {
                let from = &src[z * image + y * row..][..row];
                unsafe { std::ptr::copy_nonoverlapping(from.as_ptr(), data.add(z * dst_image + y * dst_row), row) };
            }
        }
    });
}

fn dump_at(t: &Shared<gfx::Texture>) {
    static TIMES: LazyLock<Mutex<Vec<f32>>> = LazyLock::new(|| {
        let mut v: Vec<f32> = std::env::var("NX_DUMP_AT").unwrap_or_default().split(',').filter_map(|x| x.trim().parse().ok()).collect();
        v.sort_by(|a, b| b.partial_cmp(a).unwrap());
        Mutex::new(v)
    });
    let now = gfx::clock();
    let mut times = TIMES.lock().unwrap();
    if times.last().is_some_and(|&at| now >= at) {
        let at = times.pop().unwrap();
        let _ = std::fs::create_dir_all("work/frame");
        gfx::dump_png(&t.0, &format!("work/frame/t{at:05.1}.png"));
        eprintln!("[gfx] screen at {at}s dumped");
    }
}

fn collect_updates(cmds: &[Cmd], depth: usize, out: &mut BTreeMap<u64, u64>, programs: &mut std::collections::BTreeSet<(u64, u64)>) {
    for c in cmds {
        match c {
            Cmd::UpdateUniform { addr, size, offset, data } => {
                let s = out.entry(*addr).or_default();
                *s = (*s).max(*size).max(offset + data.len() as u64);
            }
            Cmd::Program(p) => programs.extend(p.iter().copied()),
            Cmd::Call(handles) if depth < 8 => {
                for h in handles {
                    if let Some(sub) = lookup(*h) {
                        collect_updates(&sub, depth + 1, out, programs);
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::Held;

    #[test]
    fn only_frames_missing_many_draws_are_held_back() {
        let mut h = Held::default();
        assert!(h.present(0, 3200));
        assert!(!h.present(700, 3200), "the race behind the loading screen, missing a fifth of its draws, is held back");
        assert!(h.present(30, 2500), "a frame missing about 1% of its draws is shown");
        assert!(h.present(13, 1200), "a race frame missing about 1% of its draws is shown");
        assert!(!h.present(4, 143), "a menu fade missing 4 of 143 draws turns grey and is held back");
    }

    #[test]
    fn never_two_frames_in_a_row_are_held_back() {
        let mut h = Held::default();
        assert!(!h.present(700, 3200));
        assert!(h.present(700, 3200), "the second frame in a row is shown, so a hold costs at most one repeated frame");
        assert!(!h.present(700, 3200));
    }
}
