// Mush8 - shaders.rs
// Constantin Clerc (c22dev) - 0.1
use super::super::objects::STATE;
use crate::gfx::{self, Shared};
use nxshader::{Interface, TexKind};
use objc2_foundation::NSString;
use objc2_metal::*;
use super::super::fx::{FastMap, FastSet};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::{AcqRel, Relaxed}};
use std::sync::{Arc, LazyLock, Mutex};

pub type Function = objc2::runtime::ProtocolObject<dyn MTLFunction>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Stage {
    pub hash: u64,
    pub code: u64,
    pub len: usize,
    pub constants: u64,
    pub fragment: bool,
    pub local_size: Option<[u32; 3]>,
}

pub struct Compiled {
    pub func: Shared<Function>,
    pub lib: Shared<Library>,
    pub entry: String,
    pub iface: Interface,
    pub source: Option<(u64, [u32; 3])>,
}

type Key = (u64, u32, u32, u32);

pub enum Lookup {
    Ready(Arc<Compiled>),
    Pending,
    Failed,
}

#[derive(Clone, Copy)]
struct Recent {
    live: bool,
    code: u64,
    ctl: u64,
    quick: [u8; 0x60],
    stage: Option<Stage>,
}

impl Default for Recent {
    fn default() -> Self {
        Self { live: false, code: 0, ctl: 0, quick: [0; 0x60], stage: None }
    }
}

#[derive(Default)]
pub struct Shaders {
    stages: FastMap<(u64, u64), Option<Stage>>,
    funcs: FastMap<Key, Option<Arc<Compiled>>>,
    pending: Jobs<Key>,
    recent: [Recent; 4],
    recent_i: usize,
    prefetched: FastSet<u64>,
}

static DONE: LazyLock<Mutex<Vec<(Key, Option<Arc<Compiled>>)>>> = LazyLock::new(Default::default);

pub static COMPILE_WAIT: LazyLock<std::time::Duration> =
    LazyLock::new(|| std::time::Duration::from_millis(std::env::var("NX_COMPILE_WAIT_MS").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(250)));

type Job = Box<dyn FnOnce() + Send>;

fn job_queue(name: &'static str, threads: usize) -> Mutex<std::sync::mpsc::Sender<Job>> {
    let (tx, rx) = std::sync::mpsc::channel::<Job>();
    let rx = Arc::new(Mutex::new(rx));
    for i in 0..threads {
        let rx = rx.clone();
        std::thread::Builder::new()
            .name(format!("{name}-{i}"))
            .spawn(move || loop {
                let job = rx.lock().unwrap().recv();
                match job {
                    Ok(job) => objc2::rc::autoreleasepool(|_| job()),
                    Err(_) => return,
                }
            })
            .unwrap();
    }
    Mutex::new(tx)
}

pub fn spawn_urgent(f: Job) {
    static QUEUE: LazyLock<Mutex<std::sync::mpsc::Sender<Job>>> = LazyLock::new(|| job_queue("shader-urgent", 2));
    let _ = QUEUE.lock().unwrap().send(f);
}

pub fn spawn_job(f: Job) {
    static QUEUE: LazyLock<Mutex<std::sync::mpsc::Sender<Job>>> =
        LazyLock::new(|| job_queue("shader", std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(2).max(2)));
    let _ = QUEUE.lock().unwrap().send(f);
}

static BUDGET_T0: LazyLock<std::time::Instant> = LazyLock::new(std::time::Instant::now);
static DEADLINE: AtomicU64 = AtomicU64::new(0);

pub fn reset_wait_budget() {
    DEADLINE.store((BUDGET_T0.elapsed() + *COMPILE_WAIT).as_nanos() as u64, Relaxed);
}

pub fn wait_over() -> bool {
    BUDGET_T0.elapsed().as_nanos() as u64 >= DEADLINE.load(Relaxed)
}

pub struct Jobs<K>(FastMap<K, (Arc<AtomicBool>, bool)>);

impl<K> Default for Jobs<K> {
    fn default() -> Self {
        Self(FastMap::default())
    }
}

impl<K: std::hash::Hash + Eq> Jobs<K> {
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn contains(&self, k: &K) -> bool {
        self.0.contains_key(k)
    }

    pub fn remove(&mut self, k: &K) {
        self.0.remove(k);
    }

    fn enqueue(&mut self, key: K, urgent: bool) -> Option<Arc<AtomicBool>> {
        match self.0.get_mut(&key) {
            Some((taken, queued)) if urgent && !*queued => {
                *queued = true;
                Some(taken.clone())
            }
            Some(_) => None,
            None => {
                let taken = Arc::new(AtomicBool::new(false));
                self.0.insert(key, (taken.clone(), urgent));
                Some(taken)
            }
        }
    }

    pub fn spawn(&mut self, key: K, urgent: bool, make: impl FnOnce() -> Job) {
        let Some(taken) = self.enqueue(key, urgent) else { return };
        let job = once(taken, make());
        if urgent { spawn_urgent(job) } else { spawn_job(job) }
    }
}

