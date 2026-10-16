//! Controller and keyboard input to [`Event`]s. The controller layout is the one of `analysis/tools/ctrlcli.c`
//! (Oculus Touch profile: trigger, grip, A/X, B/Y, thumbstick, thumbstick click, menu). Pure: [`InputMapper`]
//! turns successive snapshots into events with edge detection and a latched thumbstick.

use crate::patterns::align::COARSE_UNITS;
use crate::state::Event;

/// One controller as sampled in a frame.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Hand {
    /// The runtime reports the controller as active (awake and bound).
    pub active: bool,
    /// A (right hand) or X (left hand).
    pub a: bool,
    /// B (right hand) or Y (left hand).
    pub b: bool,
    pub stick: [f32; 2],
    pub stick_click: bool,
    pub trigger: f32,
    pub grip: f32,
    pub menu: bool,
}

/// Both controllers: index 0 = left, 1 = right.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RawInput {
    pub hand: [Hand; 2],
}

/// Stick deflection that counts as a push, and the (smaller) deflection that releases it.
pub const STICK_ON: f32 = 0.6;
pub const STICK_OFF: f32 = 0.3;
/// Analog buttons (trigger, grip) count as pressed above this.
pub const ANALOG_ON: f32 = 0.7;

#[derive(Clone, Debug, Default)]
pub struct InputMapper {
    prev: [Hand; 2],
    /// Latched stick direction per hand and axis: -1, 0, +1.
    latch: [[i8; 2]; 2],
    prev_analog: [[bool; 2]; 2],
}

impl InputMapper {
    pub fn new() -> InputMapper {
        InputMapper::default()
    }

    /// Events caused by the change from the previous snapshot to `now`.
    pub fn update(&mut self, now: &RawInput) -> Vec<Event> {
        let mut ev = Vec::new();
        for h in 0..2 {
            let (cur, prev) = (now.hand[h], self.prev[h]);
            if !cur.active {
                // an inactive controller reports nothing; forget its state so a wake-up press is a fresh edge
                self.prev[h] = Hand::default();
                self.latch[h] = [0, 0];
                self.prev_analog[h] = [false, false];
                continue;
            }
            if cur.a && !prev.a {
                ev.push(Event::NextPattern);
            }
            if cur.b && !prev.b {
                ev.push(Event::PrevPattern);
            }
            if cur.menu && !prev.menu {
                ev.push(Event::Accept);
            }
            if cur.stick_click && !prev.stick_click {
                ev.push(Event::CycleEye);
            }
            let analog = [cur.trigger > ANALOG_ON, cur.grip > ANALOG_ON];
            if analog[0] && !self.prev_analog[h][0] {
                ev.push(Event::ToggleAuto);
            }
            if analog[1] && !self.prev_analog[h][1] {
                ev.push(Event::ToggleHud);
            }
            self.prev_analog[h] = analog;
            for axis in 0..2 {
                let v = cur.stick[axis];
                let l = &mut self.latch[h][axis];
                if v.abs() < STICK_OFF {
                    *l = 0;
                } else if v > STICK_ON && *l != 1 {
                    *l = 1;
                    ev.push(if axis == 0 {
                        Event::NextStep
                    } else {
                        Event::StickVert(1)
                    });
                } else if v < -STICK_ON && *l != -1 {
                    *l = -1;
                    ev.push(if axis == 0 {
                        Event::PrevStep
                    } else {
                        Event::StickVert(-1)
                    });
                }
            }
            self.prev[h] = cur;
        }
        ev
    }
}

