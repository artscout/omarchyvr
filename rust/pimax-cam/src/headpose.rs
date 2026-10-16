//! Head pose output for 6DoF tracking (phase 2): IMU-rate prediction from the newest tracker (VIO) pose,
//! blending of corrections so the room never jumps, and the supervisor's fallback states, all in one place.
//!
//! ```text
//!  IMU 1 kHz ---------------------------> Predictor (ring buffer, bias-corrected gyro, ZOH integration)
//!  VIO 25 Hz (pos, q, vel; y-up) -------> Predictor base states  +  Supervisor (acquire / lose / reset / re-snap)
//!  query(t, 3DoF relation) -> eval(current configuration, t)  + correction offset (decays, rate limited)
//! ```
//!
//! * **Rotation stays IMU-fast:** between tracker poses the orientation is the last VIO orientation integrated
//!   with the gyro up to the newest IMU sample (then constant angular velocity to the query time).
//! * **Position** = last VIO position + VIO velocity + IMU acceleration (gravity removed) up to the newest IMU
//!   sample, then constant velocity.
//! * **No jumps:** whenever what produces the output changes (a new VIO pose corrects the dead-reckoned one, the
//!   supervisor acquires / loses / re-snaps / recentres) the difference at the query time goes into a correction
//!   offset (position in world metres, rotation as a world-frame rotation vector about the head) so the output is
//!   continuous. The offset then decays to zero with a time constant (`tau_*`), never faster than a rate limit
//!   that grows with head speed (corrections hide in motion, HoloLens style). Huge differences (`snap_*`) are
//!   applied at once: sliding a room for metres is worse than one cut.
//! * The supervisor ([`crate::slam::Supervisor`]) keeps deciding when the tracker is trusted; with re-snap on it
//!   restores the old room alignment when tracking comes back in the same tracker world, so content returns to
//!   its place in the room instead of staying shifted by what the head did while tracking was lost.

use crate::geom::{self, Vec3};
use crate::slam::{
    events, qconj, qmul, qnorm, qrot, yaw_of, Quat, StillParams, Supervisor, SupervisorParams,
    TrackOutput, TrackSample, TrackState, QUAT_ID,
};
use std::collections::VecDeque;

/// Standard gravity (m/s²).
pub const GRAVITY: f64 = 9.80665;
/// Gravity in the y-up world.
pub const G_WORLD: Vec3 = [0.0, -GRAVITY, 0.0];

/// Relation flag bits (same values as Monado's `xrt_space_relation_flags`).
pub mod flags {
    /// Orientation valid.
    pub const ORIENTATION_VALID: u32 = 1;
    /// Position valid.
    pub const POSITION_VALID: u32 = 2;
    /// Linear velocity valid.
    pub const LINEAR_VELOCITY_VALID: u32 = 4;
    /// Angular velocity valid.
    pub const ANGULAR_VELOCITY_VALID: u32 = 8;
    /// Orientation tracked.
    pub const ORIENTATION_TRACKED: u32 = 16;
    /// Position tracked.
    pub const POSITION_TRACKED: u32 = 32;
    /// Everything.
    pub const ALL: u32 = 63;
}

/// `exp` map: rotation vector (rad) -> unit quaternion.
pub fn qexp(w: Vec3) -> Quat {
    let th = geom::norm(w);
    if th < 1e-12 {
        return qnorm([w[0] * 0.5, w[1] * 0.5, w[2] * 0.5, 1.0]);
    }
    let s = (th * 0.5).sin() / th;
    [w[0] * s, w[1] * s, w[2] * s, (th * 0.5).cos()]
}

/// `log` map: unit quaternion -> rotation vector (rad), shortest way.
pub fn qlog(q: Quat) -> Vec3 {
    let mut q = qnorm(q);
    if q[3] < 0.0 {
        q = [-q[0], -q[1], -q[2], -q[3]];
    }
    let v = [q[0], q[1], q[2]];
    let s = geom::norm(v);
    if s < 1e-12 {
        return geom::scale(v, 2.0);
    }
    geom::scale(v, 2.0 * s.atan2(q[3]) / s)
}

/// One IMU sample in the tracker's IMU frame (body = OpenXR head axes), raw (not bias corrected).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuSample {
    /// Monotonic ns (same clock as the tracker).
    pub ts_ns: i64,
    /// Specific force, m/s².
    pub accel: Vec3,
    /// Angular rate, rad/s.
    pub gyro: Vec3,
}

/// IMU (body) state in a gravity-aligned y-up frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BodyState {
    /// Monotonic ns.
    pub ts_ns: i64,
    /// Position.
    pub p: Vec3,
    /// Orientation (body -> frame).
    pub q: Quat,
    /// Velocity.
    pub v: Vec3,
}

/// Predictor parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct PredictorParams {
    /// IMU samples kept (1 kHz: 4 s).
    pub imu_cap: usize,
    /// Tracker poses kept.
    pub vio_cap: usize,
    /// Gyro bias low-pass time constant (s); 0 = no bias estimation.
    pub bias_tau_s: f64,
    /// Largest accepted bias sample (rad/s).
    pub bias_max: f64,
    /// Use the accelerometer between tracker poses (else constant velocity).
    pub use_accel: bool,
    /// Fused position (0 = off: every tracker pose is the new base as it is). Each tracker pose then only
    /// pulls the IMU-propagated position by this fraction of the difference (`fuse_alpha_y` vertically) and
    /// the velocity by `fuse_beta` x difference / interval plus `fuse_vel_w` x (tracker velocity - propagated):
    /// an alpha-beta filter driven by the accelerometer. The tracker's pose-to-pose noise and its noisy newest
    /// velocity no longer reach the output as steps; the IMU carries the motion between poses.
    pub fuse_alpha: f64,
    /// See `fuse_alpha`.
    pub fuse_alpha_y: f64,
    /// See `fuse_alpha`.
    pub fuse_beta: f64,
    /// See `fuse_alpha`.
    pub fuse_vel_w: f64,
    /// Differences above this (m) reset the fused state to the tracker pose.
    pub fuse_reset_m: f64,
    /// Speed-adaptive: the tracker velocity weight rises from `fuse_vel_w` at rest to 1 at this head speed (m/s;
    /// 0 = fixed): smoothing where corrections would show, the tracker's own velocity (no lag) in fast motion.
    pub fuse_fast_speed: f64,
}

impl Default for PredictorParams {
    fn default() -> Self {
        PredictorParams {
            imu_cap: 4000,
            vio_cap: 64,
            bias_tau_s: 10.0,
            bias_max: 0.15,
            use_accel: true,
            fuse_alpha: 0.0,
            fuse_alpha_y: 0.0,
            fuse_beta: 0.0,
            fuse_vel_w: 0.0,
            fuse_reset_m: 0.3,
            fuse_fast_speed: 0.0,
        }
    }
}