fn once(taken: Arc<AtomicBool>, job: Job) -> Job {
    Box::new(move || {
        if !taken.swap(true, AcqRel) {
            job();
        }
    })
}

pub fn sync_mode() -> bool {
    static S: LazyLock<bool> = LazyLock::new(|| std::env::var_os("NX_SYNC_SHADERS").is_some());
    *S
}

fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| (h ^ x as u64).wrapping_mul(0x100_0000_01b3))
}

fn rd32(addr: u64) -> u32 {
    unsafe { (addr as *const u32).read_unaligned() }
}

fn compute_stage(code: u64, ctl: u64) -> Option<Stage> {
    let local_size = [0x748, 0x74c, 0x750].map(|o| rd32(ctl + o));
    if local_size.iter().any(|&n| n == 0 || n > 1024) {
        return None;
    }
    let len = rd32(ctl + 0x700) as usize;
    if !(8..=0x10_0000).contains(&len) {
        return None;
    }
    let blob = unsafe { std::slice::from_raw_parts(code as *const u8, len) };
    Some(Stage { hash: fnv1a(blob) ^ 0xC0, code, len, constants: code + len as u64, fragment: false, local_size: Some(local_size) })
}

fn read_stage(code: u64, ctl: u64) -> Option<Stage> {
    if rd32(code) != 0x1234_5678 {
        return compute_stage(code, ctl);
    }
    let mut len = rd32(ctl + 0x700) as u64;
    let pool_end = STATE.lock().unwrap().pool_at(code).map(|(_, p)| p.cpu + p.size)?;
    len = len.clamp(0x100, 0x10_0000).min(pool_end - code);
    let blob = unsafe { std::slice::from_raw_parts(code as *const u8, len as usize) };
    let w0 = u32::from_le_bytes(blob[0x30..0x34].try_into().unwrap());
    Some(Stage {
        hash: fnv1a(&blob[0x30..]),
        code,
        len: len as usize,
        constants: code + rd32(ctl + 0x700) as u64,
        fragment: (w0 >> 10) & 0xF == 5,
        local_size: None,
    })
}

fn validate_stage(code: u64, ctl: u64, cached: Option<Stage>) {
    static ENABLED: LazyLock<bool> = LazyLock::new(|| std::env::var_os("NX_VALIDATE_STAGE_CACHE").is_some());
    if !*ENABLED { return; }
    let current = read_stage(code, ctl);
    if current != cached {
        static REPORTED: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        if REPORTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 32 {
            eprintln!("[shader] stale stage cache code {code:#x} ctl {ctl:#x}: cached {cached:?}, current {current:?}");
        }
    }
}

impl Shaders {
    pub fn stage(&mut self, code: u64, ctl: u64) -> Option<Stage> {
        let quick = unsafe { std::slice::from_raw_parts((code + 0x30) as *const u8, 0x60) };
        for slot in &self.recent {
            if slot.live && slot.code == code && slot.ctl == ctl && slot.quick.as_slice() == quick {
                validate_stage(code, ctl, slot.stage);
                return slot.stage;
            }
        }
        let qh = fnv1a(quick);
        let key = (code, ctl ^ qh);
        let s = if let Some(s) = self.stages.get(&key) {
            validate_stage(code, ctl, *s);
            *s
        } else {
            let s = read_stage(code, ctl);
            self.stages.insert(key, s);
            s
        };
        let mut bytes = [0u8; 0x60];
        bytes.copy_from_slice(quick);
        self.recent[self.recent_i] = Recent { live: true, code, ctl, quick: bytes, stage: s };
        self.recent_i = (self.recent_i + 1) % self.recent.len();
        s
    }

