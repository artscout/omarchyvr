//! Offline replay of the driver's still gate (`pimax_cam::slam::StillParams`): what the 6DoF output does while
//! the headset lies still and the tracker, with no landmarks, dead-reckons away.
//!
//! Input: a driver EuRoC recording (1 kHz IMU) and one Basalt run over it (`monado-cli slambatch`
//! `tracking.csv`, no resets). The same [`HeadTracker`] the driver uses gets the IMU, the tracker poses (25 Hz,
//! delivered 25 ms late, y-up like `slam_rel_to_yup`), and a 3DoF orientation from a gravity-corrected gyro
//! filter (the driver's 3DoF fusion has its own tilt, so the 3DoF and tracker attitudes differ by a little, as
//! live). It is queried at 90 Hz for 20 ms ahead.
//!
//! Tracker resets are emulated: after a reset request the tracker continues as a fresh world that starts at
//! the current pose with zero velocity (`p - p_r - v_r (t - t_r)`), gravity-aligned from the accelerometer (the
//! first tracker's attitude offset to the 3DoF filter) in another yaw, which is what a restarted VIO with the
//! same IMU error does; two frames are skipped for the restart.
//!
//! Measured: still-gate firings, 6DoF losses (drops to 3DoF), time without a tracked position, and how much the
//! output moves while the IMU says the headset lies still (it should not move at all).

use crate::euroc::ImuRow;
use crate::traj::PoseRow;
use pimax_cam::geom::{self, Vec3};
use pimax_cam::headpose::{flags, HeadParams, HeadTracker, ImuSample, Rel, VioSample};
use pimax_cam::slam::{
    events, qangle_deg, qconj, qmul, qnorm, qrot, qyaw, yaw_of, Quat, SupervisorParams, QUAT_ID,
};

/// Tracker world (z up) -> driver world (y up), as `SLAM_TO_YUP` in `pimax_camera.c` (x y z w).
const SLAM_TO_YUP: Quat = [
    std::f64::consts::FRAC_1_SQRT_2,
    0.0,
    0.0,
    -std::f64::consts::FRAC_1_SQRT_2,
];

/// Replay parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct GateParams {
    /// Output parameters (the still rule is `head.still`).
    pub head: HeadParams,
    /// Head origin in the IMU frame (m).
    pub head_in_imu: Vec3,
    /// Tracker pose delivery latency (ms).
    pub latency_ms: f64,
    /// Query rate (Hz) and prediction (ms).
    pub query_hz: f64,
    /// See `query_hz`.
    pub predict_ms: f64,
    /// Tracker frames skipped after a reset.
    pub reset_skip: usize,
    /// Tracker velocity from the poses: `false` = causal (second-order backward difference, noisy like a
    /// filter's newest estimate), `true` = central difference (uses the next pose: optimistic).
    pub central_vel: bool,
}

impl Default for GateParams {
    fn default() -> Self {
        GateParams {
            head: HeadParams::default(),
            head_in_imu: [0.049, -0.015, 0.030],
            latency_ms: 25.0,
            query_hz: 90.0,
            predict_ms: 20.0,
            reset_skip: 2,
            central_vel: false,
        }
    }
}

