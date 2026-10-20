// Native projection adaptation: keep geometry/UI proportions while expanding
// the visible area. Only the verified game's projection classes are affected.
use crate::{display_profile as profile, loader};
use std::sync::{
    OnceLock,
    atomic::{AtomicBool, AtomicU64, Ordering::Relaxed},
};

pub const BASE_ASPECT: f64 = 16.0 / 9.0;
static TARGET: AtomicU64 = AtomicU64::new(BASE_ASPECT.to_bits());
static ENABLED: AtomicBool = AtomicBool::new(false);
static HOOKS: OnceLock<Hooks> = OnceLock::new();
thread_local! { static ROOT_DEPTH:std::cell::Cell<usize>=const {std::cell::Cell::new(0)};}
struct Hooks {
    getters: [u64; 3],
    matrix: u64,
    vtable: u64,
}

pub fn active() -> bool {
    ENABLED.load(Relaxed)
}
pub fn aspect() -> f64 {
    f64::from_bits(TARGET.load(Relaxed))
}
pub fn update_size(w: f64, h: f64) {
    if w <= 0.0 || h <= 0.0 {
        return;
    }
    let a = w / h;
    if !(0.5..=5.0).contains(&a) {
        return;
    }
    let old = f64::from_bits(TARGET.swap(a.to_bits(), Relaxed));
    if (old - a).abs() > 0.002 {
        eprintln!("[display] surface {w:.0}x{h:.0}, native aspect {a:.5}");
    }
}

// Preserve the original horizontal/vertical field of view, expanding whichever
// axis the screen has more room for. Scaling clip coordinates, not the final
// photograph, keeps a circle round on the destination display.
fn clip_scale(a: f64) -> [f32; 2] {
    [
        (BASE_ASPECT / a).min(1.0) as f32,
        (a / BASE_ASPECT).min(1.0) as f32,
    ]
}
fn trace(kind: &str, obj: u64, p: &[f32]) {
    static ON: std::sync::LazyLock<bool> =
        std::sync::LazyLock::new(|| std::env::var("NX_PROJECTION_TRACE").is_ok());
    static SEEN: std::sync::Mutex<Vec<u64>> = std::sync::Mutex::new(Vec::new());
    if !*ON {
        return;
    }
    let mut seen = SEEN.lock().unwrap();
    if !seen.contains(&obj) && seen.len() < 100 {
        seen.push(obj);
        eprintln!("[projection] {kind} {obj:#x}: {p:?}");
    }
}
unsafe fn scale_matrix(out: u64) {
    let s = clip_scale(aspect());
    let m = unsafe { std::slice::from_raw_parts_mut(out as *mut f32, 16) };
    for i in 0..4 {
        m[i] *= s[0];
        m[i + 4] *= s[1];
    }
}

extern "C" fn perspective(obj: u64, out: u64) {
    let h = HOOKS.get().unwrap();
    let f: extern "C" fn(u64, u64) = unsafe { std::mem::transmute(h.matrix) };
    f(obj, out);
    let a = unsafe { *((obj + 0xb0) as *const f32) };
    trace("perspective", obj, unsafe {
        std::slice::from_raw_parts((obj + 0x98) as *const f32, 9)
    });
    let near = unsafe { *((obj + 0x98) as *const f32) };
    let far = unsafe { *((obj + 0x9c) as *const f32) };
    let fovy = unsafe { *((obj + 0xa0) as *const f32) };
    // The menu compositor also uses a perspective camera. Its UI coordinates
    // are corrected by Pane layout, so correcting that camera a second time
    // would squeeze every menu and HUD horizontally.
    let ui_camera =
        near == 1.0 && far == 10000.0 && (fovy - std::f32::consts::FRAC_PI_4).abs() < 0.00001;
    if active() && !ui_camera && (a as f64 - BASE_ASPECT).abs() < 0.01 {
        unsafe {
            scale_matrix(out);
        }
    }
}
fn get(obj: u64, i: usize) -> u64 {
    let h = HOOKS.get().unwrap();
    if active() && unsafe { *(obj as *const u64) == h.vtable } {
        unsafe {
            *((obj + 8) as *mut u8) = 1;
        }
    }
    let f: extern "C" fn(u64) -> u64 = unsafe { std::mem::transmute(h.getters[i]) };
    f(obj)
}
#[unsafe(no_mangle)]
pub extern "C" fn mk8_projection_get(obj: u64) -> u64 {
    get(obj, 0)
}
#[unsafe(no_mangle)]
pub extern "C" fn mk8_projection_device_get(obj: u64) -> u64 {
    get(obj, 1)
}

