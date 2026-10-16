//! Smoothing of the combined gaze for UI use: a One-Euro filter (Casiez et al. 2012) on the gaze tangent, with a
//! saccade snap.
//!
//! One-Euro is a first-order low-pass whose cutoff rises with the speed: `fc = min_cutoff + beta * |speed|` (speed in
//! degrees per second, itself low-passed at `d_cutoff`). At fixation it smooths the ~1 degree sample noise strongly,
//! during a saccade it opens up. Because the speed estimate needs a few samples to rise, a saccade would still be
//! followed with some lag; the snap removes it: when the raw gaze is more than `snap_deg` from the output for two
//! consecutive samples, the filter jumps to the raw value (the eye has landed somewhere else).
//!
//! Defaults (`min_cutoff` 1 Hz, `beta` 0.03 per deg/s, `d_cutoff` 2 Hz, snap 3 degrees): about 0.2 degrees of residual
//! jitter at fixation for 1 degree RMS input noise at 120 Hz, and no visible lag after saccades larger than 3 degrees.

use crate::correction::{dir_of, tangent_of};
use crate::track::{Gaze, Sample};

/// Filter settings (the `[filter]` section of the calibration file).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FilterParams {
    /// Off: the samples pass unchanged.
    pub enabled: bool,
    /// Cutoff at rest, Hz.
    pub min_cutoff_hz: f64,
    /// Cutoff increase per degree per second of speed.
    pub beta: f64,
    /// Cutoff of the speed estimate, Hz.
    pub d_cutoff_hz: f64,
    /// Jump straight to the raw gaze when it stays this far away (degrees, 0 = never).
    pub snap_deg: f64,
}

impl Default for FilterParams {
    fn default() -> Self {
        FilterParams {
            enabled: true,
            min_cutoff_hz: 1.0,
            beta: 0.03,
            d_cutoff_hz: 2.0,
            snap_deg: 3.0,
        }
    }
}

fn alpha(cutoff_hz: f64, dt: f64) -> f64 {
    let tau = 1.0 / (2.0 * std::f64::consts::PI * cutoff_hz.max(1e-6));
    1.0 / (1.0 + tau / dt)
}

/// The filter state for one 2D signal in tangent units.
#[derive(Debug, Clone, Default)]
pub struct OneEuro {
    x: Option<[f64; 2]>,
    dx: [f64; 2],
    t: f64,
    far: u32,
}

/// Samples further apart than this restart the filter.
pub const MAX_GAP_S: f64 = 0.1;

impl OneEuro {
    /// Filter `x` (tangent) at time `t` seconds.
    pub fn filter(&mut self, p: &FilterParams, t: f64, x: [f64; 2]) -> [f64; 2] {
        let Some(prev) = self.x else {
            self.x = Some(x);
            self.dx = [0.0; 2];
            self.t = t;
            return x;
        };
        let dt = t - self.t;
        if dt.is_nan() || dt <= 1e-5 || dt > MAX_GAP_S {
            self.x = Some(x);
            self.dx = [0.0; 2];
            self.t = t;
            self.far = 0;
            return x;
        }
        self.t = t;
        // the raw value far from the output twice in a row: a saccade landed, snap
        let off_deg = ((x[0] - prev[0]).powi(2) + (x[1] - prev[1]).powi(2)).sqrt().atan().to_degrees();
        if p.snap_deg > 0.0 && off_deg > p.snap_deg {
            self.far += 1;
            if self.far >= 2 {
                self.far = 0;
                self.x = Some(x);
                self.dx = [0.0; 2];
                return x;
            }
        } else {
            self.far = 0;
        }
        let ad = alpha(p.d_cutoff_hz, dt);
        let raw_dx = [(x[0] - prev[0]) / dt, (x[1] - prev[1]) / dt];
        self.dx = [
            self.dx[0] + ad * (raw_dx[0] - self.dx[0]),
            self.dx[1] + ad * (raw_dx[1] - self.dx[1]),
        ];
        let speed_dps = (self.dx[0].powi(2) + self.dx[1].powi(2)).sqrt().to_degrees();
        let a = alpha(p.min_cutoff_hz + p.beta * speed_dps, dt);
        let y = [prev[0] + a * (x[0] - prev[0]), prev[1] + a * (x[1] - prev[1])];
        self.x = Some(y);
        y
    }

