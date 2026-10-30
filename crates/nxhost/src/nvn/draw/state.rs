// Mush8 - state.rs
// Constantin Clerc (c22dev) - 0.1
use super::shaders::Compiled;
use super::textures::compare_fn;
use crate::gfx::{self, Shared};
use objc2_metal::*;
use objc2::rc::Retained;
use objc2_foundation::{NSArray, NSString, NSURL};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};

pub type Pipeline = objc2::runtime::ProtocolObject<dyn MTLRenderPipelineState>;

use super::super::fx::FastMap;
pub type DepthState = objc2::runtime::ProtocolObject<dyn MTLDepthStencilState>;

#[derive(Clone, Copy)]
pub struct GState {
    pub program: [(u64, u64); 8],
    pub program_len: u8,
    pub attribs: [u32; 16],
    pub streams: [(u32, u32); 16],
    pub vbufs: [(u64, u64); 16],
    pub ubos: [[(u64, u64); 15]; 3],
    pub textures: [[u64; 32]; 3],
    pub storage: [[(u64, u64); 16]; 3],
    pub color_state: u32,
    pub channel_mask: u32,
    pub blend: [u64; 8],
    pub depth_stencil: u64,
    pub polygon: u32,
    pub viewport: Option<[f32; 4]>,
    pub scissor: Option<[i32; 4]>,
    pub blend_color: [f32; 4],
    pub stencil_ref: [u32; 2],
    pub stencil_read: [u32; 2],
    pub stencil_write: [u32; 2],
    pub texture_pool: u64,
    pub sampler_pool: u64,
    pub depth_clamp: bool,
    pub rasterizer_discard: bool,
    pub depth_range: (f32, f32),
    pub polygon_offset: [f32; 3],
    pub render_enable: bool,
}

impl Default for GState {
    fn default() -> Self {
        Self {
            program: [(0, 0); 8],
            program_len: 0,
            attribs: [0; 16],
            streams: [(0, 0); 16],
            vbufs: [(0, 0); 16],
            ubos: [[(0, 0); 15]; 3],
            textures: [[0; 32]; 3],
            storage: [[(0, 0); 16]; 3],
            color_state: 0,
            channel_mask: 0xFFFF_FFFF,
            blend: std::array::from_fn(|i| i as u64),
            depth_stencil: 0,
            polygon: 2 << 3 | 1 << 2,
            viewport: None,
            scissor: None,
            blend_color: [0.0; 4],
            stencil_ref: [0; 2],
            stencil_read: [0xFF; 2],
            stencil_write: [0xFF; 2],
            texture_pool: 0,
            sampler_pool: 0,
            depth_clamp: false,
            rasterizer_discard: false,
            depth_range: (0.0, 1.0),
            polygon_offset: [0.0; 3],
            render_enable: true,
        }
    }
}

impl GState {
    pub fn stages(&self) -> &[(u64, u64)] {
        &self.program[..self.program_len as usize]
    }

