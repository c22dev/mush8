// Mush8 - loader.rs
// Constantin Clerc (c22dev) - 0.1
use crate::kernel;
use crate::mem::{Block, PAGE, perm, state};
use anyhow::{Context, Result, bail, ensure};
use nxabi::keypage;
use nxfs::module::{self, Dynamic};
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

pub struct Module {
    pub name: String,
    pub module_id: Vec<u8>,
    pub base: u64,
    pub size: u64,
    pub text: u64,
    pub data: (u64, u64),
    pub keypage: u64,
    pub tramp: (u64, u64),
    pub dynamic: Dynamic,
}

fn align_up(v: u64, a: u64) -> u64 {
    (v + a - 1) & !(a - 1)
}
fn align_down(v: u64, a: u64) -> u64 {
    v & !(a - 1)
}

fn read_layout(path: &Path) -> Result<HashMap<String, (u64, u64)>> {
    let text = std::fs::read_to_string(path).with_context(|| path.display().to_string())?;
    let mut m = HashMap::new();
    for line in text.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        let hex = |s: &str| u64::from_str_radix(s.trim_start_matches("0x"), 16);
        if f.len() == 3 {
            m.insert(f[0].to_string(), (hex(f[1])?, hex(f[2])?));
        }
    }
    Ok(m)
}

fn read_module_id(path: &Path) -> Result<Vec<u8>> {
    let text = std::fs::read_to_string(path)?;
    let line = text.lines().find_map(|l| l.strip_prefix("module_id ")).context("layout has no module_id")?;
    (0..line.len() / 2).map(|i| u8::from_str_radix(&line[2 * i..2 * i + 2], 16).map_err(Into::into)).collect()
}

static TRACED: Mutex<Vec<(String, u64)>> = Mutex::new(Vec::new());

fn traced(idx: u64, a: &crate::stubs::Args) -> u64 {
    let (name, real) = TRACED.lock().unwrap()[idx as usize].clone();
    let f: extern "C" fn(u64, u64, u64, u64, u64, u64, u64, u64) -> u64 = unsafe { std::mem::transmute(real as usize) };
    let r = f(a.0[0], a.0[1], a.0[2], a.0[3], a.0[4], a.0[5], a.0[6], a.0[7]);
    eprintln!("[trace] {name}({:#x}, {:#x}, {:#x}, {:#x}) -> {r:#x}", a.0[0], a.0[1], a.0[2], a.0[3]);
    if std::env::var_os("NX_TRACE_SYMS_BT").is_some() {
        let (fp, lr) = unsafe { (*(a as *const _ as *const u64).sub(2), *(a as *const _ as *const u64).sub(1)) };
        eprintln!("        at {}", crate::diag::symbolize(lr));
        crate::diag::backtrace(fp);
    }
    r
}

static UNIMPL: Mutex<Vec<(String, bool)>> = Mutex::new(Vec::new());

fn unimpl(idx: u64, _args: &crate::stubs::Args) -> u64 {
    let mut u = UNIMPL.lock().unwrap();
    if let Some((name, seen)) = u.get_mut(idx as usize) {
        if !*seen {
            *seen = true;
            eprintln!("[hle] UNIMPL {}", nxfs::nso::demangle(name));
        }
    }
    0
}

