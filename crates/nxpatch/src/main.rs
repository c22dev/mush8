// Mush8 - main.rs
// Constantin Clerc (c22dev) - 0.1
mod asm;
mod classify;
mod equiv;
mod ips;
mod rewrite;

use anyhow::{Context, Result};
use std::collections::BTreeMap;
use std::fmt::Write as _;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(about = "Offline rewriter for Switch NSO modules (x18, svc, system registers)")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    Classify {
        nso: Vec<PathBuf>,
        #[arg(long, default_value_t = 3)]
        examples: usize,
    },
    Rewrite {
        nso: Vec<PathBuf>,
        #[arg(long)]
        out: PathBuf,
        #[arg(long)]
        patch: Vec<PathBuf>,
        #[arg(long)]
        module: Vec<PathBuf>,
    },
    Verify {
        nso: Vec<PathBuf>,
        #[arg(long, value_parser = parse_hex)]
        from: Option<u64>,
        #[arg(long)]
        limit: Option<usize>,
    },
    Xref {
        nso: PathBuf,
        #[arg(value_parser = parse_hex)]
        target: u64,
    },
    NvnUsage { nso: PathBuf },
    Stamp {
        #[arg(long)]
        patch: Vec<PathBuf>,
        #[arg(long)]
        module: Vec<PathBuf>,
    },
    Dis {
        nso: PathBuf,
        #[arg(value_parser = parse_hex)]
        offset: u64,
        #[arg(long, default_value_t = 8)]
        before: u64,
        #[arg(long, default_value_t = 8)]
        after: u64,
    },
}

fn fnv(h: u64, bytes: impl IntoIterator<Item = u8>) -> u64 {
    bytes.into_iter().fold(h, |h, b| (h ^ b as u64).wrapping_mul(0x100_0000_01b3))
}

fn stamp(patches: &[PathBuf]) -> Result<String> {
    let src = [include_str!("main.rs"), include_str!("rewrite.rs"), include_str!("asm.rs"), include_str!("classify.rs"), include_str!("equiv.rs"), include_str!("ips.rs")];
    let h = fnv(0xcbf2_9ce4_8422_2325, src.iter().flat_map(|s| s.bytes()));
    if patches.is_empty() {
        return Ok(format!("{h:016x}"));
    }
    let mut p = 0xcbf2_9ce4_8422_2325u64;
    for path in patches {
        p = fnv(p, std::fs::read(path).with_context(|| path.display().to_string())?);
    }
    Ok(format!("{h:016x}-{p:016x}"))
}

// subsdk1 to subsdk9 are free in Mario Kart 8 Deluxe, whose own modules are main, subsdk0 and sdk
fn is_extra_module(name: &str) -> bool {
    name.strip_prefix("subsdk").is_some_and(|n| n.len() == 1 && ('1'..='9').contains(&n.chars().next().unwrap()))
}

fn remove_extra_modules(out: &std::path::Path) -> Result<()> {
    for e in std::fs::read_dir(out)? {
        let path = e?.path();
        let name = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if is_extra_module(name.split('.').next().unwrap_or_default()) {
            std::fs::remove_file(&path)?;
        }
    }
    Ok(())
}

