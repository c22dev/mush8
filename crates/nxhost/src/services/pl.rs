// Mush8 - pl.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub, write_buffer};
use crate::kernel::{KERNEL, Obj};
use std::sync::{Arc, OnceLock};

const SHARED_MEMORY_SIZE: u64 = 0x110_0000;
const BFTTF_KEY: [u8; 4] = [0x49, 0x62, 0x18, 0x06];

const FONTS: [(u64, &str); 6] = [
    (0x0100000000000811, "/nintendo_udsg-r_std_003.bfttf"),
    (0x0100000000000814, "/nintendo_udsg-r_org_zh-cn_003.bfttf"),
    (0x0100000000000814, "/nintendo_udsg-r_ext_zh-cn_003.bfttf"),
    (0x0100000000000813, "/nintendo_udjxh-db_zh-tw_003.bfttf"),
    (0x0100000000000812, "/nintendo_udsg-r_ko_003.bfttf"),
    (0x0100000000000810, "/nintendo_ext_003.bfttf"),
];

struct Fonts {
    shm: Arc<crate::kernel::SharedMemory>,
    entries: Vec<(u32, u32)>,
}

fn fonts() -> &'static Fonts {
    static F: OnceLock<Fonts> = OnceLock::new();
    F.get_or_init(|| {
        let shm = Arc::new(crate::shm::create(SHARED_MEMORY_SIZE).expect("font shared memory"));
        let cfg = super::fs::CONFIG.get().expect("fs not configured");
        let mut entries = Vec::new();
        let mut pos = 0u64;
        // Explicit local SFNT fallback; never download or synthesize firmware fonts.
        let fallback = std::env::var_os("NX_FALLBACK_FONT").and_then(|p| std::fs::read(p).ok());
        let mut fallback_entry = None;
        for (id, file) in FONTS {
            let data = super::fs::system_data(cfg, id).and_then(|s| {
                let fs = nxfs::romfs::RomFs::open(s).ok()?;
                let f = fs.files().into_iter().find(|f| f.path == file)?;
                fs.storage().read_vec(f.offset, f.size as usize).ok()
            });
            let Some(data) = data.filter(|d| d.len() > 8 && pos + d.len() as u64 <= SHARED_MEMORY_SIZE) else {
                if let Some(data) = fallback.as_ref().filter(|d| d.starts_with(&[0,1,0,0]) || d.starts_with(b"OTTO")) {
                    let entry = *fallback_entry.get_or_insert_with(|| {
                        assert!(pos + data.len() as u64 <= SHARED_MEMORY_SIZE, "local font exceeds shared memory");
                        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), (shm.host + pos) as *mut u8, data.len()) };
                        let entry = (pos as u32, data.len() as u32);
                        pos += data.len() as u64;
                        entry
                    });
                    eprintln!("[pl] {file}: local host font fallback ({} bytes)", entry.1);
                    entries.push(entry);
                    continue;
                }
                eprintln!("[pl] font {file} unavailable");
                entries.push((0, 0));
                continue;
            };
            let mut data = data;
            for (i, b) in data[8..].iter_mut().enumerate() {
                *b ^= BFTTF_KEY[i % 4];
            }
            unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), (shm.host + pos) as *mut u8, data.len()) };
            entries.push(((pos + 8) as u32, (data.len() - 8) as u32));
            pos += data.len() as u64;
        }
        Fonts { shm, entries }
    })
}

pub struct Pl;

impl Service for Pl {
    fn name(&self) -> String {
        "pl:u".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let f = fonts();
        let ty = req.u32(0) as usize;
        match req.cmd {
            0 => {}
            1 => rsp.push_u32(1),
            2 => rsp.push_u32(f.entries.get(ty).map_or(0, |e| e.1)),
            3 => rsp.push_u32(f.entries.get(ty).map_or(0, |e| e.0)),
            4 => rsp.copy_handles.push(KERNEL.lock().add(Obj::SharedMemory(f.shm.clone()))),
            5 => {
                let n = f.entries.len();
                let words = |v: Vec<u32>| v.iter().flat_map(|x| x.to_le_bytes()).collect::<Vec<u8>>();
                let bufs: Vec<_> = req.b.iter().chain(&req.c).filter(|b| b.addr != 0).copied().collect();
                let lists = [
                    words((0..n as u32).collect()),
                    words(f.entries.iter().map(|e| e.0).collect()),
                    words(f.entries.iter().map(|e| e.1).collect()),
                ];
                for (b, l) in bufs.iter().zip(lists.iter()) {
                    write_buffer(b, l);
                }
                rsp.data.push(1);
                rsp.data.extend_from_slice(&[0, 0, 0]);
                rsp.push_u32(n as u32);
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}