    pub fn prefetch(&mut self, programs: &std::collections::BTreeSet<(u64, u64)>) {
        let mut files = std::collections::BTreeSet::new();
        for &(code, ctl) in programs {
            let Some(s) = self.stage(code, ctl) else { continue };
            if s.local_size.is_some() || !self.prefetched.insert(s.hash) {
                continue;
            }
            let blob = unsafe { std::slice::from_raw_parts(s.code as *const u8, s.len) };
            let Some(gk) = nxshader::corpus::key(blob) else { continue };
            let index = AOT.index.read().unwrap();
            let libs = AOT.libs.lock().unwrap();
            for msl in GUEST_LIBS.get(&gk).into_iter().flatten() {
                if let Some(f) = index.get(msl).filter(|f| !libs.contains_key(*f)) {
                    files.insert(f.clone());
                }
            }
        }
        if files.len() < 2 {
            return;
        }
        let t0 = std::time::Instant::now();
        let files: Vec<String> = files.into_iter().collect();
        let next = std::sync::atomic::AtomicUsize::new(0);
        let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(files.len());
        crate::kernel::blocking(|| {
            std::thread::scope(|sc| {
                for _ in 0..workers {
                    sc.spawn(|| {
                        objc2::rc::autoreleasepool(|_| {
                            while let Some(f) = files.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                                let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&format!("{}/{f}", aot_dir())));
                                let l = gfx::gpu().device.0.newLibraryWithURL_error(&url).ok().map(Shared);
                                AOT.libs.lock().unwrap().entry(f.clone()).or_insert(l);
                            }
                        })
                    });
                }
            })
        });
        if t0.elapsed().as_millis() >= 20 {
            eprintln!("[shader] loaded {} shader libraries ahead of the draws in {} ms", files.len(), t0.elapsed().as_millis());
        }
    }

    pub fn function(&mut self, s: &Stage, extra_outputs: u32, uint_targets: u32, sint_targets: u32, wait: bool) -> Lookup {
        let key = (s.hash, extra_outputs, uint_targets, sint_targets);
        let t0 = std::time::Instant::now();
        loop {
            if !self.pending.is_empty() {
                self.drain();
            }
            if let Some(f) = self.funcs.get(&key) {
                return f.clone().map_or(Lookup::Failed, Lookup::Ready);
            }
            if !self.pending.contains(&key) {
                break;
            }
            if !wait || wait_over() {
                return Lookup::Pending;
            }
            self.pending.spawn(key, true, || compile_job(s, key));
            crate::kernel::blocking(|| std::thread::sleep(std::time::Duration::from_millis(1)));
        }
        let blob = unsafe { std::slice::from_raw_parts(s.code as *const u8, s.len) };
        dump_requested(blob, s.hash, extra_outputs, uint_targets, sint_targets);
        let compute = s.local_size.is_some();
        if !compute {
            if let Some(hit) = nxshader::corpus::key(blob).and_then(|gk| guest_compiled(gk, extra_outputs, uint_targets, sint_targets)) {
                self.funcs.insert(key, Some(hit.clone()));
                return Lookup::Ready(hit);
            }
        } else {
            if let Some(hit) = WARM_COMPUTE.lock().unwrap().get(&s.hash).map(|(c, _)| c.clone()) {
                self.funcs.insert(key, Some(hit.clone()));
                return Lookup::Ready(hit);
            }
            record_compute(s.hash, blob);
        }
        let hash = s.hash;
        if sync_mode() {
            let r = crate::kernel::blocking(|| compile(blob, compute, extra_outputs, uint_targets, sint_targets, hash));
            self.funcs.insert(key, r.clone());
            return r.map_or(Lookup::Failed, Lookup::Ready);
        }
        self.pending.spawn(key, wait, || compile_job(s, key));
        if !wait {
            return Lookup::Pending;
        }
        loop {
            self.drain();
            if let Some(f) = self.funcs.get(&key) {
                if t0.elapsed().as_millis() >= 5 {
                    eprintln!("[shader] {hash:016x}: compiled while the render thread waited (new render target), {} ms", t0.elapsed().as_millis());
                }
                return f.clone().map_or(Lookup::Failed, Lookup::Ready);
            }
            if wait_over() {
                if t0.elapsed().as_millis() >= 5 {
                    eprintln!("[shader] {hash:016x}: still compiling after {} ms, drawing without it for now", t0.elapsed().as_millis());
                }
                return Lookup::Pending;
            }
            crate::kernel::blocking(|| std::thread::sleep(std::time::Duration::from_millis(1)));
        }
    }

    fn drain(&mut self) {
        for (k, r) in std::mem::take(&mut *DONE.lock().unwrap()) {
            self.pending.remove(&k);
            self.funcs.entry(k).or_insert(r);
        }
    }
}

fn compile_job(s: &Stage, key: Key) -> Job {
    let blob = unsafe { std::slice::from_raw_parts(s.code as *const u8, s.len) }.to_vec();
    let compute = s.local_size.is_some();
    let (hash, extra_outputs, uint_targets, sint_targets) = key;
    Box::new(move || {
        let r = compile(&blob, compute, extra_outputs, uint_targets, sint_targets, hash);
        DONE.lock().unwrap().push((key, r));
    })
}

pub type ComputePipeline = objc2::runtime::ProtocolObject<dyn MTLComputePipelineState>;

const COMPUTE_DIR: &str = "work/compute";

static WARM_COMPUTE: LazyLock<Mutex<HashMap<u64, (Arc<Compiled>, Option<Shared<ComputePipeline>>)>>> = LazyLock::new(Default::default);

fn record_compute(hash: u64, blob: &[u8]) {
    let path = format!("{COMPUTE_DIR}/{hash:016x}.bin");
    if !std::path::Path::new(&path).exists() {
        let _ = std::fs::create_dir_all(COMPUTE_DIR);
        let _ = std::fs::write(&path, blob);
    }
}

pub(super) fn warm_kernel(hash: u64) -> Option<Shared<ComputePipeline>> {
    WARM_COMPUTE.lock().unwrap().get(&hash).and_then(|(_, p)| p.clone())
}

