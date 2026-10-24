// Mush8 - haptics.rs
// Constantin Clerc (c22dev) - 0.1
use crate::input::MAX_PLAYERS;
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2::{AnyThread, msg_send};
use objc2_core_haptics::*;
use objc2_foundation::NSArray;
use objc2_game_controller::{GCController, GCHapticsLocalityDefault};
use std::sync::{Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Value {
    pub amp_low: f32,
    pub freq_low: f32,
    pub amp_high: f32,
    pub freq_high: f32,
}

impl Value {
    pub const IDLE: Value = Value { amp_low: 0.0, freq_low: 160.0, amp_high: 0.0, freq_high: 320.0 };

    fn feel(&self) -> (f32, f32) {
        let (lo, hi) = (self.amp_low.clamp(0.0, 1.0), self.amp_high.clamp(0.0, 1.0));
        if lo + hi <= 0.0 || lo.is_nan() || hi.is_nan() {
            return (0.0, 0.0);
        }
        let freq = (lo * self.freq_low + hi * self.freq_high) / (lo + hi);
        (lo.max(hi * 0.12).powf(0.6), ((freq - 80.0) / 320.0).clamp(0.0, 1.0))
    }
}

struct Ctl(Retained<GCController>);
unsafe impl Send for Ctl {}

enum Target {
    Apple(Ctl),
    #[cfg(target_os = "macos")]
    Sdl(sdl3_sys::everything::SDL_JoystickID),
}

#[cfg(target_os = "macos")]
const RUMBLE_MS: u32 = 2000;
#[cfg(target_os = "macos")]
const RESEND: Duration = Duration::from_millis(500);

#[cfg(target_os = "macos")]
// Like the yuzu-family SDL driver (citron sdl_driver.cpp RumblePlay): the low band drives the
// low-frequency motor and the high band the high-frequency one, weaker as the frequency rises.
// The strength stays linear, without the emulator's boost of weak values.
fn sdl_strength(v: [Value; 2]) -> (u16, u16) {
    let band = |amp: fn(&Value) -> (f32, f32), start: f32, width: f32| {
        let s = v.iter().map(amp).map(|(a, f)| {
            let a = if a.is_nan() { 0.0 } else { a.clamp(0.0, 1.0) };
            a * (1.0 - (f - start).max(0.0) / width).max(0.3)
        });
        (s.fold(0.0, f32::max) * 65535.0).round() as u16
    };
    (band(|v| (v.amp_low, v.freq_low), 140.0, 400.0), band(|v| (v.amp_high, v.freq_high), 200.0, 700.0))
}

#[cfg(target_os = "macos")]
fn resend_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|t| now.duration_since(t) >= RESEND)
}

// Whether to send `strength` now, and the new state: the strength in force and when it was last sent.
// The state is kept even when a send fails, so a held rumble is retried at the next resend.
#[cfg(target_os = "macos")]
type SdlState = Option<((u16, u16), Instant)>;

#[cfg(target_os = "macos")]
fn sdl_step(state: SdlState, strength: (u16, u16), now: Instant) -> (bool, SdlState) {
    let sent = state.map_or((0, 0), |(v, _)| v);
    let send = strength != sent || (strength != (0, 0) && resend_due(state.map(|(_, t)| t), now));
    let next = if !send { state } else { (strength != (0, 0)).then_some((strength, now)) };
    (send, next)
}

struct Shared {
    values: [[Value; 2]; MAX_PLAYERS],
    controllers: [Option<Target>; MAX_PLAYERS],
    dirty: bool,
}

static SHARED: Mutex<Shared> =
    Mutex::new(Shared { values: [[Value::IDLE; 2]; MAX_PLAYERS], controllers: [const { None }; MAX_PLAYERS], dirty: false });
static WAKE: Condvar = Condvar::new();

fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("NX_RUMBLE").as_deref() != Ok("0"))
}

pub fn send(player: usize, side: usize, v: Value) {
    let mut s = SHARED.lock().unwrap();
    if s.values[player][side] == v {
        return;
    }
    s.values[player][side] = v;
    if enabled() {
        s.dirty = true;
        drop(s);
        start();
        WAKE.notify_one();
    }
}

pub fn last(player: usize, side: usize) -> Value {
    SHARED.lock().unwrap().values[player][side]
}

pub fn set_controllers(slots: &[Option<crate::input::Ctl>; MAX_PLAYERS]) {
    let mut s = SHARED.lock().unwrap();
    s.controllers = std::array::from_fn(|i| match &slots[i] {
        Some(crate::input::Ctl::Apple(c)) => Some(Target::Apple(Ctl(c.clone()))),
        #[cfg(target_os = "macos")]
        Some(crate::input::Ctl::Sdl(p)) => Some(Target::Sdl(p.id())),
        None => None,
    });
    s.dirty = true;
    WAKE.notify_one();
}

fn start() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        std::thread::Builder::new().name("haptics".into()).spawn(run).unwrap();
    });
}

