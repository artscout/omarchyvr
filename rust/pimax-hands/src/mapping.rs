//! Controller inputs -> finger curls.
//!
//! The Dream Air controllers have no finger sensors, so the hand is a guess from what the fingers must be doing:
//! * index finger: on the trigger, curl follows the (filtered) trigger value,
//! * middle, ring, little: on the grip, curl follows the (filtered) squeeze value,
//! * thumb: resting on whichever of A/X, B/Y, the stick or the menu button it touches; floating up otherwise.
//!   Touch bits are binary and noisy (they blink when the trigger or grip is squeezed), so a small state machine picks
//!   the target (a touch must last a moment, clicks and stick deflection count at once, the most recent contact wins,
//!   short gaps are bridged) and a critically damped smoother moves the thumb there instead of snapping.
//!
//! Everything is a pure function of the inputs and their timestamps (no clock, no allocation, no panics), which is
//! what the unit and property tests rely on.

use crate::filter::{OneEuro, SmoothDamp};

/// Bits of [`Input::flags`] (same values in `include/pimax_hands.h`).
pub mod flags {
    /// Thumb touches A (right controller) / X (left).
    pub const TOUCH_BTN1: u32 = 1 << 0;
    /// Thumb touches B (right) / Y (left).
    pub const TOUCH_BTN2: u32 = 1 << 1;
    /// Thumb touches the thumbstick.
    pub const TOUCH_STICK: u32 = 1 << 2;
    /// A / X pressed.
    pub const CLICK_BTN1: u32 = 1 << 3;
    /// B / Y pressed.
    pub const CLICK_BTN2: u32 = 1 << 4;
    /// Thumbstick pressed.
    pub const CLICK_STICK: u32 = 1 << 5;
    /// Menu (left) / system (right) button pressed.
    pub const CLICK_MENU: u32 = 1 << 6;
    /// Every defined bit.
    pub const ALL: u32 = 0x7f;
}

/// One controller report, already decoded. Analog values are 0..1 (sticks -1..1).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Input {
    pub trigger: f32,
    pub grip: f32,
    pub stick_x: f32,
    pub stick_y: f32,
    pub flags: u32,
}

/// Where the thumb rests.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum ThumbState {
    /// Not touching anything: thumb up.
    Rest = 0,
    Btn1 = 1,
    Btn2 = 2,
    Stick = 3,
    Menu = 4,
}

impl ThumbState {
    fn from_index(i: usize) -> Self {
        match i {
            0 => Self::Btn1,
            1 => Self::Btn2,
            2 => Self::Stick,
            3 => Self::Menu,
            _ => Self::Rest,
        }
    }
}

/// Tuning. All curls are normalised: 0 = finger straight, 1 = fully closed fist.
#[derive(Clone, Debug)]
pub struct Params {
    /// Trigger filter (One Euro): resting cutoff in Hz, speed coefficient in s.
    pub trigger_min_cutoff: f32,
    pub trigger_beta: f32,
    /// Grip filter.
    pub grip_min_cutoff: f32,
    pub grip_beta: f32,
    /// Cutoff of the speed estimate of both filters, Hz.
    pub d_cutoff: f32,
    /// Analog values below this are treated as 0 (and the rest rescaled), against ADC noise.
    pub deadzone: f32,
    /// Index curl with the trigger released / fully pulled.
    pub index_rest: f32,
    pub index_full: f32,
    /// Middle, ring and little curl with the grip released / fully squeezed.
    pub grip_rest: f32,
    pub grip_full: f32,
    /// Thumb curl resting on each place, and the extra when that button is pressed (not just touched).
    pub thumb_rest: f32,
    pub thumb_btn1: f32,
    pub thumb_btn2: f32,
    pub thumb_stick: f32,
    pub thumb_menu: f32,
    pub thumb_click_extra: f32,
    /// A touch bit must stay on this long before the thumb moves there, seconds. The capacitive bits blip for
    /// 50-100 ms when the trigger or grip is squeezed (real captures); a click or a stick deflection counts at once.
    pub engage_s: f32,
    /// A contact is kept this long after its touch bit drops (flicker bridge), seconds.
    pub release_hold_s: f32,
    /// Stick deflection that counts as touching it even without the touch bit (on / off, hysteresis).
    pub stick_on: f32,
    pub stick_off: f32,
    /// Thumb smoother: time to the target and speed limit (curl units per second).
    pub thumb_smooth_time_s: f32,
    pub thumb_max_speed: f32,
    /// A gap longer than this between two reports restarts the filters from the new sample.
    pub reset_gap_s: f32,
}

