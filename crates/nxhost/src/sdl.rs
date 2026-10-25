// SDL is set up and polled only on the HID service thread that calls Pads::poll.
// SDL_RumbleGamepad is the one call from another thread (haptics), and SDL locks its joystick state for it.
use crate::input::*;
use sdl3_sys::everything::*;

const TRIGGER_PRESSED: i16 = 16384;

pub fn init() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| unsafe {
        // SDL would catch SIGINT and SIGTERM and only queue a quit event, which we never pump
        SDL_SetHint(SDL_HINT_NO_SIGNAL_HANDLERS, c"1".as_ptr());
        if !SDL_Init(SDL_INIT_GAMEPAD) {
            eprintln!("[input] SDL init failed: {}", error());
            return false;
        }
        SDL_SetJoystickEventsEnabled(false);
        SDL_SetGamepadEventsEnabled(false);
        true
    })
}

unsafe fn error() -> String {
    unsafe { std::ffi::CStr::from_ptr(SDL_GetError()).to_string_lossy().into_owned() }
}

pub fn connected() -> Vec<SDL_JoystickID> {
    unsafe {
        SDL_UpdateGamepads();
        let mut n = 0;
        let ids = SDL_GetGamepads(&mut n);
        if ids.is_null() {
            return Vec::new();
        }
        let out = std::slice::from_raw_parts(ids, n.max(0) as usize).to_vec();
        SDL_free(ids.cast());
        out
    }
}

// SDL checks that the gamepad object is still valid, so a pad closed by the input thread just returns false.
pub fn rumble(id: SDL_JoystickID, low: u16, high: u16, ms: u32) -> bool {
    unsafe {
        let gamepad = SDL_GetGamepadFromID(id);
        !gamepad.is_null() && SDL_RumbleGamepad(gamepad, low, high, ms)
    }
}

// Remembers pads that SDL lists but cannot open, so they are tried once and not on every poll.
#[derive(Default)]
pub struct OpenOnce {
    failed: Vec<SDL_JoystickID>,
}

impl OpenOnce {
    pub fn open<T>(&mut self, id: SDL_JoystickID, open: impl FnOnce() -> Option<T>) -> Option<T> {
        if self.failed.contains(&id) {
            return None;
        }
        let pad = open();
        if pad.is_none() {
            self.failed.push(id);
        }
        pad
    }

    pub fn keep(&mut self, connected: &[SDL_JoystickID]) {
        self.failed.retain(|id| connected.contains(id));
    }
}

pub struct Reading {
    pub buttons: u64,
    pub sticks: [f32; 4],
}

pub struct Pad {
    id: SDL_JoystickID,
    gamepad: *mut SDL_Gamepad,
}

unsafe impl Send for Pad {}

impl Pad {
    pub fn open(id: SDL_JoystickID) -> Option<Pad> {
        let gamepad = unsafe { SDL_OpenGamepad(id) };
        if gamepad.is_null() {
            eprintln!("[input] SDL could not open gamepad {}: {}", id.0, unsafe { error() });
            return None;
        }
        Some(Pad { id, gamepad })
    }

    pub fn id(&self) -> SDL_JoystickID {
        self.id
    }

    pub fn name(&self) -> String {
        let name = unsafe { SDL_GetGamepadName(self.gamepad) };
        if name.is_null() { "Controller".into() } else { unsafe { std::ffi::CStr::from_ptr(name) }.to_string_lossy().into_owned() }
    }

    pub fn set_player(&self, i: usize) {
        unsafe { SDL_SetGamepadPlayerIndex(self.gamepad, i as i32) };
    }

    pub fn enable_motion(&self) {
        for sensor in [SDL_SENSOR_ACCEL, SDL_SENSOR_GYRO] {
            unsafe {
                if SDL_GamepadHasSensor(self.gamepad, sensor) {
                    SDL_SetGamepadSensorEnabled(self.gamepad, sensor, true);
                }
            }
        }
    }

    pub fn read(&self) -> Reading {
        unsafe {
            let axis_raw = |a| SDL_GetGamepadAxis(self.gamepad, a);
            Reading {
                buttons: buttons_from(
                    |b| SDL_GetGamepadButton(self.gamepad, b),
                    axis_raw(SDL_GAMEPAD_AXIS_LEFT_TRIGGER),
                    axis_raw(SDL_GAMEPAD_AXIS_RIGHT_TRIGGER),
                ),
                sticks: [
                    axis(axis_raw(SDL_GAMEPAD_AXIS_LEFTX)),
                    -axis(axis_raw(SDL_GAMEPAD_AXIS_LEFTY)),
                    axis(axis_raw(SDL_GAMEPAD_AXIS_RIGHTX)),
                    -axis(axis_raw(SDL_GAMEPAD_AXIS_RIGHTY)),
                ],
            }
        }
    }

    pub fn motion(&self) -> Option<Motion> {
        unsafe {
            if !SDL_GamepadHasSensor(self.gamepad, SDL_SENSOR_GYRO) {
                return None;
            }
            let (mut accel, mut gyro) = ([0.0f32; 3], [0.0f32; 3]);
            SDL_GetGamepadSensorData(self.gamepad, SDL_SENSOR_ACCEL, accel.as_mut_ptr(), 3);
            SDL_GetGamepadSensorData(self.gamepad, SDL_SENSOR_GYRO, gyro.as_mut_ptr(), 3);
            Some(motion_from(accel, gyro))
        }
    }
}

