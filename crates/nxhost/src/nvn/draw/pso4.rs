// Mush8 - pso4.rs
// Constantin Clerc (c22dev) - 0.1
use super::super::fx::FastMap;
use super::shaders::Compiled;
use super::state::{Pipeline, Targets, blend_factor, blend_op};
use crate::gfx::{self, Shared};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::sel;
use objc2_foundation::{NSObjectProtocol, NSString, NSURL};
use objc2_metal::*;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering::{Acquire, Relaxed, Release}};
use std::sync::{Arc, Condvar, LazyLock, Mutex, PoisonError};
use std::time::{Duration, Instant};

type Compiler = ProtocolObject<dyn MTL4Compiler>;
type Archive = ProtocolObject<dyn MTL4Archive>;
type Serializer = ProtocolObject<dyn MTL4PipelineDataSetSerializer>;

pub type BaseKey = (usize, u64, usize, u64);

const FLUSH_AFTER: Duration = Duration::from_secs(10);
const COMPACT_ABOVE: usize = 8;
const PREBUILT_FLUSH: usize = 1000;

struct Period {
    compiler: Shared<Compiler>,
    serializer: Shared<Serializer>,
    active: AtomicUsize,
    captured: AtomicUsize,
}

enum Base {
    Building,
    Ready(Option<Shared<Pipeline>>),
}

pub struct Pso4 {
    plain: Shared<Compiler>,
    period: Mutex<Arc<Period>>,
    prebuilt: Vec<Shared<Archive>>,
    own: Vec<Shared<Archive>>,
    old_files: Mutex<Vec<PathBuf>>,
    compact: bool,
    dir: PathBuf,
    bases: Mutex<FastMap<BaseKey, Base>>,
    changed: Condvar,
    last_new: Mutex<Option<Instant>>,
    flushing: Mutex<()>,
    stats: Stats,
}

#[derive(Default)]
struct Stats {
    archived: AtomicUsize,
    built: AtomicUsize,
    specialized: AtomicUsize,
    failed: AtomicUsize,
}

thread_local! {
    static PREWARMING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn set_prewarming(on: bool) {
    PREWARMING.with(|p| p.set(on));
}

fn supported(dev: &gfx::Device) -> bool {
    dev.respondsToSelector(sel!(newCompilerWithDescriptor:error:)) && dev.respondsToSelector(sel!(newArchiveWithURL:error:))
}

pub fn get() -> Option<&'static Pso4> {
    PSO4.as_ref()
}

static PSO4: LazyLock<Option<Pso4>> = LazyLock::new(|| {
    if std::env::var_os("NX_PSO_MTL4").is_some_and(|v| v == "0") {
        return None;
    }
    let dev = &gfx::gpu().device.0;
    if !supported(dev) {
        eprintln!("[gfx] no Metal 4 compiler on this system: pipelines are built whole");
        return None;
    }
    let plain = Shared(dev.newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new()).ok()?);
    let dir = PathBuf::from(std::env::var("NX_PSO_ARCHIVE").unwrap_or_else(|_| "work/pso4".into()));
    let _ = std::fs::create_dir_all(&dir);
    let t0 = Instant::now();
    let prebuilt: Vec<_> = if std::env::var_os("NX_PSO_BUNDLE").is_some_and(|v| v == "0") {
        Vec::new()
    } else {
        let arch = dev.architecture().name().to_string();
        let mut files = archive_files(&prebuilt_dir().join(&arch), "p");
        files.extend(archive_files(&prebuilt_dir(), "p"));
        files.iter().filter_map(|p| open_archive(p)).collect()
    };
    let own_files = archive_files(&dir, "");
    let own: Vec<_> = own_files.iter().filter_map(|p| open_archive(p)).collect();
    let compact = own_files.len() > COMPACT_ABOVE;
    eprintln!(
        "[gfx] Metal 4 pipelines: {} prebuilt and {} device archive file(s) opened in {} ms{}",
        prebuilt.len(),
        own.len(),
        t0.elapsed().as_millis(),
        if compact { ", compacting at this start" } else { "" }
    );
    let period = Mutex::new(Arc::new(new_period(dev)?));
    std::thread::Builder::new().name("pso-archive".into()).spawn(flush_loop).ok()?;
    Some(Pso4 {
        plain,
        period,
        prebuilt,
        own,
        old_files: Mutex::new(if compact { own_files } else { Vec::new() }),
        compact,
        dir,
        bases: Mutex::new(FastMap::default()),
        changed: Condvar::new(),
        last_new: Mutex::new(None),
        flushing: Mutex::new(()),
        stats: Stats::default(),
    })
});