    pub fn set_program(&mut self, stages: &[(u64, u64)]) {
        let n = stages.len().min(self.program.len());
        self.program[..n].copy_from_slice(&stages[..n]);
        self.program_len = n as u8;
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Res {
    pub cb: [u64; 18],
    pub vb: [u64; 16],
    pub vb_stride: [u32; 16],
    pub vb_divisor: [u32; 16],
    pub attr_format: [u32; 16],
    pub attr_stream: [u32; 16],
    pub attr_offset: [u32; 16],
    pub base_vertex: i32,
    pub base_instance: u32,
    pub flags: u32,
    pub alpha_ref: f32,
    pub alpha_func: u32,
    pub heap_base: u64,
    pub heap_gpu: [u64; gfx::MAX_HEAP_CHUNKS],
    pub heap_size: u64,
    pub heap_chunk_size: u64,
    pub zero_gpu: u64,
}

#[cfg(all(test, target_os = "macos"))]
mod segmented_heap_tests {
    use super::*;
    use objc2::runtime::ProtocolObject;

    #[test]
    #[ignore = "requires a real Metal GPU"]
    fn gpu_reads_and_writes_across_heap_chunks() {
        objc2::rc::autoreleasepool(|_| unsafe {
            let page = crate::mem::PAGE as usize;
            let size = page * 5;
            let chunk = page * 2;
            let mem = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_PRIVATE | libc::MAP_ANON, -1, 0);
            assert_ne!(mem, libc::MAP_FAILED);
            std::env::set_var("NX_GPU_HEAP_CHUNK_BYTES", chunk.to_string());
            let mapped = gfx::wrap_heap(mem as u64, size as u64);
            std::env::remove_var("NX_GPU_HEAP_CHUNK_BYTES");
            assert!(mapped);
            let h = gfx::heap().unwrap();
            assert_eq!(h.live().map(|c| c.size).collect::<Vec<_>>(), vec![chunk as u64, chunk as u64, page as u64]);
            assert!(h.buffer_range(h.base + chunk as u64 - 4, 8).is_none());
            assert!(h.buffer_range(h.base + (size - 4) as u64, 4).is_some());
            assert!(h.buffer_range(h.base + (size - 4) as u64, 8).is_none());
            let offsets = [0, chunk - 4, chunk, chunk * 2 - 4, chunk * 2, size - 4];
            let values = [17u32, 41, 82, 123, 164, 205];
            let words = std::slice::from_raw_parts_mut(mem as *mut u32, size / 4);
            for (&off, &value) in offsets.iter().zip(&values) {
                words[off / 4] = value;
            }
            let dev = &gfx::gpu().device.0;
            let offsets = offsets.map(|v| format!("{v}ul")).join(", ");
            let source = format!("{}\nkernel void heap_probe(constant Res& r [[buffer(0)]], device uint* out [[buffer(1)]]) {{ ulong offsets[6] = {{{offsets}}}; for (uint i = 0; i < 6; ++i) {{ out[i] = *gmem(r, r.heap_base + offsets[i]); *gmem(r, r.heap_base + offsets[i]) = out[i] + 1000u; }} out[6] = *gmem(r, r.heap_base + r.heap_size); out[7] = *gmem(r, r.heap_base - 4ul); }}", nxshader::prelude());
            let lib = dev.newLibraryWithSource_options_error(&NSString::from_str(&source), None).unwrap();
            let func = lib.newFunctionWithName(&NSString::from_str("heap_probe")).unwrap();
            let pipeline = dev.newComputePipelineStateWithFunction_error(&func).unwrap();
            let out = dev.newBufferWithLength_options(32, MTLResourceOptions::StorageModeShared).unwrap();
            let mut res: Res = std::mem::zeroed();
            res.heap_base = h.base;
            res.heap_size = h.size;
            res.heap_chunk_size = h.chunk_size;
            res.heap_gpu = h.gpu_addresses();
            res.zero_gpu = gfx::gpu().zeros.0.gpuAddress();
            *(gfx::gpu().zeros.0.contents().as_ptr() as *mut u32) = 0;
            let cb = gfx::gpu().queue.0.commandBuffer().unwrap();
            let enc = cb.computeCommandEncoder().unwrap();
            enc.setComputePipelineState(&pipeline);
            for chunk in h.live() {
                enc.useResource_usage(ProtocolObject::from_ref(&*chunk.buffer.0), MTLResourceUsage::Read | MTLResourceUsage::Write);
            }
            enc.useResource_usage(ProtocolObject::from_ref(&*gfx::gpu().zeros.0), MTLResourceUsage::Read);
            enc.setBytes_length_atIndex(std::ptr::NonNull::from(&res).cast(), size_of::<Res>(), 0);
            enc.setBuffer_offset_atIndex(Some(&out), 0, 1);
            enc.dispatchThreadgroups_threadsPerThreadgroup(MTLSize { width: 1, height: 1, depth: 1 }, MTLSize { width: 1, height: 1, depth: 1 });
            enc.endEncoding();
            cb.commit();
            cb.waitUntilCompleted();
            assert_eq!(cb.status(), MTLCommandBufferStatus::Completed);
            assert_eq!(std::slice::from_raw_parts(out.contents().as_ptr() as *const u32, 8), &[17, 41, 82, 123, 164, 205, 0, 0]);
            for (off, value) in [0, chunk - 4, chunk, chunk * 2 - 4, chunk * 2, size - 4].into_iter().zip(values) {
                assert_eq!(words[off / 4], value + 1000);
            }
        });
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Targets {
    pub colors: Vec<Option<MTLPixelFormat>>,
    pub depth: Option<MTLPixelFormat>,
    pub stencil: bool,
}

pub(super) fn blend_factor(f: u32) -> MTLBlendFactor {
    use MTLBlendFactor as B;
    match f {
        1 => B::Zero,
        2 => B::One,
        3 => B::SourceColor,
        4 => B::OneMinusSourceColor,
        5 => B::SourceAlpha,
        6 => B::OneMinusSourceAlpha,
        7 => B::DestinationAlpha,
        8 => B::OneMinusDestinationAlpha,
        9 => B::DestinationColor,
        10 => B::OneMinusDestinationColor,
        11 => B::SourceAlphaSaturated,
        16 => B::Source1Color,
        17 => B::OneMinusSource1Color,
        18 => B::Source1Alpha,
        19 => B::OneMinusSource1Alpha,
        0x61 => B::BlendColor,
        0x62 => B::OneMinusBlendColor,
        0x63 => B::BlendAlpha,
        0x64 => B::OneMinusBlendAlpha,
        _ => B::One,
    }
}

pub(super) fn blend_op(e: u32) -> MTLBlendOperation {
    match e {
        2 => MTLBlendOperation::Subtract,
        3 => MTLBlendOperation::ReverseSubtract,
        4 => MTLBlendOperation::Min,
        5 => MTLBlendOperation::Max,
        _ => MTLBlendOperation::Add,
    }
}

fn stencil_op(o: u32) -> MTLStencilOperation {
    use MTLStencilOperation as S;
    match o {
        2 => S::Zero,
        3 => S::Replace,
        4 => S::IncrementClamp,
        5 => S::DecrementClamp,
        6 => S::Invert,
        7 => S::IncrementWrap,
        8 => S::DecrementWrap,
        _ => S::Keep,
    }
}

#[derive(Clone, Copy)]
struct PipeKey {
    words: [u64; 32],
    len: u8,
}

impl PipeKey {
    fn new() -> Self {
        Self { words: [0; 32], len: 0 }
    }

    fn push(&mut self, w: u64) {
        if (self.len as usize) < self.words.len() {
            self.words[self.len as usize] = w;
            self.len += 1;
        }
    }
}

impl PartialEq for PipeKey {
    fn eq(&self, other: &Self) -> bool {
        self.words[..self.len as usize] == other.words[..other.len as usize]
    }
}
impl Eq for PipeKey {}
impl Hash for PipeKey {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.words[..self.len as usize].hash(state);
    }
}

#[derive(Default)]
pub struct States {
    pipelines: FastMap<PipeKey, Option<Shared<Pipeline>>>,
    pending: super::shaders::Jobs<PipeKey>,
    depth: FastMap<(u64, [u32; 4], bool, bool), Shared<DepthState>>,
}

static DONE: LazyLock<Mutex<Vec<(PipeKey, Option<Shared<Pipeline>>)>>> = LazyLock::new(Default::default);
enum Slot {
    Building,
    Ready(Shared<Pipeline>),
    Claimed,
}

static PREWARMED: LazyLock<Mutex<HashMap<Recorded, Slot>>> = LazyLock::new(Default::default);
static PREWARM_CHANGED: Condvar = Condvar::new();
const PREWARM_WAIT: std::time::Duration = std::time::Duration::from_millis(100);

struct Building<'a>(&'a Recorded);

impl Drop for Building<'_> {
    fn drop(&mut self) {
        let mut m = PREWARMED.lock().unwrap_or_else(PoisonError::into_inner);
        if matches!(m.get(self.0), Some(Slot::Building)) {
            m.remove(self.0);
            PREWARM_CHANGED.notify_all();
        }
    }
}

