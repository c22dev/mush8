// Mush8 - hid.rs
// Constantin Clerc (c22dev) - 0.1
use crate::ipc::{Request, Response, Service, Stub};
use crate::kernel::{KERNEL, Obj};
use std::sync::{Arc, OnceLock};

pub const SHARED_MEMORY_SIZE: u64 = 0x40000;

pub fn shared_memory() -> &'static Arc<crate::kernel::SharedMemory> {
    static SHM: OnceLock<Arc<crate::kernel::SharedMemory>> = OnceLock::new();
    SHM.get_or_init(|| Arc::new(crate::shm::create(SHARED_MEMORY_SIZE).expect("hid shared memory")))
}

const NPAD: u64 = 0x9A00;
const NPAD_SIZE: u64 = 0x5000;
const LIFO_ENTRIES: u64 = 17;
const GENERIC_ENTRY: u64 = 0x30;
const LIFO_FULLKEY: u64 = 0x28;
const LIFO_HANDHELD: u64 = 0x378;
const SIXAXIS_ENTRY: u64 = 0x68;
const SIXAXIS_LIFOS: [u64; 6] = [0x1758, 0x1E60, 0x2568, 0x2C70, 0x3378, 0x3A80];
const DEVICE_TYPE: u64 = 0x4188;
const TOUCH_LIFO: u64 = 0x400;
const TOUCH_ENTRY: u64 = 0x298;
const TOUCH_STATE: u64 = 0x28;
const TOUCH_W: f32 = 1280.0;
const TOUCH_H: f32 = 720.0;

const STYLE_FULLKEY: u32 = 1;
const DEVICE_FULLKEY: u32 = 1;
const ATTR_CONNECTED: u32 = 1;
const ATTR_WIRED: u32 = 2;

#[derive(Default, Clone, Copy)]
pub struct PadState {
    pub buttons: u64,
    pub left: (i32, i32),
    pub right: (i32, i32),
}

pub static BUTTONS: [std::sync::atomic::AtomicU64; crate::input::MAX_PLAYERS] = [const { std::sync::atomic::AtomicU64::new(0) }; crate::input::MAX_PLAYERS];

static SUPPORTED_STYLES: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(u32::MAX);
static HOLD_TYPE: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

static STYLE_EVENTS: std::sync::Mutex<Vec<(usize, u32)>> = std::sync::Mutex::new(Vec::new());

fn npad_index(id: u32) -> Option<usize> {
    match id {
        0..=7 => Some(id as usize),
        0x20 => Some(8),
        0x10 => Some(9),
        _ => None,
    }
}

fn signal_style_events(index: usize) {
    let handles: Vec<u32> = STYLE_EVENTS.lock().unwrap().iter().filter(|(i, _)| *i == index).map(|(_, h)| *h).collect();
    for h in handles {
        let o = KERNEL.lock().get(h);
        if let Some(Obj::ReadableEvent(e)) = o {
            crate::kernel::signal_event(&e);
        }
    }
}

fn write<T: Copy>(addr: u64, v: T) {
    unsafe { (addr as *mut T).write_volatile(v) }
}
fn read<T: Copy>(addr: u64) -> T {
    unsafe { (addr as *const T).read_volatile() }
}

fn lifo_slot(lifo: u64, entry: u64) -> (u64, u64) {
    let tail = (read::<u64>(lifo + 0x10) + 1) % LIFO_ENTRIES;
    (tail, lifo + 0x20 + tail * entry)
}

fn lifo_commit(lifo: u64, sampling: u64, tail: u64) {
    std::sync::atomic::fence(std::sync::atomic::Ordering::Release);
    write(lifo, sampling);
    write(lifo + 0x08, LIFO_ENTRIES);
    write(lifo + 0x10, tail);
    write(lifo + 0x18, (read::<u64>(lifo + 0x18) + 1).min(LIFO_ENTRIES));
}

fn push_generic(lifo: u64, sampling: u64, p: &PadState, attributes: u32) {
    let (tail, e) = lifo_slot(lifo, GENERIC_ENTRY);
    write(e, sampling);
    write(e + 0x08, sampling);
    write(e + 0x10, p.buttons);
    write(e + 0x18, p.left.0);
    write(e + 0x1c, p.left.1);
    write(e + 0x20, p.right.0);
    write(e + 0x24, p.right.1);
    write(e + 0x28, attributes);
    lifo_commit(lifo, sampling, tail);
}

