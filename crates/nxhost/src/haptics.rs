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

struct Shared {
    values: [[Value; 2]; MAX_PLAYERS],
    controllers: [Option<Ctl>; MAX_PLAYERS],
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
        Some(crate::input::Ctl::Apple(c)) => Some(Ctl(c.clone())),
        _ => None,
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
}

fn run() {
    let mut outs: [Out; MAX_PLAYERS] = std::array::from_fn(|_| Out { source: Source::None, engine: None, retry: None });
    loop {
        let (values, controllers) = {
            let mut s = SHARED.lock().unwrap();
            while !s.dirty {
                let active = outs.iter().any(|o| o.engine.as_ref().is_some_and(|e| e.started.is_some()));
                s = WAKE.wait_timeout(s, Duration::from_millis(if active { 500 } else { 5000 })).unwrap().0;
                if active {
                    break;
                }
            }
            s.dirty = false;
            let ctls: [Option<Retained<GCController>>; MAX_PLAYERS] = std::array::from_fn(|i| s.controllers[i].as_ref().map(|c| c.0.clone()));
            (s.values, ctls)
        };
        objc2::rc::autoreleasepool(|_| {
            for (i, out) in outs.iter_mut().enumerate() {
                let source = match &controllers[i] {
                    Some(c) => Source::Controller(Retained::as_ptr(c) as usize),
                    #[cfg(target_os = "ios")]
                    None if i == 0 => Source::Device,
                    None => Source::None,
                };
                if source != out.source {
                    out.source = source;
                    out.engine = None;
                    out.retry = None;
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