pub fn prebuilt_dir() -> PathBuf {
    Path::new(super::shaders::aot_dir()).join("pso4")
}

fn new_period(dev: &gfx::Device) -> Option<Period> {
    let sd = MTL4PipelineDataSetSerializerDescriptor::new();
    sd.setConfiguration(MTL4PipelineDataSetSerializerConfiguration::CaptureBinaries);
    let serializer = dev.newPipelineDataSetSerializerWithDescriptor(&sd);
    let cd = MTL4CompilerDescriptor::new();
    cd.setPipelineDataSetSerializer(Some(&serializer));
    let compiler = dev.newCompilerWithDescriptor_error(&cd).ok()?;
    Some(Period { compiler: Shared(compiler), serializer: Shared(serializer), active: AtomicUsize::new(0), captured: AtomicUsize::new(0) })
}

fn archive_files(dir: &Path, prefix: &str) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
        .into_iter()
        .flatten()
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|e| e == "mtl4a"))
        .filter(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| if prefix.is_empty() { n.as_bytes()[0].is_ascii_digit() } else { n.starts_with(prefix) }))
        .collect();
    v.sort();
    v
}

fn looks_like_archive(p: &Path) -> bool {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(p) else { return false };
    let Ok(len) = f.metadata().map(|m| m.len()) else { return false };
    let mut b = Vec::with_capacity(8 + 20 * 64);
    if f.take(8 + 20 * 64).read_to_end(&mut b).is_err() {
        return false;
    }
    let be = |o: usize| b.get(o..o + 4).map(|w| u32::from_be_bytes(w.try_into().unwrap()));
    match be(0) {
        Some(0xCAFE_BABE | 0xCBFE_BABE) => {
            let Some(n) = be(4).filter(|&n| (1..=64).contains(&n)) else { return false };
            (0..n as usize).all(|i| {
                let e = 8 + 20 * i;
                matches!((be(e + 8), be(e + 12)), (Some(off), Some(size)) if size > 0 && off as u64 + size as u64 <= len)
            })
        }
        Some(_) => len > 32 && u32::from_le_bytes(b[..4].try_into().unwrap()) == 0xFEED_FACF,
        None => false,
    }
}

fn open_archive(p: &Path) -> Option<Shared<Archive>> {
    if !looks_like_archive(p) {
        eprintln!("[gfx] pipeline archive {} is not an archive, ignored", p.display());
        return None;
    }
    let url = NSURL::fileURLWithPath(&NSString::from_str(&p.to_string_lossy()));
    match gfx::gpu().device.0.newArchiveWithURL_error(&url) {
        Ok(a) => Some(Shared(a)),
        Err(e) => {
            eprintln!("[gfx] pipeline archive {} not opened: {}", p.display(), e.localizedDescription());
            None
        }
    }
}

pub fn base_key(vs: &Compiled, fs: &Compiled) -> BaseKey {
    let id = |c: &Compiled| (Retained::as_ptr(&c.lib.0) as *const () as usize, fnv(c.entry.as_bytes()));
    let (a, b) = id(vs);
    let (c, d) = id(fs);
    (a, b, c, d)
}

fn fnv(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100_0000_01b3))
}

fn function(c: &Compiled) -> Retained<MTL4LibraryFunctionDescriptor> {
    let d = MTL4LibraryFunctionDescriptor::new();
    d.setLibrary(Some(&c.lib.0));
    d.setName(Some(&NSString::from_str(&c.entry)));
    d
}

fn descriptor(vs: &Compiled, fs: &Compiled) -> Retained<MTL4RenderPipelineDescriptor> {
    let d = MTL4RenderPipelineDescriptor::new();
    d.setVertexFunctionDescriptor(Some(&function(vs)));
    d.setFragmentFunctionDescriptor(Some(&function(fs)));
    d
}

