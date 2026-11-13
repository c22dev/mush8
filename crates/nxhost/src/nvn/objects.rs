// Mush8 - objects.rs
// Constantin Clerc (c22dev) - 0.1
use super::fx::FastMap;
use crate::stubs::Args;
use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex};

fn rd<T: Copy>(addr: u64) -> T {
    unsafe { (addr as *const T).read_unaligned() }
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryPool {
    pub cpu: u64,
    pub size: u64,
    pub flags: u32,
    pub generation: u64,
}

#[derive(Debug, Clone, Copy)]
pub struct Buffer {
    pub pool: u64,
    pub offset: u64,
    pub size: u64,
}

#[derive(Clone)]
pub struct Texture {
    pub builder: [u8; 0x68],
}

impl Texture {
    fn u32(&self, off: usize) -> u32 {
        u32::from_le_bytes(self.builder[off..off + 4].try_into().unwrap())
    }
    pub fn target(&self) -> u32 {
        self.u32(0x1c)
    }
    pub fn width(&self) -> u32 {
        self.u32(0x20)
    }
    pub fn height(&self) -> u32 {
        self.u32(0x24)
    }
    pub fn depth(&self) -> u32 {
        self.u32(0x28)
    }
    pub fn levels(&self) -> u32 {
        self.u32(0x2c)
    }
    pub fn format(&self) -> u32 {
        self.u32(0x30)
    }
}

#[derive(Default)]
pub struct State {
    pub pools: BTreeMap<u64, MemoryPool>,
    pub buffers: FastMap<u64, Buffer>,
    pub registered_textures: FastMap<(u64, u32), [u8; 0x68]>,
    pub registered_samplers: FastMap<(u64, u32), [u8; 0x60]>,
    pub programs: FastMap<u64, Vec<(u64, u64)>>,
    pub virtual_maps: FastMap<u64, Vec<(u64, u64, u64, u64)>>,
    pub windows: FastMap<u64, (Vec<u64>, u32)>,
}

const TEXTURE_MAGIC: u64 = 0x5845_5458_4E56_4E58;
const BUFFER_MAGIC: u64 = 0x5246_4642_4E56_4E58;

fn buffer(s: &State, obj: u64) -> Option<Buffer> {
    if let Some(b) = s.buffers.get(&obj) {
        return Some(*b);
    }
    if obj == 0 || rd::<u64>(obj) != BUFFER_MAGIC {
        return None;
    }
    Some(Buffer { pool: rd(obj + 8), offset: rd(obj + 0x10), size: rd(obj + 0x18) })
}

pub fn texture(obj: u64) -> Option<Texture> {
    if obj == 0 || rd::<u64>(obj) != TEXTURE_MAGIC {
        return None;
    }
    let mut builder = [0u8; 0x68];
    unsafe { std::ptr::copy_nonoverlapping((obj + 8) as *const u8, builder.as_mut_ptr(), 0x68) };
    Some(Texture { builder })
}

pub fn apply_view(b: &mut [u8; 0x68], view: u64, keep_target: bool) {
    if view == 0 {
        return;
    }
    let flags = rd::<u8>(view);
    if flags & 4 != 0 {
        let from = u32::from_le_bytes(b[0x30..0x34].try_into().unwrap());
        let to = rd::<u32>(view + 8);
        b[0x30..0x34].copy_from_slice(&to.to_le_bytes());
        let ((fw, fh, _), (tw, th, _)) = (super::format_info(from), super::format_info(to));
        if (fw, fh) != (1, 1) && (tw, th) == (1, 1) {
            for (o, bd) in [(0x20, fw), (0x24, fh)] {
                let v = u32::from_le_bytes(b[o..o + 4].try_into().unwrap());
                b[o..o + 4].copy_from_slice(&v.div_ceil(bd).to_le_bytes());
            }
        }
    }
    if flags & 8 != 0 {
        for i in 0..4 {
            b[0x38 + 4 * i..0x3c + 4 * i].copy_from_slice(&rd::<u32>(view + 0xc + 4 * i as u64).to_le_bytes());
        }
    }
    if flags & 0x20 != 0 && !keep_target {
        b[0x1c..0x20].copy_from_slice(&rd::<u32>(view + 0x20).to_le_bytes());
    }
    if flags & 3 != 0 {
        let layer = if flags & 2 != 0 { rd::<u16>(view + 4) } else { 0 };
        let level = if flags & 1 != 0 { rd::<u8>(view + 1) } else { 0 };
        b[0x64..0x66].copy_from_slice(&layer.to_le_bytes());
        b[0x66] = level;
        b[0x67] = SUBRESOURCE_TAG;
    }
}

const SUBRESOURCE_TAG: u8 = 0xB7;

pub fn view_subresource(b: &[u8; 0x68]) -> (u32, u32) {
    if b[0x67] == SUBRESOURCE_TAG { (u16::from_le_bytes([b[0x64], b[0x65]]) as u32, b[0x66] as u32) } else { (0, 0) }
}

const SAMPLER_MAGIC: u64 = 0x504D_4153_4E56_4E58;

pub fn sampler(obj: u64) -> Option<[u8; 0x60]> {
    if obj == 0 || rd::<u64>(obj) != SAMPLER_MAGIC {
        return None;
    }
    let mut b = [0u8; 0x60];
    unsafe { std::ptr::copy_nonoverlapping((obj + 8) as *const u8, b.as_mut_ptr().add(8), 0x40) };
    Some(b)
}

unsafe extern "C" {
    fn nx_pool_storage();
}
std::arch::global_asm!(".globl _nx_pool_storage", ".p2align 2", "_nx_pool_storage:", "ldr x0, [x0, #0xf8]", "ret");

static POOL_VTABLE: LazyLock<[u64; 8]> = LazyLock::new(|| {
    let f = nx_pool_storage as *const () as usize as u64;
    [0, 0, 0, 0, 0, f, 0, f]
});

fn init_pool_struct(obj: u64, p: &MemoryPool) {
    unsafe {
        (obj as *mut u64).write_unaligned(POOL_VTABLE.as_ptr() as u64);
        ((obj + 0x10) as *mut u32).write_unaligned(p.flags & 0x3fff_ffff);
        ((obj + 0x30) as *mut u64).write_unaligned(p.size);
        ((obj + 0xf8) as *mut u64).write_unaligned(p.cpu);
    }
}

const TEXTURE_FIELDS: &[std::ops::Range<usize>] = &[8..0x4c, 0x58..0x5a, 0x5c..0x60];

fn same_object(a: u64, b: u64, magic: u64, fields: &[std::ops::Range<usize>]) -> bool {
    if a == 0 || b == 0 || rd::<u64>(a) != magic || rd::<u64>(b) != magic {
        return false;
    }
    fields.iter().all(|r| {
        let len = r.end - r.start;
        let (x, y) = unsafe { (std::slice::from_raw_parts((a + 8 + r.start as u64) as *const u8, len), std::slice::from_raw_parts((b + 8 + r.start as u64) as *const u8, len)) };
        x == y
    })
}

pub static STATE: LazyLock<Mutex<State>> = LazyLock::new(|| Mutex::new(State::default()));

fn state() -> std::sync::MutexGuard<'static, State> {
    STATE.lock().unwrap()
}

