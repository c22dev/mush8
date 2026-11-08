// Mush8 - am.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Act, Request, Response, Service, Stub, Table, new_event, write_buffer};
use crate::kernel::{KERNEL, Obj};
use std::collections::VecDeque;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

const NO_MESSAGES: u32 = 0x680;
const NO_DATA_IN_CHANNEL: u32 = 0x2680;
const NOT_AVAILABLE: u32 = 0x480;
const APPLET_ERROR: u32 = 0x0E;
const CONTROLLER_SUPPORT_CANCELED: u32 = 2;

const FOCUS_STATE_CHANGED: u32 = 15;
const IN_FOCUS: u8 = 1;

pub const USER_ID: [u8; 16] = *b"mk8dx-host-user\0";

pub fn open() -> Arc<dyn Service> {
    Arc::new(Table { name: "appletOE", cmds: &[(0, Act::Obj(|| Arc::new(Table { name: "am::IApplicationProxy", cmds: PROXY })))] })
}

static PROXY: &[(u32, Act)] = &[
    (0, Act::Obj(|| COMMON.clone())),
    (1, Act::Obj(|| Arc::new(Table { name: "am::ISelfController", cmds: SELF_CONTROLLER }))),
    (2, Act::Obj(|| Arc::new(Table { name: "am::IWindowController", cmds: &[(1, Act::U64(0x100)), (10, Act::Ok)] }))),
    (3, Act::Obj(|| Arc::new(Table { name: "am::IAudioController", cmds: AUDIO }))),
    (4, Act::Obj(|| Arc::new(Table { name: "am::IDisplayController", cmds: &[] }))),
    (11, Act::Obj(|| Arc::new(LibraryAppletCreator))),
    (20, Act::Obj(|| Arc::new(ApplicationFunctions))),
    (1000, Act::Obj(|| Arc::new(Table { name: "am::IDebugFunctions", cmds: &[] }))),
];

static SELF_CONTROLLER: &[(u32, Act)] = &[
    (9, Act::Event(true)),
    (40, Act::U64(1)),
    (69, Act::U32(0)),
    (90, Act::U64(0)),
    (91, Act::Event(false)),
];

const ONE_F32: [u8; 4] = 1.0f32.to_le_bytes();
static AUDIO: &[(u32, Act)] = &[(1, Act::Data(&ONE_F32)), (2, Act::Data(&ONE_F32))];

static COMMON: std::sync::LazyLock<Arc<CommonStateGetter>> = std::sync::LazyLock::new(|| {
    Arc::new(CommonStateGetter { queue: Mutex::new(VecDeque::from([FOCUS_STATE_CHANGED])), event: new_event(true) })
});

struct CommonStateGetter {
    queue: Mutex<VecDeque<u32>>,
    event: u32,
}

impl CommonStateGetter {
    fn push(&self, m: u32) {
        self.queue.lock().unwrap().push_back(m);
        signal(self.event);
    }
}