enum Claim {
    Ready(Shared<Pipeline>, Option<std::time::Duration>),
    Compiling,
    Build,
    Record,
}

fn claim(r: &Recorded, wait: bool, block: fn(&mut dyn FnMut())) -> Claim {
    let mut waited = None;
    let mut m = PREWARMED.lock().unwrap_or_else(PoisonError::into_inner);
    loop {
        match m.get(r) {
            Some(Slot::Ready(_)) => {
                let Some(Slot::Ready(p)) = m.insert(r.clone(), Slot::Claimed) else { unreachable!() };
                return Claim::Ready(p, waited.map(|t0: std::time::Instant| t0.elapsed()));
            }
            Some(Slot::Building) if !wait => return Claim::Compiling,
            Some(Slot::Building) => {
                let left = PREWARM_WAIT.saturating_sub(waited.get_or_insert_with(std::time::Instant::now).elapsed());
                if left.is_zero() {
                    m.insert(r.clone(), Slot::Claimed);
                    return Claim::Build;
                }
                drop(m);
                let mut woke = None;
                block(&mut || {
                    let m = PREWARMED.lock().unwrap_or_else(PoisonError::into_inner);
                    woke = Some(PREWARM_CHANGED.wait_timeout_while(m, left, |m| matches!(m.get(r), Some(Slot::Building))).unwrap_or_else(PoisonError::into_inner).0);
                });
                m = woke.unwrap_or_else(|| PREWARMED.lock().unwrap_or_else(PoisonError::into_inner));
            }
            Some(Slot::Claimed) => return Claim::Build,
            None => {
                m.insert(r.clone(), Slot::Claimed);
                return Claim::Record;
            }
        }
    }
}