impl State {
    pub fn pool_offset_cpu(&self, pool: u64, offset: u64) -> Option<u64> {
        let p = self.pools.get(&pool)?;
        if p.cpu != 0 {
            return Some(p.cpu + offset);
        }
        let (voff, _, phys, poff) = *self.virtual_maps.get(&pool)?.iter().find(|m| offset >= m.0 && offset < m.0 + m.1)?;
        let pp = self.pools.get(&phys)?;
        (pp.cpu != 0).then(|| pp.cpu + poff + (offset - voff))
    }

    pub fn pool_at(&self, addr: u64) -> Option<(u64, MemoryPool)> {
        let (&k, &p) = self.pools.range(..=addr).next_back()?;
        (addr < p.cpu + p.size).then_some((k, p))
    }
}

pub type HostFn = fn(&Args) -> u64;

pub fn host_impl(name: &str) -> Option<HostFn> {
    Some(match name {
        "nvnMemoryPoolInitialize" => |a| {
            let b = a.0[1];
            static GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let generation = GENERATION.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            let p = MemoryPool { cpu: rd(b + 8), size: rd(b + 0x10), flags: rd(b + 0x18), generation };
            init_pool_struct(a.0[0], &p);
            crate::gfx::heap_touch(p.cpu, p.size);
            state().pools.insert(a.0[0], p);
            1
        },
        "nvnMemoryPoolFinalize" => |a| {
            let p = state().pools.get(&a.0[0]).copied();
            if let Some(p) = p {
                super::draw::free_pool(a.0[0], p);
            }
            0
        },
        "nvnMemoryPoolFlushMappedRange" | "nvnMemoryPoolInvalidateMappedRange" | "nvnMemoryPoolSetDebugLabel" => |_| 0,
        "nvnMemoryPoolMapVirtual" => |a| {
            let mut s = state();
            for i in 0..a.0[1] {
                let r = a.0[2] + 0x28 * i;
                let (phys, poff, voff, size): (u64, u64, u64, u64) = (rd(r), rd(r + 8), rd(r + 0x10), rd(r + 0x18));
                let maps = s.virtual_maps.entry(a.0[0]).or_default();
                maps.retain(|m| m.0 + m.1 <= voff || m.0 >= voff + size);
                if phys != 0 {
                    maps.push((voff, size, phys, poff));
                }
            }
            1
        },

        "nvnBufferInitialize" => |a| {
            let b = a.0[1];
            let buf = Buffer { pool: rd(b + 8), offset: rd(b + 0x10), size: rd(b + 0x18) };
            unsafe {
                let o = a.0[0] as *mut u64;
                o.write_unaligned(BUFFER_MAGIC);
                o.add(1).write_unaligned(buf.pool);
                o.add(2).write_unaligned(buf.offset);
                o.add(3).write_unaligned(buf.size);
            }
            state().buffers.insert(a.0[0], buf);
            1
        },
        "nvnBufferFinalize" => |a| {
            state().buffers.remove(&a.0[0]);
            if a.0[0] != 0 {
                unsafe { (a.0[0] as *mut u64).write_unaligned(0) };
            }
            0
        },
        "nvnBufferGetAddress" | "nvnBufferMap" => |a| {
            let s = state();
            let addr = buffer(&s, a.0[0]).and_then(|b| s.pools.get(&b.pool).map(|p| p.cpu + b.offset));
            if addr.is_none() {
                static WARNED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
                if !WARNED.swap(true, std::sync::atomic::Ordering::Relaxed) {
                    let lr = unsafe { *(a as *const _ as *const u64).sub(1) };
                    eprintln!("[nvn] buffer {:#x} has no address (asked at {})", a.0[0], crate::diag::symbolize(lr));
                }
            }
            addr.unwrap_or(0)
        },
        "nvnBufferGetSize" => |a| buffer(&state(), a.0[0]).map_or(0, |b| b.size),
        "nvnBufferFlushMappedRange" | "nvnBufferInvalidateMappedRange" | "nvnBufferSetDebugLabel" => |_| 0,

        "nvnTextureInitialize" => |a| {
            let mut builder = [0u8; 0x68];
            unsafe { std::ptr::copy_nonoverlapping(a.0[1] as *const u8, builder.as_mut_ptr(), 0x68) };
            unsafe {
                (a.0[0] as *mut u64).write_unaligned(TEXTURE_MAGIC);
                std::ptr::copy_nonoverlapping(builder.as_ptr(), (a.0[0] + 8) as *mut u8, 0x68);
            }
            1
        },
        "nvnTextureFinalize" => |_| 0,
        "nvnTextureGetWidth" => |a| texture(a.0[0]).map_or(0, |t| t.width() as u64),
        "nvnTextureGetHeight" => |a| texture(a.0[0]).map_or(0, |t| t.height() as u64),
        "nvnTextureGetDepth" => |a| texture(a.0[0]).map_or(0, |t| t.depth() as u64),
        "nvnTextureGetTarget" => |a| texture(a.0[0]).map_or(0, |t| t.target() as u64),
        "nvnTextureGetFormat" => |a| texture(a.0[0]).map_or(0, |t| t.format() as u64),
        "nvnTextureGetLevels" => |a| texture(a.0[0]).map_or(0, |t| t.levels() as u64),
        "nvnTextureGetStorageSize" => |a| {
            let b = texture(a.0[0]).map(|t| t.builder);
            b.map_or(0, |b| super::texture_builder_storage(b.as_ptr() as u64).0)
        },
        "nvnTextureSetDebugLabel" | "nvnSamplerSetDebugLabel" => |_| 0,
        "nvnTextureWriteTexels" => |a| {
            if let Some(t) = texture(a.0[0]) {
                super::draw::write_texels(t.builder, a.0[1], rd(a.0[2]), a.0[3] as *const u8, None);
            }
            0
        },
        "nvnTextureWriteTexelsStrided" => |a| {
            if let Some(t) = texture(a.0[0]) {
                super::draw::write_texels(t.builder, a.0[1], rd(a.0[2]), a.0[3] as *const u8, Some((a.0[4] as usize, a.0[5] as usize)));
            }
            0
        },
        "nvnTextureReadTexels" => |a| {
            if let Some(t) = texture(a.0[0]) {
                super::draw::read_texels(t.builder, a.0[1], rd(a.0[2]), a.0[3] as *mut u8, None);
            }
            0
        },
        "nvnTextureReadTexelsStrided" => |a| {
            if let Some(t) = texture(a.0[0]) {
                super::draw::read_texels(t.builder, a.0[1], rd(a.0[2]), a.0[3] as *mut u8, Some((a.0[4] as usize, a.0[5] as usize)));
            }
            0
        },
        "nvnTextureFlushTexels" | "nvnTextureInvalidateTexels" => |_| 0,
        "nvnTextureCompare" => |a| same_object(a.0[0], a.0[1], TEXTURE_MAGIC, TEXTURE_FIELDS) as u64,
        "nvnSamplerCompare" => |a| same_object(a.0[0], a.0[1], SAMPLER_MAGIC, &[0..0x40]) as u64,
        "nvnSamplerInitialize" => |a| {
            let mut b = [0u8; 0x60];
            unsafe { std::ptr::copy_nonoverlapping(a.0[1] as *const u8, b.as_mut_ptr(), 0x60) };
            unsafe {
                (a.0[0] as *mut u64).write_unaligned(SAMPLER_MAGIC);
                std::ptr::copy_nonoverlapping(b.as_ptr().add(8), (a.0[0] + 8) as *mut u8, 0x40);
            }
            1
        },
        "nvnSamplerFinalize" => |_| 0,
        "nvnTexturePoolInitialize" | "nvnSamplerPoolInitialize" => |_| 1,
        "nvnTexturePoolFinalize" | "nvnSamplerPoolFinalize" => |_| 0,
        "nvnTexturePoolRegisterTexture" => |a| {
            let mut s = state();
            if let Some(mut b) = texture(a.0[2]).map(|t| t.builder) {
                apply_view(&mut b, a.0[3], false);
                s.registered_textures.insert((a.0[0], a.0[1] as u32), b);
            }
            0
        },
        "nvnSamplerPoolRegisterSampler" => |a| {
            let mut s = state();
            if let Some(b) = sampler(a.0[2]) {
                s.registered_samplers.insert((a.0[0], a.0[1] as u32), b);
            }
            0
        },
        "nvnSamplerPoolRegisterSamplerBuilder" => |a| {
            let mut b = [0u8; 0x60];
            unsafe { std::ptr::copy_nonoverlapping(a.0[2] as *const u8, b.as_mut_ptr(), 0x60) };
            state().registered_samplers.insert((a.0[0], a.0[1] as u32), b);
            0
        },
        "nvnDeviceGetTextureHandle" => |a| ((a.0[1] as u32 | (a.0[2] as u32) << 20) as i32 as i64 as u64) | 1 << 32,
        "nvnDeviceGetImageHandle" => |a| a.0[1] as i32 as i64 as u64,

        "nvnSyncInitialize" => |_| 1,
        "nvnSyncFinalize" => |_| 0,
        "nvnQueueFenceSync" => super::cmd::fence_sync,
        "nvnSyncWait" => super::cmd::sync_wait,

        "nvnQueueInitialize" => |_| 1,
        "nvnQueueFinalize" | "nvnQueueFlush" => |_| 0,
        "nvnQueueFinish" => |_| {
            super::draw::finish();
            0
        },
        "nvnQueueSubmitCommands" => super::cmd::submit,
        "nvnQueuePresentTexture" => super::cmd::present,
        "nvnWindowInitialize" => |a| {
            let b = a.0[1];
            let (count, list): (u32, u64) = (rd(b + 8), rd(b + 0x10));
            let textures = (0..count as u64).map(|i| rd::<u64>(list + 8 * i)).collect();
            state().windows.insert(a.0[0], (textures, 0));
            1
        },
        "nvnWindowFinalize" => |_| 0,
        "nvnWindowSetCrop" => |a| {
            crate::gfx::set_crop(a.0[1] as i32, a.0[2] as i32, a.0[3] as i32, a.0[4] as i32);
            0
        },
        "nvnWindowSetPresentInterval" => |a| {
            crate::gfx::set_present_interval(a.0[1] as u32);
            0
        },
        "nvnQueueAcquireTexture" => |a| acquire(a.0[1], a.0[2]),
        "nvnWindowAcquireTexture" => |a| acquire(a.0[0], a.0[2]),

        "nvnCommandBufferInitialize" => |_| 1,
        "nvnCommandBufferFinalize" => |_| 0,
        "nvnCommandBufferBeginRecording" => |a| {
            super::cmd::begin(a.0[0]);
            0
        },
        "nvnCommandBufferEndRecording" => |a| super::cmd::end(a.0[0]),
        "nvnCommandBufferAddControlMemory" => |a| {
            super::cmd::add_control_memory(a.0[0], a.0[1]);
            0
        },
        "nvnCommandBufferSetRenderTargets" => super::cmd::set_render_targets,
        "nvnCommandBufferClearColor" => super::cmd::clear_color,
        "nvnCommandBufferClearDepthStencil" => super::cmd::clear_depth_stencil,
        "nvnCommandBufferCallCommands" => super::cmd::call_commands,
        "nvnCommandBufferBindUniformBuffer" => super::cmd::bind_uniform_buffer,
        "nvnCommandBufferBindTexture" => super::cmd::bind_texture,
        name if super::cmd::host_impl(name).is_some() => return super::cmd::host_impl(name),
        "nvnCommandBufferGetCommandMemoryUsed" | "nvnCommandBufferGetControlMemoryUsed" => |a| super::cmd::memory_used(a.0[0]),

        "nvnProgramInitialize" => |a| {
            state().programs.insert(a.0[0], Vec::new());
            1
        },
        "nvnProgramFinalize" => |a| {
            state().programs.remove(&a.0[0]);
            0
        },
        "nvnProgramSetShaders" => |a| {
            let stages: Vec<(u64, u64)> = (0..a.0[1]).map(|i| (rd::<u64>(a.0[2] + 16 * i), rd::<u64>(a.0[2] + 16 * i + 8))).collect();
            if std::env::var_os("NX_DUMP_SHADERS").is_some() {
                dump_shaders(a.0[0], &stages);
            }
            state().programs.insert(a.0[0], stages);
            1
        },

        "nvnDeviceSetWindowOriginMode" => |a| {
            eprintln!("[nvn] window origin mode {}", a.0[1]);
            super::draw::set_origin_upper_left(a.0[1] as u32 == 1);
            0
        },
        "nvnDeviceSetDepthMode" => |a| {
            eprintln!("[nvn] depth mode {}", a.0[1]);
            super::draw::set_depth_zero_to_one(a.0[1] as u32 == 1);
            0
        },
        "nvnDeviceInstallDebugCallback" | "nvnDeviceApplyDeferredFinalizes" => |_| 0,
        "nvnDeviceGetTimestampInNanoseconds" => |_| {
            static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
            START.get_or_init(std::time::Instant::now).elapsed().as_nanos() as u64
        },
        _ => return None,
    })
}