fn parse_hex(s: &str) -> Result<u64, std::num::ParseIntError> {
    u64::from_str_radix(s.trim_start_matches("0x"), 16)
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::Classify { nso, examples } => {
            for p in nso {
                let m = nxfs::nso::Nso::parse(&std::fs::read(&p)?)?;
                println!("== {}", p.display());
                classify::report(m.text_bytes(), examples)?;
            }
        }
        Cmd::Rewrite { mut nso, out, patch, module } => {
            std::fs::create_dir_all(&out)?;
            let _ = std::fs::remove_file(out.join("nxpatch.stamp"));
            remove_extra_modules(&out)?;
            for m in &module {
                let name = m.file_name().unwrap().to_string_lossy();
                anyhow::ensure!(is_extra_module(&name), "{}: a mod can add only the modules subsdk1 to subsdk9", m.display());
                anyhow::ensure!(!nso.iter().any(|n| n.file_name() == m.file_name()), "two mods add {name}: turn one of them off");
                nso.push(m.clone());
            }
            let _ = std::fs::remove_file(out.join("nxpatch.skipped"));
            let mut skipped = String::new();
            let mut patches = Vec::new();
            for p in &patch {
                match ips::load(p) {
                    Ok(c) => patches.push(c),
                    Err(e) => {
                        println!("skipped {}: {e:#}", p.display());
                        writeln!(skipped, "{}\t{}", p.display(), format!("{e:#}").replace(['\t', '\n'], " "))?;
                    }
                }
            }
            let mut used = vec![false; patches.len()];
            for p in nso {
                let name = p.file_name().unwrap().to_string_lossy().into_owned();
                let mut m = nxfs::nso::Nso::parse(&std::fs::read(&p)?)?;
                let id = hex::encode(&m.module_id[..20]);
                let mut skip: Vec<u64> = Vec::new();
                for (i, c) in patches.iter().enumerate().filter(|(_, c)| c.matches(&m.module_id)) {
                    for g in rewrite::GAME_PATCHES.iter().filter(|g| g.module_id == id && c.overlaps(g.site, 4 * g.from.len() as u64)) {
                        match rewrite::group_of(&id, g.site) {
                            Some(group) if group.optional => {
                                if !skip.contains(&g.site) {
                                    println!("{name}: {} replaces Mush8's {} patch", c.source.display(), group.name);
                                    skip.extend(group.sites);
                                }
                            }
                            group => anyhow::bail!(
                                "{} changes {name} at {:#x}, where Mush8 needs its own {} patch: turn that mod off",
                                c.source.display(),
                                g.site,
                                group.map_or("", |g| g.name)
                            ),
                        }
                    }
                    if let Some(&s) = rewrite::aspect_stores(&id).iter().find(|&&s| c.overlaps(s, 4)) {
                        anyhow::bail!("{} changes {name} at {s:#x}, where Mush8 needs its own Fill screen patch: turn that mod off", c.source.display());
                    }
                    let bytes = c.apply(&mut m.image).with_context(|| name.clone())?;
                    println!("{name}: applied {} ({bytes} bytes)", c.source.display());
                    used[i] = true;
                }
                let o = rewrite::rewrite_skipping(&m, &skip).with_context(|| name.clone())?;
                write_output(&out, &name, &m, &o)?;
            }
            for (c, _) in patches.iter().zip(&used).filter(|(_, u)| !**u) {
                println!("skipped {}: it is for build {}, not this game update", c.source.display(), c.build_id);
                writeln!(skipped, "{}\tit is for build {}, not this game update", c.source.display(), c.build_id)?;
            }
            std::fs::write(out.join("nxpatch.skipped"), skipped)?;
            std::fs::write(out.join("nxpatch.stamp"), stamp(&[patch, module].concat())?)?;
        }
        Cmd::Stamp { patch, module } => println!("{}", stamp(&[patch, module].concat())?),
        Cmd::Verify { nso, from, limit } => {
            let mut failed = false;
            for p in nso {
                let m = nxfs::nso::Nso::parse(&std::fs::read(&p)?)?;
                let mut o = rewrite::rewrite(&m)?;
                o.patches.retain(|x| x.start >= from.unwrap_or(0));
                o.patches.truncate(limit.unwrap_or(usize::MAX));
                let mut original = m.image.clone();
                original.resize((o.bss.0 + o.bss.1) as usize, 0);
                let t = std::time::Instant::now();
                let s = equiv::verify(&original, &o)?;
                println!(
                    "{}: {} patches equivalent, {} untested (original faulted in every trial), {} FAILED  [{:.1}s]",
                    p.display(), s.tested, s.untested.len(), s.failures.len(), t.elapsed().as_secs_f64()
                );
                for f in s.failures.iter().take(30) {
                    println!("  FAIL {f}");
                }
                if !s.untested.is_empty() {
                    println!("  untested: {}", s.untested.iter().take(20).map(|a| format!("{a:#x}")).collect::<Vec<_>>().join(" "));
                }
                failed |= !s.failures.is_empty();
            }
            if failed {
                std::process::exit(1);
            }
        }
        Cmd::Xref { nso, target } => {
            let m = nxfs::nso::Nso::parse(&std::fs::read(&nso)?)?;
            for (at, kind) in xrefs(m.text_bytes(), target) {
                println!("{at:#010x}  {kind}");
            }
        }
        Cmd::NvnUsage { nso } => nvn_usage(&nxfs::nso::Nso::parse(&std::fs::read(&nso)?)?)?,
        Cmd::Dis { nso, offset, before, after } => {
            let m = nxfs::nso::Nso::parse(&std::fs::read(&nso)?)?;
            let names = symbolize(&m)?;
            let start = offset.saturating_sub(before * 4) as usize;
            let end = ((offset + after * 4 + 4) as usize).min(m.image.len());
            let cs = classify::capstone()?;
            for a in (start..end).step_by(4) {
                let mark = if a as u64 == offset { ">" } else { " " };
                if let Some(n) = names.get(&(a as u64)) {
                    println!("              <{n}>:");
                }
                let w = u32::from_le_bytes(m.image[a..a + 4].try_into()?);
                let insns = cs.disasm_count(&m.image[a..a + 4], a as u64, 1)?;
                let Some(i) = insns.iter().next() else {
                    println!("{mark} {a:#010x}  .word {w:#010x}");
                    continue;
                };
                let note = if w & 0x7C00_0000 == 0x1400_0000 {
                    let t = (a as i64 + asm::sext(w & 0x03FF_FFFF, 26) * 4) as u64;
                    names.get(&t).map(|n| format!("  ; {n}")).unwrap_or_default()
                } else {
                    String::new()
                };
                println!("{mark} {a:#010x}  {} {}{note}", i.mnemonic().unwrap_or(""), i.op_str().unwrap_or(""));
            }
        }
    }
    Ok(())
}

