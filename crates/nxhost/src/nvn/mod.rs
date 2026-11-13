// Mush8 - mod.rs
// Constantin Clerc (c22dev) - 0.1
mod cmd;
mod draw;
mod fx;
pub use draw::{precompile, prewarm};
mod objects;
pub(crate) mod tex;

use crate::stubs::{self, Args};
use std::collections::HashMap;
use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU64, Ordering::Relaxed};
use std::sync::{Mutex, OnceLock};

struct DriverTables {
    module_id: &'static str,
    name_offsets: u64,
    names: u64,
    procs: u64,
    count: usize,
    property_helper: u64,
    format_table_got: u64,
}

const DRIVERS: &[DriverTables] = &[DriverTables {
    module_id: "ba520906609c503227871a314a94fb5f6ccbe444",
    name_offsets: 0x81b5d0,
    names: 0x818360,
    procs: 0x9a1368,
    count: 477,
    property_helper: 0x2d3a68,
    format_table_got: 0x99b7f8,
}];

struct Driver {
    procs: HashMap<String, u64>,
    property_helper: u64,
    format_table: u64,
}

static DRIVER: OnceLock<Driver> = OnceLock::new();

pub fn init(sdk_base: u64, module_id: &[u8]) -> anyhow::Result<()> {
    let id = hex_id(module_id);
    let t = DRIVERS.iter().find(|t| t.module_id == id).ok_or_else(|| anyhow::anyhow!("unknown sdk build {id}"))?;
    let mut procs = HashMap::new();
    for i in 0..t.count as u64 {
        let off = unsafe { *((sdk_base + t.name_offsets + 4 * i) as *const u32) } as u64;
        let name = unsafe { CStr::from_ptr((sdk_base + t.names + off) as *const c_char) }.to_string_lossy();
        let addr = unsafe { *((sdk_base + t.procs + 8 * i) as *const u64) };
        procs.insert(format!("nvn{name}"), addr);
    }
    let format_table = unsafe { *((sdk_base + t.format_table_got) as *const u64) };
    let _ = DRIVER.set(Driver { procs, property_helper: sdk_base + t.property_helper, format_table });
    Ok(())
}

fn hex_id(id: &[u8]) -> String {
    id.iter().take(20).map(|b| format!("{b:02x}")).collect()
}

fn is_driver_passthrough(name: &str) -> bool {
    const STATES: [&str; 8] = [
        "BlendState", "ChannelMaskState", "ColorState", "DepthStencilState", "MultisampleState", "PolygonState",
        "VertexAttribState", "VertexStreamState",
    ];
    let Some(rest) = name.strip_prefix("nvn") else { return false };
    STATES.iter().any(|s| rest.starts_with(s))
        || matches!(rest, "MemoryPoolMap" | "MemoryPoolGetBufferAddress" | "MemoryPoolGetFlags" | "MemoryPoolGetSize")
        || rest.starts_with("TextureView")
        || (rest.contains("Builder") && (rest.contains("BuilderSet") || rest.contains("BuilderGet")))
}

type HostFn = fn(&Args) -> u64;

fn host_impl(name: &str) -> Option<HostFn> {
    if let Some(f) = objects::host_impl(name) {
        return Some(f);
    }
    Some(match name {
        "nvnDeviceInitialize" => |_| 1,
        "nvnDeviceGetInteger" => device_get_integer,
        "nvnTextureBuilderGetStorageSize" => |a| texture_builder_storage(a.0[0]).0,
        "nvnTextureBuilderGetStorageAlignment" => |a| texture_builder_storage(a.0[0]).1,
        "nvnTextureBuilderGetZCullStorageSize" => |_| 0,
        "nvnTextureBuilderGetStorageClass" => |a| {
            let b = TextureBuilder::read(a.0[0]);
            if b.target == TARGET_BUFFER || b.flags & (FLAG_LINEAR | FLAG_LINEAR_RENDER_TARGET) != 0 { 0 } else { 0xFE }
        },
        _ => return None,
    })
}