fn push_touch(lifo: u64, sampling: u64, touches: &[(u32, f32, f32, u32)]) {
    let (tail, e) = lifo_slot(lifo, TOUCH_ENTRY);
    let touches = &touches[..touches.len().min(16)];
    write(e, sampling);
    write(e + 0x08, sampling);
    write(e + 0x10, touches.len() as i32);
    write(e + 0x14, 0u32);
    for (i, &(finger, x, y, attr)) in touches.iter().enumerate() {
        let t = e + 0x18 + i as u64 * TOUCH_STATE;
        write(t, 0u64);
        write(t + 0x08, attr);
        write(t + 0x0c, finger);
        write(t + 0x10, (x * TOUCH_W).clamp(0.0, TOUCH_W - 1.0) as u32);
        write(t + 0x14, (y * TOUCH_H).clamp(0.0, TOUCH_H - 1.0) as u32);
        write(t + 0x18, 15u32);
        write(t + 0x1c, 15u32);
        write(t + 0x20, 0u32);
        write(t + 0x24, 0u32);
    }
    lifo_commit(lifo, sampling, tail);
}

static AT_REST: [std::sync::atomic::AtomicBool; crate::input::MAX_PLAYERS] = [const { std::sync::atomic::AtomicBool::new(true) }; crate::input::MAX_PLAYERS];

fn push_sixaxis(lifo: u64, sampling: u64, m: Option<&crate::input::Motion>, fused: &crate::motion::Orientation) {
    let (tail, e) = lifo_slot(lifo, SIXAXIS_ENTRY);
    let (accel, gyro) = m.map_or(([0.0, 0.0, -1.0], [0.0; 3]), |m| (m.accel, fused.gyro(m.gyro.map(|r| r / std::f32::consts::TAU))));
    write(e, sampling);
    write(e + 0x08, 5_000_000u64);
    write(e + 0x10, sampling);
    write(e + 0x18, accel);
    write(e + 0x24, gyro);
    write(e + 0x30, fused.rotation());
    write(e + 0x3c, fused.orientation());
    write(e + 0x60, m.is_some() as u32);
    lifo_commit(lifo, sampling, tail);
}

fn start_input_thread() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let base = shared_memory().host;
        crate::input::SCREEN_ACTIVE.store(true, std::sync::atomic::Ordering::Relaxed);
        std::thread::Builder::new()
            .name("hid".into())
            .spawn(move || {
                let mut pads = crate::input::Pads::new();
                let mut present = [false; crate::input::MAX_PLAYERS];
                let mut sampling = 0u64;
                let mut fused = [crate::motion::Orientation::default(); crate::input::MAX_PLAYERS];
                let mut moving = [false; crate::input::MAX_PLAYERS];
                let mut last = std::time::Instant::now();
                loop {
                    let dt = last.elapsed().as_secs_f32().min(0.1);
                    last = std::time::Instant::now();
                    sampling += 1;
                    let states = pads.poll();
                    for (i, st) in states.iter().enumerate() {
                        let npad = base + NPAD + i as u64 * NPAD_SIZE;
                        if st.is_some() != present[i] {
                            present[i] = st.is_some();
                            let (style, device) = if present[i] { (STYLE_FULLKEY, DEVICE_FULLKEY) } else { (0, 0) };
                            write(npad, style);
                            write(npad + DEVICE_TYPE, device);
                            if !present[i] {
                                push_generic(npad + LIFO_FULLKEY, sampling, &PadState::default(), 0);
                            }
                            signal_style_events(i);
                        }
                        if let Some(p) = st {
                            push_generic(npad + LIFO_FULLKEY, sampling, p, ATTR_CONNECTED | ATTR_WIRED);
                        }
                        let sample = if present[i] { crate::input::MOTION.lock().unwrap()[i] } else { None };
                        let (motion, from_pad) = (sample.map(|(m, _)| m), sample.is_some_and(|(_, pad)| pad));
                        let still = motion.is_none_or(|m| m.gyro.iter().all(|r| r.abs() < 0.05));
                        AT_REST[i].store(still, std::sync::atomic::Ordering::Relaxed);
                        if motion.is_some() && !std::mem::replace(&mut moving[i], true) {
                            eprintln!("[hid] motion data for player {}", i + 1);
                        }
                        match motion {
                            Some(m) => fused[i].update(m.accel, m.gyro.map(|r| r / std::f32::consts::TAU), from_pad, dt),
                            None => fused[i] = crate::motion::Orientation::default(),
                        }
                        if present[i] {
                            for l in SIXAXIS_LIFOS {
                                push_sixaxis(npad + l, sampling, motion.as_ref(), &fused[i]);
                            }
                        }
                        BUTTONS[i].store(st.map_or(0, |p| p.buttons), std::sync::atomic::Ordering::Relaxed);
                    }
                    push_generic(base + NPAD + 8 * NPAD_SIZE + LIFO_HANDHELD, sampling, &PadState::default(), 0);
                    push_touch(base + TOUCH_LIFO, sampling, &crate::input::screen_sample());
                    std::thread::sleep(std::time::Duration::from_millis(5));
                }
            })
            .unwrap();
    });
}