/// IMU dead reckoning from the tracker's poses.
#[derive(Clone, Debug)]
pub struct Predictor {
    /// Parameters.
    pub p: PredictorParams,
    imu: VecDeque<ImuSample>,
    vio: VecDeque<BodyState>,
    /// Gyro bias estimate (rad/s, body frame), from tracker vs gyro rotation.
    pub gyro_bias: Vec3,
    /// Bias samples taken.
    pub bias_samples: u64,
    /// Take the next tracker pose as it is even with fusion on (headset lying still: the hold owns it).
    pub fuse_hold: bool,
    /// Headset lying still: no accelerometer dead reckoning (its bias would only add drift between poses).
    pub lying: bool,
}

impl Predictor {
    /// New, empty.
    pub fn new(p: PredictorParams) -> Predictor {
        Predictor {
            p,
            imu: VecDeque::new(),
            vio: VecDeque::new(),
            gyro_bias: [0.0; 3],
            bias_samples: 0,
            fuse_hold: false,
            lying: false,
        }
    }

    /// Add an IMU sample (older-or-equal timestamps are dropped).
    pub fn push_imu(&mut self, s: ImuSample) {
        if self.imu.back().is_some_and(|l| s.ts_ns <= l.ts_ns) {
            return;
        }
        self.imu.push_back(s);
        while self.imu.len() > self.p.imu_cap.max(16) {
            self.imu.pop_front();
        }
    }

    /// True when the newest `window_ns` of IMU samples look like a headset lying still: per-axis standard
    /// deviation below `gyro_std` (rad/s) and `accel_std` (m/s²). A worn headset never gets that quiet; a
    /// headset on a desk or cushion shows ~0.001 rad/s and ~0.01 m/s².
    pub fn imu_still(&self, window_ns: i64, gyro_std: f64, accel_std: f64) -> bool {
        let Some(last) = self.imu.back() else {
            return false;
        };
        let from = last.ts_ns - window_ns;
        if self.imu.front().is_none_or(|f| f.ts_ns > from + window_ns / 10) {
            return false; // window not covered yet
        }
        let (mut n, mut sg, mut sg2, mut sa, mut sa2) = (0.0, [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3]);
        for s in self.imu.iter().rev().take_while(|s| s.ts_ns >= from) {
            n += 1.0;
            for k in 0..3 {
                sg[k] += s.gyro[k];
                sg2[k] += s.gyro[k] * s.gyro[k];
                sa[k] += s.accel[k];
                sa2[k] += s.accel[k] * s.accel[k];
            }
        }
        if n < 10.0 {
            return false;
        }
        let var = |s: f64, s2: f64| (s2 / n - (s / n) * (s / n)).max(0.0);
        (0..3).all(|k| {
            var(sg[k], sg2[k]) < gyro_std * gyro_std && var(sa[k], sa2[k]) < accel_std * accel_std
        })
    }

    /// Newest IMU timestamp.
    pub fn last_imu_ns(&self) -> Option<i64> {
        self.imu.back().map(|s| s.ts_ns)
    }

    /// Drop the tracker poses (tracker reset: new world).
    pub fn clear_vio(&mut self) {
        self.vio.clear();
    }

    /// Add a tracker pose; `false` if not newer than the last one. Updates the gyro bias estimate from the
    /// rotation between this pose and the previous one (when `learn_bias`).
    pub fn push_vio(&mut self, st: BodyState, learn_bias: bool) -> bool {
        if let Some(prev) = self.vio.back().copied() {
            if st.ts_ns <= prev.ts_ns {
                return false;
            }
            let dt = (st.ts_ns - prev.ts_ns) as f64 / 1e9;
            if learn_bias && self.p.bias_tau_s > 0.0 && (0.01..=0.2).contains(&dt) {
                if let Some(raw) = self.gyro_delta_raw(prev.ts_ns, st.ts_ns) {
                    // raw = d_true * exp(b dt)  =>  b = log(d_vio^-1 raw) / dt
                    let d_vio = qmul(qconj(qnorm(prev.q)), qnorm(st.q));
                    let b = geom::scale(qlog(qmul(qconj(d_vio), raw)), 1.0 / dt);
                    if geom::norm(b) < self.p.bias_max {
                        let a = (dt / self.p.bias_tau_s).min(1.0);
                        self.gyro_bias =
                            geom::add(self.gyro_bias, geom::scale(geom::sub(b, self.gyro_bias), a));
                        self.bias_samples += 1;
                    }
                }
            }
        }
        let mut st = BodyState {
            q: qnorm(st.q),
            ..st
        };
        if self.p.fuse_alpha > 0.0 && !self.fuse_hold {
            if let Some(prev) = self.vio.back().copied() {
                let dt = (st.ts_ns - prev.ts_ns) as f64 / 1e9;
                let (pr, _) = self.propagate(&prev, st.ts_ns);
                let r = geom::sub(st.p, pr.p);
                if dt > 0.0 && dt < 0.25 && geom::norm(r) < self.p.fuse_reset_m {
                    let a = self.p.fuse_alpha;
                    let ay = self.p.fuse_alpha_y;
                    let b = self.p.fuse_beta / dt;
                    let mut w = self.p.fuse_vel_w;
                    if self.p.fuse_fast_speed > 0.0 {
                        let f = (geom::norm(pr.v) / self.p.fuse_fast_speed).min(1.0);
                        w += (1.0 - w) * f;
                    }
                    st.p = geom::add(pr.p, [a * r[0], ay * r[1], a * r[2]]);
                    st.v = geom::add(
                        geom::add(pr.v, geom::scale(r, b)),
                        geom::scale(geom::sub(st.v, pr.v), w),
                    );
                }
            }
        }
        self.vio.push_back(st);
        while self.vio.len() > self.p.vio_cap.max(2) {
            self.vio.pop_front();
        }
        true
    }

    /// Newest tracker pose.
    pub fn latest(&self) -> Option<BodyState> {
        self.vio.back().copied()
    }

    /// Newest tracker pose at or before `t` (the oldest one for earlier times).
    pub fn base_at(&self, t: i64) -> Option<BodyState> {
        let i = self.vio.partition_point(|s| s.ts_ns <= t);
        if i == 0 {
            self.vio.front().copied()
        } else {
            self.vio.get(i - 1).copied()
        }
    }

    /// Raw (not bias corrected) gyro rotation from `t0` to `t1` (body frame, `q(t1) = q(t0) * delta`).
    fn gyro_delta_raw(&self, t0: i64, t1: i64) -> Option<Quat> {
        let first = self.imu.front()?;
        if t0 < first.ts_ns - 2_000_000 || self.imu.back()?.ts_ns < t1 {
            return None;
        }
        let mut q = QUAT_ID;
        let mut t = t0;
        let i0 = self.imu.partition_point(|s| s.ts_ns <= t0);
        for s in self.imu.range(i0..) {
            let end = s.ts_ns.min(t1);
            if end > t {
                q = qmul(q, qexp(geom::scale(s.gyro, (end - t) as f64 / 1e9)));
                t = end;
            }
            if s.ts_ns >= t1 {
                break;
            }
        }
        Some(qnorm(q))
    }