fn acquire(window: u64, index_out: u64) -> u64 {
    let mut s = state();
    let Some((textures, next)) = s.windows.get_mut(&window) else { return 1 };
    let n = textures.len().max(1) as u32;
    unsafe { *(index_out as *mut i32) = *next as i32 };
    *next = (*next + 1) % n;
    0
}

fn dump_shaders(program: u64, stages: &[(u64, u64)]) {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    if n >= 40 {
        return;
    }
    let _ = std::fs::create_dir_all("work/shaders");
    for (i, &(code, control)) in stages.iter().enumerate() {
        let ctl = unsafe { std::slice::from_raw_parts(control as *const u8, 0x1000) };
        let _ = std::fs::write(format!("work/shaders/p{n:03}_s{i}.control"), ctl);
        let s = state();
        let len = s.pool_at(code).map_or(0, |(_, p)| (p.cpu + p.size - code).min(0x10000));
        drop(s);
        if len > 0 {
            let c = unsafe { std::slice::from_raw_parts(code as *const u8, len as usize) };
            let _ = std::fs::write(format!("work/shaders/p{n:03}_s{i}.code"), c);
        }
        eprintln!("[shader] program {program:#x} #{n} stage {i}: code {code:#x} control {control:#x}");
    }
}

#[cfg(test)]
mod buffer_tests {
    use super::*;

