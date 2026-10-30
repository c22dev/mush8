// Mush8 - vi.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Act, Request, Response, Service, Stub, Table, new_event, write_buffer};
use std::sync::Arc;

pub fn open(name: &str) -> Arc<dyn Service> {
    Arc::new(Table {
        name: match name {
            "vi:m" => "vi:m",
            "vi:s" => "vi:s",
            _ => "vi:u",
        },
        cmds: &[
            (0, Act::Obj(|| Arc::new(DisplayService))),
            (1, Act::Obj(|| Arc::new(DisplayService))),
            (2, Act::Obj(|| Arc::new(DisplayService))),
        ],
    })
}

pub fn docked() -> bool {
    static D: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("NX_HANDHELD").map_or(true, |v| v == "0"));
    *D
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Output {
    Handheld,
    Full,
    Uhd,
}

pub const UHD_HEAP: u64 = 0xFFE0_0000;
pub fn output() -> Output {
    static O: std::sync::LazyLock<Output> = std::sync::LazyLock::new(|| {
        if !docked() {
            return Output::Handheld;
        }
        if !std::env::var("NX_4K").is_ok_and(|v| v == "1") {
            return Output::Full;
        }
        let heap = crate::kernel::aspace().layout.heap_size.min(crate::gfx::max_buffer_len());
        if heap >= UHD_HEAP {
            return Output::Uhd;
        }
        eprintln!("[vi] 4K needs a {UHD_HEAP:#x}-byte guest heap the GPU can map, this Mac allows {heap:#x}");
        Output::Full
    });
    *O
}

pub fn resolution() -> (u32, u32) {
    match output() {
        Output::Uhd => (3840, 2160),
        Output::Full => (1920, 1080),
        Output::Handheld => (1280, 720),
    }
}

const DISPLAY_ID: u64 = 0;
const LAYER_ID: u64 = 1;

struct DisplayService;

impl Service for DisplayService {
    fn name(&self) -> String {
        "vi::IApplicationDisplayService".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            100 | 103 => rsp.objects.push(Arc::new(super::binder::HosBinderDriver)),
            101 => rsp.objects.push(Arc::new(Table { name: "vi::ISystemDisplayService", cmds: &[] })),
            102 => rsp.objects.push(Arc::new(Table { name: "vi::IManagerDisplayService", cmds: &[(2010, Act::U64(LAYER_ID))] })),
            1000 => {
                if let Some(b) = req.b.first() {
                    let mut info = vec![0u8; 0x60];
                    info[..7].copy_from_slice(b"Default");
                    info[0x40] = 1;
                    info[0x48..0x50].copy_from_slice(&1u64.to_le_bytes());
                    let (w, h) = resolution();
                    info[0x50..0x58].copy_from_slice(&(w as u64).to_le_bytes());
                    info[0x58..0x60].copy_from_slice(&(h as u64).to_le_bytes());
                    write_buffer(b, &info);
                }
                rsp.push_u64(1);
            }
            1010 | 1011 => rsp.push_u64(DISPLAY_ID),
            1020 | 1101 | 2021 | 2031 | 2101 => {}
            1102 => {
                let (w, h) = resolution();
                rsp.push_u64(w as u64);
                rsp.push_u64(h as u64);
            }
            2020 | 2030 => {
                if req.cmd == 2030 {
                    rsp.push_u64(LAYER_ID);
                }
                let parcel = native_window_parcel();
                if let Some(b) = req.b.first() {
                    write_buffer(b, &parcel);
                }
                rsp.push_u64(parcel.len() as u64);
            }
            2102 => rsp.push_u64(req.u64(0)),
            5202 => rsp.copy_handles.push(vsync_event()),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

fn native_window_parcel() -> Vec<u8> {
    let mut data = Vec::new();
    for w in [2u32  , 1  , 1  , 0, 0, 0] {
        data.extend_from_slice(&w.to_le_bytes());
    }
    data.extend_from_slice(b"dispdrv\0");
    data.extend_from_slice(&[0u8; 8]);
    let mut p = Vec::new();
    for w in [data.len() as u32, 0x10, 4, 0x10 + data.len() as u32] {
        p.extend_from_slice(&w.to_le_bytes());
    }
    p.extend(data);
    p.extend_from_slice(&0u32.to_le_bytes());
    p
}

fn vsync_event() -> u32 {
    use std::sync::OnceLock;
    static EVENT: OnceLock<u32> = OnceLock::new();
    *EVENT.get_or_init(|| {
        let h = new_event(false);
        std::thread::Builder::new()
            .name("vsync".into())
            .spawn(move || {
                let period = std::time::Duration::from_nanos(16_666_667);
                let mut next = std::time::Instant::now();
                loop {
                    next += if crate::gfx::turbo() { period / 8 } else { period };
                    let now = std::time::Instant::now();
                    if next > now {
                        std::thread::sleep(next - now);
                    } else if now - next > period * 4 {
                        next = now;
                    }
                    let e = crate::kernel::KERNEL.lock().get(h);
                    if let Some(crate::kernel::Obj::ReadableEvent(e)) = e {
                        crate::kernel::signal_event(&e);
                    }
                }
            })
            .unwrap();
        h
    })
}
