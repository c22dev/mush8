// Optional-resource compatibility for firmware-free hosts.
//
// Adapted from lyccccy's firmware-free work in c22dev/mush8#3.
//
// Mario Kart 8 Deluxe 4.0.1 assumes the Mii system archive exists in a few
// places.  The guards below are instruction- and build-ID-checked before any
// guest code is changed, so a different game build fails safely.
use crate::{loader, services};
use std::collections::{HashMap, HashSet};
use std::ffi::CStr;
use std::sync::{LazyLock, Mutex, OnceLock};

const QUERY: &str = "_ZN2nn2fs29QueryMountSystemDataCacheSizeEPmNS_3ncm12SystemDataIdE";
const MOUNT: &str = "_ZN2nn2fs15MountSystemDataEPKcNS_3ncm12SystemDataIdEPvm";
const OPEN: &str = "_ZN2nn2fs8OpenFileEPNS0_10FileHandleEPKci";
const UNMOUNT: &str = "_ZN2nn2fs7UnmountEPKc";

static ORIGINAL: OnceLock<[u64; 4]> = OnceLock::new();
static ABSENT: LazyLock<Mutex<HashSet<String>>> = LazyLock::new(Default::default);
static MAIN_BASE: OnceLock<u64> = OnceLock::new();

#[unsafe(no_mangle)]
pub extern "C" fn mk8_empty_avatar(_info: u64, heap: u64, _extra: u64) -> u64 {
    let base = *MAIN_BASE.get().unwrap();
    let alloc: extern "C" fn(u64, u64, u64) -> u64 = unsafe { std::mem::transmute(base + 0x5faba8) };
    let ctor: extern "C" fn(u64) = unsafe { std::mem::transmute(base + 0x814074) };
    let obj = alloc(0x1588, heap, 8);
    if obj == 0 {
        return 0;
    }
    unsafe { std::ptr::write_bytes(obj as *mut u8, 0, 0x1588) };
    ctor(obj);
    // A real, constructed object with no geometry, images, or pending GPU work.
    unsafe { *((obj + 0x34) as *mut u8) = 1 };
    obj
}

pub fn patch_missing_avatar_job(mods: &[loader::Module]) -> anyhow::Result<()> {
    if present(0x0100_0000_0000_0802) && !cfg!(all(embedded_modules, missing_mii_profile)) {
        return Ok(());
    }
    let main = mods.iter().find(|m| m.name == "main").unwrap();
    let id: String = main.module_id.iter().map(|b| format!("{b:02x}")).collect();
    anyhow::ensure!(
        id == "158c0c7399aabd7bf83795c2ef3484c700000000000000000000000000000000",
        "missing-avatar profile requires verified 4.0.1 main"
    );
    let _ = MAIN_BASE.set(main.base);

    #[cfg(embedded_modules)]
    {
        anyhow::ensure!(cfg!(missing_mii_profile), "iOS without MiiModel requires the baked missing-Mii profile");
        eprintln!("[compat] using signed, baked empty-avatar profile (no executable writes)");
        return Ok(());
    }

    #[cfg(not(embedded_modules))]
    {
        let branch = |from: u64, to: u64| 0x1400_0000 | ((((to as i64 - from as i64) / 4) as u32) & 0x03ff_ffff);
        unsafe {
            let p = (main.base + 0x813dd4) as *mut u32;
            anyhow::ensure!(*p == 0xa9bd57f6, "unexpected Mii avatar constructor instruction");
            // ldr x16, pc+8; br x16; .quad host factory
            *p = 0x58000050;
            *p.add(1) = 0xd61f0200;
            std::ptr::write_unaligned(p.add(2) as *mut u64, mk8_empty_avatar as *const () as usize as u64);

            let guard = main.tramp.0 + main.tramp.1;
            anyhow::ensure!(
                (guard + 140).next_multiple_of(crate::mem::PAGE) == guard.next_multiple_of(crate::mem::PAGE),
                "no padding for Mii callback guards"
            );
            let gp = (main.base + guard) as *mut u32;
            std::ptr::copy_nonoverlapping(
                [0xb4000080, 0xf9400008, 0xf9402d08, branch(guard + 12, 0x810478), branch(guard + 16, 0x81047c)].as_ptr(),
                gp,
                5,
            );
            let site = (main.base + 0x810470) as *mut u32;
            anyhow::ensure!(*site == 0xf9400008, "unexpected Mii thumbnail callback instruction");
            *site = branch(0x810470, guard);

            let guard2 = guard + 20;
            let site2 = (main.base + 0x80f61c) as *mut u32;
            anyhow::ensure!(*site2 == 0xf9400308, "unexpected Mii lobby callback instruction");
            std::ptr::copy_nonoverlapping(
                [0xb40000b8, 0xf9400308, 0xf9402d08, 0xaa1803e0, branch(guard2 + 16, 0x80f628), branch(guard2 + 20, 0x80f62c)].as_ptr(),
                (main.base + guard2) as *mut u32,
                6,
            );
            *site2 = branch(0x80f61c, guard2);

            let guard3 = guard + 44;
            let site3 = (main.base + 0x810d4c) as *mut u32;
            anyhow::ensure!(*site3 == 0xf9401913, "unexpected Mii layout attachment instruction");
            std::ptr::copy_nonoverlapping(
                [0xb5000048, branch(guard3 + 4, 0x810edc), 0xf9401913, branch(guard3 + 12, 0x810d50)].as_ptr(),
                (main.base + guard3) as *mut u32,
                4,
            );
            *site3 = branch(0x810d4c, guard3);

            let guard4 = guard + 60;
            let site4 = (main.base + 0x3e1d5c) as *mut u32;
            anyhow::ensure!(*site4 == 0x1e2703e8, "unexpected LAN Mii transform instruction");
            std::ptr::copy_nonoverlapping(
                [0xb5000040, branch(guard4 + 4, 0x3e1da0), 0x1e2703e8, branch(guard4 + 12, 0x3e1d60)].as_ptr(),
                (main.base + guard4) as *mut u32,
                4,
            );
            *site4 = branch(0x3e1d5c, guard4);

            let guard5 = guard + 76;
            let site5 = (main.base + 0x818d00) as *mut u32;
            anyhow::ensure!(*site5 == 0xa9bd57f6, "unexpected Mii expression prologue");
            std::ptr::copy_nonoverlapping(
                [0xb5000040, 0xd65f03c0, 0xa9bd57f6, branch(guard5 + 12, 0x818d04)].as_ptr(),
                (main.base + guard5) as *mut u32,
                4,
            );
            *site5 = branch(0x818d00, guard5);

            for (i, off) in [0x3eeb5c, 0x3eeb88, 0x3f35c0].into_iter().enumerate() {
                let thunk = guard + 92 + i as u64 * 16;
                let site = (main.base + off) as *mut u32;
                anyhow::ensure!(*site == 0xb942f808, "unexpected LAN Mii expression query");
                std::ptr::copy_nonoverlapping(
                    [0x2a1f03e8, 0xb4000040, 0xb942f808, branch(thunk + 12, off + 4)].as_ptr(),
                    (main.base + thunk) as *mut u32,
                    4,
                );
                *site = branch(off, thunk);
            }
        }
        eprintln!("[compat] MiiModel unavailable: native empty avatar object (Mii graphics disabled)");
        Ok(())
    }
}