    /// Bias-corrected gyro rotation from `t0` to `t1`.
    pub fn gyro_delta(&self, t0: i64, t1: i64) -> Option<Quat> {
        let raw = self.gyro_delta_raw(t0, t1)?;
        let dt = (t1 - t0) as f64 / 1e9;
        Some(qmul(raw, qexp(geom::scale(self.gyro_bias, -dt))))
    }

    /// Propagate `base` to `t` (>= base time; earlier times return `base`) with the IMU: zero-order hold (sample i
    /// covers the interval ending at its timestamp), bias-corrected gyro, gravity-removed acceleration. Past the
    /// newest IMU sample: constant angular velocity and constant velocity. Returns the state and the body angular
    /// velocity used last.
    pub fn propagate(&self, base: &BodyState, t: i64) -> (BodyState, Vec3) {
        let mut st = *base;
        let bias = self.gyro_bias;
        let mut w_last = self
            .imu
            .back()
            .map(|s| geom::sub(s.gyro, bias))
            .unwrap_or([0.0; 3]);
        if t <= base.ts_ns {
            return (st, w_last);
        }
        let i0 = self.imu.partition_point(|s| s.ts_ns <= base.ts_ns);
        for s in self.imu.range(i0..) {
            if st.ts_ns >= t {
                break;
            }
            let end = s.ts_ns.min(t);
            let dt = (end - st.ts_ns) as f64 / 1e9;
            let w = geom::sub(s.gyro, bias);
            let a = if self.p.use_accel && !self.lying {
                geom::add(qrot(st.q, s.accel), G_WORLD)
            } else {
                [0.0; 3]
            };
            st.p = geom::add(
                st.p,
                geom::add(geom::scale(st.v, dt), geom::scale(a, 0.5 * dt * dt)),
            );
            st.v = geom::add(st.v, geom::scale(a, dt));
            st.q = qnorm(qmul(st.q, qexp(geom::scale(w, dt))));
            st.ts_ns = end;
            w_last = w;
        }
        if st.ts_ns < t {
            let dt = (t - st.ts_ns) as f64 / 1e9;
            st.p = geom::add(st.p, geom::scale(st.v, dt));
            st.q = qnorm(qmul(st.q, qexp(geom::scale(w_last, dt))));
            st.ts_ns = t;
        }
        (st, w_last)
    }

    /// State at `t` from the newest tracker pose at or before `t`.
    pub fn state_at(&self, t: i64) -> Option<(BodyState, Vec3)> {
        let b = self.base_at(t)?;
        Some(self.propagate(&b, t))
    }
}

/// A relation (pose + velocities), y-up world, `flags` as Monado.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rel {
    /// Position.
    pub pos: Vec3,
    /// Orientation.
    pub q: Quat,
    /// Linear velocity.
    pub lin_vel: Vec3,
    /// Angular velocity (world frame).
    pub ang_vel: Vec3,
    /// [`flags`] bits.
    pub flags: u32,
}

impl Default for Rel {
    fn default() -> Self {
        Rel {
            pos: [0.0; 3],
            q: QUAT_ID,
            lin_vel: [0.0; 3],
            ang_vel: [0.0; 3],
            flags: 0,
        }
    }
}

/// Output smoothing and re-snap parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct HeadParams {
    /// Blend corrections (false = raw prediction, phase-1 behaviour).
    pub smooth: bool,
    /// Position correction time constant (s).
    pub tau_pos_s: f64,
    /// Rotation correction time constant (s).
    pub tau_rot_s: f64,
    /// Position correction rate limit at rest (m/s) ...
    pub v_min: f64,
    /// ... plus this fraction of the head speed.
    pub k_v: f64,
    /// Rotation correction rate limit at rest (rad/s) ...
    pub w_min: f64,
    /// ... plus this fraction of the head angular speed.
    pub k_w: f64,
    /// Larger position differences are applied at once (m).
    pub snap_pos: f64,
    /// Larger rotation differences are applied at once (rad).
    pub snap_rot: f64,
    /// Restore the old room alignment when tracking returns in the same tracker world.
    pub resnap: bool,
    /// Re-snap only if the room yaw agrees with the 3DoF heading within this (deg).
    pub resnap_yaw_deg: f64,
    /// ... and the head is within `max(resnap_min_m, resnap_speed * lost time)` of where it was held.
    pub resnap_speed: f64,
    /// See `resnap_speed`.
    pub resnap_min_m: f64,
    /// Learn the gyro bias from the tracker while tracking.
    pub learn_bias: bool,
    /// Headset lying still: hold the position, reset a drifting tracker ([`StillParams`]).
    pub still: StillParams,
    /// Stillness test window (ns) ...
    pub still_window_ns: i64,
    /// ... per-axis standard deviation limits (rad/s, m/s²) to enter the still state ...
    pub still_gyro_std: f64,
    /// See `still_gyro_std`.
    pub still_accel_std: f64,
    /// ... and the larger ones that end it (hysteresis: a cushion or desk with a fan nearby flickered around a
    /// single 0.03 m/s² limit in the 16:24 live run, and every flicker re-anchored the hold on the drifted
    /// tracker). Worn walks never go below 0.027 rad/s.
    pub still_exit_gyro_std: f64,
    /// See `still_exit_gyro_std`.
    pub still_exit_accel_std: f64,
    /// Corrections whose step reaches this (m) are counted and queued for the log ([`HeadTracker::take_big`]).
    pub big_corr_m: f64,
    /// Fused position: `[alpha, alpha vertical, beta, tracker velocity weight, fast speed]`
    /// ([`PredictorParams::fuse_alpha`]); alpha 0 = off.
    pub fuse: [f64; 5],
    /// Position corrections decay with this time constant (s) while the head is at rest, going over to
    /// `tau_pos_s` at `rest_speed` (m/s): a correction slides slowly where it would show and is hidden in
    /// motion (0 = always `tau_pos_s`).
    pub tau_rest_s: f64,
    /// See `tau_rest_s`.
    pub rest_speed: f64,
}

impl Default for HeadParams {
    fn default() -> Self {
        HeadParams {
            smooth: true,
            tau_pos_s: 0.15,
            tau_rot_s: 0.10,
            v_min: 0.10,
            k_v: 0.5,
            w_min: 3f64.to_radians(),
            k_w: 0.3,
            snap_pos: 1.5,
            snap_rot: 60f64.to_radians(),
            resnap: true,
            resnap_yaw_deg: 3.0,
            resnap_speed: 1.5,
            resnap_min_m: 0.5,
            learn_bias: true,
            still: StillParams::default(),
            still_window_ns: 500_000_000,
            still_gyro_std: 0.006,
            still_accel_std: 0.06,
            still_exit_gyro_std: 0.012,
            still_exit_accel_std: 0.15,
            big_corr_m: 0.02,
            fuse: [0.15, 0.075, 0.06, 0.5, 0.5],
            tau_rest_s: 0.4,
            rest_speed: 0.3,
        }
    }
}

fn env_f64(name: &str) -> Option<f64> {
    std::env::var(name).ok().and_then(|v| v.trim().parse().ok())
}