impl Default for Params {
    fn default() -> Self {
        Self {
            trigger_min_cutoff: 3.0,
            trigger_beta: 6.0,
            grip_min_cutoff: 2.0,
            grip_beta: 4.0,
            d_cutoff: 1.0,
            deadzone: 0.02,
            index_rest: 0.10,
            index_full: 0.85,
            grip_rest: 0.25,
            grip_full: 1.0,
            thumb_rest: 0.0,
            thumb_btn1: 0.45,
            thumb_btn2: 0.50,
            thumb_stick: 0.55,
            thumb_menu: 0.50,
            thumb_click_extra: 0.15,
            engage_s: 0.12,
            release_hold_s: 0.15,
            stick_on: 0.20,
            stick_off: 0.12,
            thumb_smooth_time_s: 0.07,
            thumb_max_speed: 8.0,
            reset_gap_s: 0.5,
        }
    }
}

/// Longest step fed to the filters, seconds (a stalled thread must not make them jump).
const MAX_DT_S: f32 = 0.1;

/// Normalised curl of each finger.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Curls {
    pub little: f32,
    pub ring: f32,
    pub middle: f32,
    pub index: f32,
    pub thumb: f32,
}

impl Curls {
    /// `[little, ring, middle, index, thumb]`, the order of Monado's `u_hand_tracking_values`.
    pub fn to_array(self) -> [f32; 5] {
        [self.little, self.ring, self.middle, self.index, self.thumb]
    }
}