/// Results.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GateResult {
    /// Replayed time (s).
    pub duration_s: f64,
    /// Time the IMU said "lying still" (s) and the number of such periods.
    pub still_s: f64,
    /// See `still_s`.
    pub still_periods: usize,
    /// Still-gate firings (tracker reset because it drifted while still).
    pub firings: u64,
    /// 6DoF losses (drops to 3DoF) of any reason, and acquisitions.
    pub losses: u64,
    /// See `losses`.
    pub acquisitions: u64,
    /// Time without a tracked position after the first acquisition (s).
    pub no_6dof_s: f64,
    /// Largest output movement from where a still period began (mm / deg): the headset did not move.
    pub still_move_mm: f64,
    /// See `still_move_mm`.
    pub still_move_deg: f64,
    /// Largest output step between two queries while still (mm / deg).
    pub still_step_mm: f64,
    /// See `still_step_mm`.
    pub still_step_deg: f64,
    /// Largest blended correction (mm / deg), whole run.
    pub max_corr_mm: f64,
    /// See `max_corr_mm`.
    pub max_corr_deg: f64,
    /// Corrections (tracker pose or supervisor changes) and those with a step >= 20 mm, mostly vertical ones.
    pub corrections: u64,
    /// See `corrections`.
    pub big: u64,
    /// See `corrections`.
    pub big_vertical: u64,
    /// Correction step p50 / p95 / max (mm) and the largest pending offset (mm).
    pub step_mm: [f64; 3],
    /// See `step_mm`.
    pub max_offset_mm: f64,
    /// Mean head speed at the big steps (m/s).
    pub big_speed: f64,
    /// Worn, head nearly still (gyro < 0.15 rad/s over 0.3 s, not lying): time (s), output jitter RMS
    /// vertical / horizontal (mm, minus a 0.5 s moving average), and the tracker's own vertical jitter.
    pub quiet_s: f64,
    /// See `quiet_s`.
    pub jitter_v_mm: f64,
    /// See `quiet_s`.
    pub jitter_h_mm: f64,
    /// See `quiet_s`.
    pub vio_jitter_v_mm: f64,
    /// Output error against the hindsight tracker trajectory (3-pose average, interpolated at the display
    /// time) while tracking: RMS / p95 / max (mm), and its vertical RMS (mm).
    pub err_mm: [f64; 3],
    /// See `err_mm`.
    pub err_v_mm: f64,
    /// Big steps while the head moved slower than 0.25 m/s (the visible ones).
    pub big_slow: u64,
    /// Vertical wander while quiet: RMS of the position minus its 2 s centred average (mm).
    pub wander_v_mm: f64,
    /// While the head moved slower than 0.25 m/s: RMS speed at which pending corrections slid the output
    /// (mm/s), and the time it slid faster than 2 cm/s (s): the visible sliding.
    pub slide_rms_mm_s: f64,
    /// See `slide_rms_mm_s`.
    pub slide_time_s: f64,
}

/// RMS (vertical, horizontal; mm) of the position minus its centred moving average over +-`half` ns, over
/// samples whose whole window is quiet.
fn jitter(series: &[(i64, Vec3, bool)], half: i64) -> (f64, f64) {
    let (mut n, mut sv, mut sh) = (0.0, 0.0, 0.0);
    let (mut j0, mut j1) = (0usize, 0usize);
    let mut sum = [0.0; 3];
    let mut quiet_n = 0usize;
    for i in 0..series.len() {
        let t = series[i].0;
        while j1 < series.len() && series[j1].0 <= t + half {
            sum = geom::add(sum, series[j1].1);
            quiet_n += series[j1].2 as usize;
            j1 += 1;
        }
        while series[j0].0 < t - half {
            sum = geom::sub(sum, series[j0].1);
            quiet_n -= series[j0].2 as usize;
            j0 += 1;
        }
        if j1 >= series.len() || quiet_n != j1 - j0 {
            continue;
        }
        let d = geom::sub(series[i].1, geom::scale(sum, 1.0 / (j1 - j0) as f64));
        n += 1.0;
        sv += d[1] * d[1];
        sh += d[0] * d[0] + d[2] * d[2];
    }
    if n == 0.0 {
        return (0.0, 0.0);
    }
    ((sv / n).sqrt() * 1e3, (sh / n).sqrt() * 1e3)
}

/// Gravity-corrected gyro integration (Mahony, proportional only): the 3DoF orientation, y-up world.
struct Fusion {
    q: Quat,
    bias: Vec3,
    init: bool,
}