impl HeadParams {
    /// Defaults with `PIMAX_SLAM_SMOOTH` (0/1), `PIMAX_SLAM_TAU_MS` (position; rotation = 2/3 of it),
    /// `PIMAX_SLAM_RESNAP` (0/1), `PIMAX_SLAM_STILL` (0 = no still hold/reset, else the reset distance in cm),
    /// `PIMAX_SLAM_STILL_REZERO` (0 = the old drop to 3DoF on a still reset, for A/B only),
    /// `PIMAX_SLAM_FUSE` (0 = off, or `alpha,alpha_y,beta,vel_w,fast_speed`), `PIMAX_SLAM_TAU_REST_MS` (0 = off),
    /// `PIMAX_SLAM_BIG_CORR_MM` (log threshold) overrides.
    pub fn from_env() -> HeadParams {
        let mut p = HeadParams::default();
        if let Some(v) = env_f64("PIMAX_SLAM_SMOOTH") {
            p.smooth = v != 0.0;
        }
        if let Some(v) = env_f64("PIMAX_SLAM_TAU_MS") {
            let v = v.clamp(5.0, 2000.0) / 1000.0;
            p.tau_pos_s = v;
            p.tau_rot_s = v * 2.0 / 3.0;
        }
        if let Some(v) = env_f64("PIMAX_SLAM_RESNAP") {
            p.resnap = v != 0.0;
        }
        // Output smoothing (2026-10-15, analysis/slam-drift.md "Corrections while worn"): fused position and
        // slow decay at rest are on by default; each can be switched off for A/B.
        match std::env::var("PIMAX_SLAM_FUSE").ok().map(|v| v.trim().to_string()) {
            Some(v) if v == "0" || v == "off" => p.fuse = [0.0; 5],
            Some(v) => {
                let f: Vec<f64> = v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
                if f.len() == 5 && f[0] >= 0.0 && f[0] <= 1.0 {
                    p.fuse = [f[0], f[1], f[2], f[3], f[4]];
                }
            }
            None => {}
        }
        if let Some(v) = env_f64("PIMAX_SLAM_TAU_REST_MS") {
            p.tau_rest_s = v.clamp(0.0, 5000.0) / 1000.0;
        }
        if let Some(v) = env_f64("PIMAX_SLAM_BIG_CORR_MM") {
            p.big_corr_m = v.clamp(1.0, 1000.0) / 1000.0;
        }
        if let Some(v) = env_f64("PIMAX_SLAM_STILL_REZERO") {
            p.still.rezero = v != 0.0;
        }
        if let Some(v) = env_f64("PIMAX_SLAM_STILL") {
            p.still.hold = v > 0.0;
            if v > 0.0 && v != 1.0 {
                p.still.reset_m = v.clamp(0.5, 100.0) / 100.0;
            }
        }
        p
    }
}

/// What produces the output: the supervisor's decision plus the tracker pose the prediction starts from.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Config {
    state: TrackState,
    w_t: Vec3,
    w_q: Quat,
    fix3dof: Quat,
    held: Vec3,
    base: Option<BodyState>,
}

/// One tracker pose as the driver hands it over (y-up tracker world, IMU frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VioSample {
    /// Pose time (monotonic ns).
    pub ts_ns: i64,
    /// IMU position.
    pub pos: Vec3,
    /// IMU orientation.
    pub q: Quat,
    /// IMU velocity.
    pub vel: Vec3,
    /// 3DoF orientation of the IMU at `ts_ns` (for the supervisor).
    pub q3dof: Quat,
}

/// Correction statistics (for logs and tests).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CorrStats {
    /// Corrections blended in.
    pub blended: u64,
    /// Corrections applied at once (too large).
    pub snapped: u64,
    /// Largest blended position correction (m).
    pub max_pos: f64,
    /// Largest blended rotation correction (rad).
    pub max_rot: f64,
    /// Corrections whose offset reached `HeadParams::big_corr_m`, and those of them mostly vertical.
    pub big: u64,
    /// See `big`.
    pub big_vertical: u64,
}

/// One correction (a new tracker pose or a supervisor change moved what the output would be).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CorrEvent {
    /// Host time (ns).
    pub ts_ns: i64,
    /// This change alone: old output - new uncorrected output, minus the offset that was still pending (m, world).
    pub step: Vec3,
    /// The whole offset after it (what the output has to slide off, m, world; y = vertical).
    pub offset: Vec3,
    /// Rotation offset after it (rad).
    pub rot: f64,
    /// Head speed (m/s) and angular speed (rad/s) at that moment.
    pub speed: f64,
    /// See `speed`.
    pub ang_speed: f64,
    /// Only the tracker pose changed (no acquire / loss / re-snap / recenter / still hold).
    pub pose_only: bool,
}

/// Supervisor + predictor + correction blending (see the module docs).
#[derive(Clone, Debug)]
pub struct HeadTracker {
    /// The supervisor (state machine and room alignment).
    pub sup: Supervisor,
    /// The predictor.
    pub pred: Predictor,
    /// Parameters.
    pub p: HeadParams,
    head_in_imu: Vec3,
    out: TrackOutput,
    emitted: Option<Config>,
    corr_p: Vec3,
    corr_r: Vec3,
    corr_ns: Option<i64>,
    speed: f64,
    ang_speed: f64,
    still: bool,
    /// Correction counters.
    pub corr: CorrStats,
    /// Every correction (offline analysis; `None` = off).
    pub corr_log: Option<Vec<CorrEvent>>,
    big_q: VecDeque<CorrEvent>,
}

impl HeadTracker {
    /// New tracker; `head_in_imu` = head origin in the IMU frame (m).
    pub fn new(sp: SupervisorParams, p: HeadParams, head_in_imu: Vec3) -> HeadTracker {
        let mut sup = Supervisor::new(sp, head_in_imu);
        if p.resnap {
            sup.set_resnap(Some(crate::slam::Resnap {
                yaw_deg: p.resnap_yaw_deg,
                speed_m_s: p.resnap_speed,
                min_m: p.resnap_min_m,
            }));
        }
        sup.set_still_params(p.still);
        let out = sup.update(0, None);
        HeadTracker {
            sup,
            pred: Predictor::new(PredictorParams {
                fuse_alpha: p.fuse[0],
                fuse_alpha_y: p.fuse[1],
                fuse_beta: p.fuse[2],
                fuse_vel_w: p.fuse[3],
                fuse_fast_speed: p.fuse[4],
                ..PredictorParams::default()
            }),
            p,
            head_in_imu,
            out,
            emitted: None,
            corr_p: [0.0; 3],
            corr_r: [0.0; 3],
            corr_ns: None,
            speed: 0.0,
            ang_speed: 0.0,
            still: false,
            corr: CorrStats::default(),
            corr_log: None,
            big_q: VecDeque::new(),
        }
    }

    /// Latest supervisor output.
    pub fn output(&self) -> &TrackOutput {
        &self.out
    }

    /// Current correction offset (position m, rotation vector rad).
    pub fn correction(&self) -> (Vec3, Vec3) {
        (self.corr_p, self.corr_r)
    }

