// Mush8 - main.rs
// Constantin Clerc (c22dev) - 0.1
mod aspect;
mod audio;
mod diag;
mod dnssd;
mod game;
mod gfx;
mod guest;
mod haptics;
mod hle;
mod input;
mod ipc;
mod kernel;
mod loader;
mod mem;
mod mods;
mod motion;
mod movie;
mod remote;
mod services;
#[cfg(target_os = "macos")]
mod sdl;
mod shm;
mod stubs;
mod nvn;
mod svc;
mod webpad;

use anyhow::{Context, Result};
use clap::Parser;
use std::path::PathBuf;
use std::sync::OnceLock;

static INITS: OnceLock<Vec<u64>> = OnceLock::new();

extern "C" fn init_done() {}

extern "C" fn init_modules() {
    for &f in INITS.get().unwrap() {
        let f: extern "C" fn() = unsafe { std::mem::transmute(f as usize) };
        f();
    }
}

struct Npdm {
    priority: u8,
    core: u8,
    stack_size: u32,
}

fn read_npdm(path: &PathBuf) -> Result<Npdm> {
    let b = std::fs::read(path).with_context(|| path.display().to_string())?;
    anyhow::ensure!(b.len() >= 0x20 && &b[..4] == b"META", "not an NPDM");
    anyhow::ensure!(b[0xC] & 1 != 0, "32-bit ARM (AArch32) code is not supported; Mush8 requires AArch64");
    Ok(Npdm { priority: b[0xE], core: b[0xF], stack_size: u32::from_le_bytes(b[0x1C..0x20].try_into()?) })
}

fn update_file(p: &std::path::Path) -> PathBuf {
    if !p.exists() && p == std::path::Path::new("update.nsp") && std::path::Path::new("upd4-0-1.nsp").exists() {
        return PathBuf::from("upd4-0-1.nsp");
    }
    p.to_path_buf()
}

fn program(n: &nxfs::Nsp) -> Result<&nxfs::nca::Nca> {
    n.find(nxfs::nca::ContentType::Program).context("no Program NCA")
}

fn load_dlc(args: &Args, keys: &nxfs::keys::Keys) -> Vec<(u64, nxfs::storage::DynStorage)> {
    let default = PathBuf::from("dlc.nsp");
    let paths = if args.dlc.is_empty() && default.exists() { vec![default] } else { args.dlc.clone() };
    let mut out = Vec::new();
    for p in paths {
        let found = (|| -> Result<Vec<(u64, nxfs::storage::DynStorage)>> {
            let nsp = nxfs::Nsp::open(&p, keys)?;
            let mut v = Vec::new();
            for nca in nsp.ncas.iter().filter(|n| n.content_type == nxfs::nca::ContentType::PublicData) {
                let sec = nca.sections.iter().find(|s| s.fs_type == nxfs::nca::FsType::RomFs).context("no RomFS")?;
                v.push((nca.program_id, nca.fs_storage(sec)?));
            }
            Ok(v)
        })();
        match found {
            Ok(v) => {
                for (id, _) in &v {
                    eprintln!("[host] add-on content {id:016x} ({})", p.display());
                }
                out.extend(v);
            }
            Err(e) => eprintln!("[host] {}: {e:#}", p.display()),
        }
    }
    out
}

#[derive(Parser)]
#[command(about = "Run rewritten Switch modules natively")]
struct Args {
    #[arg(long, default_value = "work/v-upd/patched")]
    patched: PathBuf,
    #[arg(long, default_value = "work/v-upd/exefs/main.npdm")]
    npdm: PathBuf,
    #[arg(long, default_value = "prod.keys")]
    keys: PathBuf,
    #[arg(long, default_value = "base.nsp")]
    base: PathBuf,
    #[arg(long, default_value = "update.nsp")]
    update: PathBuf,
    #[arg(long, default_value = "work/save")]
    save: PathBuf,
    #[arg(long, default_value = "work/sdmc")]
    sdmc: PathBuf,
    #[arg(long, default_value = "system")]
    system: PathBuf,
    #[arg(long, default_value = "Firmware")]
    firmware: PathBuf,
    #[arg(long)]
    dlc: Vec<PathBuf>,
    #[arg(long = "mod")]
    mods: Vec<PathBuf>,
    #[arg(long)]
    precompile: bool,
}

#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    start(Args::parse())?;
    gfx::run_app("Mush8", 720.0 * aspect::ratio(), 720.0)
}

