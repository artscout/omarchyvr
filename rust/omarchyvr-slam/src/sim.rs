//! Offline evaluation of the 6DoF head pose output (`pimax_cam::headpose`) on a real head trajectory: the
//! Windows PVR pose log (640 Hz, smoothed) is taken as the true motion; from it we synthesise the 1 kHz IMU
//! (with noise and gyro bias) and the tracker poses (25 Hz, noise, delivery latency), run the same
//! [`HeadTracker`] the driver uses, query it at display rate for display time, and measure the error against the
//! truth: absolute error, frame-to-frame step error (what the eye sees as jitter or jumps), and the corrections.
//! Optional tracker dropouts test the loss / re-snap path on real motion.

use crate::traj::{interp, PoseRow};
use pimax_cam::geom::{self, Vec3};
use pimax_cam::headpose::{
    flags, qexp, qlog, CorrStats, HeadParams, HeadTracker, ImuSample, Rel, VioSample, G_WORLD,
};
use pimax_cam::slam::{
    qangle_deg, qconj, qmul, qnorm, qrot, qyaw, Quat, SupervisorParams, TrackState,
};

/// Simulation parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct SimParams {
    /// Tracker position noise (m, per axis, white per pose).
    pub vio_noise_m: f64,
    /// Tracker rotation noise (deg, per axis).
    pub vio_rot_noise_deg: f64,
    /// Tracker velocity noise (m/s, per axis).
    pub vio_vel_noise: f64,
    /// Tracker rate (Hz).
    pub vio_hz: f64,
    /// Delivery latency of a tracker pose after its image time (ms).
    pub latency_ms: f64,
    /// Display prediction (query time - now, ms).
    pub predict_ms: f64,
    /// Query rate (Hz).
    pub query_hz: f64,
    /// Gyro white noise per 1 kHz sample (rad/s).
    pub gyro_noise: f64,
    /// Accel white noise per sample (m/s²).
    pub accel_noise: f64,
    /// Constant gyro bias (rad/s).
    pub gyro_bias: Vec3,
    /// Tracker silent in these windows (s from the start), same tracker world afterwards.
    pub dropouts: Vec<(f64, f64)>,
    /// Output parameters.
    pub head: HeadParams,
    /// Smoothing sigma applied to the truth before differentiating (s).
    pub truth_sigma_s: f64,
    /// RNG seed.
    pub seed: u64,
}

impl Default for SimParams {
    /// Noise levels of the IMU model in `pimax_cam::slam::ImuNoise` (4e-4 rad/s/√Hz, 4e-3 m/s²/√Hz at 1 kHz),
    /// a 2 mm / 0.05 deg tracker, 25 ms delivery (12 ms USB + VIO), 20 ms display prediction at 90 Hz.
    fn default() -> Self {
        SimParams {
            vio_noise_m: 0.002,
            vio_rot_noise_deg: 0.05,
            vio_vel_noise: 0.01,
            vio_hz: 25.0,
            latency_ms: 25.0,
            predict_ms: 20.0,
            query_hz: 90.0,
            gyro_noise: 4e-4 * 1000f64.sqrt(),
            accel_noise: 4e-3 * 1000f64.sqrt(),
            gyro_bias: [0.005, -0.008, 0.004],
            dropouts: Vec::new(),
            head: HeadParams::default(),
            truth_sigma_s: 0.015,
            seed: 1,
        }
    }
}

/// Results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SimResult {
    /// Queries evaluated (tracked, after the warm-up).
    pub queries: usize,
    /// Position error p50 / p95 / max (mm), after removing the constant room offset.
    pub pos_err_mm: [f64; 3],
    /// Rotation error p50 / p95 / max (deg).
    pub rot_err_deg: [f64; 3],
    /// Frame-to-frame position step error RMS / p95 / max (mm): output step minus true step.
    pub step_err_mm: [f64; 3],
    /// Frame-to-frame rotation step error RMS / p95 / max (deg).
    pub rot_step_err_deg: [f64; 3],
    /// Correction counters.
    pub corr: CorrStats,
    /// Acquisitions / losses.
    pub acquisitions: u64,
    /// Losses.
    pub losses: u64,
    /// Final supervisor reason.
    pub reason: String,
    /// Learned gyro bias error (deg/s).
    pub bias_err_deg_s: f64,
}

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^= z >> 31;
        (z >> 11) as f64 / (1u64 << 53) as f64
    }
    fn gauss(&mut self) -> f64 {
        (0..12).map(|_| self.next()).sum::<f64>() - 6.0
    }
    fn vec(&mut self, s: f64) -> Vec3 {
        [self.gauss() * s, self.gauss() * s, self.gauss() * s]
    }
}