#[derive(Clone, Copy, PartialEq)]
enum Source {
    None,
    Controller(usize),
    #[cfg(target_os = "macos")]
    Sdl(u32),
    #[cfg(target_os = "ios")]
    Device,
}

struct Engine {
    _engine: Retained<CHHapticEngine>,
    player: Retained<ProtocolObject<dyn CHHapticPatternPlayer>>,
    started: Option<Instant>,
    sent: (f32, f32),
}

impl Engine {
    unsafe fn new(ctl: Option<&GCController>) -> Option<Engine> {
        unsafe {
            let engine: Retained<CHHapticEngine> = match ctl {
                Some(c) => msg_send![&*c.haptics()?, createEngineWithLocality: GCHapticsLocalityDefault],
                None => {
                    if !CHHapticEngine::capabilitiesForHardware().supportsHaptics() {
                        return None;
                    }
                    let e = CHHapticEngine::initAndReturnError(CHHapticEngine::alloc()).ok()?;
                    e.setPlaysHapticsOnly(true);
                    e
                }
            };
            engine.startAndReturnError().ok()?;
            let param = |id, v| CHHapticEventParameter::initWithParameterID_value(CHHapticEventParameter::alloc(), id, v);
            let params = NSArray::from_retained_slice(&[
                param(CHHapticEventParameterIDHapticIntensity, 1.0),
                param(CHHapticEventParameterIDHapticSharpness, 0.5),
            ]);
            let event = CHHapticEvent::initWithEventType_parameters_relativeTime_duration(
                CHHapticEvent::alloc(),
                CHHapticEventTypeHapticContinuous,
                &params,
                0.0,
                30.0,
            );
            let pattern =
                CHHapticPattern::initWithEvents_parameters_error(CHHapticPattern::alloc(), &NSArray::from_retained_slice(&[event]), &NSArray::new()).ok()?;
            let player = engine.createPlayerWithPattern_error(&pattern).ok()?;
            Some(Engine { _engine: engine, player, started: None, sent: (-1.0, -1.0) })
        }
    }