pub(super) fn warm_compute() -> usize {
    let files: Vec<(u64, std::path::PathBuf)> = std::fs::read_dir(COMPUTE_DIR)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| {
            let p = e.path();
            let h = u64::from_str_radix(p.file_stem()?.to_str()?, 16).ok()?;
            (p.extension()? == "bin").then_some((h, p))
        })
        .collect();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let done = std::sync::atomic::AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(4, |n| n.get()).min(files.len().max(1));
    std::thread::scope(|sc| {
        for _ in 0..workers {
            sc.spawn(|| {
                unsafe { libc::pthread_set_qos_class_self_np(libc::qos_class_t::QOS_CLASS_USER_INITIATED, 0) };
                while let Some((hash, path)) = files.get(next.fetch_add(1, std::sync::atomic::Ordering::Relaxed)) {
                    objc2::rc::autoreleasepool(|_| {
                        let Ok(blob) = std::fs::read(path) else { return };
                        if fnv1a(&blob) ^ 0xC0 != *hash {
                            let _ = std::fs::remove_file(path);
                            return;
                        }
                        let Some(c) = compile(&blob, true, 0, 0, 0, *hash) else { return };
                        let p = gfx::gpu().device.0.newComputePipelineStateWithFunction_error(&c.func.0).ok().map(Shared);
                        WARM_COMPUTE.lock().unwrap().insert(*hash, (c, p));
                        done.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    });
                }
            });
        }
    });
    done.into_inner()
}

fn dump_requested(blob: &[u8], hash: u64, extra_outputs: u32, uint_targets: u32, sint_targets: u32) {
    static WANT: LazyLock<Option<u64>> = LazyLock::new(|| std::env::var("NX_DUMP_SHADER").ok().and_then(|v| u64::from_str_radix(&v, 16).ok()));
    let Some(want) = *WANT else { return };
    if hash != want && nxshader::corpus::key(blob) != Some(want) {
        return;
    }
    let _ = std::fs::create_dir_all("work/msl_rt");
    let _ = std::fs::write(format!("work/msl_rt/{want:016x}.bin"), blob);
    match nxshader::translate_with(blob, extra_outputs, uint_targets, sint_targets) {
        Ok(t) => drop(std::fs::write(format!("work/msl_rt/{want:016x}.metal"), &t.msl)),
        Err(e) => eprintln!("[shader] {want:016x}: translation failed: {e}"),
    }
    eprintln!("[shader] {want:016x} dumped to work/msl_rt");
}

pub(super) fn compile(blob: &[u8], compute: bool, extra_outputs: u32, uint_targets: u32, sint_targets: u32, hash: u64) -> Option<Arc<Compiled>> {
    let t0 = std::time::Instant::now();
    let r = compile_inner(blob, compute, extra_outputs, uint_targets, sint_targets, hash);
    let ms = t0.elapsed().as_millis();
    if ms > 500 {
        eprintln!("[shader] {hash:016x}: compiling took {ms} ms");
    }
    r
}

fn compile_inner(blob: &[u8], compute: bool, extra_outputs: u32, uint_targets: u32, sint_targets: u32, hash: u64) -> Option<Arc<Compiled>> {
    if !compute {
        if let Some(hit) = nxshader::corpus::key(blob).and_then(|gk| guest_compiled(gk, extra_outputs, uint_targets, sint_targets)) {
            return Some(hit);
        }
    }
    if std::env::var_os("NX_DUMP_MSL").is_some() {
        let _ = std::fs::create_dir_all("work/msl_rt");
        let _ = std::fs::write(format!("work/msl_rt/{hash:016x}.bin"), blob);
    }
    let t = if compute { nxshader::translate_compute(blob, 0) } else { nxshader::translate_with(blob, extra_outputs, uint_targets, sint_targets) };
    let t = match t {
        Ok(t) => t,
        Err(e) => {
            eprintln!("[shader] {hash:016x}: translation failed: {e}");
            return None;
        }
    };
    if std::env::var_os("NX_DUMP_MSL").is_some() {
        let _ = std::fs::create_dir_all("work/msl_rt");
        let _ = std::fs::write(format!("work/msl_rt/{hash:016x}_{extra_outputs:x}.metal"), &t.msl);
    }
    let source = if compute { None } else { nxshader::corpus::key(blob).map(|k| (k, [extra_outputs, uint_targets, sint_targets])) };
    let (key, src, entry) = cache_name(&t);
    if let Some((lib, func)) = aot_function(key, &entry) {
        return Some(Arc::new(Compiled { func, lib, entry, iface: t.interface, source }));
    }
    let msl = format!("{}{src}", nxshader::prelude());
    let opts = MTLCompileOptions::new();
    opts.setMathMode(MTLMathMode::Safe);
    opts.setPreserveInvariance(true);
    let dev = &gfx::gpu().device.0;
    let lib = match dev.newLibraryWithSource_options_error(&NSString::from_str(&msl), Some(&opts)) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("[shader] {hash:016x}: Metal compile failed: {}", e.localizedDescription());
            let _ = std::fs::create_dir_all("work/msl_fail");
            let _ = std::fs::write(format!("work/msl_fail/{hash:016x}.metal"), &msl);
            return None;
        }
    };
    let func = lib.newFunctionWithName(&NSString::from_str(&entry))?;
    Some(Arc::new(Compiled { func: Shared(func), lib: Shared(lib), entry, iface: t.interface, source }))
}

