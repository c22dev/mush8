// Mush8 - input.rs
// Constantin Clerc (c22dev) - 0.1
use crate::services::hid::PadState;
use objc2_game_controller::*;

pub const A: u64 = 1 << 0;
pub const B: u64 = 1 << 1;
pub const X: u64 = 1 << 2;
pub const Y: u64 = 1 << 3;
pub const STICK_L: u64 = 1 << 4;
pub const STICK_R: u64 = 1 << 5;
pub const L: u64 = 1 << 6;
pub const R: u64 = 1 << 7;
pub const ZL: u64 = 1 << 8;
pub const ZR: u64 = 1 << 9;
pub const PLUS: u64 = 1 << 10;
pub const MINUS: u64 = 1 << 11;
pub const LEFT: u64 = 1 << 12;
pub const UP: u64 = 1 << 13;
pub const RIGHT: u64 = 1 << 14;
pub const DOWN: u64 = 1 << 15;
const STICK_DIRS: u32 = 16;

const STICK_MAX: f32 = 32767.0;

fn scripted(player: usize) -> u64 {
    static SCRIPT: std::sync::LazyLock<Vec<(f32, u64, usize)>> = std::sync::LazyLock::new(|| {
        let names = [
            ("A", A), ("B", B), ("X", X), ("Y", Y), ("L", L), ("R", R), ("ZL", ZL), ("ZR", ZR), ("PLUS", PLUS),
            ("MINUS", MINUS), ("LSTICK", STICK_L), ("RSTICK", STICK_R), ("LEFT", LEFT), ("UP", UP), ("RIGHT", RIGHT), ("DOWN", DOWN),
        ];
        std::env::var("NX_AUTOINPUT").unwrap_or_default().split(',').filter_map(|e| {
            let (t, b) = e.trim().split_once(':')?;
            let (b, player) = b.split_once('@').map_or(Some((b, 1)), |(b, p)| Some((b, p.parse().ok()?)))?;
            Some((t.parse().ok()?, names.iter().find(|(n, _)| n.eq_ignore_ascii_case(b))?.1, player))
        }).collect()
    });
    let t = crate::gfx::clock();
    SCRIPT.iter().filter(|(at, _, p)| *p == player + 1 && t >= *at && t < at + 0.15).fold(0, |m, (_, b, _)| m | b)
}

pub const MAX_PLAYERS: usize = 8;

#[derive(Clone, Copy, PartialEq, Debug)]
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub enum PadDriver {
    Apple,
    Sdl,
}

impl PadDriver {
    pub fn parse(v: Option<&str>) -> PadDriver {
        if v == Some("sdl") { PadDriver::Sdl } else { PadDriver::Apple }
    }
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
pub fn pad_driver() -> PadDriver {
    static DRIVER: std::sync::LazyLock<PadDriver> = std::sync::LazyLock::new(|| {
        if cfg!(target_os = "macos") { PadDriver::parse(std::env::var("NX_PAD_DRIVER").ok().as_deref()) } else { PadDriver::Apple }
    });
    *DRIVER
}

fn use_sdl() -> bool {
    #[cfg(target_os = "macos")]
    return pad_driver() == PadDriver::Sdl && crate::sdl::init();
    #[cfg(not(target_os = "macos"))]
    false
}

pub enum Ctl {
    Apple(objc2::rc::Retained<GCController>),
    #[cfg(target_os = "macos")]
    Sdl(crate::sdl::Pad),
}

impl Ctl {
    unsafe fn motion(&self) -> Option<Motion> {
        match self {
            Ctl::Apple(c) => unsafe { controller_motion(c) },
            #[cfg(target_os = "macos")]
            Ctl::Sdl(p) => p.motion(),
        }
    }