    fn call(name: &str, obj: u64, builder: u64) -> u64 {
        host_impl(name).unwrap()(&Args([obj, builder, 0, 0, 0, 0, 0, 0], [0; 8]))
    }

    struct Fixture {
        storage: Box<[u8; 64]>,
        pool: Box<[u64; 32]>,
        object: Box<[u64; 16]>,
    }

    impl Fixture {
        fn new() -> Self {
            let mut f = Self { storage: Box::new([0; 64]), pool: Box::new([0; 32]), object: Box::new([0; 16]) };
            let pool_builder = [0, f.storage.as_ptr() as u64, 64, 0];
            assert_eq!(call("nvnMemoryPoolInitialize", f.pool.as_mut_ptr() as u64, pool_builder.as_ptr() as u64), 1);
            let builder = [0, f.pool.as_ptr() as u64, 8, 24];
            assert_eq!(call("nvnBufferInitialize", f.object.as_mut_ptr() as u64, builder.as_ptr() as u64), 1);
            f
        }

        fn address(&self) -> u64 {
            self.storage.as_ptr() as u64 + 8
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let mut s = STATE.lock().unwrap();
            s.buffers.remove(&(self.object.as_ptr() as u64));
            s.pools.remove(&(self.pool.as_ptr() as u64));
        }
    }

