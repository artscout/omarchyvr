//! Drift governor (`PIMAX_SLAM_DRIFT_HOLD`): tracker translation that the accelerometer cannot explain is put into
//! the room alignment (like the desk still hold), so the head output does not slide with a drifting tracker while
//! it is worn. Rotation is never touched.
//!
//! Session 2026-10-18 11:58-12:06 (seated, worn): Basalt's position slid 4.1 m right and 1.6 m up in 8 minutes,
//! 1.5-3.3 cm/s in a quiet 20 s (gyro 1.5-3.7 deg/s, |a| sd under 0.1 m/s²) and 1.2 m in 70 s of fast head turns,
//! while Monado reported "0 lost" and the output followed the tracker (ai/LESSONS.md row 77).
//!
//! The governor looks at the NECK, the pivot a seated head turns about (`neck_in_head` from the head origin):
//! turning moves the head on an arc around it, which the rotation explains, while the neck itself only moves when
//! the body moves (a lean, standing up, walking), and that shakes the accelerometer.
//!
//! * Motion evidence `L`: the neck's linear acceleration (specific force minus the rotation terms
//!   `alpha x r + w x (w x r)`, turned into the world with the tracker attitude carried on by the gyro, gravity
//!   removed), as the root of the summed per-axis variances over the last `act_window_ns`. A variance, not an
//!   integral: the accelerometer bias and gravity leaking through a slightly wrong attitude are nearly constant
//!   over a second and drop out, while integrating them (tried first) made up 2-3 cm/s of "evidence" at rest.
//!   A worn head at rest shows 0.02-0.05 m/s², a lean 0.1-0.4, steps 0.5-2.
//! * Allowed neck speed = `floor + gain_s x max(0, L - noise)` (a smooth motion of peak speed v shows an
//!   acceleration spread of about v / (0.45 T) for a motion lasting T, so `gain_s` ~ 1 s lets a 1-2 s lean through).
//! * The tracker's neck speed over the last `window_ns`; what exceeds the allowed speed is held (moved into the
//!   room alignment), fully up to `band_lo` and fading out to nothing at `band_hi` (`turn_band_*` while
//!   turning, where Basalt's slide came in faster bursts): Basalt's slide is slow
//!   (centimetres per second), so anything faster is left to the tracker even when the IMU is unsure.

use crate::geom::{self, Vec3};
use crate::headpose::{qexp, BodyState, ImuSample, G_WORLD};
use crate::slam::{qmul, qnorm, qrot, Quat};
use std::collections::VecDeque;

/// Governor parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct DriftParams {
    /// Hold unexplained tracker translation (`PIMAX_SLAM_DRIFT_HOLD=0` / the app switch off = the old behaviour).
    pub on: bool,
    /// Neck pivot relative to the head origin in the IMU frame (OpenXR axes: x right, y up, z back), m.
    pub neck_in_head: Vec3,
    /// Window (ns) of the tracker neck speed.
    pub window_ns: i64,
    /// Window (ns) of the motion evidence.
    pub act_window_ns: i64,
    /// Evidence level (m/s²) of a worn head at rest (and of the IMU's own noise) ...
    pub noise: f64,
    /// ... and the speed (m/s) allowed per m/s² above it.
    pub gain_s: f64,
    /// Always allowed (m/s).
    pub floor: f64,
    /// The head counts as turning while the bias-corrected gyro passed this (rad/s) within `turn_hold_ns`: the
    /// world-frame evidence is unreliable then (the neck's true place is known only to a few cm, and 0.1 rad of
    /// rotation per 40 ms moves a wrong pivot by millimetres per pose), so only the rotation-free magnitude counts:
    /// the standard deviation of |neck specific force| over `act_window_ns` above `turn_noise` (fast turns alone
    /// reach 0.15-0.2 m/s², steps 0.5-2) allows `gain_s` per m/s².
    pub turn_rate: f64,
    /// See `turn_rate`.
    pub turn_hold_ns: i64,
    /// See `turn_rate`.
    pub turn_noise: f64,
    /// Unexplained speed held completely up to this (m/s) ...
    pub band_lo: f64,
    /// ... and not at all from this one (m/s).
    pub band_hi: f64,
    /// The same band while turning (m/s): Basalt slid in bursts of 0.1-0.2 m/s during the fast turns.
    pub turn_band_lo: f64,
    /// See `turn_band_lo`.
    pub turn_band_hi: f64,
    /// A log report at most this often (ns) ...
    pub report_every_ns: i64,
    /// ... and only when at least this much (m) was held since the last one.
    pub report_min_m: f64,
}