pub(crate) fn gfx_integer_block_format(format: u32) -> bool {
    matches!(format, 23 | 24 | 47 | 48)
}

fn format_info(format: u32) -> (u32, u32, u32) {
    let e = DRIVER.get().unwrap().format_table + 0x34 * format as u64;
    let b = |o: u64| unsafe { *((e + o) as *const u8) } as u32;
    (b(8).max(1), b(9).max(1), b(0x10).max(1))
}

struct TextureBuilder {
    flags: u32,
    target: u32,
    width: u32,
    height: u32,
    depth: u32,
    levels: u32,
    format: u32,
    samples: u32,
    packaged: u16,
    stride: u32,
}

impl TextureBuilder {
    fn read(b: u64) -> Self {
        let u = |o: u64| unsafe { *((b + o) as *const u32) };
        Self {
            flags: u(0x18),
            target: u(0x1c),
            width: u(0x20),
            height: u(0x24),
            depth: u(0x28),
            levels: u(0x2c),
            format: u(0x30),
            samples: u(0x34),
            packaged: unsafe { *((b + 0x58) as *const u16) },
            stride: u(0x5c),
        }
    }
}

const TARGET_1D_ARRAY: u32 = 3;
const TARGET_2D_ARRAY: u32 = 4;
const TARGET_2D_MS_ARRAY: u32 = 6;
const TARGET_3D: u32 = 2;
const TARGET_CUBE: u32 = 8;
const TARGET_CUBE_ARRAY: u32 = 9;
const TARGET_BUFFER: u32 = 10;
const FLAG_LINEAR: u32 = 0x10;
const FLAG_LINEAR_RENDER_TARGET: u32 = 0x40;

pub(crate) fn texture_layout(builder: u64) -> Option<(tex::Shape, tex::Layout)> {
    let b = TextureBuilder::read(builder);
    let (bw, bh, bpb) = format_info(b.format);
    if b.target == TARGET_BUFFER || b.flags & (FLAG_LINEAR | FLAG_LINEAR_RENDER_TARGET) != 0 {
        return None;
    }
    let (sx, sy) = match b.samples {
        2 => (2, 1),
        4 => (2, 2),
        8 => (4, 2),
        16 => (4, 4),
        _ => (1, 1),
    };
    let (height, depth, layers) = match b.target {
        TARGET_1D_ARRAY => (1, 1, b.height),
        TARGET_2D_ARRAY | TARGET_2D_MS_ARRAY | TARGET_CUBE_ARRAY => (b.height, 1, b.depth),
        TARGET_CUBE => (b.height, 1, 6),
        TARGET_3D => (b.height, b.depth, 1),
        _ => (b.height, 1, 1),
    };
    let shape = tex::Shape {
        width: b.width * sx,
        height: height * sy,
        depth,
        levels: b.levels.max(1),
        layers: layers.max(1),
        block_w: bw,
        block_h: bh,
        bytes_per_block: bpb,
    };
    let (gy, gz) = if b.packaged & 1 != 0 {
        (((b.packaged >> 4) & 0xF) as u32, ((b.packaged >> 8) & 0xF) as u32)
    } else {
        (
            tex::default_block_log2((shape.height.div_ceil(bh)).div_ceil(tex::GOB_HEIGHT), 4),
            tex::default_block_log2(depth, 4),
        )
    };
    Some((shape, tex::block_linear(&shape, gy, gz)))
}

pub(crate) fn texture_builder_storage(builder: u64) -> (u64, u64) {
    let b = TextureBuilder::read(builder);
    let (_, bh, bpb) = format_info(b.format);
    if b.target == TARGET_BUFFER {
        return (b.width as u64 * bpb as u64, if bpb == 12 { 4 } else { bpb as u64 });
    }
    if b.flags & (FLAG_LINEAR | FLAG_LINEAR_RENDER_TARGET) != 0 {
        let align = if b.flags & FLAG_LINEAR_RENDER_TARGET != 0 { 0x80 } else { 0x20 };
        return (b.stride as u64 * b.height.div_ceil(bh) as u64, align);
    }
    (texture_layout(builder).map_or(0, |(_, l)| l.total_size), 0x200)
}

