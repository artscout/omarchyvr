//! Bookkeeping and verdict of `xr-pinchtest` (the OpenXR glue is in `src/bin/xr_pinch.rs`).
//!
//! The Pimax controllers offer `/interaction_profiles/ext/hand_interaction_ext` (pinch = trigger) to apps that only
//! suggest hand-interaction bindings. The test binds `pinch_ext/value` and `pinch_ext/ready_ext` on that profile only,
//! then checks that the runtime picked the profile for the hands, that the action is active and, with the scripted
//! demo input (`PIMAX_CTRL_HANDS_DEMO=1`, the trigger sweeps 0 -> 1 -> 0), that the value covers the whole range.

/// The interaction profile the controllers must report.
pub const HAND_INTERACTION_PROFILE: &str = "/interaction_profiles/ext/hand_interaction_ext";

/// What one hand saw.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PinchStats {
    pub samples: u32,
    pub active: u32,
    pub ready: u32,
    pub min: Option<f32>,
    pub max: Option<f32>,
    /// `xrGetCurrentInteractionProfile` for this hand (last non-empty answer).
    pub profile: Option<String>,
}

impl PinchStats {
    /// One synced sample: value / active of the pinch action, `ready_ext`.
    pub fn add(&mut self, value: f32, active: bool, ready: bool) {
        self.samples += 1;
        if !active {
            return;
        }
        self.active += 1;
        if ready {
            self.ready += 1;
        }
        self.min = Some(self.min.map_or(value, |m| m.min(value)));
        self.max = Some(self.max.map_or(value, |m| m.max(value)));
    }

    /// `Ok` with a one-line summary, or `Err` with what failed. `demo`: expect the scripted trigger sweep.
    pub fn verdict(&self, demo: bool) -> Result<String, String> {
        let summary = format!(
            "{} samples, active {}, ready {}, value {} .. {}, profile {}",
            self.samples,
            self.active,
            self.ready,
            self.min.map_or("-".into(), |v| format!("{v:.2}")),
            self.max.map_or("-".into(), |v| format!("{v:.2}")),
            self.profile.as_deref().unwrap_or("(none)")
        );
        let mut errs = vec![];
        if self.profile.as_deref() != Some(HAND_INTERACTION_PROFILE) {
            errs.push(format!(
                "current interaction profile is not {HAND_INTERACTION_PROFILE}"
            ));
        }
        if self.samples == 0 || (self.active as f64) < 0.5 * self.samples as f64 {
            errs.push("pinch action active in less than half of the samples".to_string());
        }
        if demo {
            if !self.min.is_some_and(|v| v < 0.1) || !self.max.is_some_and(|v| v > 0.9) {
                errs.push(
                    "the demo trigger sweep (0 -> 1) did not show up in pinch_ext/value"
                        .to_string(),
                );
            }
            if self.ready == 0 {
                errs.push("pinch_ext/ready_ext never true (finger on the trigger)".to_string());
            }
        }
        if errs.is_empty() {
            Ok(summary)
        } else {
            Err(format!("{summary}: {}", errs.join("; ")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn swept() -> PinchStats {
        let mut s = PinchStats {
            profile: Some(HAND_INTERACTION_PROFILE.into()),
            ..Default::default()
        };
        for i in 0..=20 {
            let v = i as f32 / 20.0;
            s.add(v, true, v > 0.06);
        }
        s
    }

    #[test]
    fn a_full_sweep_on_the_right_profile_passes() {
        assert!(swept().verdict(true).is_ok());
        assert!(swept().verdict(false).is_ok());
    }

    #[test]
    fn wrong_profile_inactive_or_no_sweep_fail() {
        let mut s = swept();
        s.profile = Some("/interaction_profiles/oculus/touch_controller".into());
        assert!(s
            .verdict(false)
            .unwrap_err()
            .contains("not /interaction_profiles"));

        let mut idle = PinchStats {
            profile: Some(HAND_INTERACTION_PROFILE.into()),
            ..Default::default()
        };
        for _ in 0..10 {
            idle.add(0.0, true, false);
        }
        assert!(
            idle.verdict(false).is_ok(),
            "an idle hand is fine without --demo"
        );
        let e = idle.verdict(true).unwrap_err();
        assert!(e.contains("sweep") && e.contains("ready_ext"));

        let mut inactive = swept();
        for _ in 0..100 {
            inactive.add(0.0, false, false);
        }
        assert!(inactive
            .verdict(false)
            .unwrap_err()
            .contains("less than half"));
        assert!(PinchStats::default().verdict(false).is_err());
    }
}
