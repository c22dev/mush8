// Mush8 - memory.rs
// Constantin Clerc (c22dev) - 0.1
use std::sync::LazyLock;

const MB: u64 = 1 << 20;

fn env_mb(name: &str, default: u64) -> u64 {
    std::env::var(name).ok().and_then(|v| v.trim().parse::<u64>().ok()).unwrap_or(default) * MB
}

pub static TRIM: LazyLock<u64> = LazyLock::new(|| env_mb("NX_TRIM_MB", 400));
pub static EMERGENCY: LazyLock<u64> = LazyLock::new(|| env_mb("NX_TRIM_EMERGENCY_MB", 250));
pub const TIGHT: u64 = 600 * MB;

static LIMIT: LazyLock<u64> = LazyLock::new(|| env_mb("NX_MEM_LIMIT_MB", if cfg!(target_os = "ios") { 3070 } else { 0 }));

pub fn footprint() -> u64 {
    unsafe extern "C" {
        fn proc_pid_rusage(pid: i32, flavor: i32, buffer: *mut u64) -> i32;
    }
    let mut info = [0u64; 64];
    if unsafe { proc_pid_rusage(std::process::id() as i32, 0, info.as_mut_ptr()) } == 0 { info[9] } else { 0 }
}

#[cfg(target_os = "ios")]
fn os_left() -> u64 {
    unsafe extern "C" {
        fn os_proc_available_memory() -> usize;
    }
    match unsafe { os_proc_available_memory() } as u64 {
        0 => u64::MAX,
        n => n,
    }
}

#[cfg(not(target_os = "ios"))]
fn os_left() -> u64 {
    u64::MAX
}

pub fn left() -> u64 {
    let own = if *LIMIT > 0 { LIMIT.saturating_sub(footprint()) } else { u64::MAX };
    os_left().min(own)
}

pub fn physical() -> u64 {
    let mut size = 0u64;
    let mut len = std::mem::size_of::<u64>();
    let r = unsafe { libc::sysctlbyname(c"hw.memsize".as_ptr(), (&mut size as *mut u64).cast(), &mut len, std::ptr::null_mut(), 0) };
    if r == 0 { size } else { 0 }
}

pub static TEX_DROP: LazyLock<u32> = LazyLock::new(|| {
    let auto = if cfg!(target_os = "ios") && physical() > 0 && physical() < 7 << 30 { 1 } else { 0 };
    let drop = match std::env::var("NX_TEX_DROP").ok().as_deref().map(str::trim) {
        Some("auto") | Some("") | None => auto,
        Some(v) => v.parse::<u32>().unwrap_or(auto).min(3),
    };
    eprintln!("[mem] textures: {drop} sharpest mip level(s) skipped, {} MB of RAM", physical() / MB);
    drop
});

pub static TEX_DROP_MIN: LazyLock<u32> = LazyLock::new(|| std::env::var("NX_TEX_DROP_MIN").ok().and_then(|v| v.trim().parse().ok()).unwrap_or(128));

pub fn levels_to_skip(width: u32, height: u32, levels: u32) -> u32 {
    skip_for(width, height, levels, *TEX_DROP, *TEX_DROP_MIN)
}

fn skip_for(width: u32, height: u32, levels: u32, drop: u32, min: u32) -> u32 {
    let mut skip = drop.min(levels.saturating_sub(1));
    while skip > 0 && (width.max(height) >> skip) < min {
        skip -= 1;
    }
    skip
}

#[cfg(test)]
mod tests {
    use super::skip_for as skip;

    #[test]
    fn skip_stops_at_the_minimum_size() {
        assert_eq!(skip(1024, 1024, 11, 3, 128), 3);
        assert_eq!(skip(256, 128, 9, 3, 128), 1);
        assert_eq!(skip(128, 128, 8, 3, 128), 0);
        assert_eq!(skip(2048, 2048, 1, 3, 128), 0);
        assert_eq!(skip(2048, 2048, 2, 3, 128), 1);
    }
}
