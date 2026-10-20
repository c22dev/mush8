// App-local XUSB input for the exact Razer Wolverine V3 TE 8K PC profile.
use crate::input::{A, B, X, Y, L, R, ZL, ZR, PLUS, MINUS, STICK_L, STICK_R, LEFT, UP, RIGHT, DOWN};
use crate::services::hid::PadState;

unsafe extern "C" {
    fn mk8WolverinePoll(bytes: *mut u8, capacity: usize) -> u64;
    fn mk8WolverinePreferSystem(preferred: i32);
}

pub fn poll(prefer_system: bool) -> Option<(u64, PadState)> {
    use std::sync::atomic::{AtomicBool, Ordering::Relaxed};
    static PREFERRED: AtomicBool = AtomicBool::new(false);
    if PREFERRED.swap(prefer_system, Relaxed) != prefer_system {
        unsafe { mk8WolverinePreferSystem(i32::from(prefer_system)); }
    }
    if prefer_system { return None; }
    let mut packet = [0; 20];
    let id = unsafe { mk8WolverinePoll(packet.as_mut_ptr(), packet.len()) };
    (id != 0).then(|| decode(&packet).map(|pad| (id, pad))).flatten()
}

fn stick(packet: &[u8], offset: usize, deadzone: f32) -> (i32, i32) {
    let axis = |i| i16::from_le_bytes([packet[i], packet[i + 1]]) as f32 / 32768.0;
    let (x, y) = (axis(offset), axis(offset + 2)); // XUSB and the guest both use positive Y upwards.
    let length = x.hypot(y);
    if length <= deadzone { return (0, 0); }
    let scale = ((length - deadzone) / (1.0 - deadzone)).min(1.0) / length;
    ((x * scale * 32767.0) as i32, (y * scale * 32767.0) as i32)
}

pub(crate) fn decode(packet: &[u8]) -> Option<PadState> {
    if packet.len() != 20 || packet[0] != 0 || packet[1] != 20 { return None; }
    let source = u16::from_le_bytes([packet[2], packet[3]]);
    // Match the existing GameController Xbox layout by physical button position:
    // Xbox B/right -> Switch A, Xbox A/bottom -> Switch B.
    let map = [(0, UP), (1, DOWN), (2, LEFT), (3, RIGHT), (4, PLUS), (5, MINUS),
               (6, STICK_L), (7, STICK_R), (8, L), (9, R), (12, B), (13, A), (14, Y), (15, X)];
    let mut buttons = map.into_iter().filter(|(bit, _)| source & (1 << bit) != 0).fold(0, |mask, (_, b)| mask | b);
    if packet[4] > 30 { buttons |= ZL; }
    if packet[5] > 30 { buttons |= ZR; }
    Some(PadState { buttons, left: stick(packet, 6, 7849.0 / 32768.0), right: stick(packet, 10, 8689.0 / 32768.0), ..Default::default() })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn packet(buttons: u16) -> [u8; 20] {
        let mut p = [0; 20]; p[1] = 20; p[2..4].copy_from_slice(&buttons.to_le_bytes()); p
    }
    #[test]
    fn validates_packet_and_maps_every_button_position() {
        for (bit, expected) in [(0, UP), (1, DOWN), (2, LEFT), (3, RIGHT), (4, PLUS), (5, MINUS),
            (6, STICK_L), (7, STICK_R), (8, L), (9, R), (12, B), (13, A), (14, Y), (15, X)] {
            assert_eq!(decode(&packet(1 << bit)).unwrap().buttons, expected);
        }
        assert!(decode(&[0; 19]).is_none());
        let mut bad = packet(0); bad[0] = 1; assert!(decode(&bad).is_none());
        bad[0] = 0; bad[1] = 19; assert!(decode(&bad).is_none());
        // Guide has no guest Home-menu mapping; reserved bits must not become buttons.
        assert_eq!(decode(&packet((1 << 10) | (1 << 11))).unwrap().buttons, 0);
    }
    #[test]
    fn axes_keep_up_positive_and_deadzone_and_triggers_release() {
        let mut p = packet(0);
        p[6..8].copy_from_slice(&1000i16.to_le_bytes()); p[8..10].copy_from_slice(&(-2000i16).to_le_bytes());
        assert_eq!(decode(&p).unwrap().left, (0, 0));
        p[6..8].copy_from_slice(&0i16.to_le_bytes()); p[8..10].copy_from_slice(&32767i16.to_le_bytes());
        p[10..12].copy_from_slice(&(-32768i16).to_le_bytes());
        p[4] = 30; p[5] = 31;
        let pad = decode(&p).unwrap();
        assert!(pad.left.1 > 32760 && pad.right.0 == -32767);
        assert_eq!(pad.buttons, ZR);
        let released = decode(&packet(0)).unwrap();
        assert_eq!((released.buttons, released.left, released.right), (0, (0, 0), (0, 0)));
    }
    #[test]
    fn captured_a_rb_and_trigger_reports() {
        let mut p = packet(0x1200); // Xbox A and RB, from this model's XUSB capture.
        p[4] = 255; p[5] = 255;
        assert_eq!(decode(&p).unwrap().buttons, B | R | ZL | ZR);
    }
}
