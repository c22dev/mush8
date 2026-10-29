// Mush8 - mods.rs
// Constantin Clerc (c22dev) - 0.1
use anyhow::{Context, Result, bail};
use nxfs::storage::{DynStorage, HostFile, Mem};
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;

const ARCHIVES: &[&str] = &["sarc", "szs", "bntx", "msbt"];
const TITLE_ID: &str = "0100152000022000";

enum Node {
    File(PathBuf),
    Archive(BTreeMap<String, Node>),
}

struct Change<'a> {
    by: &'a str,
    node: Node,
}

fn child(dir: &Path, name: &str) -> Option<PathBuf> {
    std::fs::read_dir(dir).ok()?.flatten().map(|e| e.path()).find(|p| p.is_dir() && p.file_name().is_some_and(|n| n.eq_ignore_ascii_case(name)))
}

fn is_archive(name: &str) -> bool {
    name.rsplit_once('.').is_some_and(|(_, ext)| ARCHIVES.iter().any(|a| a.eq_ignore_ascii_case(ext)))
}

fn entries(dir: &Path) -> Result<Vec<(String, PathBuf)>> {
    let mut out: Vec<(String, PathBuf)> = std::fs::read_dir(dir)
        .with_context(|| dir.display().to_string())?
        .flatten()
        .map(|e| (e.file_name().to_string_lossy().into_owned(), e.path()))
        .filter(|(n, _)| !n.starts_with('.'))
        .collect();
    out.sort();
    Ok(out)
}

fn root(dir: &Path) -> Result<PathBuf> {
    let files = entries(dir)?;
    if let Some(name) = dir.file_name().and_then(|n| n.to_str()) {
        if name.len() == 16 && name.bytes().all(|b| b.is_ascii_hexdigit()) && !name.eq_ignore_ascii_case(TITLE_ID) {
            bail!("{}: this title is not Mario Kart 8 Deluxe ({TITLE_ID})", dir.display());
        }
    }
    let atmosphere = child(dir, "atmosphere");
    let mut candidates = vec![dir.to_path_buf()];
    if let Some(title) = atmosphere.as_ref().and_then(|p| child(p, "contents")).and_then(|p| child(&p, TITLE_ID)) {
        candidates.push(title);
    }
    if let Some(title) = child(dir, "contents").and_then(|p| child(&p, TITLE_ID)) {
        candidates.push(title);
    }
    if let Some(title) = child(dir, TITLE_ID) {
        candidates.push(title);
    }
    for candidate in candidates {
        if child(&candidate, "romfs").is_some() || child(&candidate, "exefs").is_some()
            || entries(&candidate)?.iter().any(|(n, _)| n.eq_ignore_ascii_case("exefs.nsp") || n.eq_ignore_ascii_case("romfs.bin") || n.to_ascii_lowercase().ends_with(".stub")) {
            return Ok(candidate);
        }
    }
    if files.iter().any(|(n, p)| n.eq_ignore_ascii_case("exefs_patches") && p.is_dir())
        || atmosphere.as_ref().and_then(|p| child(p, "exefs_patches")).is_some() {
        return Ok(dir.to_path_buf());
    }
    bail!("{}: no RomFS/ExeFS mod for Mario Kart 8 Deluxe ({TITLE_ID}), or exefs_patches directory", dir.display())
}