impl Fusion {
    fn push(&mut self, s: &ImuRow, dt: f64) {
        let a = s.accel;
        let an = geom::norm(a);
        if !self.init {
            if an < 1.0 {
                return;
            }
            // Shortest rotation taking the measured up (body) to world +y.
            let u = geom::scale(a, 1.0 / an);
            let c = geom::cross(u, [0.0, 1.0, 0.0]);
            self.q = qnorm([c[0], c[1], c[2], 1.0 + u[1]]);
            self.init = true;
            return;
        }
        let mut w = geom::sub(s.gyro, self.bias);
        if (an - 9.81).abs() < 1.0 {
            let up_pred = qrot(qconj(self.q), [0.0, 1.0, 0.0]);
            let e = geom::cross(geom::scale(a, 1.0 / an), up_pred);
            w = geom::add(w, geom::scale(e, 0.5));
        }
        let h = geom::scale(w, 0.5 * dt);
        self.q = qnorm(qmul(self.q, [h[0], h[1], h[2], 1.0]));
    }
}

fn to_yup(r: &PoseRow) -> (Vec3, Quat) {
    (qrot(SLAM_TO_YUP, r.p), qnorm(qmul(SLAM_TO_YUP, r.q)))
}

/// Mean gyro of the first second when it looks still (the driver's bias window), else zero.
fn startup_bias(imu: &[ImuRow]) -> Vec3 {
    let n = imu
        .iter()
        .take_while(|s| s.ts - imu[0].ts < 1_000_000_000)
        .count();
    if n < 100 {
        return [0.0; 3];
    }
    let mut m = [0.0; 3];
    for s in &imu[..n] {
        m = geom::add(m, s.gyro);
    }
    m = geom::scale(m, 1.0 / n as f64);
    let quiet = imu[..n]
        .iter()
        .all(|s| geom::norm(geom::sub(s.gyro, m)) < 0.05);
    if quiet {
        m
    } else {
        [0.0; 3]
    }
}