impl Drop for Pad {
    fn drop(&mut self) {
        unsafe { SDL_CloseGamepad(self.gamepad) };
    }
}

pub fn buttons_from(pressed: impl Fn(SDL_GamepadButton) -> bool, left_trigger: i16, right_trigger: i16) -> u64 {
    let map = [
        (SDL_GAMEPAD_BUTTON_EAST, A),
        (SDL_GAMEPAD_BUTTON_SOUTH, B),
        (SDL_GAMEPAD_BUTTON_NORTH, X),
        (SDL_GAMEPAD_BUTTON_WEST, Y),
        (SDL_GAMEPAD_BUTTON_LEFT_SHOULDER, L),
        (SDL_GAMEPAD_BUTTON_RIGHT_SHOULDER, R),
        (SDL_GAMEPAD_BUTTON_START, PLUS),
        (SDL_GAMEPAD_BUTTON_BACK, MINUS),
        (SDL_GAMEPAD_BUTTON_LEFT_STICK, STICK_L),
        (SDL_GAMEPAD_BUTTON_RIGHT_STICK, STICK_R),
        (SDL_GAMEPAD_BUTTON_DPAD_LEFT, LEFT),
        (SDL_GAMEPAD_BUTTON_DPAD_UP, UP),
        (SDL_GAMEPAD_BUTTON_DPAD_RIGHT, RIGHT),
        (SDL_GAMEPAD_BUTTON_DPAD_DOWN, DOWN),
    ];
    let mut buttons = map.iter().filter(|(b, _)| pressed(*b)).fold(0, |m, (_, bit)| m | bit);
    if left_trigger > TRIGGER_PRESSED {
        buttons |= ZL;
    }
    if right_trigger > TRIGGER_PRESSED {
        buttons |= ZR;
    }
    buttons
}

pub fn axis(v: i16) -> f32 {
    (v as f32 / 32767.0).clamp(-1.0, 1.0)
}

// Converts SDL gamepad axes to the ones the game reads, the same way as the yuzu-family emulators
// (citron src/input_common/drivers/sdl_driver.cpp UpdateMotion). The game wants gravity in g, SDL gives the felt force in m/s².
pub fn motion_from(accel: [f32; 3], gyro: [f32; 3]) -> Motion {
    let g = SDL_STANDARD_GRAVITY;
    Motion { accel: [-accel[0] / g, accel[2] / g, -accel[1] / g], gyro: [gyro[0], -gyro[2], gyro[1]] }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn init_keeps_default_quit_signals() {
        assert!(init());
        for sig in [libc::SIGINT, libc::SIGTERM] {
            let mut old: libc::sigaction = unsafe { std::mem::zeroed() };
            unsafe { libc::sigaction(sig, std::ptr::null(), &mut old) };
            assert_eq!(old.sa_sigaction, libc::SIG_DFL, "signal {sig} handler was replaced");
        }
    }

    #[test]
    fn open_once_skips_pads_that_failed() {
        let mut once = OpenOnce::default();
        let (a, b) = (SDL_JoystickID(1), SDL_JoystickID(2));
        let mut calls = 0;
        assert_eq!(once.open(a, || { calls += 1; None::<()> }), None);
        assert_eq!(once.open(a, || { calls += 1; Some(()) }), None);
        assert_eq!(calls, 1);
        assert_eq!(once.open(b, || Some(())), Some(()));
        // a pad that left is forgotten
        once.keep(&[b]);
        assert_eq!(once.open(a, || Some(())), Some(()));
    }

    #[test]
    fn face_buttons_match_switch_positions() {
        let only = |want: SDL_GamepadButton| move |b: SDL_GamepadButton| b == want;
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_EAST), 0, 0), A);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_SOUTH), 0, 0), B);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_NORTH), 0, 0), X);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_WEST), 0, 0), Y);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_START), 0, 0), PLUS);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_BACK), 0, 0), MINUS);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_LEFT_SHOULDER), 0, 0), L);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_RIGHT_STICK), 0, 0), STICK_R);
        assert_eq!(buttons_from(only(SDL_GAMEPAD_BUTTON_DPAD_UP), 0, 0), UP);
    }

    #[test]
    fn triggers_use_threshold() {
        let none = |_: SDL_GamepadButton| false;
        assert_eq!(buttons_from(none, 8000, 8000), 0);
        assert_eq!(buttons_from(none, 20000, 0), ZL);
        assert_eq!(buttons_from(none, 0, 32767), ZR);
    }

    #[test]
    fn axis_range() {
        assert_eq!(axis(0), 0.0);
        assert_eq!(axis(32767), 1.0);
        assert_eq!(axis(-32768), -1.0);
    }

    // the same conversion as the yuzu-family SDL driver (citron src/input_common/drivers/sdl_driver.cpp UpdateMotion)
    #[test]
    fn motion_matches_emulator_axes() {
        let close = |a: [f32; 3], b: [f32; 3]| a.iter().zip(b).all(|(x, y)| (x - y).abs() < 1e-6);
        let g = SDL_STANDARD_GRAVITY;
        assert!(close(motion_from([g, 2.0 * g, 3.0 * g], [0.0; 3]).accel, [-1.0, 3.0, -2.0]));
        assert!(close(motion_from([0.0, g, 0.0], [0.0; 3]).accel, [0.0, 0.0, -1.0]));
        assert!(close(motion_from([0.0; 3], [1.0, 2.0, 3.0]).gyro, [1.0, -3.0, 2.0]));
    }
}