    /// Feed one IMU sample.
    pub fn push_imu(&mut self, s: ImuSample) {
        self.pred.push_imu(s);
    }

    /// Supervisor update with the tracker's newest pose (or `None`); repeated timestamps are ignored by both.
    pub fn update(&mut self, now_ns: i64, s: Option<VioSample>) -> TrackOutput {
        let tracking_before = self.sup.state() == TrackState::Tracking;
        let (g, a) = if self.still {
            (self.p.still_exit_gyro_std, self.p.still_exit_accel_std)
        } else {
            (self.p.still_gyro_std, self.p.still_accel_std)
        };
        self.still = self.p.still.hold && self.pred.imu_still(self.p.still_window_ns, g, a);
        let still = self.still;
        self.sup.set_still(still);
        self.pred.lying = still;
        let ts = s.map(|v| TrackSample {
            ts_ns: v.ts_ns.max(0) as u64,
            pos: v.pos,
            q: v.q,
            q3dof: v.q3dof,
        });
        let o = self.sup.update(now_ns.max(0) as u64, ts);
        if o.events & events::RESET_WANTED != 0 {
            self.pred.clear_vio();
        }
        if let Some(v) = s {
            let learn = self.p.learn_bias && tracking_before && o.state == TrackState::Tracking;
            self.pred.fuse_hold = still || !tracking_before;
            self.pred.push_vio(
                BodyState {
                    ts_ns: v.ts_ns,
                    p: v.pos,
                    q: v.q,
                    // Lying still: whatever velocity a landmark-less tracker integrated is not real.
                    v: if still { [0.0; 3] } else { v.vel },
                },
                learn,
            );
        }
        self.out = o.clone();
        o
    }

    fn config(&self) -> Config {
        Config {
            state: self.out.state,
            w_t: self.out.world_from_slam.0,
            w_q: self.out.world_from_slam.1,
            fix3dof: self.out.fix3dof,
            held: self.out.held_pos,
            base: self.pred.latest(),
        }
    }

    /// Output of a configuration at `t` (no correction).
    fn eval(&self, c: &Config, t: i64, rel3dof: &Rel) -> Rel {
        if c.state == TrackState::Tracking {
            if let Some(b) = c.base {
                let b = if t < b.ts_ns {
                    self.pred.base_at(t).unwrap_or(b)
                } else {
                    b
                };
                let (st, w_body) = self.pred.propagate(&b, t);
                let r = qrot(st.q, self.head_in_imu);
                let head_slam = geom::add(st.p, r);
                let w_slam = qrot(st.q, w_body);
                let v_head = geom::add(st.v, geom::cross(w_slam, r));
                return Rel {
                    pos: geom::add(qrot(c.w_q, head_slam), c.w_t),
                    q: qnorm(qmul(c.w_q, st.q)),
                    lin_vel: qrot(c.w_q, v_head),
                    ang_vel: qrot(c.w_q, w_slam),
                    flags: flags::ALL,
                };
            }
        }
        let mut f = rel3dof.flags & !(flags::POSITION_TRACKED | flags::LINEAR_VELOCITY_VALID);
        if c.state == TrackState::Lost {
            f |= flags::POSITION_VALID;
        }
        Rel {
            pos: c.held,
            q: qnorm(qmul(c.fix3dof, rel3dof.q)),
            lin_vel: [0.0; 3],
            ang_vel: qrot(c.fix3dof, rel3dof.ang_vel),
            flags: f,
        }
    }

    /// Let the correction decay up to `now_ns` (time constant + rate limit).
    fn decay(&mut self, now_ns: i64) {
        let Some(t0) = self.corr_ns else {
            self.corr_ns = Some(now_ns);
            return;
        };
        if now_ns <= t0 {
            return;
        }
        let dt = (now_ns - t0) as f64 / 1e9;
        self.corr_ns = Some(now_ns);
        let shrink = |c: Vec3, tau: f64, limit: f64| -> Vec3 {
            let n = geom::norm(c);
            if n < 1e-12 {
                return [0.0; 3];
            }
            let want = n * (1.0 - (-dt / tau.max(1e-3)).exp());
            let step = want.min(limit * dt);
            geom::scale(c, (n - step) / n)
        };
        let vl = self.p.v_min + self.p.k_v * self.speed;
        let wl = self.p.w_min + self.p.k_w * self.ang_speed;
        let mut tau = self.p.tau_pos_s;
        if self.p.tau_rest_s > tau && self.p.rest_speed > 0.0 {
            let rest = (1.0 - self.speed / self.p.rest_speed).clamp(0.0, 1.0);
            tau += (self.p.tau_rest_s - tau) * rest;
        }
        self.corr_p = shrink(self.corr_p, tau, vl);
        self.corr_r = shrink(self.corr_r, self.p.tau_rot_s, wl);
    }

    /// Head pose at `at_ns`. `rel3dof` = the 3DoF relation for the same time (used while not tracking);
    /// `now_ns` = host time (drives the correction decay).
    pub fn head(&mut self, now_ns: i64, at_ns: i64, rel3dof: &Rel) -> Rel {
        self.decay(now_ns);
        let cur = self.config();
        let prev = self.emitted;
        let mut raw = self.eval(&cur, at_ns, rel3dof);
        // Continuity: when the configuration changed since the last output, put the difference at this query
        // time into the correction. Past queries (before the newest tracker pose) wait for a current one.
        let current_query = cur.base.is_none_or(|b| at_ns >= b.ts_ns);
        if prev != Some(cur) && current_query {
            if let (Some(pc), true) = (prev, self.p.smooth) {
                let old = self.eval(&pc, at_ns, rel3dof);
                let old_out_p = geom::add(old.pos, self.corr_p);
                let old_out_q = qmul(qexp(self.corr_r), old.q);
                let np = geom::sub(old_out_p, raw.pos);
                let nr = qlog(qmul(old_out_q, qconj(raw.q)));
                if geom::norm(np) > self.p.snap_pos || geom::norm(nr) > self.p.snap_rot {
                    self.corr_p = [0.0; 3];
                    self.corr_r = [0.0; 3];
                    self.corr.snapped += 1;
                } else {
                    let ev = CorrEvent {
                        ts_ns: now_ns,
                        step: geom::sub(np, self.corr_p),
                        offset: np,
                        rot: geom::norm(nr),
                        speed: self.speed,
                        ang_speed: self.ang_speed,
                        pose_only: pc.state == cur.state
                            && pc.w_t == cur.w_t
                            && pc.w_q == cur.w_q
                            && pc.fix3dof == cur.fix3dof
                            && pc.held == cur.held,
                    };
                    self.corr_p = np;
                    self.corr_r = nr;
                    self.corr.blended += 1;
                    self.corr.max_pos = self.corr.max_pos.max(geom::norm(np));
                    self.corr.max_rot = self.corr.max_rot.max(geom::norm(nr));
                    if geom::norm(geom::sub(ev.step, [0.0; 3])) >= self.p.big_corr_m {
                        self.corr.big += 1;
                        let h = (ev.step[0] * ev.step[0] + ev.step[2] * ev.step[2]).sqrt();
                        if ev.step[1].abs() > h {
                            self.corr.big_vertical += 1;
                        }
                        if self.big_q.len() >= 64 {
                            self.big_q.pop_front();
                        }
                        self.big_q.push_back(ev);
                    }
                    if let Some(l) = self.corr_log.as_mut() {
                        l.push(ev);
                    }
                }
            }
            self.emitted = Some(cur);
        }
        if !self.p.smooth {
            self.corr_p = [0.0; 3];
            self.corr_r = [0.0; 3];
        }
        // Positions only carry a correction when the output has a position at all.
        raw.pos = geom::add(raw.pos, self.corr_p);
        raw.q = qnorm(qmul(qexp(self.corr_r), raw.q));
        if current_query {
            self.speed = geom::norm(raw.lin_vel);
            self.ang_speed = geom::norm(raw.ang_vel);
        }
        raw
    }

