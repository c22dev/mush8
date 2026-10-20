// Mush8 - main.rs
// Constantin Clerc (c22dev) - 0.1
mod audio;
mod diag;
mod display;
mod display_profile;
mod dnssd;
mod gfx;
mod guest;
mod haptics;
mod hle;
mod input;
#[cfg(target_os = "macos")]
mod wolverine;
mod ipc;
mod kernel;
mod loader;
mod mem;
mod movie;
mod remote;
mod resource_compat;
mod services;
mod shm;
mod stubs;
mod nvn;
mod svc;

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
    Ok(Npdm { priority: b[0xE], core: b[0xF], stack_size: u32::from_le_bytes(b[0x1C..0x20].try_into()?) })
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
    /// Use the already extracted base Program NCA from the user's XCI.
    #[arg(long)]
    base_nca: Option<PathBuf>,
    #[arg(long, default_value = "upd4-0-1.nsp")]
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
    #[arg(long)]
    precompile: bool,
    #[arg(long)]
    movies: bool,
}

#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    let exe=std::env::current_exe()?;
    let config=exe.parent().and_then(|p|p.parent()).map(|p|p.join("Resources/launch.json"));
    let args=if std::env::args_os().len()==1 && config.as_ref().is_some_and(|p|p.exists()) {
        let cfg:serde_json::Value=serde_json::from_slice(&std::fs::read(config.unwrap())?)?;
        if let Some(env)=cfg["env"].as_object(){for (k,v) in env {if k.starts_with("NX_"){if let Some(v)=v.as_str(){unsafe{std::env::set_var(k,v);}}}}}
        std::env::set_current_dir(cfg["cwd"].as_str().context("missing app cwd")?)?;
        if let Some(path)=cfg["log"].as_str(){use std::os::fd::AsRawFd;let _=std::fs::rename(path,format!("{path}.previous"));let log=std::fs::File::create(path)?;unsafe{libc::dup2(log.as_raw_fd(),2);}}
        let mut args=vec![exe.to_string_lossy().into_owned()];
        for arg in cfg["args"].as_array().context("missing app args")?{args.push(arg.as_str().context("invalid app arg")?.to_owned());}
        unsafe extern "C" { fn mk8ShowMacSettings() -> i32; }
        if unsafe { mk8ShowMacSettings() } == 0 { return Ok(()); }
        if let Some(overrides) = std::env::var_os("NX_MAC_LAUNCH_ARGS") {
            let overrides: Vec<String> = serde_json::from_str(&overrides.to_string_lossy())
                .context("invalid Mac launcher arguments")?;
            args.truncate(1);
            args.extend(overrides);
            unsafe { std::env::remove_var("NX_MAC_LAUNCH_ARGS"); }
        }
        Args::parse_from(args)
    } else {Args::parse()};
    start(args)?;
    gfx::run_app("MK8 Native", 1280.0, 720.0)
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
    if !std::path::Path::new("config.json").exists(){let _=std::fs::copy(bundle.join("default-config.json"),"config.json");}
    let mut env: Vec<(String, String)> = vec![
        ("NX_SHADER_CACHE".into(), bundle.join("shaders").display().to_string()),
        ("NX_PIPELINE_LIST".into(), bundle.join("shaders/pipelines.tsv").display().to_string()),
        ("NX_FALLBACK_FONT".into(), if bundle.join("host-font.ttf").is_file() { bundle.join("host-font.ttf") } else { bundle.join("fallback/host-font.ttf") }.display().to_string()),
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
    let mut argv=vec!["nxhost".to_owned(),"--patched".into(),bundle.join("modules").display().to_string(),"--npdm".into(),bundle.join("modules/main.npdm").display().to_string()];
    if std::path::Path::new("base.nca").exists(){argv.extend(["--base-nca".into(),"base.nca".into()]);}
    let args = Args::parse_from(argv);
    gfx::run_app(move || {
        apply_launcher_settings();
        let mut missing: Vec<&str> = ["prod.keys", "upd4-0-1.nsp"].into_iter().filter(|f| !std::path::Path::new(f).exists()).collect();
        if !std::path::Path::new("base.nsp").exists() && !std::path::Path::new("base.nca").exists(){missing.push("base.nca (or base.nsp)");}
        anyhow::ensure!(
            missing.is_empty(),
            "Missing {}.\n\nCopy prod.keys, base.nca (or base.nsp), upd4-0-1.nsp (optionally dlc.nsp/Firmware) into this app's Files folder, then relaunch.",
            missing.join(", ")
        );
        start(args)
    })
}