fn validate(mods: &[PathBuf]) -> Result<Vec<PathBuf>> {
    let roots = mods.iter().map(|p| root(p)).collect::<Result<Vec<_>>>()?;
    for dir in &roots {
        for (name, path) in entries(dir)? {
            if name.to_ascii_lowercase().ends_with(".stub") {
                bail!("{}: Atmosphere .stub file removal is not supported", path.display());
            }
            if name.eq_ignore_ascii_case("exefs.nsp") || name.eq_ignore_ascii_case("romfs.bin") {
                bail!("{}: packed mod files are not supported; extract this file to an exefs or romfs directory", path.display());
            }
        }
        if let Some(exefs) = child(dir, "exefs") {
            let files = entries(&exefs)?;
            if let Some((_, path)) = files.iter().find(|(n, _)| n.eq_ignore_ascii_case("main.npdm")) {
                let data = std::fs::read(path).with_context(|| path.display().to_string())?;
                anyhow::ensure!(data.len() >= 0x20 && &data[..4] == b"META" && data[0xC] & 1 != 0, "{}: invalid or ARM32 main.npdm; Mush8 requires AArch64", path.display());
            }
            for (name, path) in files {
                let lower = name.to_ascii_lowercase();
                if lower.ends_with(".stub") {
                    bail!("{}: Atmosphere .stub file removal is not supported", path.display());
                }
                if lower == "main" || lower == "sdk" || lower == "rtld" || (lower.starts_with("subsdk") && path.extension().is_none()) {
                    bail!("{}: native ExeFS modules are not supported; this mod needs code execution, not just a RomFS overlay", path.display());
                }
            }
        }
        if let Some(plugins) = child(dir, "romfs").and_then(|p| child(&p, "skyline")).and_then(|p| child(&p, "plugins")) {
            for (name, path) in entries(&plugins)? {
                if name.rsplit_once('.').is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("nro")) {
                    bail!("{}: Skyline NRO plugins are not supported; loading only this mod's RomFS would leave its code inactive", path.display());
                }
            }
        }
    }
    Ok(roots)
}

fn walk(dir: &Path, prefix: &str, out: &mut Vec<(String, Node)>) -> Result<()> {
    for (name, path) in entries(dir)? {
        if name.to_ascii_lowercase().ends_with(".stub") {
            bail!("{}: Atmosphere .stub file removal is not supported", path.display());
        }
        let key = if prefix.is_empty() { name.clone() } else { format!("{prefix}/{name}") };
        if path.is_dir() && is_archive(&name) {
            out.push((key, Node::Archive(archive(&path)?)));
        } else if path.is_dir() {
            walk(&path, &key, out)?;
        } else {
            out.push((key, Node::File(path)));
        }
    }
    Ok(())
}

fn archive(dir: &Path) -> Result<BTreeMap<String, Node>> {
    let mut out = Vec::new();
    walk(dir, "", &mut out)?;
    Ok(out.into_iter().collect())
}

fn png(path: &Path) -> Result<(Vec<u8>, u32, u32)> {
    let mut d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path)?));
    d.set_transformations(png::Transformations::normalize_to_color8());
    let mut r = d.read_info()?;
    let mut buf = vec![0; r.output_buffer_size().context("PNG too large")?];
    let info = r.next_frame(&mut buf)?;
    let px = &buf[..info.buffer_size()];
    let rgba = match info.color_type {
        png::ColorType::Rgba => px.to_vec(),
        png::ColorType::Rgb => px.chunks(3).flat_map(|p| [p[0], p[1], p[2], 255]).collect(),
        png::ColorType::GrayscaleAlpha => px.chunks(2).flat_map(|p| [p[0], p[0], p[0], p[1]]).collect(),
        png::ColorType::Grayscale => px.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        c => bail!("unsupported PNG color type {c:?}"),
    };
    Ok((rgba, info.width, info.height))
}