/// Keyboard command of a key typed in the terminal.
pub fn key_event(c: char) -> Option<Event> {
    match c {
        'n' | 'N' | ' ' => Some(Event::NextPattern),
        'p' | 'P' => Some(Event::PrevPattern),
        '.' | '>' => Some(Event::NextStep),
        ',' | '<' => Some(Event::PrevStep),
        'e' | 'E' => Some(Event::CycleEye),
        'a' | 'A' => Some(Event::ToggleAuto),
        'h' | 'H' => Some(Event::ToggleHud),
        'd' | 'D' => Some(Event::CycleDir),
        '[' => Some(Event::AlignVerg(-1)),
        ']' => Some(Event::AlignVerg(1)),
        '{' => Some(Event::AlignVerg(-COARSE_UNITS)),
        '}' => Some(Event::AlignVerg(COARSE_UNITS)),
        'i' | 'I' => Some(Event::AlignVert(1)),
        'k' | 'K' => Some(Event::AlignVert(-1)),
        'r' | 'R' => Some(Event::AlignReset),
        ';' => Some(Event::Tune(-1, 0)),
        '\'' => Some(Event::Tune(1, 0)),
        ':' => Some(Event::Tune(-5, 0)),
        '"' => Some(Event::Tune(5, 0)),
        '-' => Some(Event::Tune(0, -1)),
        '=' => Some(Event::Tune(0, 1)),
        '_' => Some(Event::Tune(0, -4)),
        '+' => Some(Event::Tune(0, 4)),
        'o' => Some(Event::Tune2(1)),
        'l' => Some(Event::Tune2(-1)),
        'O' => Some(Event::Tune2(5)),
        'L' => Some(Event::Tune2(-5)),
        'c' | 'C' => Some(Event::Recenter),
        'y' | 'Y' => Some(Event::Accept),
        'q' | 'Q' | '\u{1b}' | '\u{3}' => Some(Event::Quit),
        '1'..='9' => Some(Event::Goto(c as usize - '1' as usize)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn active() -> Hand {
        Hand {
            active: true,
            ..Hand::default()
        }
    }

    fn input(l: Hand, r: Hand) -> RawInput {
        RawInput { hand: [l, r] }
    }

    #[test]
    fn buttons_fire_once_per_press_on_either_hand() {
        let mut m = InputMapper::new();
        assert!(m.update(&input(active(), active())).is_empty());
        let a_right = Hand {
            a: true,
            ..active()
        };
        assert_eq!(
            m.update(&input(active(), a_right)),
            vec![Event::NextPattern]
        );
        assert!(
            m.update(&input(active(), a_right)).is_empty(),
            "held button repeats nothing"
        );
        assert!(m.update(&input(active(), active())).is_empty());
        let x_left = Hand {
            a: true,
            ..active()
        };
        assert_eq!(m.update(&input(x_left, active())), vec![Event::NextPattern]);
        let b = Hand {
            b: true,
            ..active()
        };
        assert_eq!(m.update(&input(b, active())), vec![Event::PrevPattern]);
        let click = Hand {
            stick_click: true,
            ..active()
        };
        assert_eq!(m.update(&input(active(), click)), vec![Event::CycleEye]);
    }

    #[test]
    fn stick_is_latched_with_hysteresis() {
        let mut m = InputMapper::new();
        let s = |x: f32, y: f32| {
            input(
                active(),
                Hand {
                    stick: [x, y],
                    ..active()
                },
            )
        };
        assert!(
            m.update(&s(0.5, 0.0)).is_empty(),
            "below the push threshold"
        );
        assert_eq!(m.update(&s(0.9, 0.0)), vec![Event::NextStep]);
        assert!(m.update(&s(1.0, 0.0)).is_empty(), "stays latched");
        assert!(
            m.update(&s(0.4, 0.0)).is_empty(),
            "between thresholds: still latched"
        );
        assert!(m.update(&s(0.9, 0.0)).is_empty());
        assert!(m.update(&s(0.1, 0.0)).is_empty(), "released");
        assert_eq!(m.update(&s(0.9, 0.0)), vec![Event::NextStep]);
        assert_eq!(
            m.update(&s(-0.9, 0.0)),
            vec![Event::PrevStep],
            "straight through the centre without release"
        );
        assert!(m.update(&s(0.0, 0.0)).is_empty());
        assert_eq!(m.update(&s(0.0, 0.8)), vec![Event::StickVert(1)]);
        assert!(m.update(&s(0.0, 0.8)).is_empty());
        assert!(m.update(&s(0.0, 0.0)).is_empty());
        assert_eq!(m.update(&s(0.0, -0.8)), vec![Event::StickVert(-1)]);
    }

    #[test]
    fn menu_button_accepts_once_per_press() {
        let mut m = InputMapper::new();
        let menu = Hand {
            menu: true,
            ..active()
        };
        assert_eq!(m.update(&input(menu, active())), vec![Event::Accept]);
        assert!(m.update(&input(menu, active())).is_empty());
        assert!(m.update(&input(active(), active())).is_empty());
        assert_eq!(m.update(&input(menu, active())), vec![Event::Accept]);
    }

    #[test]
    fn trigger_and_grip_toggle_on_the_rising_edge() {
        let mut m = InputMapper::new();
        let t = |tr: f32, gr: f32| {
            input(
                Hand {
                    trigger: tr,
                    grip: gr,
                    ..active()
                },
                active(),
            )
        };
        assert!(m.update(&t(0.5, 0.0)).is_empty());
        assert_eq!(m.update(&t(0.9, 0.0)), vec![Event::ToggleAuto]);
        assert!(m.update(&t(1.0, 0.0)).is_empty());
        assert!(m.update(&t(0.0, 0.0)).is_empty());
        assert_eq!(m.update(&t(0.0, 0.95)), vec![Event::ToggleHud]);
        assert_eq!(m.update(&t(0.0, 0.0)), vec![]);
        assert_eq!(
            m.update(&t(0.8, 0.8)),
            vec![Event::ToggleAuto, Event::ToggleHud]
        );
    }

    #[test]
    fn inactive_controllers_are_silent_and_wake_up_with_a_fresh_edge() {
        let mut m = InputMapper::new();
        let pressed_but_inactive = Hand {
            a: true,
            active: false,
            ..Hand::default()
        };
        assert!(m
            .update(&input(pressed_but_inactive, pressed_but_inactive))
            .is_empty());
        // wakes up with the button still down: that is a press
        let awake = Hand {
            a: true,
            active: true,
            ..Hand::default()
        };
        assert_eq!(m.update(&input(active(), awake)), vec![Event::NextPattern]);
        // goes to sleep and wakes again with the button held: a new press
        assert!(m.update(&input(active(), Hand::default())).is_empty());
        assert_eq!(m.update(&input(active(), awake)), vec![Event::NextPattern]);
    }

    #[test]
    fn keyboard() {
        assert_eq!(key_event('n'), Some(Event::NextPattern));
        assert_eq!(key_event(' '), Some(Event::NextPattern));
        assert_eq!(key_event('p'), Some(Event::PrevPattern));
        assert_eq!(key_event('.'), Some(Event::NextStep));
        assert_eq!(key_event(','), Some(Event::PrevStep));
        assert_eq!(key_event('E'), Some(Event::CycleEye));
        assert_eq!(key_event('a'), Some(Event::ToggleAuto));
        assert_eq!(key_event('h'), Some(Event::ToggleHud));
        assert_eq!(key_event('d'), Some(Event::CycleDir));
        assert_eq!(key_event('q'), Some(Event::Quit));
        assert_eq!(key_event('\u{3}'), Some(Event::Quit));
        assert_eq!(key_event('1'), Some(Event::Goto(0)));
        assert_eq!(key_event('7'), Some(Event::Goto(6)));
        assert_eq!(key_event(']'), Some(Event::AlignVerg(1)));
        assert_eq!(key_event('['), Some(Event::AlignVerg(-1)));
        assert_eq!(key_event('}'), Some(Event::AlignVerg(5)));
        assert_eq!(key_event('{'), Some(Event::AlignVerg(-5)));
        assert_eq!(key_event('i'), Some(Event::AlignVert(1)));
        assert_eq!(key_event('k'), Some(Event::AlignVert(-1)));
        assert_eq!(key_event('r'), Some(Event::AlignReset));
        assert_eq!(key_event(';'), Some(Event::Tune(-1, 0)));
        assert_eq!(key_event('\''), Some(Event::Tune(1, 0)));
        assert_eq!(key_event(':'), Some(Event::Tune(-5, 0)));
        assert_eq!(key_event('"'), Some(Event::Tune(5, 0)));
        assert_eq!(key_event('-'), Some(Event::Tune(0, -1)));
        assert_eq!(key_event('='), Some(Event::Tune(0, 1)));
        assert_eq!(key_event('_'), Some(Event::Tune(0, -4)));
        assert_eq!(key_event('+'), Some(Event::Tune(0, 4)));
        assert_eq!(key_event('o'), Some(Event::Tune2(1)));
        assert_eq!(key_event('l'), Some(Event::Tune2(-1)));
        assert_eq!(key_event('O'), Some(Event::Tune2(5)));
        assert_eq!(key_event('L'), Some(Event::Tune2(-5)));
        assert_eq!(key_event('c'), Some(Event::Recenter));
        assert_eq!(key_event('y'), Some(Event::Accept));
        assert_eq!(key_event('Y'), Some(Event::Accept));
        assert_eq!(key_event('x'), None);
        assert_eq!(key_event('\n'), None);
    }
}