unsafe fn pane_name(obj: u64) -> &'static str {
    let bytes = unsafe { std::slice::from_raw_parts((obj + 0xb8) as *const u8, 24) };
    std::str::from_utf8(&bytes[..bytes.iter().position(|&b| b == 0).unwrap_or(24)]).unwrap_or("")
}
#[unsafe(no_mangle)]
pub extern "C" fn mk8_pane_calculate(obj: u64, draw: u64, ctx: u64, force: u64) {
    let h = HOOKS.get().unwrap();
    let f: extern "C" fn(u64, u64, u64, u64) = unsafe { std::mem::transmute(h.getters[2]) };
    let name = unsafe { pane_name(obj) };
    let top = name == "RootPane" && unsafe { *((obj + 0x18) as *const u64) == 0 };
    let depth = ROOT_DEPTH.get();
    let root = top && depth == 0;
    if top {
        ROOT_DEPTH.set(depth + 1);
    }
    let dims = unsafe { std::slice::from_raw_parts((obj + 0x50) as *const f32, 2) };
    if root {
        trace(name, obj, dims);
    }
    if name.starts_with("N_Capture")
        || name.starts_with("P_BlueBG")
        || name.starts_with("P_TiteleBG")
    {
        trace(name, obj, dims);
    }
    let bg = (name.starts_with("P_BG")
        || name.starts_with("P_BlueBG")
        || name.starts_with("P_TiteleBG"))
        && dims[0] >= 1200.0
        && dims[1] >= 670.0;
    let scale = clip_scale(aspect());
    let multiplier = if root {
        scale
    } else if bg {
        let cover = 1.0 / scale[0].min(scale[1]);
        [cover, cover]
    } else {
        [1.0, 1.0]
    };
    if !active() || multiplier == [1.0, 1.0] {
        f(obj, draw, ctx, force);
        if top {
            ROOT_DEPTH.set(depth);
        }
        return;
    }
    let p = (obj + 0x48) as *mut f32;
    let old = unsafe { [*p, *p.add(1)] };
    unsafe {
        *p = old[0] * multiplier[0];
        *p.add(1) = old[1] * multiplier[1];
        *((obj + 0x58) as *mut u8) |= 0x10;
    }
    f(obj, draw, ctx, 1);
    if top {
        ROOT_DEPTH.set(depth);
    }
    unsafe {
        *p = old[0];
        *p.add(1) = old[1];
        *((obj + 0x58) as *mut u8) |= 0x10;
    }
}

pub fn install(mods: &[loader::Module]) -> anyhow::Result<()> {
    let m = mods.iter().find(|m| m.name == "main").unwrap();
    let id: String = m.module_id.iter().map(|b| format!("{b:02x}")).collect();
    anyhow::ensure!(
        id == profile::MODULE_ID,
        "native aspect profile requires verified MK8DX 4.0.1"
    );
    let guard = m.tramp.0 + m.tramp.1;
    let mut getters = [0; 3];
    for (i, (site, words, _)) in profile::GETTERS.iter().enumerate() {
        #[cfg(embedded_modules)]
        let _ = (site, words);
        let stub = guard + profile::STUB_OFFSET + i as u64 * profile::STUB_SIZE;
        anyhow::ensure!(
            stub + profile::STUB_SIZE <= guard.next_multiple_of(crate::mem::PAGE),
            "projection stub exceeds trampoline padding"
        );
        #[cfg(not(embedded_modules))]
        unsafe {
            let original = std::slice::from_raw_parts((m.base + site) as *const u32, 4);
            anyhow::ensure!(
                original == words,
                "unexpected projection getter at {site:#x}"
            );
            std::ptr::copy_nonoverlapping(words.as_ptr(), (m.base + stub) as *mut u32, 4);
            let branch = 0x14000000u32
                | ((((*site + 16) as i64 - (stub + 16) as i64) / 4) as u32 & 0x3ffffff);
            *((m.base + stub + 16) as *mut u32) = branch;
            let p = (m.base + site) as *mut u32;
            *p = 0x58000050;
            *p.add(1) = 0xd61f0200;
            let host = [
                mk8_projection_get as *const (),
                mk8_projection_device_get as *const (),
                mk8_pane_calculate as *const (),
            ][i];
            std::ptr::write_unaligned(p.add(2) as *mut u64, host as usize as u64);
        }
        getters[i] = m.base + stub;
    }
    unsafe {
        anyhow::ensure!(
            *((m.base + 0x12b00a8) as *const u64) == m.base + 0x61bfa4,
            "unexpected perspective virtual method"
        );
    }
    let h = Hooks {
        getters,
        matrix: m.base + 0x61bfa4,
        vtable: m.base + 0x12b0050,
    };
    anyhow::ensure!(HOOKS.set(h).is_ok(), "projection profile installed twice");
    unsafe {
        *((m.base + 0x12b00a8) as *mut u64) = perspective as *const () as usize as u64;
    }
    ENABLED.store(
        std::env::var("NX_WIDESCREEN").as_deref() != Ok("0"),
        Relaxed,
    );
    eprintln!(
        "[display] native projection adaptation {} (3D + UI, verified 4.0.1)",
        if active() { "enabled" } else { "disabled" }
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn projection_keeps_circles_round_and_expands_view() {
        for a in [16.0 / 10.0, 16.0 / 9.0, 19.5 / 9.0, 21.0 / 9.0] {
            let [sx, sy] = clip_scale(a);
            // Original clip width/height of a circle is 1 / original aspect.
            let screen_ratio = (sx as f64 / BASE_ASPECT) * a / sy as f64;
            assert!((screen_ratio - 1.0).abs() < 1e-6);
            assert!(sx <= 1.0 && sy <= 1.0);
        }
    }
}