fn device_get_integer(a: &Args) -> u64 {
    let (prop, out) = (a.0[1] as i32, a.0[2] as *mut i32);
    let v = match prop {
        0 => 53,
        1 => 105,
        40..=42 | 46 | 47 | 50 | 66..=69 | 71..=76 | 87 | 97 => 1,
        48 => 14,
        49 => 5,
        90 => 0,
        56 => 32,
        24 => 0,
        59 => 0,
        23 | 43 | 45 | 55 | 58 | 61..=65 => {
            eprintln!("[nvn] DeviceGetInteger({prop}): hardware property not implemented, returning 0");
            0
        }
        _ => {
            let d = DRIVER.get().unwrap();
            let helper: extern "C" fn(i32, *mut i32) -> i32 = unsafe { std::mem::transmute(d.property_helper as usize) };
            let mut v = 0;
            helper(prop, &mut v);
            v
        }
    };
    unsafe { *out = v };
    0
}

struct Proc {
    name: String,
    calls: AtomicU64,
    host: Option<HostFn>,
}

static PROCS: Mutex<Vec<&'static Proc>> = Mutex::new(Vec::new());
static BY_NAME: Mutex<Option<HashMap<String, u64>>> = Mutex::new(None);
static SEQ: AtomicU64 = AtomicU64::new(0);

fn log_limit() -> u64 {
    static L: OnceLock<u64> = OnceLock::new();
    *L.get_or_init(|| std::env::var("NX_NVN_LOG").ok().and_then(|v| v.parse().ok()).unwrap_or(400))
}

fn call(tag: u64, a: &Args) -> u64 {
    let p = unsafe { &*(tag as *const Proc) };
    let n = p.calls.fetch_add(1, Relaxed);
    if let Some(f) = p.host {
        return f(a);
    }
    let seq = SEQ.fetch_add(1, Relaxed);
    if n == 0 || (seq < log_limit() && n < 4) {
        eprintln!("[nvn] #{seq} STUB {}({:#x}, {:#x}, {:#x}, {:#x})", p.name, a.0[0], a.0[1], a.0[2], a.0[3]);
    }
    if p.name.ends_with("Initialize") { 1 } else { 0 }
}

fn proc_for(name: &str) -> u64 {
    if name == "nvnDeviceGetProcAddress" {
        return device_get_proc_address as *const () as usize as u64;
    }
    if host_impl(name).is_none() && is_driver_passthrough(name) {
        if let Some(&a) = DRIVER.get().and_then(|d| d.procs.get(name)) {
            return a;
        }
    }
    let mut m = BY_NAME.lock().unwrap();
    let m = m.get_or_insert_with(HashMap::new);
    if let Some(&a) = m.get(name) {
        return a;
    }
    let p: &'static Proc = Box::leak(Box::new(Proc { name: name.to_string(), calls: AtomicU64::new(0), host: host_impl(name) }));
    PROCS.lock().unwrap().push(p);
    let addr = stubs::make(call, p as *const Proc as u64);
    m.insert(name.to_string(), addr);
    addr
}

pub extern "C" fn bootstrap_loader(name: *const c_char) -> u64 {
    let n = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    proc_for(&n)
}

extern "C" fn device_get_proc_address(_device: u64, name: *const c_char) -> u64 {
    if name.is_null() {
        return 0;
    }
    let n = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    proc_for(&n)
}

pub fn summary() {
    let procs = PROCS.lock().unwrap();
    let called: Vec<_> = procs.iter().filter(|p| p.calls.load(Relaxed) > 0).collect();
    if called.is_empty() {
        return;
    }
    eprintln!("[nvn] host/stub functions called:");
    for p in called {
        let kind = if p.host.is_some() { "host" } else { "STUB" };
        eprintln!("[nvn]   {:6}  {kind}  {}", p.calls.load(Relaxed), p.name);
    }
}