    unsafe fn name(&self) -> String {
        match self {
            Ctl::Apple(c) => unsafe { c.vendorName() }.map_or("Controller".into(), |n| n.to_string()),
            #[cfg(target_os = "macos")]
            Ctl::Sdl(p) => p.name(),
        }
    }
}

#[derive(Clone, Copy)]
pub struct Motion {
    pub accel: [f32; 3],
    pub gyro: [f32; 3],
}

// each player's latest motion, and whether it comes from a gamepad (true) or a phone (false)
pub static MOTION: std::sync::Mutex<[Option<(Motion, bool)>; MAX_PLAYERS]> = std::sync::Mutex::new([None; MAX_PLAYERS]);

fn motion_enabled() -> bool {
    static ON: std::sync::LazyLock<bool> = std::sync::LazyLock::new(|| std::env::var("NX_MOTION").is_ok_and(|v| v == "1"));
    *ON
}

unsafe fn controller_motion(c: &GCController) -> Option<Motion> {
    unsafe {
        let m = c.motion()?;
        let (g, u, r) = (m.gravity(), m.userAcceleration(), m.rotationRate());
        Some(Motion {
            accel: [(g.x + u.x) as f32, (g.y + u.y) as f32, (g.z + u.z) as f32],
            gyro: [r.x as f32, r.y as f32, r.z as f32],
        })
    }
}

#[cfg(target_os = "ios")]
unsafe fn phone_motion(mgr: &objc2_core_motion::CMMotionManager) -> Option<Motion> {
    unsafe {
        let d = mgr.deviceMotion()?;
        let (g, u, r) = (d.gravity(), d.userAcceleration(), d.rotationRate());
        let v = |x: f64, y: f64, z: f64| {
            if g.x < 0.0 { [y as f32, z as f32, x as f32] } else { [-y as f32, z as f32, -x as f32] }
        };
        Some(Motion { accel: v(g.x + u.x, g.y + u.y, g.z + u.z), gyro: v(r.x, r.y, r.z) })
    }
}

pub static TOUCH_BUTTONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub static TOUCH_STICK: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

#[cfg_attr(not(target_os = "ios"), allow(dead_code))]
pub fn set_touch_stick(x: f32, y: f32) {
    TOUCH_STICK.store((x.to_bits() as u64) << 32 | y.to_bits() as u64, std::sync::atomic::Ordering::Relaxed);
}

pub const TOUCH_START: u32 = 1;
pub const TOUCH_END: u32 = 2;
const MIN_TAP: std::time::Duration = std::time::Duration::from_millis(50);

struct ScreenTouch {
    key: usize,
    finger: u32,
    x: f32,
    y: f32,
    down: bool,
    reported: bool,
    since: std::time::Instant,
}

static SCREEN: std::sync::Mutex<Vec<ScreenTouch>> = std::sync::Mutex::new(Vec::new());
pub static SCREEN_ACTIVE: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn screen_touch(key: usize, pos: Option<(f32, f32)>) {
    if !SCREEN_ACTIVE.load(std::sync::atomic::Ordering::Relaxed) {
        return;
    }
    let mut s = SCREEN.lock().unwrap();
    match (s.iter_mut().find(|t| t.key == key && t.down), pos) {
        (Some(t), Some((x, y))) => (t.x, t.y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)),
        (Some(t), None) => t.down = false,
        (None, Some((x, y))) => {
            if let Some(finger) = (0..16).find(|f| !s.iter().any(|t| t.finger == *f)) {
                let (x, y) = (x.clamp(0.0, 1.0), y.clamp(0.0, 1.0));
                s.push(ScreenTouch { key, finger, x, y, down: true, reported: false, since: std::time::Instant::now() });
            }
        }
        (None, None) => {}
    }
}

pub fn screen_sample() -> Vec<(u32, f32, f32, u32)> {
    let mut s = SCREEN.lock().unwrap();
    let mut out = Vec::with_capacity(s.len());
    s.retain_mut(|t| {
        let attr = if !std::mem::replace(&mut t.reported, true) {
            TOUCH_START
        } else if !t.down && t.since.elapsed() >= MIN_TAP {
            TOUCH_END
        } else {
            0
        };
        out.push((t.finger, t.x, t.y, attr));
        attr != TOUCH_END
    });
    out
}

pub static SLOT_NAMES: std::sync::Mutex<[Option<String>; MAX_PLAYERS]> = std::sync::Mutex::new([const { None }; MAX_PLAYERS]);
pub static FILL_TO: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

pub struct Pads {
    slots: [Option<Ctl>; MAX_PLAYERS],
    phones: [Option<std::net::SocketAddr>; MAX_PLAYERS],
    extra: usize,
    fill: usize,
    touch_init: bool,
    phone: Phone,
    #[cfg(target_os = "macos")]
    sdl_failed: crate::sdl::OpenOnce,
}

impl Pads {
    pub fn new() -> Self {
        let extra = std::env::var("NX_EXTRA_PADS").ok().and_then(|v| v.parse().ok()).unwrap_or(0usize).min(MAX_PLAYERS - 1);
        Self {
            slots: Default::default(),
            phones: Default::default(),
            extra,
            fill: 0,
            touch_init: false,
            phone: Default::default(),
            #[cfg(target_os = "macos")]
            sdl_failed: Default::default(),
        }
    }