/// Replay `vio` (Basalt, z-up tracker world) over `imu` through the head tracker.
pub fn run(imu: &[ImuRow], vio: &[PoseRow], p: &GateParams) -> Result<GateResult, String> {
    if imu.len() < 1000 || vio.len() < 50 {
        return Err(format!(
            "too short: {} IMU samples, {} tracker poses",
            imu.len(),
            vio.len()
        ));
    }
    // Tracker poses in the y-up world with central-difference velocities.
    let yup: Vec<(i64, Vec3, Quat)> = vio
        .iter()
        .map(|r| {
            let (pp, q) = to_yup(r);
            (r.ts, pp, q)
        })
        .collect();
    let vel = |k: usize| -> Vec3 {
        if !p.central_vel && k >= 2 {
            let dt = (yup[k].0 - yup[k - 2].0) as f64 / 2e9;
            if dt <= 0.0 {
                return [0.0; 3];
            }
            let d = geom::add(
                geom::sub(geom::scale(yup[k].1, 3.0), geom::scale(yup[k - 1].1, 4.0)),
                yup[k - 2].1,
            );
            return geom::scale(d, 0.5 / dt);
        }
        let (a, b) = (k.saturating_sub(1), (k + 1).min(yup.len() - 1));
        let dt = (yup[b].0 - yup[a].0) as f64 / 1e9;
        if dt <= 0.0 {
            return [0.0; 3];
        }
        geom::scale(geom::sub(yup[b].1, yup[a].1), 1.0 / dt)
    };
    let mut h = HeadTracker::new(SupervisorParams::default(), p.head.clone(), p.head_in_imu);
    h.corr_log = Some(Vec::new());
    // Output series for the jitter: (ts, position, quiet).
    let mut series: Vec<(i64, Vec3, bool)> = Vec::new();
    // Tracker poses as fed (ts, pos, q, world) and queries (display time, output, alignment, world).
    let mut fed: Vec<(i64, Vec3, Quat, u32)> = Vec::new();
    let mut queries: Vec<(i64, Vec3, Vec3, Quat, u32)> = Vec::new();
    let mut world = 0u32;
    let mut prev_corr: Option<(i64, Vec3)> = None;
    let mut slide: Vec<f64> = Vec::new();
    let mut gyro_win: std::collections::VecDeque<(i64, f64)> = std::collections::VecDeque::new();
    let mut fus = Fusion {
        q: QUAT_ID,
        bias: startup_bias(imu),
        init: false,
    };
    // 3DoF orientation history (ts, q) for the supervisor's q3dof at a tracker pose time.
    let mut q3_hist: std::collections::VecDeque<(i64, Quat)> = std::collections::VecDeque::new();
    let lat = (p.latency_ms * 1e6) as i64;
    let q_dt = (1e9 / p.query_hz) as i64;
    let pred = (p.predict_ms * 1e6) as i64;
    let mut next_q = imu[0].ts + 1_000_000_000;
    let mut k = 0usize; // next tracker pose
    let mut skip = 0usize;
    // Reset emulation: (t_r, p_r, v_r, new world <- raw world rotation).
    let mut rebase: Option<(i64, Vec3, Vec3, Quat)> = None;
    let mut pending_reset = false;
    // First tracker attitude vs the 3DoF filter (world frame), see the reset emulation.
    let mut w0: Option<Quat> = None;
    let mut res = GateResult::default();
    let mut still = false;
    let mut still_ref: Option<(Vec3, Quat)> = None;
    let mut prev_out: Option<(Vec3, Quat)> = None;
    let mut tracked_once = false;
    let mut last_ts = imu[0].ts;
    for (i, s) in imu.iter().enumerate() {
        let dt = if i == 0 {
            0.001
        } else {
            ((s.ts - imu[i - 1].ts) as f64 / 1e9).clamp(1e-4, 0.01)
        };
        fus.push(s, dt);
        q3_hist.push_back((s.ts, fus.q));
        while q3_hist.len() > 2000 {
            q3_hist.pop_front();
        }
        h.push_imu(ImuSample {
            ts_ns: s.ts,
            accel: s.accel,
            gyro: s.gyro,
        });
        gyro_win.push_back((s.ts, geom::norm(geom::sub(s.gyro, fus.bias))));
        while gyro_win.front().is_some_and(|x| x.0 < s.ts - 300_000_000) {
            gyro_win.pop_front();
        }
        // Deliver tracker poses whose time + latency has come.
        while k < yup.len() && yup[k].0 + lat <= s.ts {
            let (ts, raw_p, raw_q) = yup[k];
            let raw_v = vel(k);
            if pending_reset {
                pending_reset = false;
                // The fresh tracker starts gravity-aligned from the accelerometer like the first one did: its
                // attitude keeps the first tracker's offset to the 3DoF filter (`w0`), without the attitude the
                // un-reset run dead-reckoned since; another yaw. new world <- raw world at the reset.
                let q3r = q3_hist.back().map(|x| x.1).unwrap_or(fus.q);
                let d = qnorm(qmul(qmul(w0.unwrap_or(QUAT_ID), q3r), qconj(raw_q)));
                let y = qyaw(yaw_of(q3r) + 0.6);
                rebase = Some((ts, raw_p, raw_v, qnorm(qmul(qconj(y), d))));
                world += 1;
                skip = p.reset_skip;
            }
            k += 1;
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let (pos, q, v) = match rebase {
                Some((tr, pr, vr, r)) => {
                    let t = (ts - tr) as f64 / 1e9;
                    let rel = geom::sub(geom::sub(raw_p, pr), geom::scale(vr, t));
                    (
                        qrot(r, rel),
                        qnorm(qmul(r, raw_q)),
                        qrot(r, geom::sub(raw_v, vr)),
                    )
                }
                None => (raw_p, raw_q, raw_v),
            };
            let q3 = q3_hist
                .iter()
                .rev()
                .find(|(t, _)| *t <= ts)
                .map(|x| x.1)
                .unwrap_or(fus.q);
            fed.push((ts, pos, q, world));
            if w0.is_none() && fus.init {
                w0 = Some(qnorm(qmul(q, qconj(q3))));
            }
            let o = h.update(
                s.ts,
                Some(VioSample {
                    ts_ns: ts,
                    pos,
                    q,
                    vel: v,
                    q3dof: q3,
                }),
            );
            if o.events & events::RESET_WANTED != 0 {
                pending_reset = true;
            }
            if o.events & events::LOST != 0 {
                res.losses += 1;
            }
            if o.events & events::ACQUIRED != 0 {
                res.acquisitions += 1;
            }
        }
        if s.ts < next_q {
            continue;
        }
        next_q += q_dt;
        let o = h.update(s.ts, None);
        if o.events & events::RESET_WANTED != 0 {
            pending_reset = true;
        }
        if o.events & events::LOST != 0 {
            res.losses += 1;
        }
        // The driver's own stillness decision (desk-quiet entry, hysteresis), from the update above.
        let now_still = h.is_still();
        let rel3 = Rel {
            pos: [0.0; 3],
            q: fus.q,
            lin_vel: [0.0; 3],
            ang_vel: qrot(fus.q, geom::sub(s.gyro, fus.bias)),
            flags: flags::ORIENTATION_VALID
                | flags::ORIENTATION_TRACKED
                | flags::ANGULAR_VELOCITY_VALID,
        };
        let out = h.head(s.ts, s.ts + pred, &rel3);
        // Slide: how fast the pending correction moved the output while the head was slow (decay only, the
        // new steps are not motion).
        let (cp, _) = h.correction();
        if let Some((pt, pc)) = prev_corr {
            let dt = (s.ts - pt) as f64 / 1e9;
            let sp = geom::norm(out.lin_vel);
            if dt > 0.0 && sp < 0.25 {
                let dec = geom::norm(cp).min(geom::norm(pc));
                let v = (geom::norm(pc) - dec).max(0.0) / dt;
                slide.push(v);
            }
        }
        prev_corr = Some((s.ts, cp));
        let step = (s.ts - last_ts) as f64 / 1e9;
        last_ts = s.ts;
        if out.flags & flags::POSITION_TRACKED != 0 {
            tracked_once = true;
        } else if tracked_once {
            res.no_6dof_s += step;
        }
        if now_still && !still {
            res.still_periods += 1;
            still_ref = Some((out.pos, out.q));
        }
        if !now_still {
            still_ref = None;
        }
        if now_still && tracked_once {
            res.still_s += step;
            if let Some((p0, q0)) = still_ref {
                res.still_move_mm = res
                    .still_move_mm
                    .max(geom::norm(geom::sub(out.pos, p0)) * 1e3);
                res.still_move_deg = res.still_move_deg.max(qangle_deg(qmul(qconj(q0), out.q)));
            }
            if let (Some((pp, pq)), true) = (prev_out, still) {
                res.still_step_mm = res
                    .still_step_mm
                    .max(geom::norm(geom::sub(out.pos, pp)) * 1e3);
                res.still_step_deg = res.still_step_deg.max(qangle_deg(qmul(qconj(pq), out.q)));
            }
        }
        still = now_still;
        prev_out = Some((out.pos, out.q));
        let tracked = out.flags & flags::POSITION_TRACKED != 0;
        let quiet = !now_still
            && tracked
            && geom::norm(out.lin_vel) < 0.1
            && gyro_win.iter().all(|x| x.1 < 0.15);
        if tracked {
            let o = h.output();
            queries.push((
                s.ts + pred,
                out.pos,
                o.world_from_slam.0,
                o.world_from_slam.1,
                world,
            ));
        }
        series.push((s.ts, out.pos, quiet));
    }
    // Error against the hindsight trajectory.
    let smooth = |k: usize| -> Vec3 {
        geom::scale(
            geom::add(geom::add(fed[k - 1].1, fed[k].1), fed[k + 1].1),
            1.0 / 3.0,
        )
    };
    let mut errs: Vec<f64> = Vec::new();
    let mut ev2 = 0.0;
    for &(t, out_p, wt, wq, wid) in &queries {
        let k = fed.partition_point(|x| x.0 <= t);
        if k < 2 || k + 1 >= fed.len() {
            continue;
        }
        let (a, b) = (k - 1, k);
        if fed[a - 1].3 != wid || fed[b + 1].3 != wid || fed[a].3 != wid || fed[b].3 != wid {
            continue;
        }
        let f = (t - fed[a].0) as f64 / (fed[b].0 - fed[a].0).max(1) as f64;
        let pa = smooth(a);
        let pb = smooth(b);
        let pos = geom::add(pa, geom::scale(geom::sub(pb, pa), f));
        let (qa, mut qb) = (fed[a].2, fed[b].2);
        if qa.iter().zip(qb.iter()).map(|(x, y)| x * y).sum::<f64>() < 0.0 {
            qb = qb.map(|x| -x);
        }
        let q = qnorm([0, 1, 2, 3].map(|i| qa[i] + (qb[i] - qa[i]) * f));
        let head = geom::add(qrot(wq, geom::add(pos, qrot(q, p.head_in_imu))), wt);
        let d = geom::sub(out_p, head);
        errs.push(geom::norm(d) * 1e3);
        ev2 += d[1] * d[1];
    }
    if !errs.is_empty() {
        let n = errs.len() as f64;
        res.err_v_mm = (ev2 / n).sqrt() * 1e3;
        let rms = (errs.iter().map(|e| e * e).sum::<f64>() / n).sqrt();
        errs.sort_by(|a, b| a.total_cmp(b));
        res.err_mm = [rms, errs[((n - 1.0) * 0.95) as usize], errs[errs.len() - 1]];
    }
    if !slide.is_empty() {
        let n = slide.len() as f64;
        res.slide_rms_mm_s = (slide.iter().map(|v| v * v).sum::<f64>() / n).sqrt() * 1e3;
        res.slide_time_s = slide.iter().filter(|v| **v > 0.02).count() as f64 / p.query_hz;
    }
    // Corrections.
    let log = h.corr_log.take().unwrap_or_default();
    let mut steps: Vec<f64> = log.iter().map(|e| geom::norm(e.step) * 1e3).collect();
    steps.sort_by(|a, b| a.total_cmp(b));
    let pct = |v: &[f64], q: f64| {
        if v.is_empty() {
            0.0
        } else {
            v[((v.len() - 1) as f64 * q).round() as usize]
        }
    };
    res.corrections = log.len() as u64;
    res.step_mm = [pct(&steps, 0.5), pct(&steps, 0.95), pct(&steps, 1.0)];
    res.max_offset_mm = log
        .iter()
        .map(|e| geom::norm(e.offset) * 1e3)
        .fold(0.0, f64::max);
    let big: Vec<_> = log.iter().filter(|e| geom::norm(e.step) >= 0.02).collect();
    res.big = big.len() as u64;
    res.big_vertical = big
        .iter()
        .filter(|e| e.step[1].abs() > (e.step[0] * e.step[0] + e.step[2] * e.step[2]).sqrt())
        .count() as u64;
    res.big_speed = big.iter().map(|e| e.speed).sum::<f64>() / (big.len().max(1) as f64);
    res.big_slow = big.iter().filter(|e| e.speed < 0.25).count() as u64;
    // Jitter while worn and nearly still (0.5 s window) and slow vertical wander (2 s window).
    (res.jitter_v_mm, res.jitter_h_mm) = jitter(&series, 250_000_000);
    res.wander_v_mm = jitter(&series, 1_000_000_000).0;
    res.quiet_s = series
        .windows(2)
        .filter(|w| w[1].2)
        .map(|w| (w[1].0 - w[0].0) as f64 / 1e9)
        .sum();
    // The tracker's own vertical jitter over the same quiet times (poses, same 0.5 s centred average).
    let (mut nv, mut svv) = (0.0, 0.0);
    for k in 6..yup.len().saturating_sub(6) {
        let t = yup[k].0;
        let qi = series.partition_point(|x| x.0 < t);
        let quiet_here = (qi.saturating_sub(23)..(qi + 23).min(series.len())).all(|i| series[i].2)
            && qi < series.len();
        if !quiet_here {
            continue;
        }
        let m = (yup[k - 6..=k + 6].iter().map(|x| x.1[1]).sum::<f64>()) / 13.0;
        nv += 1.0;
        svv += (yup[k].1[1] - m).powi(2);
    }
    if nv > 0.0 {
        res.vio_jitter_v_mm = (svv / nv).sqrt() * 1e3;
    }
    res.duration_s = (imu[imu.len() - 1].ts - imu[0].ts) as f64 / 1e9;
    res.firings = h.sup.still_resets;
    res.max_corr_mm = h.corr.max_pos * 1e3;
    res.max_corr_deg = h.corr.max_rot.to_degrees();
    Ok(res)
}