#[cfg(target_os = "ios")]
fn main() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("HOME"));
    let docs = home.join("Documents");
    let _ = std::env::set_current_dir(&docs);
    let _ = std::fs::create_dir_all("work");
    let _ = std::fs::rename("work/nxhost.log", "work/nxhost.prev.log");
    if let Ok(log) = std::fs::File::create("work/nxhost.log") {
        use std::os::fd::AsRawFd;
        unsafe { libc::dup2(log.as_raw_fd(), 2) };
    }
    let bundle = std::env::current_exe().ok().and_then(|e| e.parent().map(PathBuf::from)).unwrap_or_default();
    let mut env: Vec<(String, String)> = vec![
        ("NX_SHADER_CACHE".into(), bundle.join("shaders").display().to_string()),
        ("NX_PIPELINE_LIST".into(), bundle.join("shaders/pipelines.tsv").display().to_string()),
    ];
    for line in std::fs::read_to_string("env.txt").unwrap_or_default().lines() {
        if let Some((k, v)) = line.trim().split_once('=').filter(|(k, _)| !k.is_empty() && !k.starts_with('#')) {
            env.push((k.trim().into(), v.trim().into()));
        }
    }
    for (k, v) in env {
        unsafe { std::env::set_var(k, v) };
    }
    eprintln!("[host] iOS: files in {}, bundle {}", docs.display(), bundle.display());
    let mut args = Args::parse_from(["nxhost", "--patched", &bundle.join("modules").display().to_string(), "--npdm", &bundle.join("modules/main.npdm").display().to_string()]);
    gfx::run_app(move || {
        apply_launcher_settings();
        args.mods = ios_mods();
        let missing: Vec<&str> = ["prod.keys", "base.nsp", "update.nsp"].into_iter().filter(|f| !update_file(std::path::Path::new(f)).exists()).collect();
        anyhow::ensure!(
            missing.is_empty(),
            "Missing {}.\n\nCopy prod.keys, base.nsp, the update .nsp (and optionally dlc.nsp and the Firmware folder) into this app's folder: Finder → your iPhone or iPad → Files → Mush8, or the Files app → On My iPhone → Mush8.",
            missing.join(", ")
        );
        start(args)
    })
}

#[cfg(target_os = "ios")]
fn ios_mods() -> Vec<PathBuf> {
    use std::ffi::{CStr, c_char, c_void};
    unsafe extern "C" {
        fn mush8_each_mod(context: *mut c_void, callback: extern "C" fn(*mut c_void, *const c_char));
    }
    extern "C" fn collect(context: *mut c_void, path: *const c_char) {
        let mods = unsafe { &mut *context.cast::<Vec<PathBuf>>() };
        mods.push(PathBuf::from(unsafe { CStr::from_ptr(path) }.to_string_lossy().as_ref()));
    }
    let mut mods = Vec::new();
    unsafe { mush8_each_mod((&mut mods as *mut Vec<PathBuf>).cast(), collect) };
    mods
}

#[cfg(target_os = "ios")]
fn apply_launcher_settings() {
    let config = std::fs::read_to_string("config.json").unwrap_or_default();
    let field = |key: &str| {
        let rest = config.split(&format!("\"{key}\"")).nth(1)?;
        let rest = rest.trim_start().strip_prefix(':')?.trim_start();
        if let Some(rest) = rest.strip_prefix('"') {
            let mut out = String::new();
            let mut chars = rest.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else if c == '"' {
                    break;
                } else {
                    out.push(c);
                }
            }
            Some(out)
        } else {
            Some(rest.split([',', '\n', '}']).next()?.trim().to_string())
        }
    };
    let network = field("network").unwrap_or_else(|| if field("lan").as_deref() == Some("true") { "lan".into() } else { "off".into() });
    unsafe { std::env::set_var("NX_NET", &network) };
    if network == "lan" {
        unsafe { std::env::set_var("NX_LAN", "1") };
    }
    if let Some(server) = field("lanplay_server").filter(|s| !s.is_empty()) {
        unsafe { std::env::set_var("NX_LANPLAY_SERVER", server) };
    }
    if field("prewarm_pipelines").as_deref() != Some("false") {
        unsafe { std::env::set_var("NX_PREWARM", "1") };
    }
    if field("background_compile").as_deref() == Some("false") {
        unsafe { std::env::set_var("NX_SYNC_SHADERS", "1") };
    }
    if field("rumble").as_deref() == Some("false") {
        unsafe { std::env::set_var("NX_RUMBLE", "0") };
    }
    if let Some(min) = field("rumble_min") {
        unsafe { std::env::set_var("NX_RUMBLE_MIN", min) };
    }
    if field("gamepad_driver").as_deref() == Some("sdl") {
        unsafe { std::env::set_var("NX_PAD_DRIVER", "sdl") };
    }
    if field("spatial_audio").as_deref() == Some("false") {
        unsafe { std::env::set_var("NX_SPATIAL", "0") };
    }
    if field("motion_controls").as_deref() == Some("true") {
        unsafe { std::env::set_var("NX_MOTION", "1") };
    }
    if field("handheld").as_deref() != Some("false") {
        unsafe { std::env::set_var("NX_HANDHELD", "1") };
    }
    if field("phone_pads").as_deref() == Some("true") {
        unsafe { std::env::set_var("NX_PHONE_PADS", "1") };
        if field("browser_pads").as_deref() == Some("true") {
            unsafe { std::env::set_var("NX_BROWSER_PADS", "1") };
        }
    }
    if field("aspect").as_deref() != Some("16:9") {
        unsafe { std::env::set_var("NX_ASPECT", "auto") };
    }
    if field("hud").as_deref() == Some("center") {
        unsafe { std::env::set_var("NX_HUD", "center") };
    }
    if let Some(scale) = field("hud_scale") {
        unsafe { std::env::set_var("NX_HUD_SCALE", scale) };
    }
}