pub fn base_descriptor(vs: &Compiled, fs: &Compiled) -> Retained<MTL4RenderPipelineDescriptor> {
    let d = descriptor(vs, fs);
    for i in 0..8 {
        let a = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
        a.setPixelFormat(MTLPixelFormat::Unspecialized);
        a.setWriteMask(MTLColorWriteMask::Unspecialized);
        a.setBlendingState(MTL4BlendState::Unspecialized);
        a.setSourceRGBBlendFactor(MTLBlendFactor::Unspecialized);
        a.setDestinationRGBBlendFactor(MTLBlendFactor::Unspecialized);
        a.setSourceAlphaBlendFactor(MTLBlendFactor::Unspecialized);
        a.setDestinationAlphaBlendFactor(MTLBlendFactor::Unspecialized);
        a.setRgbBlendOperation(MTLBlendOperation::Unspecialized);
        a.setAlphaBlendOperation(MTLBlendOperation::Unspecialized);
    }
    d
}

pub fn specialized_descriptor(vs: &Compiled, fs: &Compiled, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8]) -> Retained<MTL4RenderPipelineDescriptor> {
    let d = descriptor(vs, fs);
    for i in 0..8 {
        let a = unsafe { d.colorAttachments().objectAtIndexedSubscript(i) };
        let Some(fmt) = t.colors.get(i).copied().flatten() else {
            a.setPixelFormat(MTLPixelFormat::Invalid);
            a.setBlendingState(MTL4BlendState::Disabled);
            a.setWriteMask(MTLColorWriteMask::All);
            continue;
        };
        a.setPixelFormat(fmt);
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
            a.setBlendingState(MTL4BlendState::Enabled);
            a.setRgbBlendOperation(blend_op(w0 >> 24 & 0xF));
            a.setAlphaBlendOperation(blend_op(w0 >> 28 & 0xF));
            a.setSourceRGBBlendFactor(blend_factor(w1 & 0x7F));
            a.setDestinationRGBBlendFactor(blend_factor(w1 >> 8 & 0x7F));
            a.setSourceAlphaBlendFactor(blend_factor(w1 >> 16 & 0x7F));
            a.setDestinationAlphaBlendFactor(blend_factor(w1 >> 24 & 0x7F));
        } else {
            a.setBlendingState(MTL4BlendState::Disabled);
        }
    }
    d
}