    pub fn poll(&mut self) -> [Option<PadState>; MAX_PLAYERS] {
        let had_pad = self.slots[0].is_some();
        let out = self.poll_pads();
        if self.slots[0].is_some() != had_pad || !self.touch_init {
            self.touch_init = true;
            crate::gfx::set_touch_controls(self.slots[0].is_none());
        }
        out
    }

    fn poll_pads(&mut self) -> [Option<PadState>; MAX_PLAYERS] {
        objc2::rc::autoreleasepool(|_| unsafe {
            let sdl = use_sdl();
            let connected: Vec<_> =
                if sdl { Vec::new() } else { GCController::controllers().iter().filter(|c| c.extendedGamepad().is_some()).collect() };
            #[cfg(target_os = "macos")]
            let sdl_ids = if sdl { crate::sdl::connected() } else { Vec::new() };
            #[cfg(target_os = "macos")]
            self.sdl_failed.keep(&sdl_ids);
            let mut changed = false;
            for s in self.slots.iter_mut() {
                let gone = match s {
                    Some(Ctl::Apple(c)) => !connected.iter().any(|n| std::ptr::eq(&**n, &**c)),
                    #[cfg(target_os = "macos")]
                    Some(Ctl::Sdl(p)) => !sdl_ids.contains(&p.id()),
                    None => false,
                };
                if gone {
                    *s = None;
                    changed = true;
                }
            }
            let phones = crate::remote::connected();
            for (i, s) in self.phones.iter_mut().enumerate() {
                if s.is_some_and(|a| !phones.iter().any(|(p, _)| *p == a)) {
                    eprintln!("[input] iPhone controller left player {}", i + 1);
                    *s = None;
                    changed = true;
                }
            }
            let free = |slots: &[Option<_>; MAX_PLAYERS], phones: &[Option<_>; MAX_PLAYERS]| (0..MAX_PLAYERS).find(|&i| slots[i].is_none() && phones[i].is_none());
            for c in connected {
                if self.slots.iter().flatten().any(|s| matches!(s, Ctl::Apple(s) if std::ptr::eq(&**s, &*c))) {
                    continue;
                }
                if let Some(i) = free(&self.slots, &self.phones) {
                    if i < 4 {
                        c.setPlayerIndex(GCControllerPlayerIndex(i as isize));
                    }
                    if motion_enabled() {
                        if let Some(m) = c.motion() {
                            m.setSensorsActive(true);
                        }
                    }
                    eprintln!("[input] {} is player {}", c.vendorName().map_or("gamepad".into(), |n| n.to_string()), i + 1);
                    self.slots[i] = Some(Ctl::Apple(c));
                    changed = true;
                }
            }
            #[cfg(target_os = "macos")]
            for id in sdl_ids {
                if self.slots.iter().flatten().any(|s| matches!(s, Ctl::Sdl(p) if p.id() == id)) {
                    continue;
                }
                let Some(i) = free(&self.slots, &self.phones) else { break };
                let Some(pad) = self.sdl_failed.open(id, || crate::sdl::Pad::open(id)) else { continue };
                if i < 4 {
                    pad.set_player(i);
                }
                if motion_enabled() {
                    pad.enable_motion();
                }
                eprintln!("[input] {} is player {}", pad.name(), i + 1);
                self.slots[i] = Some(Ctl::Sdl(pad));
                changed = true;
            }
            for &(addr, _) in &phones {
                if self.phones.contains(&Some(addr)) {
                    continue;
                }
                let Some(i) = free(&self.slots, &self.phones) else {
                    crate::remote::set_player(addr, 0);
                    continue;
                };
                eprintln!("[input] iPhone controller is player {}", i + 1);
                self.phones[i] = Some(addr);
                crate::remote::set_player(addr, i as u8 + 1);
                changed = true;
            }
            if changed {
                crate::haptics::set_controllers(&self.slots);
            }
            let fill = FILL_TO.load(std::sync::atomic::Ordering::Relaxed);
            changed |= std::mem::replace(&mut self.fill, fill) != fill;
            let mut out: [Option<PadState>; MAX_PLAYERS] = Default::default();
            let mut virtual_left = self.extra;
            let phone = &mut self.phone;
            for (i, slot) in self.slots.iter().enumerate() {
                let mut acc = Acc { buttons: scripted(i), ..Default::default() };
                match slot {
                    Some(Ctl::Apple(c)) => {
                        if let Some(g) = c.extendedGamepad() {
                            acc.gamepad(&g);
                        }
                    }
                    #[cfg(target_os = "macos")]
                    Some(Ctl::Sdl(p)) => acc.sdl(&p.read()),
                    None => {}
                }
                let remote = self.phones[i].and_then(|a| phones.iter().find(|(p, _)| *p == a)).map(|(_, pad)| *pad);
                if let Some(pad) = &remote {
                    acc.remote(pad);
                }
                if i == 0 {
                    acc.keyboard();
                    if slot.is_none() {
                        acc.touch();
                    }
                }
                let filler = i > 0 && i < fill && slot.is_none() && remote.is_none();
                let virtual_pad = filler || (i > 0 && slot.is_none() && virtual_left > 0);
                if virtual_pad && !filler {
                    virtual_left -= 1;
                }
                if filler {
                    out[i] = out[0];
                } else if i == 0 || slot.is_some() || remote.is_some() || virtual_pad {
                    out[i] = Some(acc.state());
                }
                let sample = if !motion_enabled() {
                    None
                } else if let Some(c) = slot {
                    c.motion().map(|m| (m, true))
                } else if let Some(pad) = &remote {
                    pad.motion.map(|m| (m, false))
                } else {
                    phone_sample(phone, i).map(|m| (m, false))
                };
                MOTION.lock().unwrap()[i] = sample;
                if !(changed || !self.touch_init) {
                    continue;
                }
                let name = match slot {
                    Some(c) => Some(c.name()),
                    None if remote.is_some() => Some("iPhone".into()),
                    None if i == 0 && cfg!(target_os = "ios") => Some("Touch controls".into()),
                    None if i == 0 => Some("Keyboard".into()),
                    None if virtual_pad => Some("Virtual controller".into()),
                    None => None,
                };
                SLOT_NAMES.lock().unwrap()[i] = name;
            }
            out
        })
    }
}