pub(super) fn aot_dir() -> &'static str {
    static DIR: LazyLock<String> = LazyLock::new(|| format!("{}/heap-segments-v2", std::env::var("NX_SHADER_CACHE").unwrap_or_else(|_| "work/shader_cache.noindex".into())));
    &DIR
}

fn aot_sdk() -> String {
    std::env::var("NX_SHADER_SDK").unwrap_or_else(|_| "macosx".into())
}

pub(super) const IOS_MIN: &str = "26.0";
const GUEST_FILE: &str = "guest.tsv";
const AOT_BATCH: usize = 64;
const AOT_FLAGS: [&str; 3] = ["-fmetal-math-mode=safe", "-fpreserve-invariance", "-w"];

pub type Library = objc2::runtime::ProtocolObject<dyn MTLLibrary>;

fn cache_name(t: &nxshader::Translated) -> (u64, String, String) {
    let key = fnv1a(&t.msl.as_bytes()[nxshader::prelude().len()..]);
    let (src, entry) = t.named(&format!("{key:016x}"));
    (key, src, entry)
}

struct Aot {
    index: std::sync::RwLock<HashMap<u64, String>>,
    libs: Mutex<HashMap<String, Option<Shared<Library>>>>,
}

fn read_index() -> HashMap<u64, String> {
    std::fs::read_to_string(format!("{}/index.tsv", aot_dir()))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, f) = l.split_once('\t')?;
            Some((u64::from_str_radix(k, 16).ok()?, f.to_string()))
        })
        .collect()
}

static AOT: LazyLock<Aot> = LazyLock::new(|| Aot { index: std::sync::RwLock::new(read_index()), libs: Mutex::new(HashMap::new()) });

struct GuestShader {
    msl: u64,
    entry: String,
    iface: Interface,
}

static GUEST: LazyLock<HashMap<(u64, u32, u32, u32), GuestShader>> = LazyLock::new(load_guest);

static GUEST_LIBS: LazyLock<HashMap<u64, Vec<u64>>> = LazyLock::new(|| {
    let mut m: HashMap<u64, Vec<u64>> = HashMap::new();
    for (&(gk, ..), g) in GUEST.iter() {
        m.entry(gk).or_default().push(g.msl);
    }
    m
});

fn kind_id(k: TexKind) -> u32 {
    match k {
        TexKind::T2D => 0,
        TexKind::T2DArray => 1,
        TexKind::T3D => 2,
        TexKind::Cube => 3,
        TexKind::CubeArray => 4,
        TexKind::Depth2D => 5,
        TexKind::Depth2DArray => 6,
        TexKind::DepthCube => 7,
    }
}

fn kind_from(n: u32) -> Option<TexKind> {
    Some(match n {
        0 => TexKind::T2D,
        1 => TexKind::T2DArray,
        2 => TexKind::T3D,
        3 => TexKind::Cube,
        4 => TexKind::CubeArray,
        5 => TexKind::Depth2D,
        6 => TexKind::Depth2DArray,
        7 => TexKind::DepthCube,
        _ => return None,
    })
}

fn pairs(map: &str) -> impl Iterator<Item = (u32, u32)> + '_ {
    map.split(',').filter(|s| !s.is_empty()).filter_map(|s| {
        let (a, b) = s.split_once(':')?;
        Some((u32::from_str_radix(a, 16).ok()?, u32::from_str_radix(b, 16).ok()?))
    })
}