    /// Oldest not yet taken correction step of at least `HeadParams::big_corr_m` (the last 64 are kept).
    pub fn take_big(&mut self) -> Option<CorrEvent> {
        self.big_q.pop_front()
    }

    /// Head position in the world for a tracker pose with the current alignment (no correction).
    pub fn head_world(&self, pos: Vec3, q: Quat) -> Vec3 {
        self.sup.head_world(pos, q)
    }

    /// Yaw (deg) of the current room alignment.
    pub fn room_yaw_deg(&self) -> f64 {
        yaw_of(self.out.world_from_slam.1).to_degrees()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slam::{qangle_deg, qyaw};

    #[test]
    fn still_state_has_hysteresis() {
        let feed = |h: &mut HeadTracker, from: i64, n: i64, amp: f64| {
            for i in from..from + n {
                let w = if i % 2 == 0 { amp } else { -amp };
                h.push_imu(ImuSample {
                    ts_ns: i * 1_000_000,
                    gyro: [0.013, 0.0, 0.0],
                    accel: [w, 9.8, 0.0],
                });
            }
            h.update(((from + n) * 1_000_000) as i64, None);
            h.still
        };
        let mut h = HeadTracker::new(SupervisorParams::default(), HeadParams::default(), [0.0; 3]);
        assert!(feed(&mut h, 0, 1000, 0.01), "desk-still enters");
        assert!(feed(&mut h, 1000, 600, 0.1), "0.1 m/s^2 vibration keeps it");
        assert!(!feed(&mut h, 1600, 600, 0.3), "real motion ends it");
        assert!(!feed(&mut h, 2200, 600, 0.1), "0.1 m/s^2 does not enter");
    }

    #[test]
    fn imu_still_detection() {
        let mut p = Predictor::new(PredictorParams::default());
        let mut seed = 1u64;
        let mut rnd = || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5
        };
        // 1 s lying still (gyro bias + 0.001 rad/s noise, accel 0.01 m/s^2), then 1 s of head sway.
        for i in 0..2000i64 {
            let amp = if i < 1000 { 1.0 } else { 20.0 };
            p.push_imu(ImuSample {
                ts_ns: i * 1_000_000,
                gyro: [0.013 + 0.003 * amp * rnd(), -0.004 + 0.003 * amp * rnd(), 0.003 * amp * rnd()],
                accel: [0.03 * amp * rnd(), 9.8 + 0.03 * amp * rnd(), 0.03 * amp * rnd()],
            });
            if i == 999 {
                assert!(p.imu_still(500_000_000, 0.004, 0.03), "still");
            }
            if i == 300 {
                assert!(!p.imu_still(500_000_000, 0.004, 0.03), "window not covered yet");
            }
        }
        assert!(!p.imu_still(500_000_000, 0.004, 0.03), "moving");
    }

    /// Smooth head motion: slow walk on a circle, head yaw/pitch oscillation. World y-up; returns body
    /// position, orientation, velocity, angular velocity (world) and acceleration (world) at t (s).
    fn truth(t: f64) -> (Vec3, Quat, Vec3, Vec3, Vec3) {
        let r = 0.8;
        let wc = 0.35; // rad/s around the circle
        let p = [
            r * (wc * t).sin(),
            1.6 + 0.03 * (1.3 * t).sin(),
            r * (wc * t).cos() - r,
        ];
        let v = [
            r * wc * (wc * t).cos(),
            0.03 * 1.3 * (1.3 * t).cos(),
            -r * wc * (wc * t).sin(),
        ];
        let a = [
            -r * wc * wc * (wc * t).sin(),
            -0.03 * 1.69 * (1.3 * t).sin(),
            -r * wc * wc * (wc * t).cos(),
        ];
        // yaw(t) = 0.6 sin(0.9 t) + 0.2 t ; pitch(t) = 0.25 sin(1.7 t)
        let yaw = 0.6 * (0.9 * t).sin() + 0.2 * t;
        let pitch = 0.25 * (1.7 * t).sin();
        let qy = qyaw(yaw);
        let qp = [(pitch / 2.0).sin(), 0.0, 0.0, (pitch / 2.0).cos()];
        let q = qmul(qy, qp);
        // angular velocity (world) = yaw_rate * Y + Ry(yaw) * pitch_rate * X
        let yr = 0.54 * (0.9 * t).cos() + 0.2;
        let pr = 0.425 * (1.7 * t).cos();
        let w = geom::add([0.0, yr, 0.0], qrot(qy, [pr, 0.0, 0.0]));
        (p, q, v, w, a)
    }

    fn imu_at(t: f64, bias: Vec3) -> ImuSample {
        let (_, q, _, w, a) = truth(t);
        let gyro = geom::add(qrot(qconj(q), w), bias);
        let accel = qrot(qconj(q), geom::sub(a, G_WORLD));
        ImuSample {
            ts_ns: (t * 1e9).round() as i64,
            accel,
            gyro,
        }
    }

    fn rel3(q: Quat) -> Rel {
        Rel {
            q,
            flags: flags::ORIENTATION_VALID | flags::ORIENTATION_TRACKED,
            ..Default::default()
        }
    }

    #[test]
    fn exp_log_roundtrip() {
        for w in [
            [0.0, 0.0, 0.0],
            [0.1, -0.2, 0.3],
            [2.0, 1.0, -1.5],
            [1e-9, 0.0, 0.0],
        ] {
            let back = qlog(qexp(w));
            assert!(geom::norm(geom::sub(back, w)) < 1e-9, "{w:?} -> {back:?}");
        }
    }

    #[test]
    fn predictor_tracks_true_motion_from_imu() {
        let mut pr = Predictor::new(PredictorParams::default());
        for i in 0..3000 {
            pr.push_imu(imu_at(i as f64 * 0.001, [0.0; 3]));
        }
        // Base at 2.0 s from truth, predict 0..60 ms ahead within the IMU span and 20 ms past it.
        let (p, q, v, _, _) = truth(2.0);
        let base = BodyState {
            ts_ns: 2_000_000_000,
            p,
            q,
            v,
        };
        for dt_ms in [10.0, 40.0, 60.0] {
            let t = 2.0 + dt_ms / 1000.0;
            let (st, _) = pr.propagate(&base, (t * 1e9) as i64);
            let (tp, tq, _, _, _) = truth(t);
            let ep = geom::norm(geom::sub(st.p, tp));
            let er = qangle_deg(qmul(qconj(st.q), tq));
            assert!(ep < 2e-4 && er < 0.02, "{dt_ms} ms: {ep} m {er} deg");
        }
        let (st, _) = pr.propagate(&base, 3_020_000_000 - 1_000_000_000 + 1_000_000_000);
        assert_eq!(st.ts_ns, 3_020_000_000);
    }