impl Service for CommonStateGetter {
    fn name(&self) -> String {
        "am::ICommonStateGetter".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.copy_handles.push(self.event),
            1 => {
                let mut q = self.queue.lock().unwrap();
                let Some(m) = q.pop_front() else { return NO_MESSAGES };
                if q.is_empty() {
                    if let Some(Obj::ReadableEvent(e)) = KERNEL.lock().get(self.event) {
                        e.signaled.store(false, Relaxed);
                    }
                }
                rsp.push_u32(m);
            }
            5 => rsp.data.push(super::vi::docked() as u8),
            6 => rsp.push_u32(super::vi::docked() as u32),
            8 => rsp.data.push(0),
            9 => rsp.data.push(IN_FOCUS),
            60 => {
                let (w, h) = super::vi::resolution();
                rsp.push_u32(w);
                rsp.push_u32(h);
            }
            61 => rsp.copy_handles.push(new_event(false)),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct ApplicationFunctions;

impl Service for ApplicationFunctions {
    fn name(&self) -> String {
        "am::IApplicationFunctions".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            1 => {
                if req.u32(0) != 2 {
                    return NO_DATA_IN_CHANNEL;
                }
                let mut d = vec![0u8; 0x88];
                d[0..4].copy_from_slice(&0xC794_97CAu32.to_le_bytes());
                d[4..8].copy_from_slice(&1u32.to_le_bytes());
                d[8..24].copy_from_slice(&USER_ID);
                rsp.objects.push(Storage::new(d));
            }
            20 => rsp.push_u64(0),
            21 => rsp.push_u64(super::set::code(super::set::language())),
            22 | 66 | 67 | 90 | 100..=102 | 121 => {}
            23 => {
                let mut v = [0u8; 0x10];
                let version = crate::game::version().as_bytes();
                v[..version.len()].copy_from_slice(version);
                rsp.data.extend_from_slice(&v);
            }
            40 => rsp.data.push(0),
            50 => rsp.data.extend_from_slice(&pseudo_device_id()),
            123 => rsp.push_u32(u32::MAX),
            130 | 140 | 150 | 160 => rsp.copy_handles.push(new_event(false)),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

#[cfg(target_os = "macos")]
fn host_uuid() -> [u8; 16] {
    let mut uuid = [0u8; 16];
    let timeout = libc::timespec { tv_sec: 1, tv_nsec: 0 };
    unsafe { libc::gethostuuid(uuid.as_mut_ptr(), &timeout) };
    uuid
}

#[cfg(not(target_os = "macos"))]
fn host_uuid() -> [u8; 16] {
    const FILE: &str = "work/device_id";
    if let Some(id) = std::fs::read(FILE).ok().and_then(|b| b.try_into().ok()) {
        return id;
    }
    let mut id = [0u8; 16];
    unsafe { libc::arc4random_buf(id.as_mut_ptr().cast(), id.len()) };
    let _ = std::fs::create_dir_all("work");
    let _ = std::fs::write(FILE, id);
    id
}

fn pseudo_device_id() -> [u8; 16] {
    let mut uuid = host_uuid();
    if let Ok(n) = std::env::var("NX_INSTANCE") {
        for (i, b) in n.bytes().enumerate() {
            uuid[i % 16] ^= b.wrapping_mul(0x9D);
        }
    }
    uuid
}

pub struct Storage(pub Arc<Mutex<Vec<u8>>>);

impl Storage {
    pub fn new(data: Vec<u8>) -> Arc<Self> {
        Arc::new(Storage(Arc::new(Mutex::new(data))))
    }
}

impl Service for Storage {
    fn name(&self) -> String {
        "am::IStorage".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.objects.push(Arc::new(StorageAccessor(self.0.clone()))),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
    fn storage_bytes(&self) -> Option<Vec<u8>> {
        Some(self.0.lock().unwrap().clone())
    }
}

struct StorageAccessor(Arc<Mutex<Vec<u8>>>);

impl Service for StorageAccessor {
    fn name(&self) -> String {
        "am::IStorageAccessor".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        let mut data = self.0.lock().unwrap();
        match req.cmd {
            0 => rsp.push_u64(data.len() as u64),
            10 => {
                let off = req.u64(0) as usize;
                if let Some(b) = req.in_buffer() {
                    let src = unsafe { std::slice::from_raw_parts(b.addr as *const u8, b.size as usize) };
                    let end = (off + src.len()).min(data.len());
                    if off < end {
                        data[off..end].copy_from_slice(&src[..end - off]);
                    }
                }
            }
            11 => {
                let off = (req.u64(0) as usize).min(data.len());
                if let Some(b) = req.out_buffer() {
                    write_buffer(&b, &data[off..]);
                }
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

const APPLET_CONTROLLER: u32 = 0x0C;
const APPLET_SWKBD: u32 = 0x11;
const APPLET_MII: u32 = 0x12;
const APPLET_WEB: u32 = 0x13;
const APPLET_OFFLINE_WEB: u32 = 0x17;

struct LibraryAppletCreator;

impl Service for LibraryAppletCreator {
    fn name(&self) -> String {
        "am::ILibraryAppletCreator".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => {
                let id = req.u32(0);
                eprintln!("[am] library applet {id:#x} (mode {})", req.u32(4));
                rsp.objects.push(Arc::new(LibraryApplet {
                    id,
                    input: Mutex::new(Vec::new()),
                    output: Mutex::new(VecDeque::new()),
                    interactive: Mutex::new(VecDeque::new()),
                    swkbd: Mutex::new(None),
                    state_event: new_event(false),
                    out_event: new_event(false),
                    interactive_event: new_event(false),
                }));
            }
            10 => rsp.objects.push(Storage::new(vec![0; req.u64(0).min(1 << 20) as usize])),
            11 => {
                let mem = req.copy_handles.first().and_then(|&h| KERNEL.lock().get(h));
                let data = match mem {
                    Some(Obj::TransferMemory { addr, size }) if addr != 0 => {
                        unsafe { std::slice::from_raw_parts(addr as *const u8, size.min(1 << 20) as usize) }.to_vec()
                    }
                    _ => vec![0; req.u64(8).min(1 << 20) as usize],
                };
                rsp.objects.push(Storage::new(data));
            }
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

struct LibraryApplet {
    id: u32,
    input: Mutex<Vec<Vec<u8>>>,
    output: Mutex<VecDeque<Vec<u8>>>,
    interactive: Mutex<VecDeque<Vec<u8>>>,
    swkbd: Mutex<Option<Swkbd>>,
    state_event: u32,
    out_event: u32,
    interactive_event: u32,
}

fn signal(handle: u32) {
    let o = KERNEL.lock().get(handle);
    if let Some(Obj::ReadableEvent(e)) = o {
        crate::kernel::signal_event(&e);
    }
}

impl LibraryApplet {
    fn run(&self) {
        let input = self.input.lock().unwrap().clone();
        if self.id == APPLET_SWKBD {
            let kb = Swkbd::parse(&input);
            self.keyboard(kb, None);
            return;
        }
        let out = match self.id {
            APPLET_CONTROLLER => Some(controller_applet(&input)),
            APPLET_MII => Some(mii_applet(&input)),
            APPLET_WEB | APPLET_OFFLINE_WEB => Some(web_applet(&input)),
            APPLET_ERROR => {
                for (i, d) in input.iter().enumerate() {
                    let hex: Vec<String> = d.iter().take(0x40).map(|b| format!("{b:02x}")).collect();
                    eprintln!("[am] error applet input {i} ({} bytes): {}", d.len(), hex.join(""));
                }
                None
            }
            _ => {
                eprintln!("[am] library applet {:#x} not implemented: it ends with no output", self.id);
                None
            }
        };
        self.finish(out);
    }

    fn finish(&self, out: Option<Vec<u8>>) {
        if let Some(o) = out {
            self.output.lock().unwrap().push_back(o);
            signal(self.out_event);
        }
        signal(self.state_event);
        COMMON.push(FOCUS_STATE_CHANGED);
    }

    fn keyboard(&self, kb: Swkbd, error: Option<String>) {
        let text = kb.ask(error);
        if kb.check && let Some(t) = &text {
            self.interactive.lock().unwrap().push_back(kb.check_request(t));
            *self.swkbd.lock().unwrap() = Some(Swkbd { last: t.clone(), ..kb });
            signal(self.interactive_event);
            return;
        }
        self.finish(Some(kb.output(text.as_deref())));
    }

    fn checked(&self, reply: &[u8]) {
        let Some(kb) = self.swkbd.lock().unwrap().take() else { return };
        let result = reply.get(..4).map_or(0, |b| u32::from_le_bytes(b.try_into().unwrap()));
        if result == SWKBD_CHECK_OK || result == SWKBD_CHECK_CONFIRM {
            let last = kb.last.clone();
            self.finish(Some(kb.output(Some(&last))));
            return;
        }
        let msg = utf16(reply.get(4..).unwrap_or(&[]));
        eprintln!("[am] keyboard: the game rejected {:?} ({result}): {msg:?}", kb.last);
        let error = (result == SWKBD_CHECK_FAILURE && !msg.is_empty()).then_some(msg);
        let kb = Swkbd { initial: kb.last.clone(), ..kb };
        crate::kernel::blocking(|| self.keyboard(kb, error));
    }
}

const SWKBD_CONFIG_SIZE: usize = 0x3D4;
const SWKBD_OUTPUT_SIZE: usize = 0x7D8;
const SWKBD_CHECK_SIZE: usize = 0x7D4;
const SWKBD_CHECK_OK: u32 = 0;
const SWKBD_CHECK_FAILURE: u32 = 1;
const SWKBD_CHECK_CONFIRM: u32 = 2;
const SWKBD_NUMERIC: u32 = 1;

#[derive(Clone, Debug, Default)]
struct Swkbd {
    numeric: bool,
    ok: String,
    header: String,
    sub: String,
    guide: String,
    max: usize,
    min: usize,
    password: bool,
    utf8: bool,
    check: bool,
    initial: String,
    last: String,
}

fn utf16(d: &[u8]) -> String {
    let units: Vec<u16> = d.chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).take_while(|&c| c != 0).collect();
    String::from_utf16_lossy(&units).chars().filter(|c| !c.is_control()).collect()
}

impl Swkbd {
    fn parse(input: &[Vec<u8>]) -> Swkbd {
        let Some(c) = input.get(1).filter(|c| c.len() >= SWKBD_CONFIG_SIZE) else {
            eprintln!("[am] keyboard: config of {} bytes is not a normal keyboard", input.get(1).map_or(0, |c| c.len()));
            return Swkbd { max: 100, ok: "OK".into(), ..Default::default() };
        };
        let u32_at = |o: usize| u32::from_le_bytes(c[o..o + 4].try_into().unwrap());
        let s = |o: usize, n: usize| utf16(&c[o..o + 2 * n]);
        let mut kb = Swkbd {
            numeric: u32_at(0) == SWKBD_NUMERIC,
            ok: Some(s(0x4, 9)).filter(|t| !t.trim().is_empty()).unwrap_or_else(|| "OK".into()),
            header: s(0x24, 65),
            sub: s(0xA6, 129),
            guide: s(0x1A8, 257),
            max: u32_at(0x3AC) as usize,
            min: u32_at(0x3B0) as usize,
            password: u32_at(0x3B4) != 0,
            utf8: c[0x3BD] != 0,
            check: c[0x3D0] != 0,
            ..Default::default()
        };
        if kb.max == 0 {
            kb.max = 100;
        }
        let (off, len) = (u32_at(0x3C0) as usize, u32_at(0x3C4) as usize);
        if let Some(w) = input.get(2).and_then(|w| w.get(off..off + 2 * len)) {
            kb.initial = utf16(w);
        }
        eprintln!("[am] keyboard: {:?} / {:?} / {:?}, {}..={} chars{}{}", kb.header, kb.sub, kb.guide, kb.min, kb.max, if kb.numeric { ", numbers" } else { "" }, if kb.check { ", checked by the game" } else { "" });
        kb
    }

    fn ask(&self, mut error: Option<String>) -> Option<String> {
        if std::env::var_os("NX_AUTOINPUT").is_some() {
            return Some(self.initial.clone());
        }
        loop {
            let sub = error.take().map_or(self.sub.clone(), |e| if self.sub.is_empty() { e } else { format!("{e}\n\n{}", self.sub) });
            let p = crate::gfx::keyboard::Prompt {
                header: self.header.clone(),
                sub,
                guide: self.guide.clone(),
                ok: self.ok.clone(),
                initial: self.initial.clone(),
                password: self.password,
                numeric: self.numeric,
            };
            let text = crate::kernel::blocking(|| crate::gfx::keyboard::prompt(p))?;
            let text: String = text.chars().filter(|c| !c.is_control()).take(self.max).collect();
            if self.numeric && !text.chars().all(|c| c.is_ascii_digit()) {
                error = Some("Only numbers are allowed.".into());
            } else if text.chars().count() < self.min {
                error = Some(format!("Enter at least {} characters.", self.min));
            } else {
                return Some(text);
            }
        }
    }

    fn encode(&self, t: &str) -> Vec<u8> {
        if self.utf8 { t.as_bytes().to_vec() } else { t.encode_utf16().flat_map(u16::to_le_bytes).collect() }
    }

    fn output(&self, text: Option<&str>) -> Vec<u8> {
        let mut out = vec![0u8; SWKBD_OUTPUT_SIZE];
        out[..4].copy_from_slice(&(text.is_none() as u32).to_le_bytes());
        let t = self.encode(text.unwrap_or_default());
        let n = t.len().min(SWKBD_OUTPUT_SIZE - 6);
        out[4..4 + n].copy_from_slice(&t[..n]);
        eprintln!("[am] keyboard: {}", text.map_or("canceled".into(), |t| format!("entered {t:?}")));
        out
    }

    fn check_request(&self, text: &str) -> Vec<u8> {
        let mut out = vec![0u8; SWKBD_CHECK_SIZE];
        let t = self.encode(text);
        let n = t.len().min(SWKBD_CHECK_SIZE - 10);
        out[..8].copy_from_slice(&(n as u64 + 8).to_le_bytes());
        out[8..8 + n].copy_from_slice(&t[..n]);
        out
    }
}

impl Service for LibraryApplet {
    fn name(&self) -> String {
        format!("am::ILibraryAppletAccessor({:#x})", self.id)
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => rsp.copy_handles.push(self.state_event),
            1 => rsp.data.push(1),
            10 => self.run(),
            20 | 25 | 30 | 50 | 60 | 150 => {}
            100 => {
                if let Some(d) = req.in_objects.first().and_then(|o| o.storage_bytes()) {
                    self.input.lock().unwrap().push(d);
                }
            }
            103 => {
                if let Some(d) = req.in_objects.first().and_then(|o| o.storage_bytes()) {
                    self.checked(&d);
                }
            }
            101 => {
                let Some(d) = self.output.lock().unwrap().pop_front() else { return NOT_AVAILABLE };
                rsp.objects.push(Storage::new(d));
            }
            104 => {
                let Some(d) = self.interactive.lock().unwrap().pop_front() else { return NOT_AVAILABLE };
                rsp.objects.push(Storage::new(d));
            }
            105 => rsp.copy_handles.push(self.out_event),
            106 => rsp.copy_handles.push(self.interactive_event),
            110 => rsp.data.push(0),
            _ => return Stub(self.name()).call(req, rsp),
        }
        0
    }
}

fn mii_applet(input: &[Vec<u8>]) -> Vec<u8> {
    let mode = input.iter().skip(1).find_map(|d| d.get(4..8))
        .map(|d| u32::from_le_bytes(d.try_into().unwrap()));
    let char_info = matches!(mode, Some(4 | 5));
    let mut out = vec![0u8; if char_info { 0x80 } else { 0x20 }];
    out[..4].copy_from_slice(&1u32.to_le_bytes());
    if !char_info {
        out[4..8].copy_from_slice(&(-1i32).to_le_bytes());
    }
    eprintln!("[am] Mii editor unavailable: returning canceled (mode {mode:?})");
    out
}

fn controller_applet(input: &[Vec<u8>]) -> Vec<u8> {
    let arg = input.get(2);
    let min = arg.and_then(|a| a.first()).map_or(1, |&v| (v as i8).max(1)) as usize;
    let max = arg.and_then(|a| a.get(1)).map_or(4, |&v| (v as i8).clamp(1, 8)) as usize;
    crate::input::FILL_TO.store(0, Relaxed);
    let mut out = vec![0u8; 0xC];
    let Some(connected) = crate::kernel::blocking(|| crate::gfx::overlay::controller_screen(min, max)) else {
        eprintln!("[am] controller applet: canceled");
        out[8..12].copy_from_slice(&CONTROLLER_SUPPORT_CANCELED.to_le_bytes());
        return out;
    };
    let players = connected.clamp(min, max.max(min));
    eprintln!("[am] controller applet: {connected} controllers connected, the game asks {min}..={max} players: {players}");
    if connected < players {
        eprintln!("[am] controller applet: players {}..={players} copy player 1", connected + 1);
        crate::input::FILL_TO.store(players, Relaxed);
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    out[0] = players as u8;
    out
}

const HELP_URL: &str = "https://github.com/c22dev/Mush8";

fn web_applet(input: &[Vec<u8>]) -> Vec<u8> {
    let mut url = String::new();
    if let Some(d) = input.first() {
        let mut off = 8;
        while off + 8 <= d.len() {
            let kind = u16::from_le_bytes([d[off], d[off + 1]]);
            let size = u16::from_le_bytes([d[off + 2], d[off + 3]]) as usize;
            let data = &d[off + 8..(off + 8 + size).min(d.len())];
            if kind == 1 {
                url = String::from_utf8_lossy(&data[..data.iter().position(|&c| c == 0).unwrap_or(data.len())]).into_owned();
            }
            off += 8 + size;
        }
    }
    eprintln!("[am] web applet {url:?}: opening {HELP_URL} instead");
    crate::gfx::open_url(HELP_URL);
    let mut out = vec![0u8; 0x1010];
    out[0..4].copy_from_slice(&1u32.to_le_bytes());
    let n = url.len().min(0xFFF);
    out[8..8 + n].copy_from_slice(&url.as_bytes()[..n]);
    out[0x1008..0x1010].copy_from_slice(&(n as u64).to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unsupported_editor_returns_a_complete_canceled_output() {
        for (mode, input_size, size) in [(1u32, 0x100, 0x20), (4, 0x100, 0x80), (5, 0x100, 0x80), (4, 0x88, 0x80)] {
            let mut arg = vec![0u8; input_size];
            arg[4..8].copy_from_slice(&mode.to_le_bytes());
            let out = mii_applet(&[vec![0; 0x20], arg]);
            assert_eq!(out.len(), size);
            assert_eq!(&out[..4], &1u32.to_le_bytes());
        }
    }

    #[test]
    fn keyboard_reads_the_config_and_writes_utf16_text() {
        let mut c = vec![0u8; 0x3E0];
        c[0..4].copy_from_slice(&SWKBD_NUMERIC.to_le_bytes());
        for (i, u) in "Code".encode_utf16().enumerate() {
            c[0x24 + 2 * i..0x26 + 2 * i].copy_from_slice(&u.to_le_bytes());
        }
        c[0x3AC..0x3B0].copy_from_slice(&12u32.to_le_bytes());
        let kb = Swkbd::parse(&[vec![0; 0x20], c]);
        assert!(kb.numeric && !kb.check);
        assert_eq!((kb.header.as_str(), kb.max, kb.ok.as_str()), ("Code", 12, "OK"));
        let out = kb.output(Some("12"));
        assert_eq!(out.len(), SWKBD_OUTPUT_SIZE);
        assert_eq!(&out[..10], &[0, 0, 0, 0, b'1', 0, b'2', 0, 0, 0]);
        assert_eq!(&kb.output(None)[..4], &1u32.to_le_bytes());
        assert_eq!(&kb.check_request("12")[..12], &[12, 0, 0, 0, 0, 0, 0, 0, b'1', 0, b'2', 0]);
    }

    #[test]
    fn canceled_output_has_no_valid_index() {
        let out = mii_applet(&[]);
        assert_eq!(out.len(), 0x20);
        assert_eq!(&out[4..8], &(-1i32).to_le_bytes());
    }
}