impl Default for DriftParams {
    fn default() -> Self {
        DriftParams {
            on: true,
            neck_in_head: [0.0, -0.10, 0.08],
            window_ns: 250_000_000,
            act_window_ns: 1_000_000_000,
            noise: 0.10,
            gain_s: 1.0,
            floor: 0.0,
            turn_rate: 15f64.to_radians(),
            turn_hold_ns: 500_000_000,
            turn_noise: 0.30,
            band_lo: 0.05,
            band_hi: 0.15,
            turn_band_lo: 0.10,
            turn_band_hi: 0.25,
            report_every_ns: 10_000_000_000,
            report_min_m: 0.01,
        }
    }
}

/// What the governor held (for the log, the app and tests).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DriftStats {
    /// Tracker poses looked at.
    pub poses: u64,
    /// Poses where something was held.
    pub held_poses: u64,
    /// Path length held (m): how far the output would have slid.
    pub held_m: f64,
    /// Net displacement held (world, m).
    pub held_vec: Vec3,
}

/// One log report: held `m` metres in the last `secs` seconds (`total_m` this session).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DriftReport {
    /// Held since the last report (m).
    pub m: f64,
    /// Seconds covered.
    pub secs: f64,
    /// Held this session (m).
    pub total_m: f64,
}

/// Running per-axis variance of a window of vectors.
#[derive(Clone, Debug, Default)]
struct VarWin {
    q: VecDeque<(i64, Vec3)>,
    s: Vec3,
    s2: Vec3,
    pushes: u64,
}

impl VarWin {
    fn push(&mut self, ts: i64, v: Vec3, window_ns: i64) {
        self.q.push_back((ts, v));
        for k in 0..3 {
            self.s[k] += v[k];
            self.s2[k] += v[k] * v[k];
        }
        while self.q.front().is_some_and(|(t, _)| *t < ts - window_ns) {
            let (_, x) = self.q.pop_front().unwrap();
            for k in 0..3 {
                self.s[k] -= x[k];
                self.s2[k] -= x[k] * x[k];
            }
        }
        self.pushes += 1;
        if self.pushes % 4096 == 0 {
            // Re-sum now and then: no rounding builds up over a session.
            (self.s, self.s2) = ([0.0; 3], [0.0; 3]);
            for (_, x) in &self.q {
                for k in 0..3 {
                    self.s[k] += x[k];
                    self.s2[k] += x[k] * x[k];
                }
            }
        }
    }

    /// Root of the summed per-axis variances; 0 until 100 samples.
    fn spread(&self) -> f64 {
        let n = self.q.len() as f64;
        if n < 100.0 {
            return 0.0;
        }
        (0..3)
            .map(|k| {
                let m = self.s[k] / n;
                (self.s2[k] / n - m * m).max(0.0)
            })
            .sum::<f64>()
            .sqrt()
    }

    fn clear(&mut self) {
        *self = VarWin::default();
    }
}

/// The governor state (owned by [`crate::headpose::HeadTracker`]).
#[derive(Clone, Debug)]
pub struct DriftGov {
    /// Parameters.
    pub p: DriftParams,
    /// Neck in the IMU frame.
    n_imu: Vec3,
    att: Option<Quat>,
    last_ts: Option<i64>,
    gyro_hist: VecDeque<(i64, Vec3)>,
    acc: VarWin,
    mag: VarWin,
    last_turn_ns: Option<i64>,
    /// Recent poses: (time, tracker neck), oldest first.
    hist: VecDeque<(i64, Vec3)>,
    /// Counters.
    pub stats: DriftStats,
    report_from: Option<i64>,
    report_m: f64,
    /// Last pose: (tracker neck speed, allowed speed, removed speed), m/s.
    pub last: (f64, f64, f64),
    /// Last pose's neck step (m) and the part held (m).
    pub last_step: (f64, f64),
}

impl DriftGov {
    /// New governor; `head_in_imu` = the head origin in the IMU frame.
    pub fn new(p: DriftParams, head_in_imu: Vec3) -> DriftGov {
        let n_imu = geom::add(head_in_imu, p.neck_in_head);
        DriftGov {
            p,
            n_imu,
            att: None,
            last_ts: None,
            gyro_hist: VecDeque::new(),
            acc: VarWin::default(),
            mag: VarWin::default(),
            last_turn_ns: None,
            hist: VecDeque::new(),
            stats: DriftStats::default(),
            report_from: None,
            report_m: 0.0,
            last: (0.0, 0.0, 0.0),
            last_step: (0.0, 0.0),
        }
    }

