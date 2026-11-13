// Mush8 - game.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result, bail};
use std::path::Path;
use std::sync::OnceLock;

const VERSIONS: &[(&str, &str)] = &[("158c0c7399aabd7bf83795c2ef3484c7", "4.0.1"), ("fe941ed5ba14be5d505698da1bbf4fe7", "3.0.5")];

static VERSION: OnceLock<&'static str> = OnceLock::new();

pub fn version() -> &'static str {
    VERSION.get().copied().unwrap_or("4.0.1")
}

fn named(id: &str) -> Option<&'static str> {
    VERSIONS.iter().find(|(p, _)| id.starts_with(p)).map(|(_, v)| *v)
}

pub fn update_main_id(update: &nxfs::Nsp) -> Result<String> {
    let prog = update.find(nxfs::nca::ContentType::Program).context("the update has no Program NCA")?;
    let exefs = prog.sections.iter().find(|s| s.fs_type == nxfs::nca::FsType::PartitionFs).context("the update has no ExeFS")?;
    let exefs = nxfs::pfs0::Pfs0::open(prog.fs_storage(exefs)?)?;
    let header = exefs.file("main").context("the update has no main module")?.read_vec(0, 0x60)?;
    anyhow::ensure!(&header[..4] == b"NSO0", "the update's main is not an NSO");
    Ok(header[0x40..0x50].iter().map(|b| format!("{b:02x}")).collect())
}

fn prepared_main_id(patched: &Path) -> Result<String> {
    let text = std::fs::read_to_string(patched.join("main.layout")).context("no prepared game code")?;
    text.lines().find_map(|l| l.strip_prefix("module_id ")).map(str::to_owned).context("main.layout has no module_id")
}

pub fn identify(update: &nxfs::Nsp, patched: &Path) -> Result<&'static str> {
    identify_id(&update_main_id(update)?, patched)
}

pub fn identify_id(id: &str, patched: &Path) -> Result<&'static str> {
    let Some(version) = named(id) else {
        bail!("this update isn't supported (main {id}): use update 4.0.1 or 3.0.5");
    };
    let prepared = prepared_main_id(patched)?;
    if !prepared.starts_with(id) {
        let built = named(&prepared).unwrap_or("another update");
        if cfg!(target_os = "ios") {
            bail!("this build of Mush8 is for update {built}, but the update file is {version}: import the {built} update, or build Mush8 again with {version}");
        }
        bail!("the game code was prepared for update {built}, but the update file is {version}: Prepare again");
    }
    let _ = VERSION.set(version);
    eprintln!("[host] Mario Kart 8 Deluxe {version}");
    Ok(version)
}