impl Pso4 {
    pub fn pipeline(&self, vs: &Compiled, fs: &Compiled, t: &Targets, color_state: u32, channel_mask: u32, blend: &[u64; 8], compile: bool) -> Option<Shared<Pipeline>> {
        let base = self.base(base_key(vs, fs), vs, fs, compile)?;
        let d = specialized_descriptor(vs, fs, t, color_state, channel_mask, blend);
        match self.plain.0.newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&d, &base.0) {
            Ok(p) => {
                self.stats.specialized.fetch_add(1, Relaxed);
                Some(Shared(p))
            }
            Err(e) => {
                self.stats.failed.fetch_add(1, Relaxed);
                eprintln!("[gfx] pipeline specialization failed: {}", e.localizedDescription());
                None
            }
        }
    }

    fn lookup(&self, d: &MTL4RenderPipelineDescriptor) -> Option<(Shared<Pipeline>, bool)> {
        for a in &self.prebuilt {
            if let Ok(p) = a.0.newRenderPipelineStateWithDescriptor_error(d) {
                return Some((Shared(p), false));
            }
        }
        for a in &self.own {
            if let Ok(p) = a.0.newRenderPipelineStateWithDescriptor_error(d) {
                return Some((Shared(p), true));
            }
        }
        None
    }

    fn base(&self, key: BaseKey, vs: &Compiled, fs: &Compiled, compile: bool) -> Option<Shared<Pipeline>> {
        let mut m = self.bases.lock().unwrap_or_else(PoisonError::into_inner);
        loop {
            match m.get(&key) {
                Some(Base::Ready(p)) => return p.clone(),
                Some(Base::Building) if !compile => return None,
                Some(Base::Building) => m = self.changed.wait(m).unwrap_or_else(PoisonError::into_inner),
                None => break,
            }
        }
        m.insert(key, Base::Building);
        drop(m);
        let d = base_descriptor(vs, fs);
        let mut p = match self.lookup(&d) {
            Some((p, own)) => {
                self.stats.archived.fetch_add(1, Relaxed);
                if own && self.compact && PREWARMING.with(|w| w.get()) { self.capture(&d).or(Some(p)) } else { Some(p) }
            }
            None => None,
        };
        let settled = p.is_some() || compile;
        if p.is_none() && compile {
            p = self.capture(&d);
            if p.is_none() {
                self.stats.failed.fetch_add(1, Relaxed);
            }
        }
        let mut m = self.bases.lock().unwrap_or_else(PoisonError::into_inner);
        if settled {
            m.insert(key, Base::Ready(p.clone()));
        } else {
            m.remove(&key);
        }
        self.changed.notify_all();
        p
    }

    fn capture(&self, d: &MTL4RenderPipelineDescriptor) -> Option<Shared<Pipeline>> {
        let period = {
            let g = self.period.lock().unwrap_or_else(PoisonError::into_inner);
            g.active.fetch_add(1, Relaxed);
            g.clone()
        };
        let r = period.compiler.0.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(d, None);
        let ok = r.is_ok();
        if ok {
            period.captured.fetch_add(1, Relaxed);
            self.stats.built.fetch_add(1, Relaxed);
        }
        period.active.fetch_sub(1, Release);
        if ok {
            *self.last_new.lock().unwrap_or_else(PoisonError::into_inner) = Some(Instant::now());
        }
        match r {
            Ok(p) => Some(Shared(p)),
            Err(e) => {
                eprintln!("[gfx] pipeline base failed: {}", e.localizedDescription());
                None
            }
        }
    }

    pub fn flush(&self) -> bool {
        let _f = self.flushing.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(next) = new_period(&gfx::gpu().device.0) else { return false };
        let old = std::mem::replace(&mut *self.period.lock().unwrap_or_else(PoisonError::into_inner), Arc::new(next));
        while old.active.load(Acquire) > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        let n = old.captured.load(Relaxed);
        if n == 0 {
            return true;
        }
        let ok = write_archive(&old.serializer.0, &self.dir, &next_name(&self.dir, ""));
        if ok {
            eprintln!("[gfx] pipeline archive: {n} new pipeline bases written to {}", self.dir.display());
        }
        ok
    }

    pub fn prewarm_done(&self) {
        let ok = self.flush();
        let old = std::mem::take(&mut *self.old_files.lock().unwrap_or_else(PoisonError::into_inner));
        if ok && !old.is_empty() {
            for f in &old {
                let _ = std::fs::remove_file(f);
            }
            eprintln!("[gfx] pipeline archive compacted: {} old file(s) removed", old.len());
        }
        self.log_stats("after prewarm");
    }

    pub fn log_stats(&self, when: &str) {
        let s = &self.stats;
        eprintln!(
            "[gfx] pipeline bases {when}: {} from archives, {} compiled, {} specializations, {} failed",
            s.archived.load(Relaxed),
            s.built.load(Relaxed),
            s.specialized.load(Relaxed),
            s.failed.load(Relaxed)
        );
    }
}

fn next_name(dir: &Path, prefix: &str) -> PathBuf {
    let n = archive_files(dir, prefix)
        .iter()
        .filter_map(|p| p.file_stem()?.to_str()?.trim_start_matches(prefix).parse::<u32>().ok())
        .max()
        .map_or(0, |n| n + 1);
    dir.join(format!("{prefix}{n:05}.mtl4a"))
}

fn write_archive(serializer: &Serializer, dir: &Path, path: &Path) -> bool {
    let _ = std::fs::create_dir_all(dir);
    let tmp = path.with_extension("tmp");
    let url = NSURL::fileURLWithPath(&NSString::from_str(&tmp.to_string_lossy()));
    match serializer.serializeAsArchiveAndFlushToURL_error(&url) {
        Ok(()) => std::fs::rename(&tmp, path).is_ok(),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            eprintln!("[gfx] pipeline archive not written: {}", e.localizedDescription());
            false
        }
    }
}