pub fn load(dir: &Path, names: &[&str], hle: &HashMap<&str, u64>) -> Result<Vec<Module>> {
    let a = kernel::aspace();
    let mut next = a.code() + PAGE;
    let mut mods = Vec::new();
    for &name in names {
        let l = read_layout(&dir.join(format!("{name}.layout")))?;
        let get = |k: &str| l.get(k).copied().with_context(|| format!("{name}.layout: missing {k}"));
        let (base, image) = match embedded::image(name, get("text")?.1, get("tramp")?.0)? {
            Some(e) => e,
            None => {
                let image = std::fs::read(dir.join(format!("{name}.bin"))).with_context(|| format!("{name}.bin"))?;
                let base = next;
                a.set(base, align_up(image.len() as u64, PAGE), Block { state: state::CODE_DATA, perm: perm::RW, attr: 0 })?;
                unsafe { std::ptr::copy_nonoverlapping(image.as_ptr(), base as *mut u8, image.len()) };
                (base, unsafe { std::slice::from_raw_parts(base as *const u8, image.len()) })
            }
        };
        let size = align_up(image.len() as u64, PAGE);
        let dynamic = module::parse(image).with_context(|| name.to_string())?;
        let module_id = read_module_id(&dir.join(format!("{name}.layout")))?;
        mods.push(Module {
            name: name.into(),
            module_id,
            base,
            size,
            text: get("text")?.1,
            data: get("data")?,
            keypage: get("keypage")?.0,
            tramp: get("tramp")?,
            dynamic,
        });
        next = next.max(base + size + PAGE);
    }

    let mut table: HashMap<&str, (u64, bool, bool)> = HashMap::new();
    for m in &mods {
        for s in m.dynamic.symbols.iter().filter(|s| s.defined() && !s.name.is_empty()) {
            let addr = m.base + s.value;
            let func = s.info & 0xF == 2;
            match table.get(s.name.as_str()) {
                None => {
                    table.insert(&s.name, (addr, s.weak(), func));
                }
                Some(&(_, true, _)) if !s.weak() => {
                    table.insert(&s.name, (addr, false, func));
                }
                _ => {}
            }
        }
    }

    let mut hle = hle.clone();
    hle.extend(crate::movie::overrides());
    let hle = &hle;
    let mut stub_of: HashMap<String, u64> = HashMap::new();
    let trace_pats: Vec<String> = std::env::var("NX_TRACE_SYMS").map(|v| v.split(',').map(String::from).collect()).unwrap_or_default();
    let mut traced_of: HashMap<String, u64> = HashMap::new();
    let mut resolve = |name: &str| -> u64 {
        if let Some(&a) = hle.get(name) {
            return a;
        }
        if let Some(&(a, _, func)) = table.get(name) {
            if func && !trace_pats.is_empty() {
                let dm = nxfs::nso::demangle(name);
                if trace_pats.iter().any(|p| dm.contains(p.as_str())) {
                    return *traced_of.entry(name.to_string()).or_insert_with(|| {
                        let mut t = TRACED.lock().unwrap();
                        t.push((dm, a));
                        crate::stubs::make(traced, t.len() as u64 - 1)
                    });
                }
            }
            return a;
        }
        *stub_of.entry(name.to_string()).or_insert_with(|| {
            let mut u = UNIMPL.lock().unwrap();
            u.push((name.to_string(), false));
            crate::stubs::make(unimpl, u.len() as u64 - 1)
        })
    };

    let mut used_hle = std::collections::HashSet::new();
    for m in &mods {
        for r in &m.dynamic.relocs {
            let p = (m.base + r.offset) as *mut u64;
            let v = match r.kind {
                module::R_AARCH64_RELATIVE => (m.base as i64 + r.addend) as u64,
                module::R_AARCH64_ABS64 | module::R_AARCH64_GLOB_DAT | module::R_AARCH64_JUMP_SLOT => {
                    let s = &m.dynamic.symbols[r.sym as usize];
                    if hle.contains_key(s.name.as_str()) {
                        used_hle.insert(s.name.clone());
                    }
                    (resolve(&s.name) as i64 + r.addend) as u64
                }
                k => bail!("{}: unsupported relocation type {k}", m.name),
            };
            unsafe { p.write_unaligned(v) };
        }
    }
    for name in hle.keys() {
        ensure!(used_hle.contains(*name), "host override {name} is referenced by no module");
    }

    eprintln!("[loader] {} imports resolved to logging stubs", UNIMPL.lock().unwrap().len());
    crate::resource_compat::patch_missing_avatar_job(&mods)?;
    crate::display::install(&mods)?;

    let freq: u64;
    unsafe { std::arch::asm!("mrs {}, cntfrq_el0", out(reg) freq) };
    let cnt_mul = ((nxabi::GUEST_TICK_HZ as u128) << 64) / freq as u128;
    ensure!(cnt_mul < 1 << 64, "host counter slower than 19.2 MHz");
    let slot = crate::guest::NX_CTX_SLOT.load(std::sync::atomic::Ordering::Relaxed);
    for m in &mods {
        let kp = m.base + m.keypage;
        unsafe {
            *((kp + keypage::CTX_SLOT as u64) as *mut u64) = slot;
            *((kp + keypage::SVC_GATE as u64) as *mut u64) = crate::guest::nx_svc_gate as *const () as usize as u64;
            *((kp + keypage::CNT_MUL as u64) as *mut u64) = cnt_mul as u64;
            *((kp + keypage::NET as u64) as *mut u8) = net_mode();
        }
        let text_end = align_up(m.text, PAGE);
        let data_start = align_down(m.data.0, PAGE);
        a.set(m.base, text_end, Block { state: state::CODE, perm: perm::RX, attr: 0 })?;
        if data_start > text_end {
            a.set(m.base + text_end, data_start - text_end, Block { state: state::CODE, perm: perm::R, attr: 0 })?;
        }
        a.set(m.base + data_start, m.keypage - data_start, Block { state: state::CODE_DATA, perm: perm::RW, attr: 0 })?;
        a.set(kp, keypage::SIZE, Block { state: state::CODE, perm: perm::R, attr: 0 })?;
        let (t0, tsize) = m.tramp;
        a.set(m.base + t0, align_up(t0 + tsize, PAGE) - t0, Block { state: state::CODE, perm: perm::RX, attr: 0 })?;
        icache_flush(m.base, m.size);
        eprintln!("[loader] {:8} at {:#x} (text {:#x}, trampolines {:#x})", m.name, m.base, m.text, tsize);
    }
    debug_breakpoints(&mods)?;
    Ok(mods)
}