fn clamp01(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Remove the dead zone and rescale so full range is preserved.
fn shape(x: f32, deadzone: f32) -> f32 {
    let dz = deadzone.clamp(0.0, 0.5);
    ((x - dz) / (1.0 - dz)).clamp(0.0, 1.0)
}

/// Replace NaN/inf/out-of-range values.
fn sanitize(i: &Input) -> Input {
    let s = |v: f32| {
        if v.is_finite() {
            v.clamp(-1.0, 1.0)
        } else {
            0.0
        }
    };
    Input {
        trigger: clamp01(i.trigger),
        grip: clamp01(i.grip),
        stick_x: s(i.stick_x),
        stick_y: s(i.stick_y),
        flags: i.flags & flags::ALL,
    }
}

/// One place the thumb can rest on.
#[derive(Clone, Copy, Debug, Default)]
struct Contact {
    /// Debounced.
    active: bool,
    /// Time (seconds on the internal clock) the contact began.
    since: f64,
    /// Seconds since the raw bit dropped (only while `active`).
    idle: f32,
    /// Seconds the raw touch bit has been on without the contact being active yet.
    pending: f32,
    /// Raw click state of this report.
    clicked: bool,
}

/// What one report says about a place the thumb can rest on.
#[derive(Clone, Copy)]
struct Evidence {
    /// Capacitive touch bit (noisy).
    touch: bool,
    /// Button pressed or stick deflected: a certain contact, counted at once.
    firm: bool,
    /// Button pressed: the thumb goes deeper than for a plain touch.
    pressed: bool,
}

impl Contact {
    fn update(&mut self, e: Evidence, dt: f32, now: f64, engage: f32, hold: f32, immediate: bool) {
        self.clicked = e.pressed;
        if e.firm || (e.touch && immediate) {
            if !self.active {
                self.active = true;
                self.since = now;
            }
            self.idle = 0.0;
            self.pending = 0.0;
        } else if e.touch {
            self.idle = 0.0;
            if self.active {
                return;
            }
            self.pending += dt;
            if self.pending >= engage {
                self.active = true;
                self.since = now - f64::from(self.pending);
            }
        } else {
            self.pending = 0.0;
            if self.active {
                self.idle += dt;
                if self.idle >= hold {
                    self.active = false;
                    self.idle = 0.0;
                }
            }
        }
    }
}

const BTN1: usize = 0;
const BTN2: usize = 1;
const STICK: usize = 2;
const MENU: usize = 3;

/// Filtered state of one hand.
#[derive(Clone, Debug)]
pub struct HandState {
    params: Params,
    trigger_f: OneEuro,
    grip_f: OneEuro,
    thumb_sd: SmoothDamp,
    contacts: [Contact; 4],
    stick_deflected: bool,
    clock: f64,
    last_ns: Option<u64>,
    selected: Option<usize>,
    curls: Curls,
}

impl HandState {
    pub fn new(params: Params) -> Self {
        let thumb_sd = SmoothDamp::new(
            params.thumb_rest,
            params.thumb_smooth_time_s,
            params.thumb_max_speed,
        );
        let mut s = Self {
            trigger_f: OneEuro::new(
                params.trigger_min_cutoff,
                params.trigger_beta,
                params.d_cutoff,
            ),
            grip_f: OneEuro::new(params.grip_min_cutoff, params.grip_beta, params.d_cutoff),
            thumb_sd,
            contacts: [Contact::default(); 4],
            stick_deflected: false,
            clock: 0.0,
            last_ns: None,
            selected: None,
            curls: Curls::default(),
            params,
        };
        s.curls = s.compose(0.0, 0.0);
        s
    }

    /// Back to the relaxed rest pose; the next update starts from its input directly.
    pub fn reset(&mut self) {
        *self = Self::new(self.params.clone());
    }

    pub fn params(&self) -> &Params {
        &self.params
    }

    /// Latest curls.
    pub fn curls(&self) -> Curls {
        self.curls
    }

    /// Where the thumb currently rests.
    pub fn thumb_state(&self) -> ThumbState {
        self.selected
            .map_or(ThumbState::Rest, ThumbState::from_index)
    }

    /// Feed one report taken at `now_ns` (any monotonic clock); returns the new curls.
    pub fn update(&mut self, now_ns: u64, input: &Input) -> Curls {
        let inp = sanitize(input);
        let gap_s = match self.last_ns {
            None => None,
            Some(last) if now_ns > last => Some((now_ns - last) as f64 * 1e-9),
            Some(_) => Some(0.0), // duplicate or going backwards: same instant
        };
        match gap_s {
            None => self.restart(&inp),
            Some(g) if g > f64::from(self.params.reset_gap_s) => self.restart(&inp),
            Some(g) => self.step(&inp, (g as f32).min(MAX_DT_S)),
        }
        match self.last_ns {
            Some(last) if now_ns <= last => {} // never let the clock go backwards
            _ => self.last_ns = Some(now_ns),
        }
        self.curls
    }

    /// Start from this input with no history (first report, or after a long silence).
    fn restart(&mut self, inp: &Input) {
        self.trigger_f.reset();
        self.grip_f.reset();
        let (t, g) = (
            self.trigger_f.update(inp.trigger, 0.0),
            self.grip_f.update(inp.grip, 0.0),
        );
        self.contacts = [Contact::default(); 4];
        self.stick_deflected = false;
        self.clock = 0.0;
        self.update_contacts(inp, 0.0, true);
        let target = self.thumb_target();
        self.thumb_sd.snap(target);
        self.curls = self.compose(t, g);
    }

    fn step(&mut self, inp: &Input, dt: f32) {
        self.clock += f64::from(dt);
        let t = self.trigger_f.update(inp.trigger, dt);
        let g = self.grip_f.update(inp.grip, dt);
        self.update_contacts(inp, dt, false);
        let target = self.thumb_target();
        self.thumb_sd.update(target, dt);
        self.curls = self.compose(t, g);
    }

    fn update_contacts(&mut self, inp: &Input, dt: f32, immediate: bool) {
        let mag = inp.stick_x.hypot(inp.stick_y);
        self.stick_deflected = if self.stick_deflected {
            mag > self.params.stick_off
        } else {
            mag > self.params.stick_on
        };
        let f = inp.flags;
        let (engage, hold, now) = (self.params.engage_s, self.params.release_hold_s, self.clock);
        let ev = |touch: u32, click: u32, deflected: bool| Evidence {
            touch: f & touch != 0,
            firm: f & click != 0 || deflected,
            pressed: f & click != 0,
        };
        let evidence = [
            ev(flags::TOUCH_BTN1, flags::CLICK_BTN1, false),
            ev(flags::TOUCH_BTN2, flags::CLICK_BTN2, false),
            ev(flags::TOUCH_STICK, flags::CLICK_STICK, self.stick_deflected),
            ev(0, flags::CLICK_MENU, false),
        ];
        for (c, e) in self.contacts.iter_mut().zip(evidence) {
            c.update(e, dt, now, engage, hold, immediate);
        }

        // The thumb goes to the most recently touched place; if that is released it falls back to an older one.
        self.selected = (0..self.contacts.len())
            .filter(|&i| self.contacts[i].active)
            .max_by(|&a, &b| {
                self.contacts[a]
                    .since
                    .total_cmp(&self.contacts[b].since)
                    .then(b.cmp(&a)) // equal times: lower index wins
            });
    }

    fn thumb_target(&self) -> f32 {
        let p = &self.params;
        match self.selected {
            None => p.thumb_rest,
            Some(i) => {
                let base = match i {
                    BTN1 => p.thumb_btn1,
                    BTN2 => p.thumb_btn2,
                    STICK => p.thumb_stick,
                    MENU => p.thumb_menu,
                    _ => p.thumb_rest,
                };
                let extra = if self.contacts[i].clicked {
                    p.thumb_click_extra
                } else {
                    0.0
                };
                (base + extra).clamp(0.0, 1.0)
            }
        }
    }

    fn compose(&self, trigger: f32, grip: f32) -> Curls {
        let p = &self.params;
        let t = shape(trigger, p.deadzone);
        let g = shape(grip, p.deadzone);
        let index = lerp(p.index_rest, p.index_full, t).clamp(0.0, 1.0);
        let lower = lerp(p.grip_rest, p.grip_full, g).clamp(0.0, 1.0);
        Curls {
            little: lower,
            ring: lower,
            middle: lower,
            index,
            thumb: self.thumb_sd.value().clamp(0.0, 1.0),
        }
    }
}

/// Joint curls of one finger in the layout of Monado's `struct u_hand_tracking_finger_value`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct FingerJoints {
    pub splay: f32,
    pub joint_curls: [f32; 4],
}