/// Truth resampled at 1 kHz and smoothed (Gaussian, sigma `sigma_s`) so it can be differentiated twice.
pub fn resample_smooth(r: &[PoseRow], sigma_s: f64) -> Result<Vec<PoseRow>, String> {
    if r.len() < 100 {
        return Err("trajectory too short".into());
    }
    let (t0, t1) = (r[0].ts, r[r.len() - 1].ts);
    let mut raw = Vec::new();
    let mut t = t0;
    while t <= t1 {
        let p = interp(r, t, 200_000_000).ok_or("gap > 200 ms in the truth trajectory")?;
        raw.push(p);
        t += 1_000_000;
    }
    let w = (3.0 * sigma_s * 1000.0).ceil() as isize;
    let ker: Vec<f64> = (-w..=w)
        .map(|k| (-(k as f64 / (sigma_s * 1000.0)).powi(2) / 2.0).exp())
        .collect();
    let mut out = Vec::with_capacity(raw.len());
    for i in w..(raw.len() as isize - w) {
        let (mut p, mut rv, mut ws) = ([0.0; 3], [0.0; 3], 0.0);
        let qc = raw[i as usize].q;
        for (j, &k) in ker.iter().enumerate() {
            let x = &raw[(i - w + j as isize) as usize];
            p = geom::add(p, geom::scale(x.p, k));
            rv = geom::add(rv, geom::scale(qlog(qmul(qconj(qc), x.q)), k));
            ws += k;
        }
        out.push(PoseRow {
            ts: raw[i as usize].ts,
            p: geom::scale(p, 1.0 / ws),
            q: qnorm(qmul(qc, qexp(geom::scale(rv, 1.0 / ws)))),
        });
    }
    Ok(out)
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    v[(((v.len() - 1) as f64) * q).round() as usize]
}

fn rms(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    (v.iter().map(|x| x * x).sum::<f64>() / v.len() as f64).sqrt()
}

/// Longest part of a trajectory without gaps longer than `max_gap_ns` (tracking losses in the truth log).
pub fn longest_segment(r: &[PoseRow], max_gap_ns: i64) -> &[PoseRow] {
    let (mut best, mut start) = ((0usize, 0usize), 0usize);
    for i in 1..=r.len() {
        if i == r.len() || r[i].ts - r[i - 1].ts > max_gap_ns {
            if i - start > best.1 - best.0 {
                best = (start, i);
            }
            start = i;
        }
    }
    &r[best.0..best.1]
}