    unsafe fn apply(&mut self, (intensity, sharpness): (f32, f32)) -> Option<()> {
        unsafe {
            if intensity < 0.02 {
                if self.started.take().is_some() {
                    self.player.stopAtTime_error(CHHapticTimeImmediate).ok()?;
                }
                return Some(());
            }
            if self.started.is_some_and(|t| t.elapsed() > Duration::from_secs(28)) {
                self.player.stopAtTime_error(CHHapticTimeImmediate).ok()?;
                self.started = None;
            }
            if (intensity - self.sent.0).abs() > 0.02 || (sharpness - self.sent.1).abs() > 0.02 || self.started.is_none() {
                let dyn_param = |id, v| CHHapticDynamicParameter::initWithParameterID_value_relativeTime(CHHapticDynamicParameter::alloc(), id, v, 0.0);
                let params = NSArray::from_retained_slice(&[
                    dyn_param(CHHapticDynamicParameterIDHapticIntensityControl, intensity),
                    dyn_param(CHHapticDynamicParameterIDHapticSharpnessControl, sharpness - 0.5),
                ]);
                if self.started.is_none() {
                    self.player.startAtTime_error(CHHapticTimeImmediate).ok()?;
                    self.started = Some(Instant::now());
                }
                self.player.sendParameters_atTime_error(&params, CHHapticTimeImmediate).ok()?;
                self.sent = (intensity, sharpness);
            }
            Some(())
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        unsafe {
            let _ = self.player.stopAtTime_error(CHHapticTimeImmediate);
        }
    }
}

struct Out {
    source: Source,
    engine: Option<Engine>,
    retry: Option<Instant>,
    sdl: Option<((u16, u16), Instant)>,
}

fn run() {
    let mut outs: [Out; MAX_PLAYERS] = std::array::from_fn(|_| Out { source: Source::None, engine: None, retry: None, sdl: None });
    loop {
        let (values, controllers, sdl_ids) = {
            let mut s = SHARED.lock().unwrap();
            while !s.dirty {
                let active = outs.iter().any(|o| o.engine.as_ref().is_some_and(|e| e.started.is_some()) || o.sdl.is_some());
                s = WAKE.wait_timeout(s, Duration::from_millis(if active { 500 } else { 5000 })).unwrap().0;
                if active {
                    break;
                }
            }
            s.dirty = false;
            let ctls: [Option<Retained<GCController>>; MAX_PLAYERS] = std::array::from_fn(|i| match &s.controllers[i] {
                Some(Target::Apple(c)) => Some(c.0.clone()),
                _ => None,
            });
            let sdl_ids: [Option<u32>; MAX_PLAYERS] = std::array::from_fn(|i| match &s.controllers[i] {
                #[cfg(target_os = "macos")]
                Some(Target::Sdl(id)) => Some(id.0),
                _ => None,
            });
            (s.values, ctls, sdl_ids)
        };
        objc2::rc::autoreleasepool(|_| {
            for (i, out) in outs.iter_mut().enumerate() {
                let source = match (&controllers[i], sdl_ids[i]) {
                    #[cfg(target_os = "macos")]
                    (_, Some(id)) => Source::Sdl(id),
                    (Some(c), _) => Source::Controller(Retained::as_ptr(c) as usize),
                    #[cfg(target_os = "ios")]
                    (None, _) if i == 0 => Source::Device,
                    _ => Source::None,
                };
                if source != out.source {
                    #[cfg(target_os = "macos")]
                    if let (Source::Sdl(old), Some(_)) = (out.source, out.sdl.take()) {
                        crate::sdl::rumble(sdl3_sys::everything::SDL_JoystickID(old), 0, 0, 0);
                    }
                    out.source = source;
                    out.engine = None;
                    out.retry = None;
                }
                #[cfg(target_os = "macos")]
                if let Source::Sdl(id) = out.source {
                    let strength = sdl_strength(values[i]);
                    let (send, next) = sdl_step(out.sdl, strength, Instant::now());
                    if send {
                        let ms = if strength == (0, 0) { 0 } else { RUMBLE_MS };
                        crate::sdl::rumble(sdl3_sys::everything::SDL_JoystickID(id), strength.0, strength.1, ms);
                    }
                    out.sdl = next;
                    continue;
                }
                let (l, r) = (values[i][0].feel(), values[i][1].feel());
                let feel = if l.0 >= r.0 { l } else { r };
                if out.source == Source::None || (out.engine.is_none() && feel.0 < 0.02) {
                    continue;
                }
                if out.engine.is_none() && out.retry.is_none_or(|t| t <= Instant::now()) {
                    out.engine = unsafe { Engine::new(controllers[i].as_deref()) };
                    if out.engine.is_none() {
                        out.retry = Some(Instant::now() + Duration::from_secs(2));
                    }
                }
                if let Some(e) = &mut out.engine
                    && unsafe { e.apply(feel) }.is_none()
                {
                    out.engine = None;
                    out.retry = Some(Instant::now() + Duration::from_millis(500));
                }
            }
        });
        std::thread::sleep(Duration::from_millis(8));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sdl_strength_follows_bands() {
        let v = |amp_low, freq_low, amp_high, freq_high| Value { amp_low, freq_low, amp_high, freq_high };
        assert_eq!(sdl_strength([Value::IDLE, Value::IDLE]), (0, 0));
        // low band drives the low-frequency motor, high band the high-frequency one, linearly
        assert_eq!(sdl_strength([v(1.0, 140.0, 0.0, 200.0), Value::IDLE]), (65535, 0));
        assert_eq!(sdl_strength([Value::IDLE, v(0.0, 140.0, 1.0, 200.0)]), (0, 65535));
        assert_eq!(sdl_strength([v(0.25, 140.0, 0.0, 200.0), Value::IDLE]), (16384, 0));
        // both sides share one pad: the stronger side wins per band
        assert_eq!(sdl_strength([v(0.5, 140.0, 0.0, 200.0), v(0.25, 140.0, 0.0, 200.0)]), (32768, 0));
        // higher frequencies feel weaker, down to 30%
        assert_eq!(sdl_strength([v(1.0, 540.0, 1.0, 900.0), Value::IDLE]), (19661, 19661));
        assert_eq!(sdl_strength([v(1.0, 340.0, 0.0, 200.0), Value::IDLE]), (32768, 0));
        let nan = Value { amp_low: f32::NAN, ..Value::IDLE };
        assert_eq!(sdl_strength([nan, nan]), (0, 0));
    }

    #[test]
    fn rumble_outlasts_a_late_resend() {
        // a wake just before a resend can push the next one to almost two resend periods
        assert!(Duration::from_millis(RUMBLE_MS as u64) > RESEND * 2 + Duration::from_millis(500));
    }

    #[test]
    fn sdl_step_keeps_retrying_held_rumble() {
        let t = Instant::now();
        let s = (1000, 2000);
        assert_eq!(sdl_step(None, (0, 0), t), (false, None));
        assert_eq!(sdl_step(None, s, t), (true, Some((s, t))));
        // the same value is sent again once the resend is due, whatever happened to the last send
        assert_eq!(sdl_step(Some((s, t)), s, t + Duration::from_millis(100)), (false, Some((s, t))));
        let later = t + RESEND;
        assert_eq!(sdl_step(Some((s, t)), s, later), (true, Some((s, later))));
        assert_eq!(sdl_step(Some((s, t)), (0, 0), later), (true, None));
    }

    #[test]
    fn resend_due() {
        let t = Instant::now();
        assert!(super::resend_due(None, t));
        assert!(!super::resend_due(Some(t), t + Duration::from_millis(100)));
        assert!(super::resend_due(Some(t), t + RESEND));
    }
}