fn flush_loop() {
    loop {
        std::thread::sleep(Duration::from_secs(1));
        let Some(p) = PSO4.as_ref() else { return };
        let due = p.last_new.lock().unwrap_or_else(PoisonError::into_inner).is_some_and(|t| t.elapsed() >= FLUSH_AFTER);
        if due && p.period.lock().unwrap_or_else(PoisonError::into_inner).captured.load(Relaxed) > 0 {
            *p.last_new.lock().unwrap_or_else(PoisonError::into_inner) = None;
            objc2::rc::autoreleasepool(|_| p.flush());
        }
    }
}

pub struct Prebuild {
    dir: PathBuf,
    period: Mutex<Option<Arc<Period>>>,
    since: AtomicUsize,
    pub built: AtomicUsize,
    pub failed: AtomicUsize,
    writing: AtomicBool,
}

impl Prebuild {
    pub fn new() -> Option<Self> {
        let dev = &gfx::gpu().device.0;
        if !supported(dev) {
            return None;
        }
        let dir = prebuilt_dir();
        for f in archive_files(&dir, "p") {
            let _ = std::fs::remove_file(f);
        }
        Some(Self { dir, period: Mutex::new(Some(Arc::new(new_period(dev)?))), since: AtomicUsize::new(0), built: AtomicUsize::new(0), failed: AtomicUsize::new(0), writing: AtomicBool::new(false) })
    }

    pub fn add(&self, vs: &Compiled, fs: &Compiled) {
        let period = {
            let g = self.period.lock().unwrap();
            let Some(p) = g.as_ref() else { return };
            p.active.fetch_add(1, Relaxed);
            p.clone()
        };
        let ok = period.compiler.0.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&base_descriptor(vs, fs), None).is_ok();
        period.active.fetch_sub(1, Release);
        if ok {
            period.captured.fetch_add(1, Relaxed);
            self.built.fetch_add(1, Relaxed);
            if self.since.fetch_add(1, Relaxed) + 1 >= PREBUILT_FLUSH && !self.writing.swap(true, Relaxed) {
                self.write(true);
                self.writing.store(false, Relaxed);
            }
        } else {
            self.failed.fetch_add(1, Relaxed);
        }
    }

    fn write(&self, more: bool) {
        let next = if more { new_period(&gfx::gpu().device.0).map(Arc::new) } else { None };
        let Some(old) = std::mem::replace(&mut *self.period.lock().unwrap(), next) else { return };
        self.since.store(0, Relaxed);
        while old.active.load(Acquire) > 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        if old.captured.load(Relaxed) > 0 {
            write_archive(&old.serializer.0, &self.dir, &next_name(&self.dir, "p"));
        }
    }

    pub fn finish(self) {
        self.write(false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, b: &[u8]) -> bool {
        let p = std::env::temp_dir().join(format!("mush8-pso4-header-{}-{name}", std::process::id()));
        std::fs::write(&p, b).unwrap();
        let r = looks_like_archive(&p);
        let _ = std::fs::remove_file(&p);
        r
    }

    fn fat(n: u32, entries: &[(u32, u32)], len: usize) -> Vec<u8> {
        let mut b = vec![0u8; len];
        b[..4].copy_from_slice(&0xCAFE_BABEu32.to_be_bytes());
        b[4..8].copy_from_slice(&n.to_be_bytes());
        for (i, &(off, size)) in entries.iter().enumerate() {
            let e = 8 + 20 * i;
            b[e + 8..e + 12].copy_from_slice(&off.to_be_bytes());
            b[e + 12..e + 16].copy_from_slice(&size.to_be_bytes());
        }
        b
    }

    #[test]
    fn archive_header_checks() {
        assert!(check("fat-ok", &fat(2, &[(64, 100), (164, 3000)], 4096)));
        assert!(!check("fat-past-end", &fat(2, &[(64, 100), (164, 4000)], 4096)));
        assert!(!check("fat-empty-slice", &fat(1, &[(64, 0)], 4096)));
        assert!(!check("fat-zero", &fat(0, &[], 4096)));
        assert!(!check("fat-too-many", &fat(65, &[], 4096)));
        assert!(!check("fat-truncated", &fat(1, &[], 12)));
        assert!(check("fat-large-tail", &fat(1, &[(64, 20000)], 20064)));
        let mut macho = vec![0u8; 64];
        macho[..4].copy_from_slice(&0xFEED_FACFu32.to_le_bytes());
        assert!(check("macho", &macho));
        assert!(!check("macho-short", &macho[..32]));
        assert!(!check("text", b"not an archive"));
        assert!(!check("short", b"ab"));
        assert!(!check("empty", b""));
        assert!(!looks_like_archive(Path::new("/nonexistent/mush8.mtl4a")));
    }
}

