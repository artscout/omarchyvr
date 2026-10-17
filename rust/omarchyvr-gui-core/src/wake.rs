//! Waking the Dream Air: the live guidance on the "Get the headset ready" card.
//!
//! Real install, 2026-10-17: without a host the headset's HID chip `34a4:0044` re-enumerates every few seconds and after
//! about two hours it switches OFF for good. USB then shows the bridge `34a4:0043`, the camera and the eye chip, but no
//! `0044`. Pressing the power button, holding it 10 s, a PC restart and putting the headset on did not bring it back;
//! only a power cycle of the headset's POWER cable did (the DisplayPort cable stays in).
//!
//! The state machine (pure; time and USB come in as arguments, so the tests need no hardware):
//! * `0044` seen: **awake**, the tick latches (the chip flaps while no host holds it).
//! * `0044` missing, `0043` there: step A "press the power button", up to [`PRESS_WAIT`] seconds.
//! * Still missing: step B (last resort, never while a VR session runs): unplug the power cable, wait
//!   [`UNPLUG_WAIT`] seconds, plug it back. Unplugging is seen on USB (the bridge vanishes); the person can also
//!   press "I unplugged it".
//! * Never appears: a warning; "Start setup" stays possible.
use crate::headset_choice::HeadsetUsb;

/// Seconds step A waits for the HID chip before step B is offered.
pub const PRESS_WAIT: f64 = 90.0;
/// Seconds the power cable stays out.
pub const UNPLUG_WAIT: f64 = 10.0;
/// Seconds after the cable is back until the chip must be there.
pub const REPLUG_WAIT: f64 = 60.0;
/// Seconds step B may take altogether before the guide gives up.
pub const CABLE_GIVE_UP: f64 = 240.0;

/// What the guide shows.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Wake {
    /// Not started (no guidance needed yet).
    Idle,
    /// Nothing of the headset on USB: connect it first.
    NotConnected,
    /// `0044` seen: done.
    Awake,
    /// Step A: press the power button; `left` seconds until step B.
    PressPower { left: f64 },
    /// Step B, the cable is still in: unplug it.
    UnplugCable,
    /// Step B, the cable is out: `left` seconds to wait.
    WaitUnplugged { left: f64 },
    /// Step B, time is up: plug the cable back in.
    PlugBack,
    /// Step B, the cable is back: waiting for the chip, `left` seconds.
    WaitWake { left: f64 },
    /// Step B is needed but a VR session runs: nothing is touched.
    BlockedBySession,
    /// Nothing worked: a warning (the setup can still start).
    GaveUp,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Phase {
    Idle,
    NotConnected,
    Awake,
    Press { since: f64 },
    Cable { since: f64, out_at: Option<f64>, back_at: Option<f64> },
    GaveUp,
}

/// The guide's state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WakeGuide {
    phase: Phase,
    blocked: bool,
    last: f64,
}

impl Default for WakeGuide {
    fn default() -> Self {
        WakeGuide { phase: Phase::Idle, blocked: false, last: 0.0 }
    }
}

impl WakeGuide {
    /// Start over (the card was opened again).
    pub fn reset(&mut self) {
        *self = WakeGuide::default();
    }

    /// One look at the USB bus. `now`: seconds on any monotonic clock. `session`: a VR session runs.
    pub fn tick(&mut self, now: f64, usb: HeadsetUsb, session: bool) {
        self.last = now;
        self.blocked = false;
        if usb == HeadsetUsb::Awake {
            self.phase = Phase::Awake;
            return;
        }
        match self.phase {
            Phase::Idle | Phase::Awake | Phase::NotConnected | Phase::GaveUp if usb == HeadsetUsb::Absent => {
                if self.phase != Phase::GaveUp {
                    self.phase = Phase::NotConnected;
                }
            }
            Phase::Awake => {}
            Phase::Idle | Phase::NotConnected => self.phase = Phase::Press { since: now },
            Phase::Press { since } => {
                if usb == HeadsetUsb::Absent {
                    // unplugged by itself during step A: connect it first
                    self.phase = Phase::NotConnected;
                } else if now - since >= PRESS_WAIT {
                    if session {
                        self.blocked = true;
                    } else {
                        self.phase = Phase::Cable { since: now, out_at: None, back_at: None };
                    }
                }
            }
            Phase::Cable { since, out_at, back_at } => {
                let (mut out_at, mut back_at) = (out_at, back_at);
                if usb == HeadsetUsb::Absent && out_at.is_none() {
                    out_at = Some(now);
                } else if usb != HeadsetUsb::Absent {
                    if let (Some(o), None) = (out_at, back_at) {
                        // the bridge is back; only a real wait counts as a power cycle
                        if now - o >= 1.0 {
                            back_at = Some(now);
                        }
                    }
                }
                self.phase = if now - since >= CABLE_GIVE_UP { Phase::GaveUp } else { Phase::Cable { since, out_at, back_at } };
            }
            Phase::GaveUp => {}
        }
    }

    /// "I unplugged it": for a cable whose removal the USB bus did not show.
    pub fn unplugged_pressed(&mut self, now: f64) {
        if let Phase::Cable { since, out_at: None, back_at } = self.phase {
            self.phase = Phase::Cable { since, out_at: Some(now), back_at };
        }
    }