    #[test]
    fn gyro_bias_is_learned_from_the_tracker() {
        let bias = [0.01, -0.02, 0.015]; // ~1 deg/s
        let mut pr = Predictor::new(PredictorParams {
            bias_tau_s: 2.0,
            ..Default::default()
        });
        let mut t = 0.0;
        let mut next_vio = 0.1;
        while t < 30.0 {
            pr.push_imu(imu_at(t, bias));
            if t >= next_vio {
                let (p, q, v, _, _) = truth(t);
                pr.push_vio(
                    BodyState {
                        ts_ns: (t * 1e9).round() as i64,
                        p,
                        q,
                        v,
                    },
                    true,
                );
                next_vio += 0.04;
            }
            t += 0.001;
        }
        let e = geom::norm(geom::sub(pr.gyro_bias, bias));
        assert!(
            e < 0.1 * geom::norm(bias),
            "bias {:?} vs {bias:?}",
            pr.gyro_bias
        );
        assert!(pr.bias_samples > 500);
    }

    /// Full loop: IMU 1 kHz, VIO 25 Hz delivered 25 ms late with noise, queries at 90 Hz for display time
    /// (now + 20 ms). Returns per-query (output, truth at the display time).
    struct Sim {
        h: HeadTracker,
        rng: u64,
    }

    impl Sim {
        fn new(p: HeadParams) -> Sim {
            let sp = SupervisorParams {
                acquire_poses: 3,
                ..Default::default()
            };
            Sim {
                h: HeadTracker::new(sp, p, [0.0, -0.02, 0.07]),
                rng: 0x1234_5678,
            }
        }
        fn gauss(&mut self) -> f64 {
            // Irwin-Hall approximation from splitmix64.
            let mut s = 0.0;
            for _ in 0..12 {
                self.rng = self.rng.wrapping_add(0x9E37_79B9_7F4A_7C15);
                let mut z = self.rng;
                z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
                z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
                z ^= z >> 31;
                s += (z >> 11) as f64 / (1u64 << 53) as f64;
            }
            s - 6.0
        }
        /// Run from t0 to t1; `vio(t)` gives the tracker pose (world-frame truth transformed by the caller) or None.
        fn run(
            &mut self,
            t0: f64,
            t1: f64,
            noise_m: f64,
            vio: &mut dyn FnMut(f64) -> Option<(Vec3, Quat, Vec3)>,
            out: &mut Vec<(f64, Rel)>,
        ) {
            let mut t = t0;
            let mut next_vio = t0;
            let mut pending: Vec<(f64, VioSample)> = Vec::new();
            let mut next_q = t0;
            while t < t1 {
                let now = (t * 1e9).round() as i64;
                self.h.push_imu(imu_at(t, [0.0; 3]));
                if t >= next_vio {
                    if let Some((p, q, v)) = vio(t) {
                        let n = [
                            self.gauss() * noise_m,
                            self.gauss() * noise_m,
                            self.gauss() * noise_m,
                        ];
                        let (_, q3, _, _, _) = truth(t);
                        pending.push((
                            t + 0.025,
                            VioSample {
                                ts_ns: now,
                                pos: geom::add(p, n),
                                q,
                                vel: v,
                                q3dof: q3,
                            },
                        ));
                    }
                    next_vio += 0.04;
                }
                if let Some(i) = pending.iter().position(|(due, _)| *due <= t) {
                    let (_, s) = pending.remove(i);
                    self.h.update(now, Some(s));
                } else {
                    self.h.update(now, None);
                }
                if t >= next_q {
                    let at = t + 0.020;
                    let (_, q3, _, w, _) = truth(at);
                    let mut r = rel3(q3);
                    r.ang_vel = w;
                    let o = self.h.head(now, (at * 1e9).round() as i64, &r);
                    out.push((at, o));
                    next_q += 1.0 / 90.0;
                }
                t += 0.001;
            }
        }
    }

    fn head_truth(t: f64, head: Vec3) -> (Vec3, Quat) {
        let (p, q, _, _, _) = truth(t);
        (geom::add(p, qrot(q, head)), q)
    }

    /// Tracker world = truth yawed and offset (as Basalt's own world would be).
    fn slam_of(t: f64, yaw: f64, off: Vec3) -> (Vec3, Quat, Vec3) {
        let (p, q, v, _, _) = truth(t);
        let wq = qyaw(yaw);
        (geom::add(qrot(wq, p), off), qmul(wq, q), qrot(wq, v))
    }

    /// First tracked output at or after `t_ref` (the initial acquisition slide has decayed by then).
    fn ref_at(out: &[(f64, Rel)], t_ref: f64) -> (f64, Rel) {
        *out.iter()
            .find(|(t, r)| *t >= t_ref && r.flags & flags::POSITION_TRACKED != 0)
            .expect("tracked output")
    }

    /// Where the output should be at `t` given a reference output (room frame defined by the tracker alignment).
    fn expected(rf: &(f64, Rel), t: f64, head: Vec3, extra: Vec3) -> Vec3 {
        let (p0, q0) = head_truth(rf.0, head);
        let (pt, _) = head_truth(t, head);
        let wq = qyaw(yaw_of(qmul(rf.1.q, qconj(q0))));
        geom::add(rf.1.pos, qrot(wq, geom::add(geom::sub(pt, p0), extra)))
    }

    /// Error of the outputs against truth after aligning the first tracked output (the room frame is defined at
    /// acquisition, so compare displacements from there).
    fn errors(out: &[(f64, Rel)], from: f64, head: Vec3) -> (f64, f64, f64) {
        let tracked: Vec<&(f64, Rel)> = out
            .iter()
            .filter(|(t, r)| *t >= from && r.flags & flags::POSITION_TRACKED != 0)
            .collect();
        let (t0, r0) = tracked[0];
        let (p0, q0) = head_truth(*t0, head);
        // world_from_truth: yaw + offset mapping truth to the output frame.
        let wq = qyaw(yaw_of(qmul(r0.q, qconj(q0))));
        let mut max_p: f64 = 0.0;
        let mut max_r: f64 = 0.0;
        let mut jit: f64 = 0.0;
        let mut prev: Option<(Vec3, Vec3)> = None;
        for (t, r) in tracked {
            let (pt, qt) = head_truth(*t, head);
            let pe = geom::add(qrot(wq, geom::sub(pt, p0)), r0.pos);
            let e = geom::sub(r.pos, pe);
            max_p = max_p.max(geom::norm(e));
            max_r = max_r.max(qangle_deg(qmul(qconj(r.q), qmul(wq, qt))));
            if let Some((pe_prev, e_prev)) = prev {
                let _ = pe_prev;
                jit = jit.max(geom::norm(geom::sub(e, e_prev)));
            }
            prev = Some((pe, e));
        }
        (max_p, max_r, jit)
    }