/// All five fingers in the order of `struct u_hand_tracking_values`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandJoints {
    pub little: FingerJoints,
    pub ring: FingerJoints,
    pub middle: FingerJoints,
    pub index: FingerJoints,
    pub thumb: FingerJoints,
}

/// Natural resting splay of index, middle, ring, little (what the Monado hand simulation starts from).
pub const SPLAY: [f32; 4] = [-0.01, 0.0, 0.01, 0.02];
/// Thumb splay used for Index controllers in Monado.
pub const THUMB_SPLAY: f32 = -0.35;

/// Spread one normalised curl over the joints of a finger exactly like
/// `u_hand_sim_simulate_for_valve_index_knuckles` does, expressed for `u_hand_sim_simulate_generic`:
/// `[metacarpal, proximal (MCP), intermediate (PIP), distal (DIP)]` flexion in radians.
pub fn to_joints(c: &Curls) -> HandJoints {
    let cl = |v: f32| clamp01(v);
    // Index: proximal swing is val_turn * 1.3, both inner joints val_turn, with val_turn = curl * 1.1.
    let idx = cl(c.index);
    let index = FingerJoints {
        splay: SPLAY[0],
        joint_curls: [0.0, idx * 1.1 * 1.3, idx * 1.1, idx * 1.1],
    };
    // The other three: val_turn = curl * 1.1 * 1.3; proximal swing val_turn * 1.3, PIP val_turn, DIP val_turn * 0.4.
    let other = |curl: f32, splay: f32| {
        let v = cl(curl) * 1.1 * 1.3;
        FingerJoints {
            splay,
            joint_curls: [0.0, v * 1.3, v, v * 0.4],
        }
    };
    let th = cl(c.thumb);
    HandJoints {
        little: other(c.little, SPLAY[3]),
        ring: other(c.ring, SPLAY[2]),
        middle: other(c.middle, SPLAY[1]),
        index,
        // Generic thumb: [0] curl of the metacarpal (x 0.08), [1] and [2] the two phalanx joints.
        thumb: FingerJoints {
            splay: THUMB_SPLAY,
            joint_curls: [th, th * 1.57, th * 1.4, 0.0],
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// Drive a state with a constant input for `ms` milliseconds at 4 ms steps (the controllers report at ~250 Hz).
    struct Rig {
        s: HandState,
        t: u64,
    }
    impl Rig {
        fn new() -> Self {
            Self {
                s: HandState::new(Params::default()),
                t: 1_000 * MS,
            }
        }
        fn run(&mut self, ms: u64, i: Input) -> Curls {
            let mut c = self.s.curls();
            for _ in 0..(ms / 4) {
                self.t += 4 * MS;
                c = self.s.update(self.t, &i);
            }
            c
        }
    }
    fn inp(trigger: f32, grip: f32, flags: u32) -> Input {
        Input {
            trigger,
            grip,
            flags,
            ..Input::default()
        }
    }

    #[test]
    fn rest_pose_is_relaxed() {
        let mut r = Rig::new();
        let c = r.run(500, Input::default());
        let p = Params::default();
        assert!((c.index - p.index_rest).abs() < 1e-3);
        for v in [c.little, c.ring, c.middle] {
            assert!((v - p.grip_rest).abs() < 1e-3);
        }
        assert!(c.thumb.abs() < 1e-3);
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn trigger_drives_only_the_index() {
        let mut r = Rig::new();
        let rest = r.run(300, Input::default());
        let full = r.run(600, inp(1.0, 0.0, 0));
        assert!(
            (full.index - Params::default().index_full).abs() < 1e-3,
            "{full:?}"
        );
        assert_eq!(full.middle, rest.middle);
        assert_eq!(full.ring, rest.ring);
        assert_eq!(full.little, rest.little);
        assert_eq!(full.thumb, rest.thumb);
    }

    #[test]
    fn grip_drives_only_the_three_lower_fingers() {
        let mut r = Rig::new();
        let rest = r.run(300, Input::default());
        let full = r.run(600, inp(0.0, 1.0, 0));
        for v in [full.little, full.ring, full.middle] {
            assert!((v - 1.0).abs() < 1e-3, "{full:?}");
        }
        assert_eq!(full.index, rest.index);
        assert_eq!(full.thumb, rest.thumb);
    }

    #[test]
    fn curl_is_monotonic_in_trigger_and_grip() {
        for pull_trigger in [true, false] {
            let mut prev = -1.0;
            for k in 0..=50 {
                let v = k as f32 / 50.0;
                let mut r = Rig::new();
                let c = if pull_trigger {
                    r.run(800, inp(v, 0.0, 0)).index
                } else {
                    r.run(800, inp(0.0, v, 0)).middle
                };
                assert!(c >= prev - 1e-4, "not monotonic at {v}: {c} < {prev}");
                prev = c;
            }
        }
    }

    #[test]
    fn deadzone_keeps_noise_out() {
        let mut r = Rig::new();
        let a = r.run(500, inp(0.0, 0.0, 0));
        let b = r.run(500, inp(0.015, 0.015, 0));
        assert_eq!(a.index, b.index);
        assert_eq!(a.middle, b.middle);
    }

    #[test]
    fn thumb_follows_touch_and_goes_back_up() {
        let mut r = Rig::new();
        r.run(200, Input::default());
        let c = r.run(600, inp(0.0, 0.0, flags::TOUCH_BTN1));
        assert!(
            (c.thumb - Params::default().thumb_btn1).abs() < 0.01,
            "{c:?}"
        );
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1);
        let c = r.run(600, Input::default());
        assert!(c.thumb < 0.01, "{c:?}");
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn thumb_places_differ_per_button() {
        let p = Params::default();
        for (fl, state, want) in [
            (flags::TOUCH_BTN1, ThumbState::Btn1, p.thumb_btn1),
            (flags::TOUCH_BTN2, ThumbState::Btn2, p.thumb_btn2),
            (flags::TOUCH_STICK, ThumbState::Stick, p.thumb_stick),
            (
                flags::CLICK_MENU,
                ThumbState::Menu,
                p.thumb_menu + p.thumb_click_extra,
            ),
        ] {
            let mut r = Rig::new();
            r.run(100, Input::default());
            let c = r.run(800, inp(0.0, 0.0, fl));
            assert_eq!(r.s.thumb_state(), state);
            assert!(
                (c.thumb - want).abs() < 0.01,
                "{state:?}: {} vs {want}",
                c.thumb
            );
        }
    }

    #[test]
    fn click_presses_deeper_than_touch() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        let touch = r.run(600, inp(0.0, 0.0, flags::TOUCH_BTN2));
        let click = r.run(600, inp(0.0, 0.0, flags::TOUCH_BTN2 | flags::CLICK_BTN2));
        assert!(click.thumb > touch.thumb + 0.1, "{touch:?} {click:?}");
        let back = r.run(600, inp(0.0, 0.0, flags::TOUCH_BTN2));
        assert!((back.thumb - touch.thumb).abs() < 0.01);
    }

    #[test]
    fn click_alone_counts_as_contact() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        r.run(200, inp(0.0, 0.0, flags::CLICK_BTN1));
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1);
    }

    #[test]
    fn most_recent_contact_wins_and_older_one_comes_back() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        r.run(200, inp(0.0, 0.0, flags::TOUCH_BTN1));
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1);
        r.run(200, inp(0.0, 0.0, flags::TOUCH_BTN1 | flags::TOUCH_STICK));
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
        r.run(200, inp(0.0, 0.0, flags::TOUCH_BTN1)); // stick released
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1);
        r.run(200, Input::default());
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn short_touch_dropouts_are_debounced() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        r.run(300, inp(0.0, 0.0, flags::TOUCH_BTN1));
        r.run(100, Input::default()); // 100 ms < 150 ms hold
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1);
        r.run(200, inp(0.0, 0.0, flags::TOUCH_BTN1));
        r.run(200, Input::default()); // 200 ms > hold
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn touch_blips_while_squeezing_do_not_move_the_thumb() {
        // Real captures: squeezing the trigger or grip makes a touch bit blink for 50-100 ms.
        let mut r = Rig::new();
        r.run(300, Input::default());
        for blip_ms in [16u64, 56, 76, 96, 100] {
            r.run(40, inp(1.0, 0.0, 0));
            let c = r.run(
                blip_ms,
                inp(1.0, 0.0, flags::TOUCH_BTN2 | flags::TOUCH_STICK),
            );
            assert_eq!(r.s.thumb_state(), ThumbState::Rest, "{blip_ms} ms blip");
            let c2 = r.run(300, Input::default());
            assert!(
                c.thumb < 0.05 && c2.thumb < 0.05,
                "thumb twitched on a {blip_ms} ms blip"
            );
        }
    }

    #[test]
    fn a_real_touch_is_taken_over_after_the_engage_time() {
        let mut r = Rig::new();
        r.run(300, Input::default());
        r.run(100, inp(0.0, 0.0, flags::TOUCH_BTN1));
        assert_eq!(r.s.thumb_state(), ThumbState::Rest, "100 ms is not enough");
        r.run(60, inp(0.0, 0.0, flags::TOUCH_BTN1));
        assert_eq!(r.s.thumb_state(), ThumbState::Btn1, "160 ms is");
    }

    #[test]
    fn clicks_and_stick_deflection_skip_the_engage_time() {
        let mut r = Rig::new();
        r.run(300, Input::default());
        r.run(8, inp(0.0, 0.0, flags::CLICK_BTN2));
        assert_eq!(r.s.thumb_state(), ThumbState::Btn2);
        r.run(400, Input::default());
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
        r.run(
            8,
            Input {
                stick_x: 0.8,
                ..Input::default()
            },
        );
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
    }

    #[test]
    fn stick_deflection_is_not_a_press() {
        let mut r = Rig::new();
        r.run(300, Input::default());
        let p = Params::default();
        let deflected = r.run(
            600,
            Input {
                stick_x: 0.8,
                ..Input::default()
            },
        );
        assert!(
            (deflected.thumb - p.thumb_stick).abs() < 0.01,
            "{deflected:?}"
        );
        let clicked = r.run(
            600,
            Input {
                stick_x: 0.8,
                flags: flags::CLICK_STICK,
                ..Input::default()
            },
        );
        assert!(
            (clicked.thumb - (p.thumb_stick + p.thumb_click_extra)).abs() < 0.01,
            "{clicked:?}"
        );
    }

    #[test]
    fn flickering_touch_bits_are_bridged() {
        // the stick bit blinks with 40-140 ms gaps while the thumb stays on it (real capture of a stick sweep)
        let mut r = Rig::new();
        r.run(300, Input::default());
        r.run(300, inp(0.0, 0.0, flags::TOUCH_STICK));
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
        for gap_ms in [40u64, 64, 100, 140, 40, 120] {
            r.run(gap_ms, Input::default());
            assert_eq!(r.s.thumb_state(), ThumbState::Stick, "{gap_ms} ms gap");
            r.run(60, inp(0.0, 0.0, flags::TOUCH_STICK));
        }
    }

    #[test]
    fn stick_deflection_implies_touch_with_hysteresis() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        let mut i = Input {
            stick_x: 0.15, // between off (0.12) and on (0.20): not yet touching
            ..Input::default()
        };
        r.run(200, i);
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
        i.stick_x = 0.5;
        r.run(200, i);
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
        i.stick_x = 0.15; // still above off: stays
        r.run(200, i);
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
        i.stick_x = 0.05;
        r.run(300, i);
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn thumb_moves_smoothly_not_in_a_jump() {
        let mut r = Rig::new();
        r.run(100, Input::default());
        let mut prev = r.s.curls().thumb;
        let mut max_step = 0.0f32;
        let mut reached_at = None;
        for k in 1..=200u64 {
            r.t += 4 * MS;
            let c = r.s.update(r.t, &inp(0.0, 0.0, flags::TOUCH_STICK));
            max_step = max_step.max((c.thumb - prev).abs());
            prev = c.thumb;
            if reached_at.is_none() && c.thumb > 0.9 * Params::default().thumb_stick {
                reached_at = Some(k * 4);
            }
        }
        assert!(max_step < 0.08, "jump {max_step}");
        let ms = reached_at.expect("never arrived");
        assert!((40..400).contains(&ms), "took {ms} ms");
    }

    #[test]
    fn long_silence_restarts_from_the_new_sample() {
        let mut r = Rig::new();
        r.run(300, inp(0.0, 0.0, flags::TOUCH_BTN1));
        r.t += 3_000 * MS; // controller was asleep
        let c = r.s.update(r.t, &inp(1.0, 1.0, flags::TOUCH_STICK));
        let p = Params::default();
        assert!((c.index - p.index_full).abs() < 1e-4, "{c:?}");
        assert!((c.middle - 1.0).abs() < 1e-4);
        assert!((c.thumb - p.thumb_stick).abs() < 1e-4);
        assert_eq!(r.s.thumb_state(), ThumbState::Stick);
    }

    #[test]
    fn first_sample_has_no_ramp() {
        let mut s = HandState::new(Params::default());
        let c = s.update(5_000 * MS, &inp(0.5, 0.5, 0));
        let p = Params::default();
        assert!((c.index - lerp(p.index_rest, p.index_full, shape(0.5, p.deadzone))).abs() < 1e-5);
    }

    #[test]
    fn backwards_or_repeated_timestamps_are_harmless() {
        let mut r = Rig::new();
        r.run(100, inp(0.5, 0.5, 0));
        let before = r.s.curls();
        let c = r.s.update(r.t, &inp(0.5, 0.5, 0)); // same instant
        assert_eq!(c, before);
        let c = r.s.update(r.t - 50 * MS, &inp(0.5, 0.5, 0)); // earlier
        assert!(c.index.is_finite());
        // time keeps going from the latest timestamp
        r.t += 4 * MS;
        let c = r.s.update(r.t, &inp(1.0, 1.0, 0));
        assert!(c.index >= before.index);
    }

    #[test]
    fn garbage_input_is_sanitised() {
        let mut s = HandState::new(Params::default());
        let mut t = 10 * MS;
        for bad in [
            f32::NAN,
            f32::INFINITY,
            f32::NEG_INFINITY,
            -5.0,
            7.0,
            f32::MAX,
            f32::MIN,
        ] {
            t += 4 * MS;
            let c = s.update(
                t,
                &Input {
                    trigger: bad,
                    grip: bad,
                    stick_x: bad,
                    stick_y: bad,
                    flags: u32::MAX,
                },
            );
            for v in c.to_array() {
                assert!(v.is_finite() && (0.0..=1.0).contains(&v), "{bad}: {c:?}");
            }
        }
    }

    #[test]
    fn reset_returns_to_rest() {
        let mut r = Rig::new();
        r.run(500, inp(1.0, 1.0, flags::TOUCH_STICK));
        r.s.reset();
        let p = Params::default();
        let c = r.s.curls();
        assert!((c.index - p.index_rest).abs() < 1e-6 && c.thumb == 0.0);
        assert_eq!(r.s.thumb_state(), ThumbState::Rest);
    }

    #[test]
    fn jitter_on_the_trigger_is_filtered() {
        let mut r = Rig::new();
        r.run(300, inp(0.5, 0.0, 0));
        let mut lo = f32::MAX;
        let mut hi = f32::MIN;
        let mut x = 0x2468_ace1u32;
        for _ in 0..500 {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            let noise = ((x as f32 / u32::MAX as f32) - 0.5) * 0.02; // +-1 %
            r.t += 4 * MS;
            let c = r.s.update(r.t, &inp(0.5 + noise, 0.0, 0));
            lo = lo.min(c.index);
            hi = hi.max(c.index);
        }
        // raw jitter would be 0.02 * (index_full - index_rest) / (1 - deadzone) ~ 0.015
        assert!(hi - lo < 0.006, "{}", hi - lo);
    }

    #[test]
    fn fast_trigger_pull_is_followed_quickly() {
        let mut r = Rig::new();
        r.run(300, Input::default());
        let target = Params::default().index_full;
        let mut reached_ms = None;
        for k in 1..=100u64 {
            r.t += 4 * MS;
            let c = r.s.update(r.t, &inp(1.0, 0.0, 0));
            if reached_ms.is_none() && c.index > 0.9 * target {
                reached_ms = Some(k * 4);
            }
        }
        let ms = reached_ms.expect("never arrived");
        assert!(ms < 160, "took {ms} ms");
    }

    #[test]
    fn joint_curls_match_the_index_knuckles_simulation() {
        // Numbers of u_hand_sim_simulate_for_valve_index_knuckles at curl 1.
        let all = to_joints(&Curls {
            little: 1.0,
            ring: 1.0,
            middle: 1.0,
            index: 1.0,
            thumb: 1.0,
        });
        let near = |a: f32, b: f32| (a - b).abs() < 1e-4;
        let i = all.index.joint_curls;
        assert!(
            near(i[0], 0.0) && near(i[1], 1.43) && near(i[2], 1.1) && near(i[3], 1.1),
            "{i:?}"
        );
        for f in [all.middle, all.ring, all.little] {
            let j = f.joint_curls;
            assert!(
                near(j[0], 0.0) && near(j[1], 1.859) && near(j[2], 1.43) && near(j[3], 0.572),
                "{j:?}"
            );
        }
        let t = all.thumb.joint_curls;
        assert!(
            near(t[0], 1.0) && near(t[1], 1.57) && near(t[2], 1.4) && near(t[3], 0.0),
            "{t:?}"
        );
        assert_eq!(all.thumb.splay, -0.35);
        assert_eq!(
            [
                all.index.splay,
                all.middle.splay,
                all.ring.splay,
                all.little.splay
            ],
            SPLAY
        );
    }

    #[test]
    fn joint_curls_zero_is_an_open_hand_and_scales_linearly() {
        let open = to_joints(&Curls::default());
        for f in [open.little, open.ring, open.middle, open.index, open.thumb] {
            assert_eq!(f.joint_curls, [0.0; 4]);
        }
        let half = to_joints(&Curls {
            index: 0.5,
            ..Curls::default()
        });
        let full = to_joints(&Curls {
            index: 1.0,
            ..Curls::default()
        });
        for k in 0..4 {
            assert!((half.index.joint_curls[k] * 2.0 - full.index.joint_curls[k]).abs() < 1e-5);
        }
    }

    #[test]
    fn joint_curls_clamp_out_of_range_input() {
        let j = to_joints(&Curls {
            little: 5.0,
            ring: -1.0,
            middle: f32::NAN,
            index: 2.0,
            thumb: f32::INFINITY,
        });
        for f in [j.little, j.ring, j.middle, j.index, j.thumb] {
            for v in f.joint_curls {
                assert!(v.is_finite() && (0.0..=2.0).contains(&v));
            }
        }
        assert_eq!(j.ring.joint_curls, [0.0; 4]);
        assert_eq!(j.middle.joint_curls, [0.0; 4]);
    }
}