    /// "Try again" after the warning.
    pub fn retry(&mut self, now: f64) {
        if self.phase == Phase::GaveUp {
            self.phase = Phase::Press { since: now };
        }
    }

    /// The headset chip was seen awake.
    pub fn is_awake(&self) -> bool {
        self.phase == Phase::Awake
    }

    /// What to show at `now`.
    pub fn view(&self, now: f64) -> Wake {
        let left = |t: f64, total: f64| (total - (now - t)).max(0.0);
        match self.phase {
            Phase::Idle => Wake::Idle,
            Phase::NotConnected => Wake::NotConnected,
            Phase::Awake => Wake::Awake,
            Phase::GaveUp => Wake::GaveUp,
            Phase::Press { since } if self.blocked => {
                let _ = since;
                Wake::BlockedBySession
            }
            Phase::Press { since } => Wake::PressPower { left: left(since, PRESS_WAIT) },
            Phase::Cable { out_at: None, .. } => Wake::UnplugCable,
            Phase::Cable { out_at: Some(o), back_at: None, .. } => {
                let l = left(o, UNPLUG_WAIT);
                if l > 0.0 {
                    Wake::WaitUnplugged { left: l }
                } else {
                    Wake::PlugBack
                }
            }
            Phase::Cable { back_at: Some(b), .. } => Wake::WaitWake { left: left(b, REPLUG_WAIT) },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HeadsetUsb::{Absent, Asleep, Awake};

    #[test]
    fn already_awake_is_a_tick() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Awake, false);
        assert_eq!(g.view(0.0), Wake::Awake);
        // the chip flaps (asleep again for a moment): the tick stays
        g.tick(2.0, Asleep, false);
        assert_eq!(g.view(2.0), Wake::Awake);
        g.tick(5.0, Absent, false);
        assert!(matches!(g.view(5.0), Wake::NotConnected));
    }

    #[test]
    fn missing_then_step_a_then_it_appears() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Asleep, false);
        assert!(matches!(g.view(0.0), Wake::PressPower { left } if (left - 90.0).abs() < 1e-9));
        g.tick(30.0, Asleep, false);
        assert!(matches!(g.view(30.0), Wake::PressPower { left } if (left - 60.0).abs() < 1e-9));
        g.tick(31.0, Awake, false);
        assert_eq!(g.view(31.0), Wake::Awake);
    }

    #[test]
    fn missing_then_step_b_the_power_cycle_then_it_appears() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Asleep, false);
        g.tick(90.0, Asleep, false);
        assert_eq!(g.view(90.0), Wake::UnplugCable, "step B only after step A ran out");
        // the person pulls the power cable: the bridge vanishes from USB
        g.tick(100.0, Absent, false);
        assert!(matches!(g.view(100.0), Wake::WaitUnplugged { left } if (left - 10.0).abs() < 1e-9));
        g.tick(105.0, Absent, false);
        assert!(matches!(g.view(105.0), Wake::WaitUnplugged { left } if (left - 5.0).abs() < 1e-9));
        assert_eq!(g.view(111.0), Wake::PlugBack);
        // plugged back: the bridge returns, the HID chip is still missing, then it comes
        g.tick(115.0, Asleep, false);
        assert!(matches!(g.view(115.0), Wake::WaitWake { .. }));
        g.tick(125.0, Awake, false);
        assert_eq!(g.view(125.0), Wake::Awake);
    }

    #[test]
    fn the_button_covers_a_removal_the_bus_did_not_show() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Asleep, false);
        g.tick(91.0, Asleep, false);
        g.unplugged_pressed(100.0);
        assert!(matches!(g.view(101.0), Wake::WaitUnplugged { .. }));
        assert_eq!(g.view(111.0), Wake::PlugBack);
    }

    #[test]
    fn never_appears_gives_a_warning_and_can_retry() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Asleep, false);
        g.tick(90.0, Asleep, false);
        g.tick(90.0 + CABLE_GIVE_UP, Asleep, false);
        assert_eq!(g.view(400.0), Wake::GaveUp);
        g.tick(500.0, Asleep, false);
        assert_eq!(g.view(500.0), Wake::GaveUp, "stays");
        g.retry(501.0);
        assert!(matches!(g.view(501.0), Wake::PressPower { .. }));
        g.tick(502.0, Awake, false);
        assert!(g.is_awake());
    }

    #[test]
    fn never_the_cable_step_during_a_session() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Asleep, true);
        g.tick(200.0, Asleep, true);
        assert_eq!(g.view(200.0), Wake::BlockedBySession);
        g.tick(201.0, Asleep, false);
        assert_eq!(g.view(201.0), Wake::UnplugCable);
    }

    #[test]
    fn nothing_on_usb_is_not_connected_not_asleep() {
        let mut g = WakeGuide::default();
        g.tick(0.0, Absent, false);
        assert_eq!(g.view(0.0), Wake::NotConnected);
        g.tick(1.0, Asleep, false);
        assert!(matches!(g.view(1.0), Wake::PressPower { .. }));
    }
}