impl States {
    pub fn pipeline(&mut self, vs: &Arc<Compiled>, vs_key: (u64, u32), fs: &Arc<Compiled>, fs_key: u64, t: &Targets, st: &GState, wait: bool) -> Result<Shared<Pipeline>, &'static str> {
        let mut key = PipeKey::new();
        for w in [vs_key.0, vs_key.1 as u64, fs_key, t.depth.map_or(0, |f| f.0 as u64), t.stencil as u64] {
            key.push(w);
        }
        for (i, c) in t.colors.iter().enumerate() {
            let enabled = st.color_state >> (8 + i) & 1 != 0;
            let mask = st.channel_mask >> (4 * i) & 0xF;
            key.push(c.map_or(0, |f| f.0 as u64) | (enabled as u64) << 32 | (mask as u64) << 33);
            if enabled {
                key.push(st.blend[i]);
            }
        }
        let t0 = std::time::Instant::now();
        let (cs, cm, blend) = (st.color_state, st.channel_mask, st.blend);
        let name = format!("{:016x}/{fs_key:016x}", vs_key.0);
        let job = || pipeline_job(key, vs, fs, t, cs, cm, blend, name.clone());
        loop {
            if !self.pending.is_empty() {
                self.drain();
            }
            if let Some(p) = self.pipelines.get(&key) {
                return p.clone().ok_or("pipeline failed");
            }
            if !self.pending.contains(&key) {
                break;
            }
            if !wait || super::shaders::wait_over() {
                return Err("pipeline compiling");
            }
            self.pending.spawn(key, true, job);
            crate::kernel::blocking(|| std::thread::sleep(std::time::Duration::from_millis(1)));
        }
        if let Some(r) = Recorded::of(vs, fs, t, cs, cm, &blend) {
            let wait = wait || super::shaders::sync_mode();
            match claim(&r, wait, |f| crate::kernel::blocking(f)) {
                Claim::Ready(p, waited) => {
                    if let Some(elapsed) = waited.filter(|d| d.as_millis() >= 5) {
                        eprintln!("[gfx] pipeline {:016x}/{fs_key:016x}: waited {} ms for prewarm", vs_key.0, elapsed.as_millis());
                    }
                    self.pipelines.insert(key, Some(p.clone()));
                    return Ok(p);
                }
                Claim::Compiling => return Err("pipeline compiling"),
                Claim::Build => {}
                Claim::Record => record(r),
            }
        }
        if let Some(p) = super::pso4::get().and_then(|p4| p4.pipeline(vs, fs, t, cs, cm, &blend, false)) {
            self.pipelines.insert(key, Some(p.clone()));
            return Ok(p);
        }
        if super::shaders::sync_mode() {
            let p = crate::kernel::blocking(|| build_pipeline(vs, fs, t, cs, cm, &blend, &name));
            self.pipelines.insert(key, p.clone());
            return p.ok_or("pipeline failed");
        }
        self.pending.spawn(key, wait, job);
        if !wait {
            return Err("pipeline compiling");
        }
        loop {
            self.drain();
            if let Some(p) = self.pipelines.get(&key) {
                if t0.elapsed().as_millis() >= 5 {
                    eprintln!("[gfx] pipeline {name}: built while the render thread waited (new render target), {} ms", t0.elapsed().as_millis());
                }
                return p.clone().ok_or("pipeline failed");
            }
            if super::shaders::wait_over() {
                if t0.elapsed().as_millis() >= 5 {
                    eprintln!("[gfx] pipeline {name}: still building after {} ms, drawing without it for now", t0.elapsed().as_millis());
                }
                return Err("pipeline compiling");
            }
            crate::kernel::blocking(|| std::thread::sleep(std::time::Duration::from_millis(1)));
        }
    }

    fn drain(&mut self) {
        for (k, p) in std::mem::take(&mut *DONE.lock().unwrap()) {
            self.pending.remove(&k);
            self.pipelines.entry(k).or_insert(p);
        }
    }

    pub fn depth_stencil(&mut self, st: &GState, has_depth: bool, has_stencil: bool) -> Shared<DepthState> {
        let masks = [st.stencil_read[0], st.stencil_read[1], st.stencil_write[0], st.stencil_write[1]];
        let key = (st.depth_stencil, masks, has_depth, has_stencil);
        self.depth
            .entry(key)
            .or_insert_with(|| {
                let (w0, w1) = (st.depth_stencil as u32, (st.depth_stencil >> 32) as u32);
                let d = MTLDepthStencilDescriptor::new();
                if has_depth && w0 & 1 != 0 {
                    d.setDepthCompareFunction(compare_fn(w0 >> 4 & 0xF));
                    d.setDepthWriteEnabled(w0 & 2 != 0);
                }
                if has_stencil && w0 & 4 != 0 {
                    for face in 0..2 {
                        let f = w1 >> (16 * face);
                        let s = MTLStencilDescriptor::new();
                        s.setStencilCompareFunction(compare_fn(f & 0xF));
                        s.setStencilFailureOperation(stencil_op(f >> 4 & 0xF));
                        s.setDepthStencilPassOperation(stencil_op(f >> 8 & 0xF));
                        s.setDepthFailureOperation(stencil_op(f >> 12 & 0xF));
                        s.setReadMask(masks[face as usize]);
                        s.setWriteMask(masks[2 + face as usize]);
                        if face == 0 {
                            d.setFrontFaceStencil(Some(&s));
                        } else {
                            d.setBackFaceStencil(Some(&s));
                        }
                    }
                }
                Shared(gfx::gpu().device.0.newDepthStencilStateWithDescriptor(&d).expect("depth stencil state"))
            })
            .clone()
    }
}

#[allow(clippy::too_many_arguments)]
fn pipeline_job(key: PipeKey, vs: &Arc<Compiled>, fs: &Arc<Compiled>, t: &Targets, cs: u32, cm: u32, blend: [u64; 8], name: String) -> Box<dyn FnOnce() + Send> {
    let (vs, fs, t) = (vs.clone(), fs.clone(), t.clone());
    Box::new(move || {
        let p = build_pipeline(&vs, &fs, &t, cs, cm, &blend, &name);
        DONE.lock().unwrap().push((key, p));
    })
}

fn build_pipeline(vs: &Compiled, fs: &Compiled, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8], name: &str) -> Option<Shared<Pipeline>> {
    let t0 = std::time::Instant::now();
    let r = build_pipeline_inner(vs, fs, t, color_state, channel_mask, blend, name);
    let ms = t0.elapsed().as_millis();
    if ms > 500 {
        eprintln!("[gfx] pipeline {name}: building took {ms} ms");
    }
    r
}