    #[test]
    fn copied_buffer_preserves_address_mapping_and_size() {
        let f = Fixture::new();
        let copy = *f.object;
        for name in ["nvnBufferGetAddress", "nvnBufferMap"] {
            assert_eq!(call(name, f.object.as_ptr() as u64, 0), f.address());
            assert_eq!(call(name, copy.as_ptr() as u64, 0), f.address(), "{name} lost the copied buffer's storage");
        }
        assert_eq!(call("nvnBufferGetSize", copy.as_ptr() as u64, 0), 24);
    }

    #[test]
    fn finalized_buffer_does_not_regain_its_address() {
        let mut f = Fixture::new();
        let obj = f.object.as_mut_ptr() as u64;
        call("nvnBufferFinalize", obj, 0);
        for name in ["nvnBufferGetAddress", "nvnBufferMap", "nvnBufferGetSize"] {
            assert_eq!(call(name, obj, 0), 0, "{name} resurrected a finalized buffer");
        }
    }

    #[test]
    fn finalizing_a_copy_does_not_finalize_the_original() {
        let f = Fixture::new();
        let mut copy = *f.object;
        let copy_addr = copy.as_mut_ptr() as u64;
        assert_eq!(call("nvnBufferGetAddress", copy_addr, 0), f.address());
        call("nvnBufferFinalize", copy_addr, 0);
        assert_eq!(call("nvnBufferGetAddress", copy_addr, 0), 0);
        assert_eq!(call("nvnBufferGetSize", copy_addr, 0), 0);
        assert_eq!(call("nvnBufferGetAddress", f.object.as_ptr() as u64, 0), f.address());
    }

    #[test]
    fn null_and_uninitialized_buffer_objects_return_zero() {
        let unknown = [0u64; 16];
        for obj in [0, unknown.as_ptr() as u64] {
            for name in ["nvnBufferGetAddress", "nvnBufferMap", "nvnBufferGetSize"] {
                assert_eq!(call(name, obj, 0), 0);
            }
        }
    }
}