#[cfg(target_os = "ios")]
pub type Phone = Option<objc2::rc::Retained<objc2_core_motion::CMMotionManager>>;
#[cfg(not(target_os = "ios"))]
type Phone = ();

#[cfg(target_os = "ios")]
unsafe fn phone_sample(phone: &mut Phone, i: usize) -> Option<Motion> {
    if i != 0 {
        return None;
    }
    unsafe {
        let mgr = phone.get_or_insert_with(|| {
            let m = objc2_core_motion::CMMotionManager::new();
            m.setDeviceMotionUpdateInterval(1.0 / 100.0);
            m.startDeviceMotionUpdates();
            m
        });
        phone_motion(mgr)
    }
}

#[cfg(not(target_os = "ios"))]
unsafe fn phone_sample(_: &mut Phone, _: usize) -> Option<Motion> {
    None
}

#[cfg(target_os = "ios")]
pub fn phone_motion_now(phone: &mut Phone) -> Option<Motion> {
    unsafe { phone_sample(phone, 0) }
}

#[derive(Default)]
struct Acc {
    buttons: u64,
    lx: f32,
    ly: f32,
    rx: f32,
    ry: f32,
}

impl Acc {
    unsafe fn gamepad(&mut self, g: &GCExtendedGamepad) {
        unsafe {
            let pressed = |b: &GCControllerButtonInput| b.isPressed();
            let map = [
                (g.buttonB(), A),
                (g.buttonA(), B),
                (g.buttonY(), X),
                (g.buttonX(), Y),
                (g.leftShoulder(), L),
                (g.rightShoulder(), R),
                (g.leftTrigger(), ZL),
                (g.rightTrigger(), ZR),
                (g.buttonMenu(), PLUS),
            ];
            for (b, bit) in map {
                if pressed(&b) {
                    self.buttons |= bit;
                }
            }
            for (b, bit) in [(g.buttonOptions(), MINUS), (g.leftThumbstickButton(), STICK_L), (g.rightThumbstickButton(), STICK_R)] {
                if b.is_some_and(|b| pressed(&b)) {
                    self.buttons |= bit;
                }
            }
            let d = g.dpad();
            for (b, bit) in [(d.left(), LEFT), (d.up(), UP), (d.right(), RIGHT), (d.down(), DOWN)] {
                if pressed(&b) {
                    self.buttons |= bit;
                }
            }
            let (ls, rs) = (g.leftThumbstick(), g.rightThumbstick());
            self.lx += ls.xAxis().value();
            self.ly += ls.yAxis().value();
            self.rx += rs.xAxis().value();
            self.ry += rs.yAxis().value();
        }
    }