/// Run the simulation on a truth trajectory (body = IMU pose; the head offset is the driver default). Only the
/// longest gap-free part (no gap > 100 ms) is used.
pub fn simulate(truth: &[PoseRow], p: &SimParams) -> Result<SimResult, String> {
    let tr = resample_smooth(longest_segment(truth, 100_000_000), p.truth_sigma_s)?;
    let dt = 0.001;
    let n = tr.len();
    let head = [0.0, -0.02, 0.07];
    let mut rng = Rng(p.seed);
    // Tracker world: truth yawed and offset (Basalt starts its own world).
    let wq = qyaw(0.7);
    let woff = [1.0, 0.0, -2.0];
    let vel = |k: usize| -> Vec3 {
        let (a, b) = (k.saturating_sub(1), (k + 1).min(n - 1));
        geom::scale(geom::sub(tr[b].p, tr[a].p), 1.0 / ((b - a) as f64 * dt))
    };
    let sp = SupervisorParams::default();
    let mut h = HeadTracker::new(sp, p.head.clone(), head);
    let t_start = tr[0].ts;
    let vio_every = ((1000.0 / p.vio_hz).round() as usize).max(1);
    let lat = (p.latency_ms.round() as usize).max(1);
    let q_every = 1.0 / p.query_hz;
    let mut next_q = 0.0;
    let mut pending: std::collections::VecDeque<(usize, VioSample)> = Default::default();
    let mut outs: Vec<(usize, Rel)> = Vec::new();
    let pred_k = p.predict_ms.max(0.0).round() as usize;
    for k in 1..n.saturating_sub(pred_k + 1) {
        let now = tr[k].ts;
        // IMU sample for the interval (k-1, k].
        let a_w = geom::scale(geom::sub(vel(k), vel(k - 1)), 1.0 / dt);
        let f = qrot(qconj(tr[k - 1].q), geom::sub(a_w, G_WORLD));
        let w = geom::scale(qlog(qmul(qconj(tr[k - 1].q), tr[k].q)), 1.0 / dt);
        h.push_imu(ImuSample {
            ts_ns: now,
            accel: geom::add(f, rng.vec(p.accel_noise)),
            gyro: geom::add(geom::add(w, p.gyro_bias), rng.vec(p.gyro_noise)),
        });
        let ts = (now - t_start) as f64 / 1e9;
        if k % vio_every == 0 && !p.dropouts.iter().any(|&(a, b)| ts >= a && ts < b) {
            let q_noise = qexp(rng.vec(p.vio_rot_noise_deg.to_radians()));
            let s = VioSample {
                ts_ns: now,
                pos: geom::add(geom::add(qrot(wq, tr[k].p), woff), rng.vec(p.vio_noise_m)),
                q: qnorm(qmul(qmul(wq, tr[k].q), q_noise)),
                vel: geom::add(qrot(wq, vel(k)), rng.vec(p.vio_vel_noise)),
                q3dof: tr[k].q,
            };
            pending.push_back((k + lat, s));
        }
        let mut delivered = false;
        while pending.front().is_some_and(|(due, _)| *due <= k) {
            let (_, s) = pending.pop_front().unwrap();
            h.update(now, Some(s));
            delivered = true;
        }
        if ts >= next_q {
            next_q += q_every;
            if !delivered {
                h.update(now, None);
            }
            let kq = k + pred_k;
            let r3 = Rel {
                q: tr[kq].q,
                ang_vel: [0.0; 3],
                flags: flags::ORIENTATION_VALID | flags::ORIENTATION_TRACKED,
                ..Default::default()
            };
            let o = h.head(now, tr[kq].ts, &r3);
            outs.push((kq, o));
        }
    }
    // Room frame = truth yaw (q3dof = truth) + unknown constant translation: estimate it robustly.
    let warm = 2000; // 2 s
    let head_truth =
        |k: usize| -> (Vec3, Quat) { (geom::add(tr[k].p, qrot(tr[k].q, head)), tr[k].q) };
    let tracked: Vec<&(usize, Rel)> = outs
        .iter()
        .filter(|(k, o)| *k > warm && o.flags & flags::POSITION_TRACKED != 0)
        .collect();
    if tracked.len() < 50 {
        return Err(format!("only {} tracked queries", tracked.len()));
    }
    let mut offs: [Vec<f64>; 3] = Default::default();
    for (k, o) in &tracked {
        let d = geom::sub(o.pos, head_truth(*k).0);
        for c in 0..3 {
            offs[c].push(d[c]);
        }
    }
    let off = [
        pct(&mut offs[0], 0.5),
        pct(&mut offs[1], 0.5),
        pct(&mut offs[2], 0.5),
    ];
    let (mut pe, mut re, mut se, mut rse) = (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    for (i, (k, o)) in tracked.iter().enumerate() {
        let (pt, qt) = head_truth(*k);
        pe.push(geom::norm(geom::sub(o.pos, geom::add(pt, off))) * 1000.0);
        re.push(qangle_deg(qmul(qconj(o.q), qt)));
        if i > 0 {
            let (kp, op) = tracked[i - 1];
            if *k - *kp > 20 {
                continue; // not consecutive (a tracking gap)
            }
            let (ptp, qtp) = head_truth(*kp);
            let step = geom::sub(geom::sub(o.pos, op.pos), geom::sub(pt, ptp));
            se.push(geom::norm(step) * 1000.0);
            let d_out = qmul(qconj(op.q), o.q);
            let d_true = qmul(qconj(qtp), qt);
            rse.push(qangle_deg(qmul(qconj(d_out), d_true)));
        }
    }
    let res = SimResult {
        queries: tracked.len(),
        pos_err_mm: [
            pct(&mut pe.clone(), 0.5),
            pct(&mut pe.clone(), 0.95),
            pct(&mut pe, 1.0),
        ],
        rot_err_deg: [
            pct(&mut re.clone(), 0.5),
            pct(&mut re.clone(), 0.95),
            pct(&mut re, 1.0),
        ],
        step_err_mm: [rms(&se), pct(&mut se.clone(), 0.95), pct(&mut se, 1.0)],
        rot_step_err_deg: [rms(&rse), pct(&mut rse.clone(), 0.95), pct(&mut rse, 1.0)],
        corr: h.corr,
        acquisitions: h.sup.acquisitions,
        losses: h.sup.losses,
        reason: h.output().reason.to_string(),
        bias_err_deg_s: geom::norm(geom::sub(h.pred.gyro_bias, p.gyro_bias)).to_degrees(),
    };
    if h.output().state != TrackState::Tracking {
        return Err(format!(
            "ended in state {:?} ({})",
            h.output().state,
            h.output().reason
        ));
    }
    Ok(res)
}

/// One-line summary.
pub fn summary(name: &str, r: &SimResult) -> String {
    format!(
        "{name:<26} err p50/p95/max {:.2}/{:.2}/{:.2} mm {:.3}/{:.3}/{:.3} deg | step err rms/p95/max {:.2}/{:.2}/{:.2} mm {:.3}/{:.3}/{:.3} deg | corr {} blended {} snapped, max {:.1} mm | {} acq {} lost ({}) | bias err {:.3} deg/s",
        r.pos_err_mm[0], r.pos_err_mm[1], r.pos_err_mm[2], r.rot_err_deg[0], r.rot_err_deg[1], r.rot_err_deg[2],
        r.step_err_mm[0], r.step_err_mm[1], r.step_err_mm[2], r.rot_step_err_deg[0], r.rot_step_err_deg[1],
        r.rot_step_err_deg[2], r.corr.blended, r.corr.snapped, r.corr.max_pos * 1000.0, r.acquisitions, r.losses,
        r.reason, r.bias_err_deg_s
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Synthetic head motion (walk + head turns) as a stand-in for the Windows log.
    fn synthetic(seconds: f64) -> Vec<PoseRow> {
        (0..(seconds * 640.0) as usize)
            .map(|i| {
                let t = i as f64 / 640.0;
                let yaw = 0.8 * (0.7 * t).sin() + 0.1 * t;
                let pitch = 0.2 * (1.3 * t).sin();
                PoseRow {
                    ts: (t * 1e9) as i64,
                    p: [
                        0.8 * (0.3 * t).sin(),
                        1.6 + 0.02 * (2.0 * t).sin(),
                        0.8 * (0.3 * t).cos(),
                    ],
                    q: qmul(
                        qyaw(yaw),
                        [(pitch / 2.0).sin(), 0.0, 0.0, (pitch / 2.0).cos()],
                    ),
                }
            })
            .collect()
    }

    #[test]
    fn smoothing_cuts_jitter_on_head_motion() {
        let tr = synthetic(20.0);
        let raw = simulate(
            &tr,
            &SimParams {
                head: HeadParams {
                    smooth: false,
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .unwrap();
        let sm = simulate(&tr, &SimParams::default()).unwrap();
        eprintln!("{}\n{}", summary("raw", &raw), summary("smoothed", &sm));
        assert!(
            sm.step_err_mm[0] < 0.5 * raw.step_err_mm[0],
            "{raw:?}\n{sm:?}"
        );
        assert!(sm.pos_err_mm[1] < 6.0 && sm.rot_err_deg[1] < 0.3, "{sm:?}");
        assert!(sm.bias_err_deg_s < 0.2, "{sm:?}");
    }

    #[test]
    fn dropout_resnaps_on_head_motion() {
        let tr = synthetic(20.0);
        let r = simulate(
            &tr,
            &SimParams {
                dropouts: vec![(8.0, 9.5)],
                ..Default::default()
            },
        )
        .unwrap();
        eprintln!("{}", summary("dropout 1.5 s", &r));
        assert_eq!(r.losses, 1);
        assert!(r.reason.contains("restored"), "{r:?}");
        assert!(r.step_err_mm[2] < 8.0, "{r:?}");
    }
}