fn start(args: Args) -> Result<()> {
    let dir = args.patched.clone();
    let npdm = read_npdm(&args.npdm)?;

    let keys = nxfs::keys::Keys::load(&args.keys)?;
    let base = nxfs::Nsp::open(&args.base, &keys)?;
    let update = nxfs::Nsp::open(&update_file(&args.update), &keys)?;
    let version = game::identify(&update, &dir)?;
    let romfs = mods::romfs(program(&update)?.patched_romfs(program(&base)?)?, &args.mods)?;
    eprintln!("[host] RomFS: {:.2} GB", nxfs::storage::Storage::size(&*romfs) as f64 / 1e9);
    if args.precompile {
        let modules: Vec<PathBuf> = ["main", "subsdk0", "sdk"].iter().map(|m| dir.join(format!("{m}.bin"))).collect();
        nvn::precompile(romfs, &modules)?;
        std::process::exit(0);
    }
    let romfs = aspect::romfs(romfs)?;
    let _ = services::fs::CONFIG.set(services::fs::Config {
        romfs,
        save_dir: if args.save == std::path::Path::new("work/save") && version != "4.0.1" { PathBuf::from(format!("work/save-{version}")) } else { args.save.clone() },
        sd_dir: args.sdmc.clone(),
        system_dir: args.system.clone(),
        firmware_dir: args.firmware.clone(),
        dlc: load_dlc(&args, &keys),
        keys,
    });

    if remote::enabled() {
        remote::host();
        if webpad::enabled() {
            webpad::host();
        }
    }
    diag::install_handlers();
    guest::init_tsd();
    let _ = kernel::KERNEL.aspace.set(mem::AddressSpace::reserve()?);
    let a = kernel::aspace();
    eprintln!("[host] guest address space at {:#x}..{:#x}", a.base, a.base + a.layout.total());

    let mods = loader::load(&dir, &["main", "subsdk0", "sdk"], &hle::table())?;
    let _ = diag::MODULES.set(
        mods.iter()
            .map(|m| {
                let mut symbols: Vec<(u64, String)> = m.dynamic.symbols.iter()
                    .filter(|s| s.defined() && !s.name.is_empty())
                    .map(|s| (s.value, nxfs::nso::demangle(&s.name)))
                    .collect();
                symbols.sort();
                diag::ModInfo { name: m.name.clone(), base: m.base, size: m.size, symbols }
            })
            .collect(),
    );
    diag::start_watch(&mods.iter().map(|m| (m.name.clone(), m.base)).collect::<Vec<_>>());
    let _ = INITS.set(mods.iter().filter_map(|m| m.dynamic.init.map(|i| m.base + i)).collect());
    let start = loader::symbol(&mods, "_ZN2nn4init5StartEmmPFvvES2_").context("nn::init::Start not found")?;
    let sdk = mods.iter().find(|m| m.name == "sdk").context("no sdk module")?;
    nvn::init(sdk.base, &sdk.module_id)?;
    if std::env::var_os("NX_PREWARM").is_some() {
        nvn::prewarm();
    }
    movie::init(&mods);
    if let Some(v) = loader::symbol(&mods, "_ZNK2nn3mii6detail11CharInfoRaw6VerifyEv") {
        let _ = services::mii::VERIFY.set(v);
    }

    let stack_size = (npdm.stack_size as u64).next_multiple_of(mem::PAGE);
    a.set(a.stack(), stack_size, mem::Block { state: mem::state::STACK, perm: mem::perm::RW, attr: 0 })?;
    let h = kernel::create_thread(start, 0, a.stack() + stack_size, npdm.priority as i32, npdm.core as i32)?;
    let t = kernel::thread(h).unwrap();
    eprintln!("[host] main thread {h:#x}: prio {} core {} stack {stack_size:#x}; entering nn::init::Start", npdm.priority, npdm.core);
    let main_base = mods[0].base;
    let j = kernel::start_thread(t, [h as u64, main_base, init_done as *const () as usize as u64, init_modules as *const () as usize as u64])?;
    std::thread::spawn(move || {
        j.join().ok();
        eprintln!("[host] main thread returned");
        std::process::exit(0);
    });
    Ok(())
}
