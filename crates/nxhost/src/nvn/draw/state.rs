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
use std::sync::{Arc, LazyLock, Mutex};

pub type Pipeline = objc2::runtime::ProtocolObject<dyn MTLRenderPipelineState>;

use super::super::fx::{FastMap, FastSet};
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

#[cfg(all(test,target_os="macos"))]
mod segmented_heap_tests {
    use super::*;
    use objc2::runtime::ProtocolObject;
    #[test]
    #[ignore = "requires a real Metal GPU"]
    fn gpu_reads_and_writes_across_heap_chunks() {
        objc2::rc::autoreleasepool(|_| unsafe {
            let page=crate::mem::PAGE as usize;
            let mem=libc::mmap(std::ptr::null_mut(),page*4,libc::PROT_READ|libc::PROT_WRITE,libc::MAP_PRIVATE|libc::MAP_ANON,-1,0);
            assert_ne!(mem,libc::MAP_FAILED);
            // The global heap retains these no-copy buffers until process exit.
            std::env::set_var("NX_GPU_HEAP_CHUNK_BYTES",page.to_string());
            assert!(gfx::wrap_heap(mem as u64,(page*4) as u64));
            std::env::remove_var("NX_GPU_HEAP_CHUNK_BYTES");
            let h=gfx::heap().unwrap();assert_eq!(h.chunks.len(),4);
            let words=std::slice::from_raw_parts_mut(mem as *mut u32,page);
            words[page/4-1]=41;words[page/4]=82;words[page/2]=123;
            let dev=&gfx::gpu().device.0;
            let source=format!("{}\nkernel void heap_probe(constant Res& r [[buffer(0)]], device uint* out [[buffer(1)]]) {{ ulong p=r.heap_base; out[0]=*gmem(r,p+{}ul); out[1]=*gmem(r,p+{}ul); out[2]=*gmem(r,p+{}ul); out[3]=*gmem(r,p+r.heap_size); *gmem(r,p+{}ul)=456u; *gmem(r,p+{}ul)=789u; }}",nxshader::prelude(),page-4,page,page*2,page-4,page);
            let lib=dev.newLibraryWithSource_options_error(&NSString::from_str(&source),None).unwrap();
            let func=lib.newFunctionWithName(&NSString::from_str("heap_probe")).unwrap();
            let pipeline=dev.newComputePipelineStateWithFunction_error(&func).unwrap();
            let out=dev.newBufferWithLength_options(16,MTLResourceOptions::StorageModeShared).unwrap();
            let mut res:Res=std::mem::zeroed();
            res.heap_base=h.base;res.heap_size=h.size;res.heap_chunk_size=h.chunk_size;res.heap_gpu=h.gpu_addresses();res.zero_gpu=gfx::gpu().zeros.0.gpuAddress();
            let cb=gfx::gpu().queue.0.commandBuffer().unwrap();let enc=cb.computeCommandEncoder().unwrap();
            enc.setComputePipelineState(&pipeline);
            for chunk in &h.chunks {enc.useResource_usage(ProtocolObject::from_ref(&*chunk.buffer.0),MTLResourceUsage::Read|MTLResourceUsage::Write);}
            enc.setBytes_length_atIndex(std::ptr::NonNull::from(&res).cast(),size_of::<Res>(),0);
            enc.setBuffer_offset_atIndex(Some(&out),0,1);
            enc.dispatchThreadgroups_threadsPerThreadgroup(MTLSize{width:1,height:1,depth:1},MTLSize{width:1,height:1,depth:1});
            enc.endEncoding();cb.commit();cb.waitUntilCompleted();
            assert_eq!(std::slice::from_raw_parts(out.contents().as_ptr() as *const u32,4),&[41,82,123,0]);
            assert_eq!(words[page/4-1],456);assert_eq!(words[page/4],789);
        });
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
pub struct Targets {
    pub colors: Vec<Option<MTLPixelFormat>>,
    pub depth: Option<MTLPixelFormat>,
    pub stencil: bool,
}

fn blend_factor(f: u32) -> MTLBlendFactor {
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

fn blend_op(e: u32) -> MTLBlendOperation {
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
    pending: FastSet<PipeKey>,
    depth: FastMap<(u64, [u32; 4], bool, bool), Shared<DepthState>>,
}

static DONE: LazyLock<Mutex<Vec<(PipeKey, Option<Shared<Pipeline>>)>>> = LazyLock::new(Default::default);
static PREWARMED: LazyLock<Mutex<HashMap<Recorded, Shared<Pipeline>>>> = LazyLock::new(Default::default);

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
        loop {
            if !self.pending.is_empty() {
                for (k, p) in std::mem::take(&mut *DONE.lock().unwrap()) {
                    self.pending.remove(&k);
                    self.pipelines.insert(k, p);
                }
            }
            if let Some(p) = self.pipelines.get(&key) {
                return p.clone().ok_or("pipeline failed");
            }
            if !self.pending.contains(&key) {
                break;
            }
            if !wait {
                return Err("pipeline compiling");
            }
            crate::kernel::blocking(|| std::thread::sleep(std::time::Duration::from_millis(1)));
        }
        let (cs, cm, blend) = (st.color_state, st.channel_mask, st.blend);
        if let Some(r) = Recorded::of(vs, fs, t, cs, cm, &blend) {
            if let Some(p) = PREWARMED.lock().unwrap().remove(&r) {
                self.pipelines.insert(key, Some(p.clone()));
                return Ok(p);
            }
            record(r);
        }
        let (jvs, jfs, jt) = (vs.clone(), fs.clone(), t.clone());
        let name = format!("{:016x}/{fs_key:016x}", vs_key.0);
        if wait || super::shaders::sync_mode() {
            let t0 = std::time::Instant::now();
            let p = crate::kernel::blocking(|| build_pipeline(&jvs, &jfs, &jt, cs, cm, &blend, &name));
            if wait && !super::shaders::sync_mode() && t0.elapsed().as_millis() >= 5 {
                eprintln!("[gfx] pipeline {name}: built while the render thread waited (new render target), {} ms", t0.elapsed().as_millis());
            }
            self.pipelines.insert(key, p.clone());
            return p.ok_or("pipeline failed");
        }
        self.pending.insert(key);
        super::shaders::spawn_job(Box::new(move || {
            let p = build_pipeline(&jvs, &jfs, &jt, cs, cm, &blend, &name);
            DONE.lock().unwrap().push((key, p));
        }));
        Err("pipeline compiling")
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
                Shared(dev.newBufferWithLength_options(size.max(CHUNK), MTLResourceOptions::StorageModeShared).expect("scratch buffer"))
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
const ARCHIVE_FILE: &str = "pipelines.metalar";

static ARCHIVES: LazyLock<Option<Shared<NSArray<Archive>>>> = LazyLock::new(|| {
    if std::env::var_os("NX_PIPELINE_ARCHIVE").is_none() {
        return None;
    }
    let dir = super::shaders::aot_dir();
    let mut files: Vec<String> = std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.starts_with("pipelines") && n.ends_with(".metalar"))
        .collect();
    files.sort();
    let archives: Vec<Retained<Archive>> = files
        .iter()
        .filter_map(|f| {
            let d = MTLBinaryArchiveDescriptor::new();
            d.setUrl(Some(&NSURL::fileURLWithPath(&NSString::from_str(&format!("{dir}/{f}")))));
            gfx::gpu().device.0.newBinaryArchiveWithDescriptor_error(&d).ok()
        })
        .collect();
    eprintln!("[gfx] {} pipeline archive(s) loaded", archives.len());
    (!archives.is_empty()).then(|| Shared(NSArray::from_retained_slice(&archives)))
});

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

pub fn prewarm() {
    std::thread::Builder::new()
        .name("prewarm".into())
        .spawn(|| {
            use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
            let t0 = std::time::Instant::now();
            let list = read_list();
            let (next, built) = (AtomicUsize::new(0), AtomicUsize::new(0));
            std::thread::scope(|s| {
                for _ in 0..2 {
                    s.spawn(|| {
                        unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_UTILITY, 0) };
                        while let Some(r) = list.get(next.fetch_add(1, Relaxed)) {
                            objc2::rc::autoreleasepool(|_| {
                                let (Some(vs), Some(fs)) = (super::shaders::aot_compiled(r.vs), super::shaders::aot_compiled(r.fs)) else { return };
                                let d = descriptor(&vs.func.0, &fs.func.0, &r.targets(), r.color_state, r.channel_mask, &r.blend);
                                if let Some(a) = &*ARCHIVES {
                                    d.setBinaryArchives(Some(&a.0));
                                }
                                if let Ok(p) = gfx::gpu().device.0.newRenderPipelineStateWithDescriptor_error(&d) {
                                    PREWARMED.lock().unwrap().insert(r.clone(), Shared(p));
                                    built.fetch_add(1, Relaxed);
                                }
                            });
                        }
                    });
                }
            });
            eprintln!("[gfx] prewarmed {}/{} recorded pipelines in {:.1} s", built.into_inner(), list.len(), t0.elapsed().as_secs_f32());
        })
        .unwrap();
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

pub fn precompile_pipelines(programs: &std::collections::BTreeMap<u64, Vec<u8>>) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let t0 = std::time::Instant::now();
    let list = read_list();
    if list.is_empty() {
        eprintln!("[gfx] no recorded pipelines ({PIPELINE_LIST})");
        return Ok(());
    }
    let archive = gfx::gpu().device.0.newBinaryArchiveWithDescriptor_error(&MTLBinaryArchiveDescriptor::new()).map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))?;
    let archive = Shared(archive);
    let function = |(key, [extra, uints, sints]): (u64, [u32; 3])| {
        let blob = programs.get(&key)?;
        super::shaders::compile(blob, false, extra, uints, sints, key)
    };
    let (next, built) = (AtomicUsize::new(0), AtomicUsize::new(0));
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let Some(r) = list.get(next.fetch_add(1, Relaxed)) else { return };
                objc2::rc::autoreleasepool(|_| {
                    let (Some(vs), Some(fs)) = (function(r.vs), function(r.fs)) else { return };
                    let d = descriptor(&vs.func.0, &fs.func.0, &r.targets(), r.color_state, r.channel_mask, &r.blend);
                    if add(&archive, &d) {
                        built.fetch_add(1, Relaxed);
                    }
                });
            });
        }
    });
    let path = format!("{}/{ARCHIVE_FILE}", super::shaders::aot_dir());
    archive.0.serializeToURL_error(&NSURL::fileURLWithPath(&NSString::from_str(&path))).map_err(|e| anyhow::anyhow!("{}", e.localizedDescription()))?;
    eprintln!("[gfx] precompiled {}/{} recorded pipelines in {:.0} s", built.into_inner(), list.len(), t0.elapsed().as_secs_f32());
    Ok(())
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