    /// Forget the state (next sample passes unchanged).
    pub fn reset(&mut self) {
        *self = OneEuro::default();
    }
}

/// Filter the combined gaze direction of a sample in place (no gaze: the filter restarts).
pub fn apply_sample(p: &FilterParams, f: &mut OneEuro, s: &mut Sample) {
    if !p.enabled {
        return;
    }
    let Some(g) = s.gaze.as_mut() else {
        f.reset();
        return;
    };
    let d = [
        f64::from(g.direction[0]),
        f64::from(g.direction[1]),
        f64::from(g.direction[2]),
    ];
    let Some((u, v)) = tangent_of(d) else {
        f.reset();
        return;
    };
    let t = s.host_ts_ns as f64 * 1e-9;
    let y = f.filter(p, t, [u, v]);
    let o = dir_of(y[0], y[1]);
    *g = Gaze {
        direction: [o[0] as f32, o[1] as f32, o[2] as f32],
        ..*g
    };
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calib::stats;


    #[test]
    fn fixation_noise_is_reduced_about_five_times() {
        let p = FilterParams::default();
        let mut f = OneEuro::default();
        let mut rng = crate::rng::Rng::new(99);
        let sigma = 1f64.to_radians();
        let mut raw = Vec::new();
        let mut out = Vec::new();
        for i in 0..1200 {
            let t = i as f64 / 120.0;
            let x = [0.1 + sigma * rng.gauss(), -0.05 + sigma * rng.gauss()];
            let y = f.filter(&p, t, x);
            if i > 240 {
                raw.push(((x[0] - 0.1).powi(2) + (x[1] + 0.05).powi(2)).sqrt().to_degrees());
                out.push(((y[0] - 0.1).powi(2) + (y[1] + 0.05).powi(2)).sqrt().to_degrees());
            }
        }
        let (r, o) = (stats(&raw).mean, stats(&out).mean);
        assert!(o < r / 4.0, "raw {r} filtered {o}");
    }

    #[test]
    fn saccade_lands_without_lag_after_the_snap() {
        let p = FilterParams::default();
        let mut f = OneEuro::default();
        let a = [0.0, 0.0];
        let b = [10f64.to_radians().tan(), 0.0];
        for i in 0..120 {
            f.filter(&p, i as f64 / 120.0, a);
        }
        let t0 = 1.0;
        let y1 = f.filter(&p, t0, b);
        let y2 = f.filter(&p, t0 + 1.0 / 120.0, b);
        assert!(y1[0] < b[0], "first sample far away is still filtered");
        assert_eq!(y2, b, "second sample far away snaps");
        // small moves (below the snap) are followed smoothly and converge within ~0.5 s
        let c = [b[0] + 1f64.to_radians(), 0.0];
        let mut y = y2;
        for i in 2..62 {
            y = f.filter(&p, t0 + i as f64 / 120.0, c);
        }
        assert!((y[0] - c[0]).abs().to_degrees() < 0.1, "{}", (y[0] - c[0]).to_degrees());
    }

    #[test]
    fn gaps_and_missing_gaze_restart_the_filter() {
        let p = FilterParams::default();
        let mut f = OneEuro::default();
        f.filter(&p, 0.0, [0.0, 0.0]);
        f.filter(&p, 1.0 / 120.0, [0.0, 0.0]);
        let y = f.filter(&p, 1.0, [0.2, 0.0]);
        assert_eq!(y, [0.2, 0.0], "a 1 s gap is a restart");
        let mut s = Sample::default();
        apply_sample(&p, &mut f, &mut s);
        assert!(s.gaze.is_none());
        let off = FilterParams {
            enabled: false,
            ..p
        };
        let mut s = Sample {
            gaze: Some(Gaze {
                origin: [0.0; 3],
                direction: [0.0, 0.0, -1.0],
                eyes: 3,
            }),
            ..Sample::default()
        };
        apply_sample(&off, &mut f, &mut s);
        assert_eq!(s.gaze.unwrap().direction, [0.0, 0.0, -1.0]);
    }
}