    #[test]
    fn noisy_tracker_is_smoothed_without_lag() {
        let head = [0.0, -0.02, 0.07];
        let mut raw_out = Vec::new();
        let mut sm_out = Vec::new();
        for (smooth, out) in [(false, &mut raw_out), (true, &mut sm_out)] {
            let mut sim = Sim::new(HeadParams {
                smooth,
                ..Default::default()
            });
            let mut vio = |t: f64| Some(slam_of(t, 0.7, [1.0, 0.0, -2.0]));
            sim.run(0.0, 8.0, 0.002, &mut vio, out);
            assert_eq!(sim.h.output().state, TrackState::Tracking);
        }
        let (rp, _, rj) = errors(&raw_out, 1.0, head);
        let (sp, sr, sj) = errors(&sm_out, 1.0, head);
        // Smoothing reduces frame-to-frame error jumps clearly and keeps the absolute error small (no lag:
        // motion comes from the IMU, only the tracker noise is filtered).
        eprintln!("noise 2 mm: raw max err {rp:.4} m, max step err {rj:.4}; smoothed {sp:.4} m, {sj:.4}, rot {sr:.3} deg");
        assert!(sj < 0.6 * rj, "jitter raw {rj} smoothed {sj}");
        assert!(sp < 0.008, "smoothed max error {sp} (raw {rp})");
        assert!(sr < 0.5, "rotation error {sr} deg");
    }

    #[test]
    fn tracker_correction_slides_instead_of_jumping() {
        let head = [0.0, -0.02, 0.07];
        let mut sim = Sim::new(HeadParams::default());
        let mut out = Vec::new();
        // At 3 s the tracker corrects itself by 6 cm (e.g. after re-observing old features).
        let mut vio = |t: f64| {
            let mut s = slam_of(t, 0.0, [0.0; 3]);
            if t >= 3.0 {
                s.0[0] += 0.06;
            }
            Some(s)
        };
        sim.run(0.0, 6.0, 0.0, &mut vio, &mut out);
        // Largest step between consecutive outputs beyond the true motion: bounded by the rate limit.
        let mut max_step: f64 = 0.0;
        for w in out.windows(2) {
            let ((t0, a), (t1, b)) = (&w[0], &w[1]);
            if a.flags & flags::POSITION_TRACKED == 0 || *t0 < 1.0 {
                continue;
            }
            let (p0, _) = head_truth(*t0, head);
            let (p1, _) = head_truth(*t1, head);
            let extra = geom::norm(geom::sub(geom::sub(b.pos, a.pos), geom::sub(p1, p0)));
            max_step = max_step.max(extra);
        }
        // 90 Hz: rate limit (0.1 + 0.5 * ~0.3 m/s) / 90 ~ 3 mm per frame; a raw 6 cm jump would show here.
        eprintln!(
            "6 cm tracker correction: max extra step {max_step:.4} m/frame, {:?}",
            sim.h.corr
        );
        assert!(max_step < 0.006, "step {max_step}");
        // The output ends on the corrected track (the shift is in the tracker world, so it stays).
        let (t_end, last) = *out.last().unwrap();
        let want = expected(&ref_at(&out, 2.5), t_end, head, [0.06, 0.0, 0.0]);
        assert!(
            geom::norm(geom::sub(last.pos, want)) < 0.003,
            "{:?} vs {want:?}",
            last.pos
        );
        // With the fused position (default) the 6 cm arrive over several poses, each a small step.
        assert!(sim.h.corr.blended > 0 && sim.h.corr.max_pos < 0.05);
    }

    #[test]
    fn loss_then_same_world_resnaps_the_room() {
        let head = [0.0, -0.02, 0.07];
        let mut sim = Sim::new(HeadParams::default());
        let mut out = Vec::new();
        // Tracker silent from 3.0 to 4.5 s (head keeps walking ~0.4 m), then back in the SAME world.
        let mut vio = |t: f64| (!(3.0..4.5).contains(&t)).then(|| slam_of(t, 0.3, [0.5, 0.0, 0.5]));
        sim.run(0.0, 9.0, 0.0, &mut vio, &mut out);
        assert_eq!(sim.h.sup.losses, 1);
        assert_eq!(sim.h.sup.acquisitions, 2);
        assert!(
            sim.h.output().reason.contains("restored"),
            "{}",
            sim.h.output().reason
        );
        // After the slide the output is back on the true room track (as if never lost).
        let (t_end, last) = *out.last().unwrap();
        let want = expected(&ref_at(&out, 2.5), t_end, head, [0.0; 3]);
        eprintln!(
            "loss 1.5 s, same world: {} -> end error {:.4} m, {:?}",
            sim.h.output().reason,
            geom::norm(geom::sub(last.pos, want)),
            sim.h.corr
        );
        assert!(
            geom::norm(geom::sub(last.pos, want)) < 0.01,
            "{:?} vs {want:?}",
            last.pos
        );
        // No visible jump at loss or re-acquisition.
        for w in out.windows(2) {
            let ((t0, a), (t1, b)) = (&w[0], &w[1]);
            if *t0 < 1.0 {
                continue;
            }
            let dq = qangle_deg(qmul(qconj(a.q), b.q));
            let (_, qa) = head_truth(*t0, head);
            let (_, qb) = head_truth(*t1, head);
            let true_dq = qangle_deg(qmul(qconj(qa), qb));
            assert!(
                (dq - true_dq).abs() < 0.5,
                "rotation jump at {t0}: {dq} vs {true_dq}"
            );
            let step = geom::norm(geom::sub(b.pos, a.pos));
            assert!(step < 0.012, "position jump at {t0}: {step}");
        }
    }

    #[test]
    fn reset_world_realigns_continuously() {
        let head = [0.0, -0.02, 0.07];
        let mut sim = Sim::new(HeadParams::default());
        let mut out = Vec::new();
        // Tracker comes back in a NEW world (other yaw and origin): no re-snap, continuous re-alignment.
        let mut vio = |t: f64| {
            if (3.0..4.0).contains(&t) {
                None
            } else if t < 3.0 {
                Some(slam_of(t, 0.0, [0.0; 3]))
            } else {
                Some(slam_of(t, 1.2, [3.0, 0.0, 1.0]))
            }
        };
        sim.run(0.0, 7.0, 0.0, &mut vio, &mut out);
        assert_eq!(sim.h.sup.acquisitions, 2);
        assert!(!sim.h.output().reason.contains("restored"));
        for w in out.windows(2) {
            let ((t0, a), (_, b)) = (&w[0], &w[1]);
            if *t0 < 1.0 {
                continue;
            }
            assert!(geom::norm(geom::sub(b.pos, a.pos)) < 0.012, "jump at {t0}");
        }
        let _ = head;
    }
}