fn parse_guest(line: &str) -> Option<((u64, u32, u32, u32), GuestShader)> {
    let f: Vec<&str> = line.split('\t').collect();
    if f.len() != 12 {
        return None;
    }
    let hex = |s: &str| u64::from_str_radix(s, 16).ok();
    let u = |s: &str| u32::from_str_radix(s, 16).ok();
    let mut iface = Interface::default();
    for (b, k) in pairs(f[10]) {
        iface.textures.insert(b, kind_from(k)?);
    }
    for (b, slot) in pairs(f[11]) {
        iface.samplers.insert(b, slot);
    }
    let gk = hex(f[0])?;
    let extra = u(f[1])?;
    let uints = u(f[2])?;
    let sints = u(f[3])?;
    let g = GuestShader {
        msl: hex(f[4])?,
        entry: f[5].to_string(),
        iface: Interface {
            textures: iface.textures,
            samplers: iface.samplers,
            attributes: u(f[6])?,
            outputs: u(f[7])?,
            color_targets: u(f[8])?,
            writes_depth: u(f[9])? & 1 != 0,
            far_plane: u(f[9])? & 2 != 0,
        },
    };
    (g.entry.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')).then(|| ((gk, extra, uints, sints), g))
}

fn load_guest() -> HashMap<(u64, u32, u32, u32), GuestShader> {
    std::fs::read_to_string(format!("{}/{GUEST_FILE}", aot_dir()))
        .unwrap_or_default()
        .lines()
        .filter_map(parse_guest)
        .collect()
}

fn guest_line(gk: u64, [extra, uints, sints]: [u32; 3], msl: u64, entry: &str, iface: &Interface) -> String {
    let list = |items: Vec<(u32, u32)>| items.into_iter().map(|(a, b)| format!("{a:x}:{b:x}")).collect::<Vec<_>>().join(",");
    let textures = list(iface.textures.iter().map(|(&b, &k)| (b, kind_id(k))).collect());
    let samplers = list(iface.samplers.iter().map(|(&b, &s)| (b, s)).collect());
    format!(
        "{gk:016x}\t{extra:x}\t{uints:x}\t{sints:x}\t{msl:016x}\t{entry}\t{:x}\t{:x}\t{:x}\t{}\t{textures}\t{samplers}\n",
        iface.attributes,
        iface.outputs,
        iface.color_targets,
        iface.writes_depth as u8 | (iface.far_plane as u8) << 1
    )
}

pub(super) fn aot_compiled((gk, [extra, uints, sints]): (u64, [u32; 3])) -> Option<Arc<Compiled>> {
    guest_compiled(gk, extra, uints, sints)
}

fn guest_compiled(gk: u64, extra: u32, uints: u32, sints: u32) -> Option<Arc<Compiled>> {
    let g = GUEST.get(&(gk, extra, uints, sints))?;
    let (lib, func) = aot_function(g.msl, &g.entry)?;
    Some(Arc::new(Compiled { func, lib, entry: g.entry.clone(), iface: g.iface.clone(), source: Some((gk, [extra, uints, sints])) }))
}

fn aot_function(key: u64, entry: &str) -> Option<(Shared<Library>, Shared<Function>)> {
    let file = AOT.index.read().unwrap().get(&key)?.clone();
    let cached = AOT.libs.lock().unwrap().get(&file).cloned();
    let lib = match cached {
        Some(l) => l,
        None => {
            let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&format!("{}/{file}", aot_dir())));
            let l = gfx::gpu().device.0.newLibraryWithURL_error(&url).ok().map(Shared);
            AOT.libs.lock().unwrap().entry(file).or_insert(l).clone()
        }
    }?;
    let func = lib.0.newFunctionWithName(&NSString::from_str(entry)).map(Shared)?;
    Some((lib, func))
}

