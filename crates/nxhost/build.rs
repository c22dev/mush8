// Mush8 - build.rs
// Constantin Clerc (c22dev) - 0.1
use std::fmt::Write;
use std::path::{Path, PathBuf};
#[path="src/display_profile.rs"]
mod display_profile;

const PAGE: u64 = 0x4000;
const MODULES: [&str; 3] = ["main", "subsdk0", "sdk"];
const FIRST_SEGMENT: u64 = 0x1_0400_0000;

fn main() {
    println!("cargo:rerun-if-changed=src/display_profile.rs");
    println!("cargo::rustc-check-cfg=cfg(embedded_modules)");
    println!("cargo::rustc-check-cfg=cfg(missing_mii_profile)");
    println!("cargo:rerun-if-env-changed=NX_MODULES_DIR");
    println!("cargo:rerun-if-env-changed=NX_MISSING_MII");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() == Ok("macos") { build_mac_launcher(); }
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
        let mut image = std::fs::read(&bin).unwrap_or_else(|e| panic!("{}: {e}", bin.display()));
        let (text, tramp) = read_layout(&layout);
        let missing_mii=*name=="main" && std::env::var_os("NX_MISSING_MII").is_some();
        let bin=if missing_mii {
            let metadata=std::fs::read_to_string(&layout).unwrap();
            assert!(metadata.lines().any(|l|l=="module_id 158c0c7399aabd7bf83795c2ef3484c700000000000000000000000000000000"));
            let words:Vec<_>=metadata.lines().find(|l|l.starts_with("tramp ")).unwrap().split_whitespace().collect();
            let guard=tramp+u64::from_str_radix(words[2].trim_start_matches("0x"),16).unwrap();
            assert_eq!(u32::from_le_bytes(image[0x810470..0x810474].try_into().unwrap()),0xf9400008);
            assert_eq!(u32::from_le_bytes(image[0x813dd4..0x813dd8].try_into().unwrap()),0xa9bd57f6);
            let branch=|from:u64,to:u64|0x14000000u32|((((to as i64-from as i64)/4) as u32)&0x3ffffff);
            image.resize((guard+140) as usize,0);
            for (i,w) in [0xb4000080,0xf9400008,0xf9402d08,branch(guard+12,0x810478),branch(guard+16,0x81047c)].iter().enumerate(){image[guard as usize+i*4..guard as usize+i*4+4].copy_from_slice(&w.to_le_bytes());}
            image[0x810470..0x810474].copy_from_slice(&branch(0x810470,guard).to_le_bytes());
            assert_eq!(u32::from_le_bytes(image[0x80f61c..0x80f620].try_into().unwrap()),0xf9400308);
            let guard2=guard+20;
            for (i,w) in [0xb40000b8,0xf9400308,0xf9402d08,0xaa1803e0,branch(guard2+16,0x80f628),branch(guard2+20,0x80f62c)].iter().enumerate(){image[guard2 as usize+i*4..guard2 as usize+i*4+4].copy_from_slice(&w.to_le_bytes());}
            image[0x80f61c..0x80f620].copy_from_slice(&branch(0x80f61c,guard2).to_le_bytes());
            assert_eq!(u32::from_le_bytes(image[0x810d4c..0x810d50].try_into().unwrap()),0xf9401913);
            let guard3=guard+44;
            for (i,w) in [0xb5000048,branch(guard3+4,0x810edc),0xf9401913,branch(guard3+12,0x810d50)].iter().enumerate(){image[guard3 as usize+i*4..guard3 as usize+i*4+4].copy_from_slice(&w.to_le_bytes());}
            image[0x810d4c..0x810d50].copy_from_slice(&branch(0x810d4c,guard3).to_le_bytes());
            assert_eq!(u32::from_le_bytes(image[0x3e1d5c..0x3e1d60].try_into().unwrap()),0x1e2703e8);
            let guard4=guard+60;
            for (i,w) in [0xb5000040,branch(guard4+4,0x3e1da0),0x1e2703e8,branch(guard4+12,0x3e1d60)].iter().enumerate(){image[guard4 as usize+i*4..guard4 as usize+i*4+4].copy_from_slice(&w.to_le_bytes());}
            image[0x3e1d5c..0x3e1d60].copy_from_slice(&branch(0x3e1d5c,guard4).to_le_bytes());
            assert_eq!(u32::from_le_bytes(image[0x818d00..0x818d04].try_into().unwrap()),0xa9bd57f6);
            let guard5=guard+76;
            for (i,w) in [0xb5000040,0xd65f03c0,0xa9bd57f6,branch(guard5+12,0x818d04)].iter().enumerate(){image[guard5 as usize+i*4..guard5 as usize+i*4+4].copy_from_slice(&w.to_le_bytes());}
            image[0x818d00..0x818d04].copy_from_slice(&branch(0x818d00,guard5).to_le_bytes());
            for (i,off) in [0x3eeb5cusize,0x3eeb88,0x3f35c0].into_iter().enumerate(){
                assert_eq!(u32::from_le_bytes(image[off..off+4].try_into().unwrap()),0xb942f808);
                let thunk=guard+92+i as u64*16;
                for (j,w) in [0x2a1f03e8,0xb4000040,0xb942f808,branch(thunk+12,off as u64+4)].iter().enumerate(){image[thunk as usize+j*4..thunk as usize+j*4+4].copy_from_slice(&w.to_le_bytes());}
                image[off..off+4].copy_from_slice(&branch(off as u64,thunk).to_le_bytes());
            }
            let baked=PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("main-missing-mii.bin");
            std::fs::write(&baked,&image).unwrap();
            println!("cargo:rustc-cfg=missing_mii_profile");
            baked
        } else {bin};
        let bin=if *name=="main" {
            let metadata=std::fs::read_to_string(&layout).unwrap();
            assert!(metadata.lines().any(|l|l==format!("module_id {}",display_profile::MODULE_ID)));
            let words:Vec<_>=metadata.lines().find(|l|l.starts_with("tramp ")).unwrap().split_whitespace().collect();
            let guard=tramp+u64::from_str_radix(words[2].trim_start_matches("0x"),16).unwrap();
            for (i,(site,original,_)) in display_profile::GETTERS.iter().enumerate() {
                for (j,w) in original.iter().enumerate(){assert_eq!(u32::from_le_bytes(image[*site as usize+j*4..*site as usize+j*4+4].try_into().unwrap()),*w);}
                let stub=guard+display_profile::STUB_OFFSET+i as u64*display_profile::STUB_SIZE;
                image.resize(image.len().max((stub+20) as usize),0);
                for (j,w) in original.iter().enumerate(){image[stub as usize+j*4..stub as usize+j*4+4].copy_from_slice(&w.to_le_bytes());}
                let branch=0x14000000u32|((((site+16) as i64-(stub+16) as i64)/4) as u32&0x3ffffff);
                image[stub as usize+16..stub as usize+20].copy_from_slice(&branch.to_le_bytes());
            }
            let baked=PathBuf::from(std::env::var("OUT_DIR").unwrap()).join("main-display.bin");
            std::fs::write(&baked,&image).unwrap();baked
        } else {bin};
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
                if *name=="main" && kind=="X" {
                    let mut hooks:Vec<(u64,&str)>=display_profile::GETTERS.iter().map(|(site,_,symbol)|(*site,*symbol)).collect();
                    if missing_mii {hooks.push((0x813dd4,"_mk8_empty_avatar"));}
                    hooks.sort_by_key(|h|h.0);
                    let mut cursor=0;
                    for (site,symbol) in hooks {
                        writeln!(asm,".incbin \"{}\", {cursor:#x}, {:#x}\nadrp x16, {symbol}@PAGE\nadd x16, x16, {symbol}@PAGEOFF\nbr x16\nnop",bin.display(),site-cursor).unwrap();
                        cursor=site+16;
                    }
                    writeln!(asm,".incbin \"{}\", {cursor:#x}, {:#x}",bin.display(),len-cursor).unwrap();
                } else {writeln!(asm, ".incbin \"{}\", {lo:#x}, {len:#x}", bin.display()).unwrap();}
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
    println!("cargo:rerun-if-changed=../../ios/Launcher.swift");
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
        .arg("../../ios/Launcher.swift")
        .arg("-o")
        .arg(format!("{out}/libmush8launcher.a"))
        .status()
        .expect("swiftc");
    assert!(status.success(), "swiftc failed");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=mush8launcher");
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

fn build_mac_launcher() {
    println!("cargo:rerun-if-changed=../../macos/WolverineUSB.swift");
    println!("cargo:rerun-if-changed=../../macos/Launcher.swift");
    let out = std::env::var("OUT_DIR").unwrap();
    let status = std::process::Command::new("xcrun")
        .args(["swiftc", "-parse-as-library", "-emit-library", "-static", "-O", "-module-name", "MK8MacSettings"])
        .args(["../../macos/Launcher.swift", "../../macos/WolverineUSB.swift"]).arg("-o").arg(format!("{out}/libmk8macsettings.a"))
        .status().expect("swiftc");
    assert!(status.success(), "Mac settings swiftc failed");
    println!("cargo:rustc-link-search=native={out}");
    println!("cargo:rustc-link-lib=static=mk8macsettings");
    // Swift concurrency uses an @rpath install name, unlike the absolute Swift core libraries.
    println!("cargo:rustc-link-arg=-Wl,-rpath,/usr/lib/swift");
    println!("cargo:rustc-link-lib=framework=SwiftUI");
    println!("cargo:rustc-link-lib=framework=IOUSBHost");
    println!("cargo:rustc-link-lib=framework=IOKit");
    let result = std::process::Command::new("xcrun").args(["--sdk", "macosx", "--show-sdk-path"]).output().unwrap();
    let sdk = String::from_utf8(result.stdout).unwrap();
    println!("cargo:rustc-link-search=native={}/usr/lib/swift", sdk.trim());
}