fn build_pipeline_inner(vs: &Compiled, fs: &Compiled, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8], name: &str) -> Option<Shared<Pipeline>> {
    if let Some(p) = super::pso4::get().and_then(|p4| p4.pipeline(vs, fs, t, color_state, channel_mask, blend, true)) {
        return Some(p);
    }
    let d = descriptor(&vs.func.0, &fs.func.0, t, color_state, channel_mask, blend);
    let dev = &gfx::gpu().device.0;
    if let Some(a) = &*ARCHIVES {
        use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
        static HITS: AtomicUsize = AtomicUsize::new(0);
        static MISSES: AtomicUsize = AtomicUsize::new(0);
        d.setBinaryArchives(Some(&a.0));
        if let Ok(p) = dev.newRenderPipelineStateWithDescriptor_options_reflection_error(&d, MTLPipelineOption::FailOnBinaryArchiveMiss, None) {
            HITS.fetch_add(1, Relaxed);
            return Some(Shared(p));
        }
        let misses = MISSES.fetch_add(1, Relaxed) + 1;
        eprintln!("[gfx] pipeline {name}: not in the archive, compiling ({} archive hits, {misses} misses)", HITS.load(Relaxed));
    }
    match dev.newRenderPipelineStateWithDescriptor_error(&d) {
        Ok(p) => Some(Shared(p)),
        Err(e) => {
            eprintln!("[gfx] pipeline {name} failed: {}", e.localizedDescription());
            None
        }
    }
}

pub type Buffer = gfx::Buffer;
const CHUNK: usize = 16 << 20;

#[derive(Default)]
pub struct Arena {
    free: Vec<Shared<Buffer>>,
    pub used: Vec<Shared<Buffer>>,
    offset: usize,
    in_flight: Vec<(Shared<gfx::CommandBuffer>, Vec<Shared<Buffer>>)>,
}

pub struct Alloc {
    pub buffer: Shared<Buffer>,
    pub offset: usize,
    pub ptr: *mut u8,
    pub gpu: u64,
    pub new_chunk: bool,
}

impl Arena {
    pub fn alloc(&mut self, size: usize) -> Alloc {
        let size = size.max(1).next_multiple_of(256);
        let mut new_chunk = false;
        if self.used.last().is_none_or(|b| self.offset + size > b.0.length()) {
            let b = self.free.iter().position(|b| b.0.length() >= size).map(|i| self.free.swap_remove(i)).unwrap_or_else(|| {
                let dev = &gfx::gpu().device.0;
                let b = dev.newBufferWithLength_options(size.max(CHUNK), MTLResourceOptions::StorageModeShared).expect("scratch buffer");
                gfx::make_resident(&b);
                Shared(b)
            });
            self.used.push(b);
            self.offset = 0;
            new_chunk = true;
        }
        let b = self.used.last().unwrap().clone();
        let offset = self.offset;
        self.offset += size;
        let ptr = unsafe { (b.0.contents().as_ptr() as *mut u8).add(offset) };
        let gpu = b.0.gpuAddress() + offset as u64;
        Alloc { buffer: b, offset, ptr, gpu, new_chunk }
    }

    pub fn alloc_bytes(&mut self, data: &[u8]) -> Alloc {
        let a = self.alloc(data.len());
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), a.ptr, data.len()) };
        a
    }

    pub fn retire(&mut self, cb: Shared<gfx::CommandBuffer>) {
        let used = std::mem::take(&mut self.used);
        self.offset = 0;
        if !used.is_empty() {
            self.in_flight.push((cb, used));
        }
    }

    pub fn recycle(&mut self) {
        let mut i = 0;
        while i < self.in_flight.len() {
            if self.in_flight[i].0.0.status() == MTLCommandBufferStatus::Completed {
                let (_, bufs) = self.in_flight.swap_remove(i);
                self.free.extend(bufs);
            } else {
                i += 1;
            }
        }
    }
}

type Archive = objc2::runtime::ProtocolObject<dyn MTLBinaryArchive>;
type Function = objc2::runtime::ProtocolObject<dyn MTLFunction>;

const PIPELINE_LIST: &str = "data/pipelines.tsv";
const USER_LIST: &str = "work/pipelines.tsv";
// Metal loads an archive in time quadratic in its pipeline count (minutes for 8k), so the archive is split
const ARCHIVE_CHUNK: usize = 256;

static ARCHIVES: LazyLock<Option<Shared<NSArray<Archive>>>> = LazyLock::new(|| {
    if std::env::var_os("NX_PIPELINE_ARCHIVE").is_none() {
        return None;
    }
    let dir = super::shaders::aot_dir();
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| is_archive(n))
        .collect();
    files.sort();
    let t0 = std::time::Instant::now();
    let archives: Vec<Retained<Archive>> = files
        .iter()
        .filter_map(|f| {
            let d = MTLBinaryArchiveDescriptor::new();
            d.setUrl(Some(&NSURL::fileURLWithPath(&NSString::from_str(&format!("{dir}/{f}")))));
            gfx::gpu().device.0.newBinaryArchiveWithDescriptor_error(&d).ok()
        })
        .collect();
    eprintln!("[gfx] {} pipeline archive(s) loaded in {:.1} s", archives.len(), t0.elapsed().as_secs_f32());
    (!archives.is_empty()).then(|| Shared(NSArray::from_retained_slice(&archives)))
});