fn write_output(out: &std::path::Path, name: &str, m: &nxfs::nso::Nso, o: &rewrite::Output) -> Result<()> {
    std::fs::write(out.join(format!("{name}.bin")), &o.image)?;
    let mut l = String::new();
    writeln!(l, "module_id {}", hex::encode(m.module_id))?;
    writeln!(l, "text 0x0 {:#x}", o.text_size)?;
    writeln!(l, "ro {:#x} {:#x}", o.ro.0, o.ro.1)?;
    writeln!(l, "data {:#x} {:#x}", o.data.0, o.data.1)?;
    writeln!(l, "bss {:#x} {:#x}", o.bss.0, o.bss.1)?;
    writeln!(l, "keypage {:#x} {:#x}", o.keypage, rewrite::keypage::SIZE)?;
    writeln!(l, "tramp {:#x} {:#x}", o.tramp.0, o.tramp.1 - o.tramp.0)?;
    std::fs::write(out.join(format!("{name}.layout")), l)?;
    let mut t = String::from("kind\tstart\tend\ttramp\ttramp_len\n");
    let mut counts: BTreeMap<String, (usize, u64)> = BTreeMap::new();
    for p in &o.patches {
        writeln!(t, "{:?}\t{:#x}\t{:#x}\t{:#x}\t{}", p.kind, p.start, p.end, p.tramp, p.tramp_len)?;
        let c = counts.entry(format!("{:?}", p.kind)).or_default();
        c.0 += 1;
        c.1 += p.tramp_len;
    }
    std::fs::write(out.join(format!("{name}.patches.tsv")), t)?;
    println!("{name}: {} patches, trampolines {} KiB", o.patches.len(), (o.tramp.1 - o.tramp.0) / 1024);
    for (k, (n, bytes)) in counts {
        println!("  {k:10} {n:6}  avg {:.1} insns", bytes as f64 / 4.0 / n as f64);
    }
    Ok(())
}

fn symbolize(m: &nxfs::nso::Nso) -> Result<BTreeMap<u64, String>> {
    let mut names = BTreeMap::new();
    let Ok(d) = nxfs::module::parse(&m.image) else { return Ok(names) };
    for s in d.symbols.iter().filter(|s| s.defined() && !s.name.is_empty()) {
        names.entry(s.value).or_insert_with(|| nxfs::nso::demangle(&s.name));
    }
    let slots: BTreeMap<u64, String> = d
        .relocs
        .iter()
        .filter(|r| r.kind == nxfs::module::R_AARCH64_JUMP_SLOT)
        .filter_map(|r| d.symbols.get(r.sym as usize).map(|s| (r.offset, nxfs::nso::demangle(&s.name))))
        .collect();
    let text = m.text_bytes();
    for a in (0..text.len().saturating_sub(16)).step_by(4) {
        let w = |k: usize| u32::from_le_bytes(text[a + 4 * k..a + 4 * k + 4].try_into().unwrap());
        if asm::is_adrp(w(0)) && w(0) & 31 == 16 && w(1) & 0xFFC0_03FF == 0xF940_0211 && w(3) == 0xD61F_0220 {
            let got = asm::adr_target(w(0), a as u64) + ((w(1) >> 10) & 0xFFF) as u64 * 8;
            if let Some(n) = slots.get(&got) {
                names.entry(a as u64).or_insert_with(|| format!("{n}@plt"));
            }
        }
    }
    Ok(names)
}