fn edit(data: &[u8], edits: &BTreeMap<String, Node>) -> Result<Vec<u8>> {
    if data.starts_with(b"Yaz0") {
        let inner = edit(&nxfs::sarc::yaz0(data)?, edits)?;
        return Ok(nxfs::sarc::yaz0_compress(&inner, nxfs::sarc::yaz0_tail(data)));
    }
    if data.starts_with(b"SARC") {
        let entries = nxfs::sarc::sarc(data)?;
        let mut contents: Vec<Vec<u8>> = entries.iter().map(|e| e.data.to_vec()).collect();
        for (key, node) in edits {
            let Some(i) = entries.iter().position(|e| e.is(key)) else {
                eprintln!("[mods] skipped {key}: the archive has no such file");
                continue;
            };
            contents[i] = match node {
                Node::File(p) => std::fs::read(p)?,
                Node::Archive(inner) => edit(&contents[i], inner).with_context(|| key.clone())?,
            };
        }
        return nxfs::sarc::rebuild(data, &contents.iter().map(Vec::as_slice).collect::<Vec<_>>());
    }
    if data.starts_with(b"BNTX") {
        let textures = nxfs::bntx::textures(data)?;
        let mut out = data.to_vec();
        for (key, node) in edits {
            let Node::File(p) = node else { bail!("{key}: a texture must be a .png file") };
            let stem = key.strip_suffix(".png").or_else(|| key.strip_suffix(".PNG")).with_context(|| format!("{key}: a texture must be a .png file"))?;
            let Some(t) = textures.iter().find(|t| t.name == stem || t.name.split('^').next() == Some(stem)) else {
                eprintln!("[mods] skipped {key}: the texture pack has no {stem}");
                continue;
            };
            let (rgba, w, h) = png(p).with_context(|| key.clone())?;
            t.write(&mut out, &rgba, w, h)?;
        }
        return Ok(out);
    }
    if data.starts_with(b"MsgStdBn") {
        let mut m = nxfs::msbt::Msbt::parse(data)?;
        for (key, node) in edits {
            let Node::File(p) = node else { bail!("{key}: a text must be a .txt file") };
            let label = key.strip_suffix(".txt").with_context(|| format!("{key}: a text must be a .txt file"))?;
            let text = std::fs::read_to_string(p)?;
            if let Err(e) = m.set_text(label, text.trim_end_matches(['\r', '\n'])) {
                eprintln!("[mods] skipped {key}: {e}");
            }
        }
        return Ok(m.build());
    }
    bail!("Mush8 cannot edit inside this kind of file")
}

pub fn romfs(romfs: DynStorage, mods: &[PathBuf]) -> Result<DynStorage> {
    let roots = validate(mods)?;
    let mut changes: HashMap<String, (String, Vec<Change>)> = HashMap::new();
    let names: Vec<String> = mods.iter().map(|m| m.file_name().map_or_else(|| m.display().to_string(), |n| n.to_string_lossy().into_owned())).collect();
    for (m, name) in roots.iter().zip(&names) {
        let Some(dir) = child(m, "romfs") else { continue };
        let mut found = Vec::new();
        walk(&dir, "", &mut found)?;
        eprintln!("[mods] {name}: {} RomFS changes", found.len());
        for (path, node) in found {
            let path = format!("/{path}");
            changes.entry(path.to_ascii_lowercase()).or_insert_with(|| (path, Vec::new())).1.push(Change { by: name, node });
        }
    }
    if changes.is_empty() {
        return Ok(romfs);
    }
    let fs = nxfs::romfs::RomFs::open(romfs)?;
    let originals: HashMap<String, nxfs::romfs::File> = fs.files().into_iter().map(|f| (f.path.to_ascii_lowercase(), f)).collect();
    let mut overlay: Vec<(String, DynStorage)> = Vec::new();
    for (lower, (path, list)) in changes {
        let path = originals.get(&lower).map_or(path, |f| f.path.clone());
        let last_file = list.iter().rposition(|c| matches!(c.node, Node::File(_)));
        if let Some(i) = last_file {
            for c in &list[..i] {
                eprintln!("[mods] {}: {path} replaces the copy from {}", list[i].by, c.by);
            }
        }
        let file = |i: usize| match &list[i].node {
            Node::File(p) => p.clone(),
            Node::Archive(_) => unreachable!(),
        };
        if let Some(i) = last_file
            && i + 1 == list.len()
        {
            overlay.push((path, Arc::new(HostFile::new(file(i))?)));
            continue;
        }
        let mut data = match (last_file, originals.get(&lower)) {
            (Some(i), _) => std::fs::read(file(i))?,
            (None, Some(f)) => fs.storage().read_vec(f.offset, f.size as usize)?,
            (None, None) => {
                eprintln!("[mods] {path} is not in the game files, so {} cannot edit it", list[0].by);
                continue;
            }
        };
        let started = std::time::Instant::now();
        for c in &list[last_file.map_or(0, |i| i + 1)..] {
            let Node::Archive(edits) = &c.node else { continue };
            match edit(&data, edits) {
                Ok(new) => data = new,
                Err(e) => eprintln!("[mods] {}: cannot edit {path}: {e:#}", c.by),
            }
        }
        eprintln!("[mods] {path} edited in {:.2} s", started.elapsed().as_secs_f32());
        overlay.push((path, Arc::new(Mem(data))));
    }
    fs.with_overlay(overlay)
}
