//! Continuous drift correction: a slowly updated per-eye offset (tangent units) on top of the calibration, fed by
//! "the wearer was looking at this" events (a recentre dot, a confirmed click on a UI element).
//!
//! Each observation moves the offset a fraction `gain` towards the observed residual (truth minus calibrated gaze);
//! residuals larger than `max_residual_deg` are ignored (the wearer was not looking there), and the total offset is
//! bounded by `max_offset_deg` (beyond that the headset has moved too much: calibrate again).

use crate::correction::dir_of;

/// Settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DriftParams {
    /// Fraction of a residual applied per observation.
    pub gain: f64,
    /// Larger residuals are not believed (degrees).
    pub max_residual_deg: f64,
    /// Bound of the accumulated offset (degrees).
    pub max_offset_deg: f64,
}

impl Default for DriftParams {
    fn default() -> Self {
        DriftParams {
            gain: 0.3,
            max_residual_deg: 5.0,
            max_offset_deg: 4.0,
        }
    }
}

/// The running offsets of both eyes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Drift {
    /// Per eye `[left, right]`, tangent units, added after the calibration map.
    pub offset: [(f64, f64); 2],
    /// Observations accepted per eye.
    pub accepted: [u32; 2],
    /// Observations rejected per eye.
    pub rejected: [u32; 2],
}

fn angle_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    crate::calib::tangent_angle_deg(a, b)
}

impl Drift {
    /// One observation for `eye`: calibrated gaze tangent (without this offset) and the true tangent. Returns
    /// whether it was used.
    pub fn observe(&mut self, p: &DriftParams, eye: usize, gaze: (f64, f64), truth: (f64, f64)) -> bool {
        let cur = self.offset[eye];
        let now = (gaze.0 + cur.0, gaze.1 + cur.1);
        if angle_deg(now, truth) > p.max_residual_deg {
            self.rejected[eye] += 1;
            return false;
        }
        let want = (truth.0 - gaze.0, truth.1 - gaze.1);
        let mut o = (cur.0 + p.gain * (want.0 - cur.0), cur.1 + p.gain * (want.1 - cur.1));
        let lim = p.max_offset_deg.to_radians().tan();
        let l = (o.0 * o.0 + o.1 * o.1).sqrt();
        if l > lim {
            o = (o.0 * lim / l, o.1 * lim / l);
        }
        self.offset[eye] = o;
        self.accepted[eye] += 1;
        true
    }

    /// Apply the offset of `eye` to a calibrated tangent.
    pub fn apply(&self, eye: usize, t: (f64, f64)) -> (f64, f64) {
        (t.0 + self.offset[eye].0, t.1 + self.offset[eye].1)
    }

    /// Unit direction of [`Drift::apply`].
    pub fn apply_dir(&self, eye: usize, t: (f64, f64)) -> [f64; 3] {
        let (u, v) = self.apply(eye, t);
        dir_of(u, v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converges_rejects_outliers_and_is_bounded() {
        let p = DriftParams::default();
        let mut d = Drift::default();
        let slip = (1.5f64.to_radians().tan(), -0.8f64.to_radians().tan());
        for _ in 0..20 {
            let gaze = (0.1 - slip.0, 0.05 - slip.1);
            assert!(d.observe(&p, 0, gaze, (0.1, 0.05)));
        }
        let (u, v) = d.apply(0, (0.1 - slip.0, 0.05 - slip.1));
        assert!(angle_deg((u, v), (0.1, 0.05)) < 0.01);
        assert_eq!(d.offset[1], (0.0, 0.0), "the other eye is untouched");
        // a glance elsewhere (10 deg) is not a drift
        assert!(!d.observe(&p, 0, (0.3, 0.05), (0.1, 0.05)));
        assert_eq!(d.rejected[0], 1);
        // bounded
        let mut d = Drift::default();
        let big = DriftParams {
            max_residual_deg: 30.0,
            gain: 1.0,
            ..p
        };
        d.observe(&big, 1, (0.0, 0.0), (0.2, 0.0));
        let l = (d.offset[1].0.powi(2) + d.offset[1].1.powi(2)).sqrt().atan().to_degrees();
        assert!((l - p.max_offset_deg).abs() < 0.01, "{l}");
    }
}