pub fn init(mods: &[loader::Module]) {
    let _ = ORIGINAL.set([
        loader::symbol(mods, QUERY).unwrap(),
        loader::symbol(mods, MOUNT).unwrap(),
        loader::symbol(mods, OPEN).unwrap(),
        loader::symbol(mods, UNMOUNT).unwrap(),
    ]);
}

fn present(id: u64) -> bool {
    services::fs::system_data(services::fs::CONFIG.get().unwrap(), id).is_some()
}

extern "C" fn query(out: *mut u64, id: u64) -> u32 {
    if !present(id) {
        unsafe { *out = 0 };
        eprintln!("[compat] optional system data {id:016x}: cache query returns TARGET_NOT_FOUND");
        return 0x7d402;
    }
    let f: extern "C" fn(*mut u64, u64) -> u32 = unsafe { std::mem::transmute(ORIGINAL.get().unwrap()[0]) };
    f(out, id)
}

extern "C" fn mount(name: *const i8, id: u64, cache: *mut u8, size: u64) -> u32 {
    if !present(id) {
        ABSENT.lock().unwrap().insert(unsafe { CStr::from_ptr(name) }.to_string_lossy().into_owned());
        eprintln!("[compat] optional system data {id:016x}: mount returns TARGET_NOT_FOUND");
        return 0x7d402;
    }
    let f: extern "C" fn(*const i8, u64, *mut u8, u64) -> u32 = unsafe { std::mem::transmute(ORIGINAL.get().unwrap()[1]) };
    f(name, id, cache, size)
}

extern "C" fn open(out: *mut u64, path: *const i8, mode: i32) -> u32 {
    let path_text = unsafe { CStr::from_ptr(path) }.to_string_lossy();
    if path_text.split_once(':').is_some_and(|(mount, _)| ABSENT.lock().unwrap().contains(mount)) {
        unsafe { *out = 0 };
        return 0x202;
    }
    let f: extern "C" fn(*mut u64, *const i8, i32) -> u32 = unsafe { std::mem::transmute(ORIGINAL.get().unwrap()[2]) };
    f(out, path, mode)
}

extern "C" fn unmount(name: *const i8) {
    if ABSENT.lock().unwrap().contains(unsafe { CStr::from_ptr(name) }.to_string_lossy().as_ref()) {
        return;
    }
    let f: extern "C" fn(*const i8) = unsafe { std::mem::transmute(ORIGINAL.get().unwrap()[3]) };
    f(name)
}

pub fn register(table: &mut HashMap<&'static str, u64>) {
    table.insert(QUERY, query as *const () as usize as u64);
    table.insert(MOUNT, mount as *const () as usize as u64);
    table.insert(OPEN, open as *const () as usize as u64);
    table.insert(UNMOUNT, unmount as *const () as usize as u64);
}
