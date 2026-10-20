// Mush8 - shaders.rs
// Constantin Clerc (c22dev) - 0.1
use super::super::objects::STATE;
use crate::gfx::{self, Shared};
use nxshader::{Interface, TexKind};
use objc2_foundation::NSString;
use objc2_metal::*;
use super::super::fx::{FastMap, FastSet};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};

pub type Function = objc2::runtime::ProtocolObject<dyn MTLFunction>;

#[derive(Clone, Copy, Debug)]
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
    pending: FastSet<Key>,
    recent: [Recent; 4],
    recent_i: usize,
}

static DONE: LazyLock<Mutex<Vec<(Key, Option<Arc<Compiled>>)>>> = LazyLock::new(Default::default);

pub fn spawn_job(f: Box<dyn FnOnce() + Send>) {
    static QUEUE: LazyLock<Mutex<std::sync::mpsc::Sender<Box<dyn FnOnce() + Send>>>> = LazyLock::new(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Box<dyn FnOnce() + Send>>();
        let rx = Arc::new(Mutex::new(rx));
        let n = std::thread::available_parallelism().map_or(4, |n| n.get()).saturating_sub(2).max(2);
        for i in 0..n {
            let rx = rx.clone();
            std::thread::Builder::new()
                .name(format!("shader-{i}"))
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
    });
    let _ = QUEUE.lock().unwrap().send(f);
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

impl Shaders {
    pub fn stage(&mut self, code: u64, ctl: u64) -> Option<Stage> {
        let quick = unsafe { std::slice::from_raw_parts((code + 0x30) as *const u8, 0x60) };
        for slot in &self.recent {
            if slot.live && slot.code == code && slot.ctl == ctl && slot.quick.as_slice() == quick {
                return slot.stage;
            }
        }
        let qh = fnv1a(quick);
        let key = (code, ctl ^ qh);
        let s = if let Some(s) = self.stages.get(&key) {
            *s
        } else {
            let s = (|| {
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
            })();
            self.stages.insert(key, s);
            s
        };
        let mut bytes = [0u8; 0x60];
        bytes.copy_from_slice(quick);
        self.recent[self.recent_i] = Recent { live: true, code, ctl, quick: bytes, stage: s };
        self.recent_i = (self.recent_i + 1) % self.recent.len();
        s
    }

    pub fn function(&mut self, s: &Stage, extra_outputs: u32, uint_targets: u32, sint_targets: u32, wait: bool) -> Lookup {
        let key = (s.hash, extra_outputs, uint_targets, sint_targets);
        loop {
            if !self.pending.is_empty() {
                for (k, r) in std::mem::take(&mut *DONE.lock().unwrap()) {
                    self.pending.remove(&k);
                    self.funcs.insert(k, r);
                }
            }
            if let Some(f) = self.funcs.get(&key) {
                return f.clone().map_or(Lookup::Failed, Lookup::Ready);
            }
            if !self.pending.contains(&key) {
                break;
            }
            if !wait {
                return Lookup::Pending;
            }
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
        }
        let blob = blob.to_vec();
        let hash = s.hash;
        if wait || sync_mode() {
            let t0 = std::time::Instant::now();
            let r = crate::kernel::blocking(|| compile(&blob, compute, extra_outputs, uint_targets, sint_targets, hash));
            if wait && !sync_mode() && t0.elapsed().as_millis() >= 5 {
                eprintln!("[shader] {hash:016x}: compiled while the render thread waited (new render target), {} ms", t0.elapsed().as_millis());
            }
            self.funcs.insert(key, r.clone());
            return r.map_or(Lookup::Failed, Lookup::Ready);
        }
        self.pending.insert(key);
        spawn_job(Box::new(move || {
            let r = compile(&blob, compute, extra_outputs, uint_targets, sint_targets, hash);
            DONE.lock().unwrap().push((key, r));
        }));
        Lookup::Pending
    }
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
    if let Some(func) = aot_function(key, &entry) {
        return Some(Arc::new(Compiled { func, iface: t.interface, source }));
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
    Some(Arc::new(Compiled { func: Shared(func), iface: t.interface, source }))
}

pub(super) fn aot_dir() -> &'static str {
    // The key excludes the shared prelude. Invalidate AOT libraries when its
    // heap mapping ABI or implementation changes (v2 adds division-free paths).
    static DIR: LazyLock<String> = LazyLock::new(|| format!("{}/heap-segments-v2",std::env::var("NX_SHADER_CACHE").unwrap_or_else(|_| "work/shader_cache.noindex".into())));
    &DIR
}

fn aot_sdk() -> String {
    std::env::var("NX_SHADER_SDK").unwrap_or_else(|_| "macosx".into())
}

pub(super) const IOS_MIN: &str = "26.0";
const GUEST_FILE: &str = "guest.tsv";
const AOT_BATCH: usize = 64;
const AOT_FLAGS: [&str; 3] = ["-fmetal-math-mode=safe", "-fpreserve-invariance", "-w"];

type Library = objc2::runtime::ProtocolObject<dyn MTLLibrary>;

fn cache_name(t: &nxshader::Translated) -> (u64, String, String) {
    let key = fnv1a(&t.msl.as_bytes()[nxshader::prelude().len()..]);
    let (src, entry) = t.named(&format!("{key:016x}"));
    (key, src, entry)
}

struct Aot {
    index: HashMap<u64, String>,
    libs: Mutex<HashMap<String, Option<Shared<Library>>>>,
}

static AOT: LazyLock<Aot> = LazyLock::new(|| {
    let index = std::fs::read_to_string(format!("{}/index.tsv", aot_dir()))
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            let (k, f) = l.split_once('\t')?;
            Some((u64::from_str_radix(k, 16).ok()?, f.to_string()))
        })
        .collect();
    Aot { index, libs: Mutex::new(HashMap::new()) }
});