pub fn precompile(romfs: nxfs::storage::DynStorage, modules: &[std::path::PathBuf]) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let t0 = std::time::Instant::now();
    let fs = nxfs::romfs::RomFs::open(romfs)?;
    let mut found = std::collections::BTreeMap::new();
    let mut pairs = std::collections::BTreeSet::new();
    for f in fs.files() {
        if let Ok(data) = fs.storage().read_vec(f.offset, f.size as usize) {
            nxshader::corpus::scan_pairs(&data, &mut found, &mut pairs);
        }
    }
    for m in modules {
        if let Ok(data) = std::fs::read(m) {
            nxshader::corpus::scan(&data, &mut found);
        }
    }
    let mut blobs: Vec<(u64, [u32; 3], &Vec<u8>)> = found.iter().map(|(k, v)| (*k, [0; 3], v)).collect();
    let variants = super::state::recorded_variants();
    blobs.extend(variants.iter().filter_map(|&(k, p)| Some((k, p, found.get(&k)?))));
    eprintln!("[shader] {} programs and {} vertex/fragment pairs found in {:.1} s ({} recorded variants)", found.len(), pairs.len(), t0.elapsed().as_secs_f32(), variants.len());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());

    let sources = Mutex::new(std::collections::BTreeMap::new());
    let guest = Mutex::new(Vec::new());
    let attrs = Mutex::new(HashMap::new());
    let translate_all = |blobs: &[(u64, [u32; 3], &Vec<u8>)]| {
        let next = AtomicUsize::new(0);
        std::thread::scope(|s| {
            for _ in 0..threads {
                s.spawn(|| loop {
                    let Some(&(gk, p, blob)) = blobs.get(next.fetch_add(1, Relaxed)) else { return };
                    let t = if p == [0; 3] { nxshader::translate(blob) } else { nxshader::translate_with(blob, p[0], p[1], p[2]) };
                    match t {
                        Ok(t) => {
                            if p == [0; 3] {
                                attrs.lock().unwrap().insert(gk, t.interface.attributes);
                            }
                            let (key, src, entry) = cache_name(&t);
                            guest.lock().unwrap().push(guest_line(gk, p, key, &entry, &t.interface));
                            sources.lock().unwrap().insert(key, src);
                        }
                        Err(e) => eprintln!("[shader] {:016x}: translation failed: {e}", fnv1a(&blob[0x30..])),
                    }
                });
            }
        });
    };
    translate_all(&blobs);
    let attrs = std::mem::take(&mut *attrs.lock().unwrap());
    let have_variant: std::collections::HashSet<(u64, [u32; 3])> = blobs.iter().map(|&(k, p, _)| (k, p)).collect();
    let pair_variants: Vec<((u64, [u32; 3]), (u64, [u32; 3]))> = pairs
        .iter()
        .filter_map(|&(v, f)| Some(((v, [*attrs.get(&f)?, 0, 0]), (f, [0; 3]))))
        .filter(|((v, _), _)| found.contains_key(v))
        .collect();
    let mut extra: Vec<(u64, [u32; 3], &Vec<u8>)> = pair_variants.iter().map(|&(v, _)| v).filter(|v| v.1 != [0; 3] && !have_variant.contains(v)).map(|(k, p)| (k, p, &found[&k])).collect();
    extra.sort_by_key(|&(k, p, _)| (k, p));
    extra.dedup_by_key(|&mut (k, p, _)| (k, p));
    eprintln!("[shader] {} vertex variants for the vertex/fragment pairs", extra.len());
    translate_all(&extra);
    let sources: Vec<(u64, String)> = sources.into_inner().unwrap().into_iter().collect();
    let mut guest = guest.into_inner().unwrap();
    guest.sort();
    guest.dedup();

    let index = AOT.index.read().unwrap().clone();
    let have = |k: &u64| index.get(k).filter(|f| std::path::Path::new(&format!("{}/{f}", aot_dir())).exists());
    let kept: Vec<String> = sources.iter().filter_map(|(k, _)| have(k).map(|f| format!("{k:016x}\t{f}\n"))).collect();
    let sources: Vec<(u64, String)> = sources.into_iter().filter(|(k, _)| have(k).is_none()).collect();
    eprintln!("[shader] {} shaders already compiled, {} to compile", kept.len(), sources.len());
    std::fs::create_dir_all(aot_dir())?;
    std::fs::write(format!("{}/{GUEST_FILE}", aot_dir()), guest.concat())?;
    let used: std::collections::HashSet<&str> = kept.iter().filter_map(|l| l.trim_end().split('\t').nth(1)).collect();
    let mut first = 0;
    for e in std::fs::read_dir(aot_dir())?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if let Some(n) = name.strip_suffix(".metallib") {
            if used.contains(name.as_str()) {
                first = first.max(n.parse::<usize>().map_or(0, |n| n + 1));
            } else {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }

    let sdk = aot_sdk();
    let batches: Vec<_> = sources.chunks(AOT_BATCH).collect();
    let index = Mutex::new(kept.concat());
    let (next, done) = (AtomicUsize::new(0), AtomicUsize::new(0));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Relaxed);
                let Some(batch) = batches.get(i) else { return };
                let n = first + i;
                let (metal, lib) = (format!("{}/{n:04}.metal", aot_dir()), format!("{n:04}.metallib"));
                let mut src = nxshader::prelude().to_string();
                for (_, s) in batch.iter() {
                    src.push_str(s);
                }
                let ok = std::fs::write(&metal, &src).is_ok()
                    && std::process::Command::new("xcrun")
                        .args(["-sdk", &sdk, "metal"])
                        .args(AOT_FLAGS)
                        .args((sdk == "iphoneos").then(|| format!("-mios-version-min={IOS_MIN}")))
                        .args([metal.as_str(), "-o", &format!("{}/{lib}", aot_dir())])
                        .status()
                        .is_ok_and(|st| st.success());
                let _ = std::fs::remove_file(&metal);
                if ok {
                    let mut idx = index.lock().unwrap();
                    for (key, _) in batch.iter() {
                        idx.push_str(&format!("{key:016x}\t{lib}\n"));
                    }
                } else {
                    eprintln!("[shader] library {lib} failed to compile: its shaders will be compiled at runtime");
                }
                let n = done.fetch_add(batch.len(), Relaxed) + batch.len();
                if n / 1000 != (n - batch.len()) / 1000 {
                    eprintln!("[shader] {n}/{}", sources.len());
                }
            });
        }
    });
    let index = index.into_inner().unwrap();
    if index.is_empty() && !sources.is_empty() {
        eprintln!(
            "[shader] warning: no shader was precompiled, so the game compiles them while it plays (black or missing \
             graphics for a moment in new scenes). Install the Metal Toolchain with `xcodebuild -downloadComponent \
             MetalToolchain`, then Prepare again"
        );
    } else {
        eprintln!("[shader] precompiled {} shaders in {:.0} s", sources.len(), t0.elapsed().as_secs_f32());
    }
    std::fs::write(format!("{}/index.tsv", aot_dir()), index)?;
    *AOT.index.write().unwrap() = read_index();
    AOT.libs.lock().unwrap().clear();
    if sdk == "iphoneos" {
        return pipeline_scripts(&pair_variants);
    }
    super::state::precompile_pipelines(&found, &pair_variants)
}

const SCRIPT_BATCH: usize = 1000;