#[cfg(all(test, target_os = "macos"))]
mod gpu_tests {
    use super::*;

    const MSL: &str = r#"
#include <metal_stdlib>
using namespace metal;
struct V { float4 pos [[position]]; float4 c; };
vertex V vmain(uint id [[vertex_id]], constant float4& c [[buffer(0)]]) {
    float2 q = float2((id << 1) & 2, id & 2);
    V v; v.pos = float4(q * 2.0 - 1.0, 0.5, 1.0); v.c = c; return v;
}
fragment float4 fmain(V v [[stage_in]]) { return v.c; }
"#;

    fn compiled(lib: &Retained<ProtocolObject<dyn MTLLibrary>>, entry: &str) -> Compiled {
        let func = lib.newFunctionWithName(&NSString::from_str(entry)).unwrap();
        Compiled { func: Shared(func), lib: Shared(lib.clone()), entry: entry.into(), iface: Default::default(), source: None }
    }

    fn draw(p: &Pipeline, color: [f32; 4], clear: [f64; 4]) -> [u8; 4] {
        let g = gfx::gpu();
        let dev = &g.device.0;
        let mk = |fmt, usage| {
            let d = unsafe { MTLTextureDescriptor::texture2DDescriptorWithPixelFormat_width_height_mipmapped(fmt, 4, 4, false) };
            d.setUsage(usage);
            d.setStorageMode(if fmt == MTLPixelFormat::Depth32Float { MTLStorageMode::Private } else { MTLStorageMode::Shared });
            dev.newTextureWithDescriptor(&d).unwrap()
        };
        let rt = mk(MTLPixelFormat::RGBA8Unorm, MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead);
        let depth = mk(MTLPixelFormat::Depth32Float, MTLTextureUsage::RenderTarget);
        let rp = MTLRenderPassDescriptor::new();
        let a = unsafe { rp.colorAttachments().objectAtIndexedSubscript(0) };
        a.setTexture(Some(&rt));
        a.setLoadAction(MTLLoadAction::Clear);
        a.setClearColor(MTLClearColor { red: clear[0], green: clear[1], blue: clear[2], alpha: clear[3] });
        a.setStoreAction(MTLStoreAction::Store);
        let da = rp.depthAttachment();
        da.setTexture(Some(&depth));
        da.setLoadAction(MTLLoadAction::Clear);
        da.setClearDepth(1.0);
        let dd = MTLDepthStencilDescriptor::new();
        dd.setDepthCompareFunction(MTLCompareFunction::Less);
        dd.setDepthWriteEnabled(true);
        let ds = dev.newDepthStencilStateWithDescriptor(&dd).unwrap();
        let cb = g.queue.0.commandBuffer().unwrap();
        let enc = cb.renderCommandEncoderWithDescriptor(&rp).unwrap();
        enc.setRenderPipelineState(p);
        enc.setDepthStencilState(Some(&ds));
        unsafe {
            enc.setVertexBytes_length_atIndex(std::ptr::NonNull::from(&color).cast(), 16, 0);
            enc.drawPrimitives_vertexStart_vertexCount(MTLPrimitiveType::Triangle, 0, 3);
        }
        enc.endEncoding();
        cb.commit();
        cb.waitUntilCompleted();
        assert_eq!(cb.status(), MTLCommandBufferStatus::Completed, "{:?}", cb.error());
        let mut px = [0u8; 4];
        unsafe {
            rt.getBytes_bytesPerRow_fromRegion_mipmapLevel(
                std::ptr::NonNull::from(&mut px).cast(),
                16,
                MTLRegion { origin: MTLOrigin { x: 1, y: 1, z: 0 }, size: MTLSize { width: 1, height: 1, depth: 1 } },
                0,
            );
        }
        px
    }