struct GuestShader {
    msl: u64,
    entry: String,
    iface: Interface,
}

static GUEST: LazyLock<HashMap<(u64, u32, u32, u32), GuestShader>> = LazyLock::new(load_guest);

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
            writes_depth: f[9] == "1",
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
        iface.writes_depth as u8
    )
}

pub(super) fn aot_compiled((gk, [extra, uints, sints]): (u64, [u32; 3])) -> Option<Arc<Compiled>> {
    guest_compiled(gk, extra, uints, sints)
}

fn guest_compiled(gk: u64, extra: u32, uints: u32, sints: u32) -> Option<Arc<Compiled>> {
    let g = GUEST.get(&(gk, extra, uints, sints))?;
    let func = aot_function(g.msl, &g.entry)?;
    Some(Arc::new(Compiled { func, iface: g.iface.clone(), source: Some((gk, [extra, uints, sints])) }))
}

fn aot_function(key: u64, entry: &str) -> Option<Shared<Function>> {
    let file = AOT.index.get(&key)?;
    let lib = AOT
        .libs
        .lock()
        .unwrap()
        .entry(file.clone())
        .or_insert_with(|| {
            let url = objc2_foundation::NSURL::fileURLWithPath(&NSString::from_str(&format!("{}/{file}", aot_dir())));
            gfx::gpu().device.0.newLibraryWithURL_error(&url).ok().map(Shared)
        })
        .clone()?;
    lib.0.newFunctionWithName(&NSString::from_str(entry)).map(Shared)
}

pub fn precompile(romfs: nxfs::storage::DynStorage, modules: &[std::path::PathBuf]) -> anyhow::Result<()> {
    use std::sync::atomic::{AtomicUsize, Ordering::Relaxed};
    let t0 = std::time::Instant::now();
    let fs = nxfs::romfs::RomFs::open(romfs)?;
    let mut found = std::collections::BTreeMap::new();
    for f in fs.files() {
        if let Ok(data) = fs.storage().read_vec(f.offset, f.size as usize) {
            nxshader::corpus::scan(&data, &mut found);
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
    eprintln!("[shader] {} programs found in {:.1} s ({} recorded variants)", found.len(), t0.elapsed().as_secs_f32(), variants.len());
    let threads = std::thread::available_parallelism().map_or(4, |n| n.get());

    let sources = Mutex::new(std::collections::BTreeMap::new());
    let guest = Mutex::new(Vec::new());
    let next = AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let Some(&(gk, p, blob)) = blobs.get(next.fetch_add(1, Relaxed)) else { return };
                let t = if p == [0; 3] { nxshader::translate(blob) } else { nxshader::translate_with(blob, p[0], p[1], p[2]) };
                match t {
                    Ok(t) => {
                        let (key, src, entry) = cache_name(&t);
                        guest.lock().unwrap().push(guest_line(gk, p, key, &entry, &t.interface));
                        sources.lock().unwrap().insert(key, src);
                    }
                    Err(e) => eprintln!("[shader] {:016x}: translation failed: {e}", fnv1a(&blob[0x30..])),
                }
            });
        }
    });
    let sources: Vec<(u64, String)> = sources.into_inner().unwrap().into_iter().collect();
    let mut guest = guest.into_inner().unwrap();
    guest.sort();
    guest.dedup();

    let have = |k: &u64| AOT.index.get(k).filter(|f| std::path::Path::new(&format!("{}/{f}", aot_dir())).exists());
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
    std::fs::write(format!("{}/index.tsv", aot_dir()), index.into_inner().unwrap())?;
    eprintln!("[shader] precompiled {} shaders in {:.0} s", sources.len(), t0.elapsed().as_secs_f32());
    if sdk == "iphoneos" || std::env::var_os("NX_PIPELINE_ARCHIVE").is_none() {
        return Ok(());
    }
    super::state::precompile_pipelines(&found)
}

pub fn integer_kind(f: MTLPixelFormat) -> u32 {
    use MTLPixelFormat as P;
    match f {
        P::R8Uint | P::R16Uint | P::R32Uint | P::RG8Uint | P::RG16Uint | P::RG32Uint | P::RGBA8Uint | P::RGBA16Uint | P::RGBA32Uint | P::RGB10A2Uint => 1,
        P::R8Sint | P::R16Sint | P::R32Sint | P::RG8Sint | P::RG16Sint | P::RG32Sint | P::RGBA8Sint | P::RGBA16Sint | P::RGBA32Sint => 2,
        _ => 0,
    }
}