fn pipeline_scripts(known_pairs: &[((u64, [u32; 3]), (u64, [u32; 3]))]) -> anyhow::Result<()> {
    let guest = load_guest();
    let index = read_index();
    let dir = std::path::Path::new(aot_dir()).join("pso4-scripts");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir)?;
    let mut pairs: Vec<_> = super::state::recorded_pairs();
    if std::env::var_os("NX_PREBUILD_PAIRS").is_none_or(|v| v != "0") {
        pairs.extend_from_slice(known_pairs);
    }
    pairs.sort();
    pairs.dedup();
    let cache = std::fs::canonicalize(aot_dir())?;
    let resolve = |(gk, [e, u, s]): (u64, [u32; 3])| {
        let g = guest.get(&(gk, e, u, s))?;
        Some((cache.join(index.get(&g.msl)?), g.entry.clone()))
    };
    let resolved: Vec<_> = pairs.iter().filter_map(|&(v, f)| Some((resolve(v)?, resolve(f)?))).collect();
    let unspecialized = r#"{"pixel_format":"Unspecialized","write_mask":"Unspecialized","blending_state":"Unspecialized","alpha_blend_operation":"Unspecialized","rgb_blend_operation":"Unspecialized","destination_alpha_blend_factor":"Unspecialized","destination_rgb_blend_factor":"Unspecialized","source_alpha_blend_factor":"Unspecialized","source_rgb_blend_factor":"Unspecialized"}"#;
    let colors = vec![unspecialized; 8].join(",");
    for (n, batch) in resolved.chunks(SCRIPT_BATCH).enumerate() {
        let mut libs: Vec<&std::path::Path> = batch.iter().flat_map(|((a, _), (b, _))| [a.as_path(), b.as_path()]).collect();
        libs.sort();
        libs.dedup();
        let lib_id = |p: &std::path::Path| libs.binary_search(&p).unwrap();
        let mut funcs: Vec<(usize, &str)> = batch.iter().flat_map(|((a, ea), (b, eb))| [(lib_id(a), ea.as_str()), (lib_id(b), eb.as_str())]).collect();
        funcs.sort();
        funcs.dedup();
        let func_id = |p: &std::path::Path, e: &str| funcs.binary_search(&(lib_id(p), e)).unwrap();
        let json_str = |s: &str| format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""));
        let libraries: Vec<String> = libs.iter().enumerate().map(|(i, p)| format!(r#"{{"label":"L{i}","path":{}}}"#, json_str(&p.to_string_lossy()))).collect();
        let functions: Vec<String> = funcs.iter().enumerate().map(|(i, (l, e))| format!(r#"{{"label":"F{i}","name":{},"library":"L{l}"}}"#, json_str(e))).collect();
        let pipelines: Vec<String> = batch
            .iter()
            .map(|((a, ea), (b, eb))| format!(r#"{{"vertex_function_descriptor":"fnd:F{}","fragment_function_descriptor":"fnd:F{}","color_attachments":[{colors}]}}"#, func_id(a, ea), func_id(b, eb)))
            .collect();
        let script = format!(
            r#"{{"version":{{"major":0,"minor":1,"sub_minor":0}},"generator":"MetalFramework","libraries":[{}],"function_descriptors":{{"library_function_descriptors":[{}]}},"pipeline_descriptors":{{"render_pipeline_descriptors":[{}]}}}}"#,
            libraries.join(","),
            functions.join(","),
            pipelines.join(",")
        );
        std::fs::write(dir.join(format!("p{n:05}.mtl4-json")), script)?;
    }
    eprintln!("[gfx] {} pipeline bases ({} pairs) written to {} scripts for metal-tt", resolved.len(), pairs.len(), resolved.len().div_ceil(SCRIPT_BATCH));
    Ok(())
}

pub fn integer_kind(f: MTLPixelFormat) -> u32 {
    use MTLPixelFormat as P;
    match f {
        P::R8Uint | P::R16Uint | P::R32Uint | P::RG8Uint | P::RG16Uint | P::RG32Uint | P::RGBA8Uint | P::RGBA16Uint | P::RGBA32Uint | P::RGB10A2Uint => 1,
        P::R8Sint | P::R16Sint | P::R32Sint | P::RG8Sint | P::RG16Sint | P::RG32Sint | P::RGBA8Sint | P::RGBA16Sint | P::RGBA32Sint => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod job_tests {
    use super::*;

    #[test]
    fn urgent_requeue_runs_the_job_once() {
        let mut jobs = Jobs::<u32>::default();
        let first = jobs.enqueue(1, false).unwrap();
        let second = jobs.enqueue(1, true).unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        assert!(jobs.enqueue(1, true).is_none());
        assert!(jobs.enqueue(1, false).is_none());
        assert!(jobs.enqueue(2, true).is_some());
        assert!(jobs.enqueue(2, true).is_none());
        let runs = Arc::new(AtomicU64::new(0));
        for taken in [first, second] {
            let r = runs.clone();
            once(taken, Box::new(move || {
                r.fetch_add(1, Relaxed);
            }))();
        }
        assert_eq!(runs.load(Relaxed), 1);
        jobs.remove(&1);
        assert!(!jobs.contains(&1));
        assert!(jobs.enqueue(1, false).is_some());
    }

    #[test]
    fn wait_budget_is_shared_until_reset() {
        DEADLINE.store(0, Relaxed);
        assert!(wait_over());
        reset_wait_budget();
        assert!(!wait_over());
        DEADLINE.store(0, Relaxed);
        assert!(wait_over());
    }
}