fn vibration_target(handle: u32) -> Option<(usize, usize)> {
    let player = npad_index(handle >> 8 & 0xff).filter(|&i| i < crate::input::MAX_PLAYERS).unwrap_or(0);
    Some((player, (handle >> 16 & 0xff == 1) as usize))
}

fn vibration_value(b: &[u8]) -> crate::haptics::Value {
    let f = |i: usize| b.get(i * 4..i * 4 + 4).map_or(0.0, |b| f32::from_le_bytes(b.try_into().unwrap()));
    crate::haptics::Value { amp_low: f(0), freq_low: f(1), amp_high: f(2), freq_high: f(3) }
}

pub struct Hid;

impl Service for Hid {
    fn name(&self) -> String {
        "hid".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => {
                start_input_thread();
                rsp.objects.push(Arc::new(AppletResource));
            }
            106 => {
                let h = crate::ipc::new_event(true);
                if let Some(i) = npad_index(req.u32(0)) {
                    STYLE_EVENTS.lock().unwrap().push((i, h));
                }
                rsp.copy_handles.push(h);
            }
            66 | 67 | 69 | 70 | 72 | 79 | 81 => {}
            68 => rsp.push_u32(0),
            71 => {
                rsp.push_u32(0);
                rsp.push_u32(0);
            }
            80 => rsp.push_u32(0),
            82 => rsp.push_u32(vibration_target(req.u32(0)).is_none_or(|(p, _)| AT_REST[p].load(std::sync::atomic::Ordering::Relaxed)) as u32),
            100 => SUPPORTED_STYLES.store(req.u32(0), std::sync::atomic::Ordering::Relaxed),
            101 => rsp.push_u32(SUPPORTED_STYLES.load(std::sync::atomic::Ordering::Relaxed)),
            120 => HOLD_TYPE.store(req.u64(8), std::sync::atomic::Ordering::Relaxed),
            121 => rsp.push_u64(HOLD_TYPE.load(std::sync::atomic::Ordering::Relaxed)),
            200 => {
                rsp.push_u32(1);
                rsp.push_u32(match req.u32(0) >> 16 & 0xff {
                    0 => 1,
                    1 => 2,
                    _ => 0,
                });
            }
            201 => {
                if let Some((p, side)) = vibration_target(req.u32(0)) {
                    crate::haptics::send(p, side, vibration_value(&req.data[4.min(req.data.len())..]));
                }
            }
            202 => {
                let v = vibration_target(req.u32(0)).map_or(crate::haptics::Value::IDLE, |(p, side)| crate::haptics::last(p, side));
                for f in [v.amp_low, v.freq_low, v.amp_high, v.freq_high] {
                    rsp.push_u32(f.to_bits());
                }
            }
            203 => rsp.objects.push(Arc::new(crate::ipc::Table { name: "hid::IActiveVibrationDeviceList", cmds: &[(0, crate::ipc::Act::Ok)] })),
            204 | 207 | 209 | 210 => {}
            205 | 211 => rsp.push_u32(1),
            206 => {
                if let (Some(handles), Some(values)) = (req.x.first(), req.x.get(1)) {
                    let n = (handles.size / 4).min(values.size / 16);
                    for i in 0..n {
                        let handle: u32 = read(handles.addr + i * 4);
                        let raw: [u8; 16] = read(values.addr + i * 16);
                        if let Some((p, side)) = vibration_target(handle) {
                            crate::haptics::send(p, side, vibration_value(&raw));
                        }
                    }
                }
            }
            208 => rsp.push_u64(0),
            _ => {
                Stub(self.name()).call(req, rsp);
            }
        }
        0
    }
}

struct AppletResource;

impl Service for AppletResource {
    fn name(&self) -> String {
        "hid::IAppletResource".into()
    }
    fn call(&self, req: &Request, rsp: &mut Response) -> u32 {
        match req.cmd {
            0 => {
                let h = KERNEL.lock().add(Obj::SharedMemory(shared_memory().clone()));
                rsp.copy_handles.push(h);
                0
            }
            _ => Stub(self.name()).call(req, rsp),
        }
    }
}