    #[test]
    #[ignore = "requires a real Metal GPU with Metal 4"]
    fn specialized_bases_draw_through_classic_encoders() {
        objc2::rc::autoreleasepool(|_| {
            let dev = &gfx::gpu().device.0;
            if !supported(dev) {
                return;
            }
            let lib = dev.newLibraryWithSource_options_error(&NSString::from_str(MSL), None).unwrap();
            let (vs, fs) = (compiled(&lib, "vmain"), compiled(&lib, "fmain"));
            let compiler = dev.newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new()).unwrap();
            let base = compiler.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&base_descriptor(&vs, &fs), None).unwrap();
            let t = Targets { colors: vec![Some(MTLPixelFormat::RGBA8Unorm)], depth: Some(MTLPixelFormat::Depth32Float), stencil: false };
            let opaque = compiler
                .newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&specialized_descriptor(&vs, &fs, &t, 0, 0xF, &[0; 8]), &base)
                .unwrap();
            assert_eq!(draw(&opaque, [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]), [255, 0, 0, 255]);
            let red_only = compiler
                .newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&specialized_descriptor(&vs, &fs, &t, 0, 0x1, &[0; 8]), &base)
                .unwrap();
            assert_eq!(draw(&red_only, [1.0, 1.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]), [255, 0, 255, 255]);
            let additive = [(1u64 << 24) | ((2u64 | 2 << 8 | 2 << 16 | 2 << 24) << 32); 8];
            let added = compiler
                .newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&specialized_descriptor(&vs, &fs, &t, 1 << 8, 0xF, &additive), &base)
                .unwrap();
            assert_eq!(draw(&added, [1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]), [255, 0, 255, 255]);
        });
    }

    #[test]
    #[ignore = "requires a real Metal GPU and NX_TEST_REAL_PAIRS"]
    fn real_game_pairs_build_bases_and_specialize() {
        let Some(dir) = std::env::var_os("NX_TEST_REAL_PAIRS").map(PathBuf::from) else { return };
        let dev = &gfx::gpu().device.0;
        let compiler = dev.newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new()).unwrap();
        let list = std::fs::read_to_string(dir.join("pairs.tsv")).unwrap();
        let (mut n, mut base_ms, mut spec_us) = (0, 0.0, 0.0);
        for l in list.lines() {
            objc2::rc::autoreleasepool(|_| {
                let f: Vec<&str> = l.split('\t').collect();
                let (vk, extra, fk) = (u64::from_str_radix(f[0], 16).unwrap(), u32::from_str_radix(f[1], 16).unwrap(), u64::from_str_radix(f[2], 16).unwrap());
                let blob = |k: u64| std::fs::read(dir.join(format!("{k:016x}.bin"))).unwrap();
                let fs = super::super::shaders::compile(&blob(fk), false, 0, 0, 0, fk).expect("fragment compiles");
                assert_eq!(fs.iface.attributes, extra, "vertex variant predicted from the fragment's inputs");
                let vs = super::super::shaders::compile(&blob(vk), false, extra, 0, 0, vk).expect("vertex compiles");
                let t0 = Instant::now();
                let base = compiler.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&base_descriptor(&vs, &fs), None).unwrap_or_else(|e| panic!("{l}: base: {}", e.localizedDescription()));
                base_ms += t0.elapsed().as_secs_f64() * 1e3;
                let colors: Vec<_> = (0..8).map(|i| (fs.iface.color_targets >> i & 1 != 0).then_some(MTLPixelFormat::RGBA16Float)).collect();
                let t = Targets { colors, depth: Some(MTLPixelFormat::Depth32Float_Stencil8), stencil: true };
                for (cs, blend) in [(0u32, [0u64; 8]), (0xFF00, [(1u64 << 24) | ((5u64 | 6 << 8 | 2 << 16 | 6 << 24) << 32); 8])] {
                    let t1 = Instant::now();
                    compiler
                        .newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&specialized_descriptor(&vs, &fs, &t, cs, 0xFFFF_FFFF, &blend), &base)
                        .unwrap_or_else(|e| panic!("{l}: specialization: {}", e.localizedDescription()));
                    spec_us += t1.elapsed().as_secs_f64() * 1e6;
                }
                n += 1;
            });
        }
        assert!(n > 0);
        eprintln!("{n} real pairs: base {:.1} ms, specialization {:.0} us on average", base_ms / n as f64, spec_us / (2 * n) as f64);
    }

    #[test]
    #[ignore = "prints the pipelines script Metal captures"]
    fn print_pipelines_script() {
        let dev = &gfx::gpu().device.0;
        let dir = std::env::temp_dir().join("mush8-script");
        std::fs::create_dir_all(&dir).unwrap();
        let src = dir.join("t.metal");
        std::fs::write(&src, MSL).unwrap();
        let lib_path = dir.join("t.metallib");
        assert!(std::process::Command::new("xcrun").args(["-sdk", "macosx", "metal", "-o"]).arg(&lib_path).arg(&src).status().unwrap().success());
        let lib = dev.newLibraryWithURL_error(&NSURL::fileURLWithPath(&NSString::from_str(&lib_path.to_string_lossy()))).unwrap();
        let (vs, fs) = (compiled(&lib, "vmain"), compiled(&lib, "fmain"));
        let sd = MTL4PipelineDataSetSerializerDescriptor::new();
        sd.setConfiguration(MTL4PipelineDataSetSerializerConfiguration::CaptureDescriptors);
        let ser = dev.newPipelineDataSetSerializerWithDescriptor(&sd);
        let cd = MTL4CompilerDescriptor::new();
        cd.setPipelineDataSetSerializer(Some(&ser));
        let c = dev.newCompilerWithDescriptor_error(&cd).unwrap();
        c.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&base_descriptor(&vs, &fs), None).unwrap();
        let data = ser.serializeAsPipelinesScriptWithError().unwrap();
        std::fs::write(dir.join("script.json"), data.to_vec()).unwrap();
    }

    #[test]
    #[ignore = "requires a real Metal GPU with Metal 4"]
    fn captured_bases_come_back_from_the_archive() {
        objc2::rc::autoreleasepool(|_| {
            let dev = &gfx::gpu().device.0;
            if !supported(dev) {
                return;
            }
            let lib = dev.newLibraryWithSource_options_error(&NSString::from_str(MSL), None).unwrap();
            let (vs, fs) = (compiled(&lib, "vmain"), compiled(&lib, "fmain"));
            let period = new_period(dev).unwrap();
            let d = base_descriptor(&vs, &fs);
            period.compiler.0.newRenderPipelineStateWithDescriptor_compilerTaskOptions_error(&d, None).unwrap();
            let dir = std::env::temp_dir().join(format!("mush8-pso4-{}", std::process::id()));
            let path = next_name(&dir, "");
            assert!(write_archive(&period.serializer.0, &dir, &path));
            let a = open_archive(&path).expect("archive opens");
            let p = a.0.newRenderPipelineStateWithDescriptor_error(&d).expect("base found in the archive");
            let t = Targets { colors: vec![Some(MTLPixelFormat::RGBA8Unorm)], depth: None, stencil: false };
            let compiler = dev.newCompilerWithDescriptor_error(&MTL4CompilerDescriptor::new()).unwrap();
            let s = compiler
                .newRenderPipelineStateBySpecializationWithDescriptor_pipeline_error(&specialized_descriptor(&vs, &fs, &t, 0, 0xF, &[0; 8]), &p)
                .unwrap();
            assert_eq!(draw(&s, [0.0, 1.0, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]), [0, 255, 0, 255]);
            std::fs::write(dir.join("00099.mtl4a"), b"not an archive").unwrap();
            assert!(open_archive(&dir.join("00099.mtl4a")).is_none());
            let _ = std::fs::remove_dir_all(&dir);
        });
    }
}