fn is_archive(name: &str) -> bool {
    name.starts_with("pipelines") && name.ends_with(".metalar")
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct Recorded {
    vs: (u64, [u32; 3]),
    fs: (u64, [u32; 3]),
    colors: Vec<u64>,
    depth: u64,
    stencil: bool,
    color_state: u32,
    channel_mask: u32,
    blend: [u64; 8],
}

impl Recorded {
    fn of(vs: &Compiled, fs: &Compiled, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8]) -> Option<Self> {
        Some(Self {
            vs: vs.source?,
            fs: fs.source?,
            colors: t.colors.iter().map(|c| c.map_or(0, |f| f.0 as u64)).collect(),
            depth: t.depth.map_or(0, |f| f.0 as u64),
            stencil: t.stencil,
            color_state,
            channel_mask,
            blend: *blend,
        })
    }

    fn line(&self) -> String {
        let v = |(k, [a, b, c]): (u64, [u32; 3])| format!("{k:016x}:{a:x}:{b:x}:{c:x}");
        let list = |x: &[u64]| x.iter().map(|n| format!("{n:x}")).collect::<Vec<_>>().join(",");
        format!(
            "{}\t{}\t{}\t{:x}\t{}\t{:x}\t{:x}\t{}",
            v(self.vs),
            v(self.fs),
            list(&self.colors),
            self.depth,
            self.stencil as u8,
            self.color_state,
            self.channel_mask,
            list(&self.blend)
        )
    }

    fn parse(l: &str) -> Option<Self> {
        let f: Vec<&str> = l.split('\t').collect();
        let hex = |s: &str| u64::from_str_radix(s, 16).ok();
        let list = |s: &str| if s.is_empty() { Some(Vec::new()) } else { s.split(',').map(hex).collect::<Option<Vec<u64>>>() };
        let v = |s: &str| {
            let p: Vec<u64> = s.split(':').map(hex).collect::<Option<_>>()?;
            (p.len() == 4).then(|| (p[0], [p[1] as u32, p[2] as u32, p[3] as u32]))
        };
        let blend: Vec<u64> = list(f.get(7)?)?;
        Some(Self {
            vs: v(f.first()?)?,
            fs: v(f.get(1)?)?,
            colors: list(f.get(2)?)?,
            depth: hex(f.get(3)?)?,
            stencil: *f.get(4)? == "1",
            color_state: hex(f.get(5)?)? as u32,
            channel_mask: hex(f.get(6)?)? as u32,
            blend: blend.try_into().ok()?,
        })
    }

    fn targets(&self) -> Targets {
        let fmt = |n: u64| (n != 0).then_some(MTLPixelFormat(n as usize));
        Targets { colors: self.colors.iter().map(|&n| fmt(n)).collect(), depth: fmt(self.depth), stencil: self.stencil }
    }
}

fn read_list() -> Vec<Recorded> {
    let shipped = std::env::var("NX_PIPELINE_LIST").unwrap_or_else(|_| PIPELINE_LIST.into());
    let mut seen = std::collections::HashSet::new();
    [shipped.as_str(), USER_LIST]
        .iter()
        .flat_map(|p| std::fs::read_to_string(p).unwrap_or_default().lines().filter_map(Recorded::parse).collect::<Vec<_>>())
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

#[derive(Default)]
struct Warm {
    started: bool,
    finished: bool,
    done: usize,
    total: usize,
}

static WARM: Mutex<Warm> = Mutex::new(Warm { started: false, finished: false, done: 0, total: 0 });
static WARM_CHANGED: Condvar = Condvar::new();

fn warm_step(finished: bool) {
    let mut w = WARM.lock().unwrap_or_else(PoisonError::into_inner);
    if finished {
        w.finished = true;
    } else {
        w.done += 1;
    }
    WARM_CHANGED.notify_all();
}

pub fn prewarm() {
    {
        let mut w = WARM.lock().unwrap_or_else(PoisonError::into_inner);
        if w.started {
            return;
        }
        w.started = true;
    }
    std::thread::Builder::new()
        .name("prewarm".into())
        .spawn(|| {
            use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
            let t0 = std::time::Instant::now();
            let list = read_list();
            WARM.lock().unwrap_or_else(PoisonError::into_inner).total = list.len();
            let tc = std::time::Instant::now();
            let kernels = super::shaders::warm_compute();
            if kernels > 0 {
                eprintln!("[gfx] prewarmed {kernels} compute shaders in {:.1} s", tc.elapsed().as_secs_f32());
            }
            let (next, built) = (AtomicUsize::new(0), AtomicUsize::new(0));
            let workers = std::env::var("NX_PREWARM_THREADS").ok().and_then(|v| v.parse().ok()).unwrap_or_else(|| std::thread::available_parallelism().map_or(4, |n| n.get())).max(1);
            std::thread::scope(|s| {
                for _ in 0..workers {
                    s.spawn(|| {
                        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0) };
                        super::pso4::set_prewarming(true);
                        while let Some(r) = list.get(next.fetch_add(1, Relaxed)) {
                            {
                                let mut m = PREWARMED.lock().unwrap_or_else(PoisonError::into_inner);
                                if m.contains_key(r) {
                                    drop(m);
                                    warm_step(false);
                                    continue;
                                }
                                m.insert(r.clone(), Slot::Building);
                            }
                            let _building = Building(r);
                            objc2::rc::autoreleasepool(|_| {
                                let (Some(vs), Some(fs)) = (super::shaders::aot_compiled(r.vs), super::shaders::aot_compiled(r.fs)) else { return };
                                let name = format!("prewarm {:016x}/{:016x}", r.vs.0, r.fs.0);
                                if let Some(p) = build_pipeline(&vs, &fs, &r.targets(), r.color_state, r.channel_mask, &r.blend, &name) {
                                    let mut m = PREWARMED.lock().unwrap_or_else(PoisonError::into_inner);
                                    if matches!(m.get(r), Some(Slot::Building)) {
                                        m.insert(r.clone(), Slot::Ready(p));
                                        PREWARM_CHANGED.notify_all();
                                    }
                                    built.fetch_add(1, Relaxed);
                                }
                            });
                            warm_step(false);
                        }
                    });
                }
            });
            eprintln!("[gfx] prewarmed {}/{} recorded pipelines in {:.1} s on {workers} workers", built.into_inner(), list.len(), t0.elapsed().as_secs_f32());
            if let Some(p4) = super::pso4::get() {
                objc2::rc::autoreleasepool(|_| p4.prewarm_done());
            }
            warm_step(true);
        })
        .unwrap();
}

