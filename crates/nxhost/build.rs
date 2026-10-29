// Mush8 - build.rs
// Constantin Clerc (c22dev) - 0.1
use std::fmt::Write;
use std::path::{Path, PathBuf};

const PAGE: u64 = 0x4000;
const MODULES: [&str; 3] = ["main", "subsdk0", "sdk"];
const FIRST_SEGMENT: u64 = 0x1_0400_0000;

fn main() {
    println!("cargo::rustc-check-cfg=cfg(embedded_modules)");
    println!("cargo:rerun-if-env-changed=NX_MODULES_DIR");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("ios") {
        return;
    }
    build_launcher();
    let Ok(dir) = std::env::var("NX_MODULES_DIR") else {
        println!("cargo:warning=NX_MODULES_DIR is not set: the iOS build has no game code");
        return;
    };
    let dir = PathBuf::from(dir);
    let mut asm = String::new();
    let mut addr = FIRST_SEGMENT;
    for (i, name) in MODULES.iter().enumerate() {
        let (bin, layout) = (dir.join(format!("{name}.bin")), dir.join(format!("{name}.layout")));
        println!("cargo:rerun-if-changed={}", bin.display());
        println!("cargo:rerun-if-changed={}", layout.display());
        let image = std::fs::read(&bin).unwrap_or_else(|e| panic!("{}: {e}", bin.display()));
        let (text, tramp) = read_layout(&layout);
        let x_end = text.next_multiple_of(PAGE);
        assert!(tramp % PAGE == 0 && tramp > x_end, "{name}: trampolines at {tramp:#x} are not on their own 16 KiB pages");
        let dynamic = nxfs::module::parse(&image).unwrap_or_else(|e| panic!("{name}: {e:#}"));
        if let Some(r) = dynamic.relocs.iter().find(|r| r.offset < x_end || r.offset >= tramp) {
            panic!("{name}: relocation at {:#x} falls in code, which is read-only on iOS", r.offset);
        }
        let end = (image.len() as u64).next_multiple_of(PAGE);
        let bin = bin.canonicalize().unwrap();
        for (kind, lo, hi, prot) in [("X", 0, x_end, "rx"), ("D", x_end, tramp, "rw"), ("T", tramp, end, "rx")] {
            let seg = format!("__NX{i}_{kind}");
            let sym = format!("_nx_mod_{name}_{}", kind.to_lowercase());
            let len = hi.min(image.len() as u64).saturating_sub(lo);
            let flags = if prot == "rx" { "regular,pure_instructions" } else { "regular" };
            writeln!(asm, ".section {seg},__{},{flags}", if prot == "rx" { "text" } else { "data" }).unwrap();
            writeln!(asm, ".p2align 14\n.globl {sym}\n.no_dead_strip {sym}\n{sym}:").unwrap();
            if len > 0 {
                writeln!(asm, ".incbin \"{}\", {lo:#x}, {len:#x}", bin.display()).unwrap();
            }
            if hi - lo > len {
                writeln!(asm, ".space {:#x}", hi - lo - len).unwrap();
            }
            println!("cargo:rustc-link-arg-bins=-Wl,-segprot,{seg},{prot},{prot}");
            println!("cargo:rustc-link-arg-bins=-Wl,-segaddr,{seg},{addr:#x}");
            addr += hi - lo;
        }
        addr += PAGE;
        writeln!(asm, ".section __DATA,__const\n.p2align 3\n.globl _nx_mod_{name}_len\n_nx_mod_{name}_len:\n.quad {}", image.len()).unwrap();
    }
    asm.push_str(".text\n");
    let out = Path::new(&std::env::var("OUT_DIR").unwrap()).join("modules.s");
    std::fs::write(&out, asm).unwrap();
    println!("cargo:rustc-cfg=embedded_modules");
}

fn build_launcher() {
    let sources = ["../../ios/Launcher.swift", "../../launcher/ModDiscovery.swift", "../../launcher/ModContents.swift", "../../launcher/GameBanana.swift", "../../launcher/ModInstaller.swift", "../../launcher/ModBrowser.swift"];
    for source in sources {
        println!("cargo:rerun-if-changed={source}");
    }
    println!("cargo:rerun-if-changed=../../launcher/Archive.h");
    let out = std::env::var("OUT_DIR").unwrap();
    let xcrun = |args: &[&str]| {
        let o = std::process::Command::new("xcrun").args(args).output().expect("xcrun");
        String::from_utf8(o.stdout).unwrap().trim().to_string()
    };
    let sdk = xcrun(&["--sdk", "iphoneos", "--show-sdk-path"]);
    let swiftc = xcrun(&["--sdk", "iphoneos", "-f", "swiftc"]);
    let status = std::process::Command::new(&swiftc)
        .args(["-parse-as-library", "-emit-library", "-static", "-O", "-module-name", "Mush8Launcher"])
        .args(["-target", "arm64-apple-ios26.0", "-sdk", &sdk])
        .args(sources)
        .args(["-import-objc-header", "../../launcher/Archive.h"])
        .arg("-o")
        .arg(format!("{out}/libmush8launcher.a"))
        .status()
        .expect("swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=mush8launcher");
    println!("cargo:rustc-link-lib=archive");
    println!("cargo:rustc-link-search=native={sdk}/usr/lib/swift");
    let toolchain = Path::new(&swiftc).parent().unwrap().parent().unwrap().join("lib/swift/iphoneos");
    println!("cargo:rustc-link-search=native={}", toolchain.display());
}

fn read_layout(path: &Path) -> (u64, u64) {
    let text = std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    let get = |key: &str| {
        let line = text.lines().find(|l| l.split_whitespace().next() == Some(key)).unwrap_or_else(|| panic!("{}: no {key}", path.display()));
        let f: Vec<&str> = line.split_whitespace().collect();
        let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16).unwrap();
        (hex(f[1]), hex(f[2]))
    };
    (get("text").1, get("tramp").0)
}