fn net_mode() -> u8 {
    match std::env::var("NX_NET").as_deref() {
        Ok("lan") => 1,
        Ok("lanplay") => 2,
        Ok("wireless") => 3,
        Ok(_) => 0,
        Err(_) => std::env::var("NX_LAN").is_ok_and(|v| v == "1") as u8,
    }
}

fn debug_breakpoints(mods: &[Module]) -> Result<()> {
    let Ok(list) = std::env::var("NX_BRK") else { return Ok(()) };
    for (i, spec) in list.split(',').filter(|s| !s.is_empty()).enumerate() {
        let (name, off) = spec.trim().split_once('+').with_context(|| format!("NX_BRK: {spec}: expected module+offset"))?;
        let off = u64::from_str_radix(off.trim_start_matches("0x"), 16).with_context(|| format!("NX_BRK: {spec}"))?;
        let m = mods.iter().find(|m| m.name == name).with_context(|| format!("NX_BRK: no module {name}"))?;
        let addr = m.base + off;
        let page = addr & !(PAGE - 1);
        crate::mem::protect(page, PAGE, perm::RW)?;
        unsafe { (addr as *mut u32).write(0xD420_0000 | ((i as u32 & 0xFFFF) << 5)) };
        crate::mem::protect(page, PAGE, perm::RX)?;
        icache_flush(page, PAGE);
        eprintln!("[loader] NX_BRK #{i} at {spec}");
    }
    Ok(())
}

fn icache_flush(addr: u64, size: u64) {
    unsafe extern "C" {
        fn sys_icache_invalidate(start: *mut libc::c_void, len: usize);
    }
    unsafe { sys_icache_invalidate(addr as *mut _, size as usize) };
}

#[cfg(embedded_modules)]
mod embedded {
    use super::{PAGE, align_up};
    use crate::kernel;
    use crate::mem::{Block, perm, state};
    use anyhow::{Result, ensure};

    std::arch::global_asm!(include_str!(concat!(env!("OUT_DIR"), "/modules.s")));

    unsafe extern "C" {
        static nx_mod_main_x: u8;
        static nx_mod_main_d: u8;
        static nx_mod_main_t: u8;
        static nx_mod_main_len: u64;
        static nx_mod_subsdk0_x: u8;
        static nx_mod_subsdk0_d: u8;
        static nx_mod_subsdk0_t: u8;
        static nx_mod_subsdk0_len: u64;
        static nx_mod_sdk_x: u8;
        static nx_mod_sdk_d: u8;
        static nx_mod_sdk_t: u8;
        static nx_mod_sdk_len: u64;
    }

    pub fn image(name: &str, text: u64, tramp: u64) -> Result<Option<(u64, &'static [u8])>> {
        let (x, d, t, len) = unsafe {
            match name {
                "main" => (&raw const nx_mod_main_x, &raw const nx_mod_main_d, &raw const nx_mod_main_t, nx_mod_main_len),
                "subsdk0" => (&raw const nx_mod_subsdk0_x, &raw const nx_mod_subsdk0_d, &raw const nx_mod_subsdk0_t, nx_mod_subsdk0_len),
                "sdk" => (&raw const nx_mod_sdk_x, &raw const nx_mod_sdk_d, &raw const nx_mod_sdk_t, nx_mod_sdk_len),
                _ => return Ok(None),
            }
        };
        let base = x as u64;
        ensure!(d as u64 == base + align_up(text, PAGE) && t as u64 == base + tramp, "{name}: linked segments are not contiguous (rebuild with matching layouts)");
        ensure!(base + align_up(len, PAGE) < crate::mem::ADDRESS_SPACE_END, "{name}: linked at {base:#x}, above the guest address space");
        let size = align_up(len, PAGE);
        kernel::aspace().map.lock().unwrap().set(base, base + size, Block { state: state::CODE_DATA, perm: perm::RW, attr: 0 });
        Ok(Some((base, unsafe { std::slice::from_raw_parts(x, len as usize) })))
    }
}

#[cfg(not(embedded_modules))]
mod embedded {
    pub fn image(_: &str, _: u64, _: u64) -> anyhow::Result<Option<(u64, &'static [u8])>> {
        Ok(None)
    }
}

pub fn symbol(mods: &[Module], name: &str) -> Option<u64> {
    mods.iter().find_map(|m| m.dynamic.symbols.iter().find(|s| s.defined() && s.name == name).map(|s| m.base + s.value))
}