/// The settings compared by `omarchyvr-slam corrections`: the current output, then each smoothing option.
pub fn variants(base: &GateParams) -> Vec<(String, GateParams)> {
    let mut old = base.clone();
    old.head.fuse = [0.0; 5];
    old.head.tau_rest_s = 0.0;
    let mut fuse_only = base.clone();
    fuse_only.head.tau_rest_s = 0.0;
    let mut rest_only = base.clone();
    rest_only.head.fuse = [0.0; 5];
    vec![
        (
            "before (PIMAX_SLAM_FUSE=0 PIMAX_SLAM_TAU_REST_MS=0)".to_string(),
            old,
        ),
        ("fused position only".to_string(), fuse_only),
        ("slow decay at rest only".to_string(), rest_only),
        ("now (default)".to_string(), base.clone()),
    ]
}

/// Corrections and jitter, one line per run.
pub fn corr_summary(name: &str, r: &GateResult) -> String {
    format!(
        "{name:<26} corrections {:>5}, >= 20 mm {:>3} ({:>2} vertical, {:>2} slow, head {:.2} m/s); wander {:>4.2} mm; slide {:>5.1} mm/s, {:>4.1} s; step p50 {:>4.1} p95 {:>5.1} \
         max {:>5.1} mm, offset max {:>5.1} mm; worn quiet {:>5.1} s: jitter vertical {:>4.2} mm, horizontal \
         {:>4.2} mm (tracker vertical {:>4.2} mm); error rms {:>5.1} p95 {:>5.1} max {:>5.1} mm (vertical rms \
         {:>4.1})",
        r.corrections,
        r.big,
        r.big_vertical,
        r.big_slow,
        r.big_speed,
        r.wander_v_mm,
        r.slide_rms_mm_s,
        r.slide_time_s,
        r.step_mm[0],
        r.step_mm[1],
        r.step_mm[2],
        r.max_offset_mm,
        r.quiet_s,
        r.jitter_v_mm,
        r.jitter_h_mm,
        r.vio_jitter_v_mm,
        r.err_mm[0],
        r.err_mm[1],
        r.err_mm[2],
        r.err_v_mm
    )
}