pub fn wait_prewarm() {
    if std::env::var_os("NX_PREWARM_WAIT").is_some_and(|v| v == "0") {
        return;
    }
    let t0 = std::time::Instant::now();
    let mut shown = false;
    crate::kernel::blocking(|| {
        let mut w = WARM.lock().unwrap_or_else(PoisonError::into_inner);
        while w.started && !w.finished {
            w = WARM_CHANGED.wait_timeout(w, std::time::Duration::from_millis(50)).unwrap_or_else(PoisonError::into_inner).0;
            let elapsed = t0.elapsed().as_secs_f32();
            let frac = if w.total == 0 { 0.0 } else { w.done as f32 / w.total as f32 };
            let left = if frac > 0.0 { elapsed / frac - elapsed } else { f32::MAX };
            if !w.finished && elapsed >= 0.5 && (shown || left >= 0.3) {
                drop(w);
                shown = true;
                objc2::rc::autoreleasepool(|_| gfx::show_progress(frac));
                w = WARM.lock().unwrap_or_else(PoisonError::into_inner);
            }
        }
    });
    if shown || t0.elapsed().as_millis() >= 50 {
        eprintln!("[gfx] first frame waited {:.1} s for the pipeline prewarm", t0.elapsed().as_secs_f32());
    }
}

fn record(r: Recorded) {
    static TARGET: LazyLock<&str> = LazyLock::new(|| if std::env::var_os("NX_RECORD_PIPELINES").is_some() { PIPELINE_LIST } else { USER_LIST });
    static KNOWN: LazyLock<Mutex<std::collections::HashSet<Recorded>>> = LazyLock::new(|| Mutex::new(read_list().into_iter().collect()));
    if KNOWN.lock().unwrap().insert(r.clone()) {
        use std::io::Write;
        if let Some(dir) = std::path::Path::new(*TARGET).parent() {
            let _ = std::fs::create_dir_all(dir);
        }
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(*TARGET) {
            let _ = writeln!(f, "{}", r.line());
        }
    }
}

fn add(archive: &Shared<Archive>, d: &MTLRenderPipelineDescriptor) -> bool {
    archive.0.addRenderPipelineFunctionsWithDescriptor_error(d).is_ok()
}

type Variant = (u64, [u32; 3]);

pub fn precompile_pipelines(programs: &std::collections::BTreeMap<u64, Vec<u8>>, known_pairs: &[(Variant, Variant)]) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let t0 = std::time::Instant::now();
    let list = read_list();
    let function = |(key, [extra, uints, sints]): (u64, [u32; 3])| {
        let blob = programs.get(&key)?;
        super::shaders::compile(blob, false, extra, uints, sints, key)
    };
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    if let Some(pre) = super::pso4::Prebuild::new() {
        let mut pairs: Vec<(Variant, Variant)> = list.iter().map(|r| (r.vs, r.fs)).collect();
        if std::env::var_os("NX_PREBUILD_PAIRS").is_none_or(|v| v != "0") {
            pairs.extend_from_slice(known_pairs);
        }
        pairs.sort();
        pairs.dedup();
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let Some(&(v, f)) = pairs.get(next.fetch_add(1, Relaxed)) else { return };
                    objc2::rc::autoreleasepool(|_| {
                        if let (Some(vs), Some(fs)) = (function(v), function(f)) {
                            pre.add(&vs, &fs);
                        }
                    });
                });
            }
        });
        let (built, failed) = (pre.built.load(Relaxed), pre.failed.load(Relaxed));
        objc2::rc::autoreleasepool(|_| pre.finish());
        eprintln!("[gfx] prebuilt {built}/{} pipeline bases ({failed} failed) in {:.0} s", pairs.len(), t0.elapsed().as_secs_f32());
    }
    if std::env::var_os("NX_PIPELINE_ARCHIVE").is_none() || list.is_empty() {
        return Ok(());
    }
    let archives = (0..list.len().div_ceil(ARCHIVE_CHUNK))
        .map(|_| gfx::gpu().device.0.newBinaryArchiveWithDescriptor_error(&MTLBinaryArchiveDescriptor::new()).map(Shared))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))?;
    let (next, built) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Relaxed);
                let Some(r) = list.get(i) else { return };
                objc2::rc::autoreleasepool(|_| {
                    let (Some(vs), Some(fs)) = (function(r.vs), function(r.fs)) else { return };
                    let d = descriptor(&vs.func.0, &fs.func.0, &r.targets(), r.color_state, r.channel_mask, &r.blend);
                    if add(&archives[i / ARCHIVE_CHUNK], &d) {
                        built.fetch_add(1, Relaxed);
                    }
                });
            });
        }
    });
    let dir = super::shaders::aot_dir();
    for f in std::fs::read_dir(dir)?.flatten() {
        if is_archive(&f.file_name().to_string_lossy()) {
            std::fs::remove_file(f.path())?;
        }
    }
    for (i, archive) in archives.iter().enumerate() {
        let path = format!("{dir}/pipelines-{i:04}.metalar");
        archive.0.serializeToURL_error(&NSURL::fileURLWithPath(&NSString::from_str(&path))).map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))?;
    }
    eprintln!("[gfx] precompiled {}/{} recorded pipelines in {:.0} s", built.into_inner(), list.len(), t0.elapsed().as_secs_f32());
    Ok(())
}