    /// Motion evidence `L` (m/s², see the module docs); 0 until the attitude is known and the window is filled.
    pub fn evidence(&self) -> f64 {
        self.acc.spread()
    }

    /// Standard deviation of |neck specific force| over the window (m/s²): rotation does not change it.
    pub fn shake(&self) -> f64 {
        self.mag.spread()
    }

    /// The head turned within `turn_hold_ns` of the newest IMU sample.
    pub fn turning(&self) -> bool {
        matches!((self.last_turn_ns, self.last_ts), (Some(t), Some(now)) if now - t <= self.p.turn_hold_ns)
    }

    /// Neck speed (m/s) the evidence allows now.
    pub fn allowed(&self) -> f64 {
        let ev = if self.turning() {
            (self.shake() - self.p.turn_noise).max(0.0)
        } else {
            (self.evidence() - self.p.noise).max(0.0)
        };
        self.p.floor + self.p.gain_s * ev
    }

    /// One IMU sample (raw); `bias` = the gyro bias in use (rad/s).
    pub fn push_imu(&mut self, s: &ImuSample, bias: Vec3) {
        if self.last_ts.is_some_and(|t| s.ts_ns <= t) {
            return;
        }
        let w = geom::sub(s.gyro, bias);
        // Angular acceleration from a lightly smoothed rate (5 ms) over 20 ms: differentiating the raw 1 kHz rate
        // would turn the gyro's noise into ~0.2 m/s² of made-up acceleration at the neck.
        let ws = match (self.gyro_hist.back(), self.last_ts) {
            (Some(&(_, prev)), Some(t0)) => {
                let k = (((s.ts_ns - t0) as f64 / 5e6).min(1.0)).max(0.0);
                geom::add(prev, geom::scale(geom::sub(w, prev), k))
            }
            _ => w,
        };
        while self.gyro_hist.len() > 1 && self.gyro_hist[1].0 <= s.ts_ns - 20_000_000 {
            self.gyro_hist.pop_front();
        }
        let alpha = match self.gyro_hist.front() {
            Some(&(t0, w0)) if s.ts_ns - t0 >= 10_000_000 && s.ts_ns - t0 <= 40_000_000 => {
                geom::scale(geom::sub(ws, w0), 1e9 / (s.ts_ns - t0) as f64)
            }
            _ => [0.0; 3],
        };
        self.gyro_hist.push_back((s.ts_ns, ws));
        if geom::norm(w) > self.p.turn_rate {
            self.last_turn_ns = Some(s.ts_ns);
        }
        let r = geom::scale(self.n_imu, -1.0);
        let a_rot = geom::add(geom::cross(alpha, r), geom::cross(w, geom::cross(w, r)));
        let f = geom::sub(s.accel, a_rot);
        self.mag.push(s.ts_ns, [geom::norm(f), 0.0, 0.0], self.p.act_window_ns);
        if let (Some(q), Some(t0)) = (self.att, self.last_ts) {
            let dt = ((s.ts_ns - t0) as f64 / 1e9).clamp(0.0, 0.01);
            let q = qnorm(qmul(q, qexp(geom::scale(w, dt))));
            self.att = Some(q);
            // f: the specific force at the neck (the IMU's minus its acceleration from the rotation alone).
            let a_w = geom::add(qrot(q, f), G_WORLD);
            self.acc.push(s.ts_ns, a_w, self.p.act_window_ns);
        }
        self.last_ts = Some(s.ts_ns);
    }

    /// Forget the motion state (tracker reset, loss, desk still hold): the next pose starts a new comparison.
    pub fn reset(&mut self) {
        self.hist.clear();
    }

    /// Speed (m/s) of tracker neck motion `u` the governor removes, given the `allowed` speed.
    pub fn removed_speed(&self, u: f64, allowed: f64) -> f64 {
        if u <= allowed {
            return 0.0;
        }
        let (lo, hi) = if self.turning() {
            (self.p.turn_band_lo, self.p.turn_band_hi)
        } else {
            (self.p.band_lo, self.p.band_hi)
        };
        let w = if u <= lo {
            1.0
        } else if u >= hi || hi <= lo {
            0.0
        } else {
            (hi - u) / (hi - lo)
        };
        (u - allowed) * w
    }

