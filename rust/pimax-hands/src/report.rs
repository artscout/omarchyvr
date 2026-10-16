//! A decoded controller report (libpimax `pimax_controller_report` / `pimax_proto::ControllerReport`) -> [`Input`].
//!
//! The bit meanings are the protocol's (analysis/controllers-linux.md); `tests/replay.rs` checks them against
//! `pimax-proto` and drives the whole mapping with the real button-by-button captures.

use crate::mapping::{flags, Input};

/// `buttons`: B (right) / Y (left).
pub const BTN_B: u32 = 0x01;
/// `buttons`: A (right) / X (left).
pub const BTN_A: u32 = 0x02;
/// `buttons`: thumbstick click.
pub const BTN_STICK: u32 = 0x08;
/// `buttons`: menu (left) / system (right).
pub const BTN_MENU: u32 = 0x10;
/// `touch` (report byte 49) bit 0: thumb on A / X.
pub const TOUCH_A: u8 = 0x01;
/// `touch` bit 1: thumb on B / Y (note the swapped order versus the presses).
pub const TOUCH_B: u8 = 0x02;
/// `touch` bit 3: thumb on the stick.
pub const TOUCH_STICK: u8 = 0x08;

/// The fields of a controller report the hand needs, as the driver receives them.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
    /// 0..255.
    pub trigger: u8,
    /// 0..255.
    pub grip: u8,
    /// -128..127, right positive.
    pub stick_x: i32,
    /// -128..127, up positive.
    pub stick_y: i32,
    /// `BTN_*` bits.
    pub buttons: u32,
    /// Raw byte 49.
    pub touch: u8,
}

/// Normalise a report: trigger and grip /255, stick /127 (clamped), buttons and touch bits to [`flags`].
pub fn input_from_report(r: &Report) -> Input {
    let bit = |set: bool, flag: u32| if set { flag } else { 0 };
    Input {
        trigger: f32::from(r.trigger) / 255.0,
        grip: f32::from(r.grip) / 255.0,
        stick_x: (r.stick_x as f32 / 127.0).clamp(-1.0, 1.0),
        stick_y: (r.stick_y as f32 / 127.0).clamp(-1.0, 1.0),
        flags: bit(r.touch & TOUCH_A != 0, flags::TOUCH_BTN1)
            | bit(r.touch & TOUCH_B != 0, flags::TOUCH_BTN2)
            | bit(r.touch & TOUCH_STICK != 0, flags::TOUCH_STICK)
            | bit(r.buttons & BTN_A != 0, flags::CLICK_BTN1)
            | bit(r.buttons & BTN_B != 0, flags::CLICK_BTN2)
            | bit(r.buttons & BTN_STICK != 0, flags::CLICK_STICK)
            | bit(r.buttons & BTN_MENU != 0, flags::CLICK_MENU),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn idle_report_is_the_default_input() {
        assert_eq!(input_from_report(&Report::default()), Input::default());
    }

    #[test]
    fn analog_values_are_normalised() {
        let i = input_from_report(&Report {
            trigger: 255,
            grip: 51,
            stick_x: 127,
            stick_y: -128,
            ..Report::default()
        });
        assert_eq!(i.trigger, 1.0);
        assert!((i.grip - 0.2).abs() < 1e-6);
        assert_eq!(i.stick_x, 1.0);
        assert_eq!(i.stick_y, -1.0); // -128/127 is clamped
    }

    #[test]
    fn every_bit_maps_to_its_own_flag() {
        for (buttons, touch, want) in [
            (BTN_A, 0, flags::CLICK_BTN1),
            (BTN_B, 0, flags::CLICK_BTN2),
            (BTN_STICK, 0, flags::CLICK_STICK),
            (BTN_MENU, 0, flags::CLICK_MENU),
            (0, TOUCH_A, flags::TOUCH_BTN1),
            (0, TOUCH_B, flags::TOUCH_BTN2),
            (0, TOUCH_STICK, flags::TOUCH_STICK),
        ] {
            let i = input_from_report(&Report {
                buttons,
                touch,
                ..Report::default()
            });
            assert_eq!(i.flags, want, "buttons {buttons:#x} touch {touch:#x}");
        }
    }

    #[test]
    fn unrelated_bits_are_ignored() {
        // byte 49 bit 4 (proximity-like, unknown) and bit 2 / 5..7 must not touch the thumb
        let i = input_from_report(&Report {
            buttons: 0x04 | 0x20 | 0x8000_0000,
            touch: 0x10 | 0x04 | 0xe0,
            ..Report::default()
        });
        assert_eq!(i.flags, 0);
    }
}