pub fn recorded_pairs() -> Vec<(Variant, Variant)> {
    read_list().iter().map(|r| (r.vs, r.fs)).collect()
}

pub fn recorded_variants() -> Vec<(u64, [u32; 3])> {
    let mut v: Vec<(u64, [u32; 3])> = read_list().iter().flat_map(|r| [r.vs, r.fs]).filter(|(_, p)| *p != [0; 3]).collect();
    v.sort();
    v.dedup();
    v
}

fn descriptor(vs: &Function, fs: &Function, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8]) -> Retained<MTLRenderPipelineDescriptor> {
    let d = MTLRenderPipelineDescriptor::new();
    d.setVertexFunction(Some(vs));
    d.setFragmentFunction(Some(fs));
    for (i, c) in t.colors.iter().enumerate() {
        let Some(fmt) = c else { continue };
        let a = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
        a.setPixelFormat(*fmt);
        let m = channel_mask >> (4 * i) & 0xF;
        let mut wm = MTLColorWriteMask::empty();
        for (bit, w) in [(1, MTLColorWriteMask::Red), (2, MTLColorWriteMask::Green), (4, MTLColorWriteMask::Blue), (8, MTLColorWriteMask::Alpha)] {
            if m & bit != 0 {
                wm |= w;
            }
        }
        a.setWriteMask(wm);
        if color_state >> (8 + i) & 1 != 0 {
            let (w0, w1) = (blend[i] as u32, (blend[i] >> 32) as u32);
            a.setBlendingEnabled(true);
            a.setRgbBlendOperation(blend_op(w0 >> 24 & 0xF));
            a.setAlphaBlendOperation(blend_op(w0 >> 28 & 0xF));
            a.setSourceRGBBlendFactor(blend_factor(w1 & 0x7F));
            a.setDestinationRGBBlendFactor(blend_factor(w1 >> 8 & 0x7F));
            a.setSourceAlphaBlendFactor(blend_factor(w1 >> 16 & 0x7F));
            a.setDestinationAlphaBlendFactor(blend_factor(w1 >> 24 & 0x7F));
        }
    }
    if let Some(f) = t.depth {
        d.setDepthAttachmentPixelFormat(f);
        if t.stencil {
            d.setStencilAttachmentPixelFormat(f);
        }
    }
    d
}

#[cfg(test)]
mod prewarm_tests {
    use super::*;

    fn key(n: u64) -> Recorded {
        Recorded::parse(&format!("{n:x}:0:0:0\tfffffffffffffffe:0:0:0\t\t0\t0\t0\t0\t0,0,0,0,0,0,0,0")).unwrap()
    }

    #[test]
    fn failed_build_releases_waiter() {
        let r = key(0xffff_ffff_ffff_fff0);
        PREWARMED.lock().unwrap().insert(r.clone(), Slot::Building);
        let builder = r.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(10));
            drop(Building(&builder));
        });
        let t0 = std::time::Instant::now();
        assert!(matches!(claim(&r, true, |f| f()), Claim::Record));
        assert!(t0.elapsed() < PREWARM_WAIT);
        assert!(matches!(PREWARMED.lock().unwrap().get(&r), Some(Slot::Claimed)));
        t.join().unwrap();
    }

    #[test]
    fn slow_build_falls_back_to_render_thread() {
        let r = key(0xffff_ffff_ffff_fff1);
        PREWARMED.lock().unwrap().insert(r.clone(), Slot::Building);
        let t0 = std::time::Instant::now();
        assert!(matches!(claim(&r, true, |f| f()), Claim::Build));
        assert!(t0.elapsed() >= PREWARM_WAIT);
        assert!(matches!(PREWARMED.lock().unwrap().get(&r), Some(Slot::Claimed)));
        drop(Building(&r));
        assert!(matches!(PREWARMED.lock().unwrap().get(&r), Some(Slot::Claimed)));
    }

    #[test]
    fn building_without_wait_reports_compiling() {
        let r = key(0xffff_ffff_ffff_fff2);
        PREWARMED.lock().unwrap().insert(r.clone(), Slot::Building);
        assert!(matches!(claim(&r, false, |f| f()), Claim::Compiling));
        assert!(matches!(PREWARMED.lock().unwrap().get(&r), Some(Slot::Building)));
    }
}
