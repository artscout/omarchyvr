//! A scripted "controller" for end-to-end tests when nobody can press buttons.
//!
//! [`demo_input`] is a pure function of the monotonic clock, so the Monado driver (which feeds it into the hand
//! state instead of real reports when `PIMAX_CTRL_HANDS_DEMO=1`) and an OpenXR test client agree on what the
//! controllers "did" at any instant without talking to each other.
//!
//! One cycle (seconds into the cycle, hand time):
//! ```text
//!  0-1 rest | 1-3 grip 0>1>0 | 3-5 trigger 0>1>0 | 5-6 touch A/X | 6-7 touch B/Y (click in the second half)
//!  7-8 stick touch + deflection | 8-10 rest
//! ```
//! The right hand runs [`DEMO_RIGHT_OFFSET_S`] later than the left one so a left/right mix-up shows.

use crate::mapping::{flags, Input};

/// Length of one cycle.
pub const DEMO_PERIOD_S: f64 = 10.0;
/// The right hand's cycle is shifted by this much.
pub const DEMO_RIGHT_OFFSET_S: f64 = 2.5;

/// 0 -> 1 -> 0 triangle over `[start, start + len]`, 0 elsewhere.
fn tri(t: f64, start: f64, len: f64) -> f32 {
    if t < start || t > start + len {
        return 0.0;
    }
    let x = (t - start) / len;
    (1.0 - (2.0 * x - 1.0).abs()) as f32
}

/// Scripted input at monotonic time `now_ns` for the left (`right == false`) or right hand.
pub fn demo_input(now_ns: u64, right: bool) -> Input {
    let t = now_ns as f64 * 1e-9 + if right { DEMO_RIGHT_OFFSET_S } else { 0.0 };
    let c = t.rem_euclid(DEMO_PERIOD_S);
    let mut i = Input {
        grip: tri(c, 1.0, 2.0),
        trigger: tri(c, 3.0, 2.0),
        ..Input::default()
    };
    if (5.0..6.0).contains(&c) {
        i.flags |= flags::TOUCH_BTN1;
    } else if (6.0..7.0).contains(&c) {
        i.flags |= flags::TOUCH_BTN2;
        if c >= 6.5 {
            i.flags |= flags::CLICK_BTN2;
        }
    } else if (7.0..8.0).contains(&c) {
        i.flags |= flags::TOUCH_STICK;
        i.stick_x = 0.6;
    }
    i
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(s: f64, right: bool) -> Input {
        demo_input((s * 1e9) as u64, right)
    }

    #[test]
    fn phases_of_the_cycle() {
        assert_eq!(at(0.5, false), Input::default());
        assert!((at(2.0, false).grip - 1.0).abs() < 1e-6);
        assert_eq!(at(2.0, false).trigger, 0.0);
        assert!((at(4.0, false).trigger - 1.0).abs() < 1e-6);
        assert_eq!(at(4.0, false).grip, 0.0);
        assert_eq!(at(5.5, false).flags, flags::TOUCH_BTN1);
        assert_eq!(at(6.2, false).flags, flags::TOUCH_BTN2);
        assert_eq!(at(6.7, false).flags, flags::TOUCH_BTN2 | flags::CLICK_BTN2);
        let s = at(7.5, false);
        assert_eq!(s.flags, flags::TOUCH_STICK);
        assert!(s.stick_x > 0.5);
        assert_eq!(at(9.0, false), Input::default());
    }

    #[test]
    fn periodic() {
        for s in [0.3, 2.2, 4.7, 5.5, 6.9, 7.2, 9.9] {
            assert_eq!(at(s, false), at(s + DEMO_PERIOD_S * 7.0, false), "{s}");
        }
    }

    #[test]
    fn right_hand_is_shifted() {
        for s in [0.0, 1.3, 3.7, 6.1, 8.8] {
            assert_eq!(at(s + DEMO_RIGHT_OFFSET_S, false), at(s, true), "{s}");
        }
        assert_ne!(at(2.0, false), at(2.0, true));
    }

    #[test]
    fn values_stay_in_range() {
        for k in 0..2000 {
            let i = at(k as f64 * 0.013, k % 2 == 0);
            assert!((0.0..=1.0).contains(&i.trigger) && (0.0..=1.0).contains(&i.grip));
            assert_eq!(i.flags & !flags::ALL, 0);
        }
    }
}