pub fn xrefs(text: &[u8], target: u64) -> Vec<(u64, &'static str)> {
    let words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let mut out = Vec::new();
    for (i, &w) in words.iter().enumerate() {
        if !asm::is_adrp(w) || asm::adr_target(w, i as u64 * 4) != target & !0xFFF {
            continue;
        }
        let rd = w & 31;
        for (j, &u) in words.iter().enumerate().skip(i + 1).take(16) {
            let rn = (u >> 5) & 31;
            if rn != rd {
                if u & 31 == rd {
                    break;
                }
                continue;
            }
            let lo = target & 0xFFF;
            let is_load = u & 0x3B00_0000 == 0x3900_0000 && (u >> 22) & 1 == 1;
            if u & 0xFF80_0000 == 0x9100_0000 && ((u >> 10) & 0xFFF) as u64 == lo {
                out.push((j as u64 * 4, "add"));
            } else if u & 0x3B00_0000 == 0x3900_0000 {
                let size = u >> 30;
                let scale = if (u >> 26) & 1 == 1 && (u >> 23) & 1 == 1 { 4 } else { size };
                if ((u >> 10) & 0xFFF) as u64 * (1 << scale) == lo {
                    out.push((j as u64 * 4, if is_load { "load" } else { "store" }));
                }
            }
            let writes_rd = u & 31 == rd && (u & 0xFF80_0000 == 0x9100_0000 || (is_load && (u >> 26) & 1 == 0));
            if writes_rd {
                break;
            }
        }
    }
    out
}

fn nvn_usage(m: &nxfs::nso::Nso) -> Result<()> {
    let text = m.text_bytes();
    let words: Vec<u32> = text.chunks_exact(4).map(|c| u32::from_le_bytes(c.try_into().unwrap())).collect();
    let cstr = |a: u64| -> Option<String> {
        let b = m.image.get(a as usize..)?;
        let end = b.iter().position(|&c| c == 0)?;
        String::from_utf8(b[..end].to_vec()).ok()
    };
    let mut regs: [Option<u64>; 32] = [None; 32];
    let mut name_at_call: Option<String> = None;
    let mut slots: BTreeMap<u64, String> = BTreeMap::new();
    for (i, &w) in words.iter().enumerate() {
        let pc = i as u64 * 4;
        let rd = (w & 31) as usize;
        let rn = ((w >> 5) & 31) as usize;
        if asm::is_adrp(w) {
            regs[rd] = Some(asm::adr_target(w, pc));
        } else if w & 0xFF80_0000 == 0x9100_0000 {
            regs[rd] = regs[rn].map(|v| v + ((w >> 10) & 0xFFF) as u64);
        } else if w & 0xFFC0_0000 == 0xF940_0000 {
            regs[rd] = regs[rn].map(|v| (v + ((w >> 10) & 0xFFF) as u64 * 8) | 1 << 63);
        } else if w & 0xFFFF_FC1F == 0xD63F_0000 {
            name_at_call = regs[1].and_then(|a| cstr(a & !(1 << 63))).filter(|n| n.starts_with("nvn"));
        } else if w & 0xFFFF_FC00 == 0xF900_0000 && rd == 0 {
            if let (Some(got), Some(name)) = (regs[rn], name_at_call.take()) {
                if got >> 63 == 1 {
                    slots.insert(got & !(1 << 63), name);
                }
            }
        } else if asm::is_uncond_branch(w) {
            regs = [None; 32];
        }
    }
    let mut uses: Vec<(usize, &String)> = slots
        .iter()
        .map(|(&got, name)| (xrefs(text, got).iter().filter(|(_, k)| *k == "load").count().saturating_sub(1), name))
        .collect();
    uses.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(b.1)));
    let used = uses.iter().filter(|u| u.0 > 0).count();
    println!("{} procs loaded, {} referenced by game code", slots.len(), used);
    for (n, name) in uses {
        let got = slots.iter().find(|(_, v)| *v == name).map(|(k, _)| *k).unwrap_or(0);
        println!("{n:5}  {name}  (got {got:#x})");
    }
    Ok(())
}