#[cfg(target_os = "ios")]
fn apply_launcher_settings() {
    let config: serde_json::Value = std::fs::read("config.json").ok()
        .and_then(|data| serde_json::from_slice(&data).ok()).unwrap_or_default();
    let field = |key: &str| -> Option<String> {
        match &config[key] {
            serde_json::Value::String(s) => Some(s.clone()),
            serde_json::Value::Bool(b) => Some(b.to_string()),
            _ => None,
        }
    };
    // Apply before the guest queries set:sys; that service caches its language.
    let language = field("language").unwrap_or_else(|| "system".into());
    unsafe {
        if language == "system" { std::env::remove_var("NX_LANGUAGE"); }
        else { std::env::set_var("NX_LANGUAGE", &language); }
    }
    eprintln!("[settings] game language: {language}");
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
    if field("motion_controls").as_deref() == Some("true") {
        unsafe { std::env::set_var("NX_MOTION", "1") };
    }
    if field("handheld").as_deref() != Some("false") {
        unsafe { std::env::set_var("NX_HANDHELD", "1") };
    }
    if field("phone_pads").as_deref() == Some("true") {
        unsafe { std::env::set_var("NX_PHONE_PADS", "1") };
    }
}

fn start(args: Args) -> Result<()> {
    let dir = args.patched.clone();
    let npdm = read_npdm(&args.npdm)?;

    let keys = nxfs::keys::Keys::load(&args.keys)?;
    let base_nca = args.base_nca.as_ref().map(|p| nxfs::open_nca(p, &keys)).transpose()?;
    let base = if base_nca.is_none() { Some(nxfs::Nsp::open(&args.base, &keys)?) } else { None };
    let base_program = match &base_nca { Some(nca) => nca, None => program(base.as_ref().unwrap())? };
    anyhow::ensure!(base_program.content_type == nxfs::nca::ContentType::Program && base_program.program_id == 0x0100152000022000, "unexpected base Program NCA");
    let update = nxfs::Nsp::open(&args.update, &keys)?;
    let romfs = program(&update)?.patched_romfs(base_program)?;
    eprintln!("[host] RomFS: {:.2} GB", nxfs::storage::Storage::size(&*romfs) as f64 / 1e9);
    if args.movies {
        movie::webm::convert_all(romfs)?;
        std::process::exit(0);
    }
    if args.precompile {
        let modules: Vec<PathBuf> = ["main", "subsdk0", "sdk"].iter().map(|m| dir.join(format!("{m}.bin"))).collect();
        nvn::precompile(romfs, &modules)?;
        std::process::exit(0);
    }
    let _ = services::fs::CONFIG.set(services::fs::Config {
        romfs,
        save_dir: args.save.clone(),
        sd_dir: args.sdmc.clone(),
        system_dir: args.system.clone(),
        firmware_dir: args.firmware.clone(),
        dlc: load_dlc(&args, &keys),
        keys,
    });

    if remote::enabled() {
        remote::host();
    }
    diag::install_handlers();
    guest::init_tsd();
    let _ = kernel::KERNEL.aspace.set(mem::AddressSpace::reserve()?);
    let a = kernel::aspace();
    eprintln!("[host] guest address space at {:#x}..{:#x}", a.base, a.base + a.layout.total());

    let mods = loader::load(&dir, &["main", "subsdk0", "sdk"], &hle::table())?;
    resource_compat::init(&mods);
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