/// One line per run.
pub fn summary(name: &str, r: &GateResult) -> String {
    format!(
        "{name:<22} {:>6.1} s, still {:>5.1} s in {:>2} periods: gate fired {:>3} ({:>4.1}/min), 6DoF lost {:>3}, \
         acquired {:>3}, no 6DoF {:>5.1} s; output while still: moved {:>5.1} mm / {:>4.2} deg, step {:>4.1} mm / \
         {:>4.2} deg; max correction {:>5.1} mm / {:>4.2} deg",
        r.duration_s,
        r.still_s,
        r.still_periods,
        r.firings,
        r.firings as f64 / (r.duration_s / 60.0).max(1e-9),
        r.losses,
        r.acquisitions,
        r.no_6dof_s,
        r.still_move_mm,
        r.still_move_deg,
        r.still_step_mm,
        r.still_step_deg,
        r.max_corr_mm,
        r.max_corr_deg
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A headset lying still for `secs` (tilted 30 deg, gyro bias), with a landmark-free tracker that
    /// dead-reckons at 0.07 m/s² (tracker world z-up, as Basalt writes it).
    fn lying(secs: f64) -> (Vec<ImuRow>, Vec<PoseRow>) {
        let t0 = 1_000_000_000_000i64;
        let tilt = 30f64.to_radians();
        // Body up (specific force direction) tilted about x.
        let up = [0.0, tilt.cos(), tilt.sin()];
        let mut imu = Vec::new();
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        for i in 0..(secs * 1000.0) as i64 {
            imu.push(ImuRow {
                ts: t0 + i * 1_000_000,
                gyro: [
                    0.004 + 0.001 * rnd(),
                    -0.013 + 0.001 * rnd(),
                    0.007 + 0.001 * rnd(),
                ],
                accel: geom::add(
                    geom::scale(up, 9.81),
                    [0.01 * rnd(), 0.01 * rnd(), 0.01 * rnd()],
                ),
            });
        }
        // Tracker orientation (z-up world): the body tilted so that its `up` maps to world z; 1.5 deg off
        // from the 3DoF filter's attitude.
        let q_yup = qnorm([
            (-(tilt + 1.5f64.to_radians()) / 2.0).sin(),
            0.0,
            0.0,
            (-(tilt + 1.5f64.to_radians()) / 2.0).cos(),
        ]);
        let q_zup = qnorm(qmul(qconj(SLAM_TO_YUP), q_yup));
        let mut vio = Vec::new();
        for i in 0..(secs * 25.0) as i64 {
            let t = i as f64 * 0.04;
            vio.push(PoseRow {
                ts: t0 + i * 40_000_000,
                p: [0.5 * 0.07 * t * t, 0.0, 0.0],
                q: q_zup,
            });
        }
        (imu, vio)
    }

    #[test]
    fn lying_still_rezero_holds_the_output_without_dropouts() {
        let (imu, vio) = lying(20.0);
        // This synthetic tracker comes back from every reset with the same 1.5 deg tilt error (Basalt starts
        // gravity aligned), so the gravity tilt correction would move the output here: off, to look at the
        // re-zero alone (headpose tests cover the tilt correction through a re-zero).
        let mut new = GateParams::default();
        new.head.tilt_tau_s = 0.0;
        let mut old = new.clone();
        old.head.still.rezero = false;
        let a = run(&imu, &vio, &old).unwrap();
        let b = run(&imu, &vio, &new).unwrap();
        eprintln!("{}\n{}", summary("old", &a), summary("rezero", &b));
        // The drifting tracker is still caught and reset in both.
        assert!(
            a.firings >= 5 && b.firings >= 5,
            "{} {}",
            a.firings,
            b.firings
        );
        // Old: every firing is a 6DoF loss; new: none, and the output does not move.
        assert!(a.losses >= a.firings);
        assert_eq!(b.losses, 0);
        assert_eq!(b.acquisitions, 1);
        // Position: the few mm left are the predictor's dead reckoning between tracker poses (same in both).
        assert!(
            b.still_move_mm < 5.0 && b.still_move_mm <= a.still_move_mm + 0.5,
            "{}",
            b.still_move_mm
        );
        // Tilt: the old yaw-only 3DoF fix swung the view by the 3DoF/tracker attitude difference (1.5 deg here)
        // at every firing; the re-zero carries the full attitude.
        assert!(a.still_move_deg > 1.0, "{}", a.still_move_deg);
        assert!(b.still_move_deg < 0.2, "{}", b.still_move_deg);
        assert!(b.max_corr_deg < 0.3 && a.max_corr_deg > 1.0);
    }
}