    /// A tracker pose while tracking (`fused` = the predictor's newest state, `q_now` = its attitude carried to the
    /// newest IMU sample, `w_q` = the room yaw). Returns the world shift to apply to the room alignment, if any:
    /// the fraction `removed / u` of this pose's neck step, with `u` the neck speed over the last `window_ns`.
    pub fn on_pose(&mut self, fused: &BodyState, q_now: Quat, w_q: Quat) -> Option<Vec3> {
        if self.att.is_none() {
            self.acc.clear();
        }
        self.att = Some(qnorm(q_now));
        let neck = geom::add(fused.p, qrot(qnorm(fused.q), self.n_imu));
        let mut shift = None;
        if self.hist.back().is_some_and(|h| fused.ts_ns <= h.0 || fused.ts_ns - h.0 > 200_000_000) {
            self.hist.clear();
        }
        if let (Some(&(_, n_prev)), true) = (self.hist.back(), self.p.on) {
            self.stats.poses += 1;
            let i = self.hist.iter().rposition(|h| fused.ts_ns - h.0 >= self.p.window_ns).unwrap_or(0);
            let (t0, n0) = self.hist[i];
            let span = (fused.ts_ns - t0) as f64 / 1e9;
            let u = geom::norm(geom::sub(neck, n0)) / span.max(1e-3);
            let allowed = self.allowed();
            let full = span >= 0.5 * self.p.window_ns as f64 / 1e9 && self.evidence() > 0.0;
            let rem = if full { self.removed_speed(u, allowed) } else { 0.0 };
            self.last = (u, allowed, rem);
            let step = geom::sub(neck, n_prev);
            self.last_step = (geom::norm(step), 0.0);
            if rem > 0.0 && u > 1e-9 {
                let ex = geom::scale(step, (rem / u).min(1.0));
                self.last_step.1 = geom::norm(ex);
                let d = geom::scale(qrot(w_q, ex), -1.0);
                self.stats.held_poses += 1;
                self.stats.held_m += geom::norm(ex);
                self.stats.held_vec = geom::sub(self.stats.held_vec, d);
                self.report_m += geom::norm(ex);
                shift = Some(d);
                // What is held is no longer the tracker's motion to compare.
                for h in self.hist.iter_mut() {
                    h.1 = geom::add(h.1, ex);
                }
            }
        }
        self.hist.push_back((fused.ts_ns, neck));
        while self.hist.len() > 2 && fused.ts_ns - self.hist[1].0 >= self.p.window_ns {
            self.hist.pop_front();
        }
        shift
    }

    /// A report for the log when enough was held since the last one (at most every `report_every_ns`).
    pub fn take_report(&mut self, now_ns: i64) -> Option<DriftReport> {
        let from = *self.report_from.get_or_insert(now_ns);
        if now_ns - from < self.p.report_every_ns {
            return None;
        }
        let m = self.report_m;
        self.report_from = Some(now_ns);
        self.report_m = 0.0;
        (m >= self.p.report_min_m).then(|| DriftReport {
            m,
            secs: (now_ns - from) as f64 / 1e9,
            total_m: self.stats.held_m,
        })
    }
}

/// The log line of a report (parsed by the app, `omarchyvr-gui-core` monlog: keep the words).
pub fn report_line(r: &DriftReport) -> String {
    format!(
        "6DoF drift: head tracking slid {:.0} cm without movement in {:.0} s; held (session total {:.0} cm)",
        r.m * 100.0,
        r.secs,
        r.total_m * 100.0
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn removal_band_is_continuous() {
        let g = DriftGov::new(DriftParams::default(), [0.0; 3]);
        assert_eq!(g.removed_speed(0.02, 0.03), 0.0);
        assert!((g.removed_speed(0.03, 0.0) - 0.03).abs() < 1e-12);
        assert!((g.removed_speed(0.05, 0.0) - 0.05).abs() < 1e-12);
        assert!(g.removed_speed(0.1, 0.0) < 0.05 && g.removed_speed(0.1, 0.0) > 0.0);
        assert_eq!(g.removed_speed(0.15, 0.0), 0.0);
        assert_eq!(g.removed_speed(0.5, 0.0), 0.0);
    }

    #[test]
    fn report_line_has_the_words_the_app_reads() {
        let l = report_line(&DriftReport { m: 0.123, secs: 10.0, total_m: 1.5 });
        assert_eq!(l, "6DoF drift: head tracking slid 12 cm without movement in 10 s; held (session total 150 cm)");
        let mut g = DriftGov::new(DriftParams::default(), [0.0; 3]);
        assert!(g.take_report(0).is_none());
        g.report_m = 0.2;
        assert!(g.take_report(5_000_000_000).is_none(), "not before 10 s");
        assert_eq!(g.take_report(10_000_000_000).map(|r| r.m), Some(0.2));
        assert!(g.take_report(20_000_000_000).is_none(), "nothing new held");
    }
}