    #[cfg(target_os = "macos")]
    fn sdl(&mut self, r: &crate::sdl::Reading) {
        self.buttons |= r.buttons;
        self.lx += r.sticks[0];
        self.ly += r.sticks[1];
        self.rx += r.sticks[2];
        self.ry += r.sticks[3];
    }

    fn remote(&mut self, pad: &crate::remote::Pad) {
        self.buttons |= pad.buttons;
        self.lx += pad.sticks[0];
        self.ly += pad.sticks[1];
        self.rx += pad.sticks[2];
        self.ry += pad.sticks[3];
    }

    fn touch(&mut self) {
        use std::sync::atomic::Ordering::Relaxed;
        self.buttons |= TOUCH_BUTTONS.load(Relaxed);
        let s = TOUCH_STICK.load(Relaxed);
        self.lx += f32::from_bits((s >> 32) as u32);
        self.ly += f32::from_bits(s as u32);
    }

    unsafe fn keyboard(&mut self) {
        unsafe {
            let Some(k) = GCKeyboard::coalescedKeyboard().and_then(|k| k.keyboardInput()) else { return };
            let down = |code: GCKeyCode| k.buttonForKeyCode(code).is_some_and(|b| b.isPressed());
            let keys = [
                (GCKeyCodeKeyX, A),
                (GCKeyCodeReturnOrEnter, A),
                (GCKeyCodeKeyZ, B),
                (GCKeyCodeDeleteOrBackspace, B),
                (GCKeyCodeKeyC, X),
                (GCKeyCodeKeyV, Y),
                (GCKeyCodeKeyQ, L),
                (GCKeyCodeKeyE, R),
                (GCKeyCodeOne, ZL),
                (GCKeyCodeThree, ZR),
                (GCKeyCodeEqualSign, PLUS),
                (GCKeyCodeTab, PLUS),
                (GCKeyCodeHyphen, MINUS),
                (GCKeyCodeLeftArrow, LEFT),
                (GCKeyCodeUpArrow, UP),
                (GCKeyCodeRightArrow, RIGHT),
                (GCKeyCodeDownArrow, DOWN),
            ];
            for (code, bit) in keys {
                if down(code) {
                    self.buttons |= bit;
                }
            }
            let axis = |neg: GCKeyCode, pos: GCKeyCode| down(pos) as i32 as f32 - down(neg) as i32 as f32;
            self.lx += axis(GCKeyCodeKeyA, GCKeyCodeKeyD);
            self.ly += axis(GCKeyCodeKeyS, GCKeyCodeKeyW);
            self.rx += axis(GCKeyCodeKeyJ, GCKeyCodeKeyL);
            self.ry += axis(GCKeyCodeKeyK, GCKeyCodeKeyI);
        }
    }

    fn state(&self) -> PadState {
        let mut p = PadState { buttons: self.buttons, ..Default::default() };
        let (lx, ly, rx, ry) = (self.lx.clamp(-1.0, 1.0), self.ly.clamp(-1.0, 1.0), self.rx.clamp(-1.0, 1.0), self.ry.clamp(-1.0, 1.0));
        p.left = ((lx * STICK_MAX) as i32, (ly * STICK_MAX) as i32);
        p.right = ((rx * STICK_MAX) as i32, (ry * STICK_MAX) as i32);
        for (i, (x, y)) in [(lx, ly), (rx, ry)].into_iter().enumerate() {
            let base = STICK_DIRS + 4 * i as u32;
            for (k, on) in [x < -0.5, y > 0.5, x > 0.5, y < -0.5].into_iter().enumerate() {
                if on {
                    p.buttons |= 1 << (base + k as u32);
                }
            }
        }
        p
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pad_driver_parse() {
        assert_eq!(PadDriver::parse(None), PadDriver::Apple);
        assert_eq!(PadDriver::parse(Some("apple")), PadDriver::Apple);
        assert_eq!(PadDriver::parse(Some("sdl")), PadDriver::Sdl);
        assert_eq!(PadDriver::parse(Some("SDL")), PadDriver::Apple);
        assert_eq!(PadDriver::parse(Some("")), PadDriver::Apple);
    }
}
