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

/// Camera image brightness handed from the auto-exposure loop to the head tracker (both live in this library
/// inside the same Monado process; no driver change needed). A leaf: a stale or missing value means "unknown".
pub mod scene {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::OnceLock;
    use std::time::Instant;

    static PACKED: AtomicU64 = AtomicU64::new(0);
    static BASE: OnceLock<Instant> = OnceLock::new();

    fn now_ms() -> u64 {
        BASE.get_or_init(Instant::now).elapsed().as_millis() as u64 + 1
    }

    /// Publish the darker pair's trimmed mean (0..255).
    pub fn publish(mean: f64) {
        if !mean.is_finite() {
            return;
        }
        let m = (mean.clamp(0.0, 255.0) * 100.0) as u64;
        PACKED.store((now_ms() << 32) | (m & 0xffff_ffff), Ordering::Relaxed);
    }

    /// The newest mean if it is at most `max_age_ms` old.
    pub fn latest(max_age_ms: u64) -> Option<f64> {
        let v = PACKED.load(Ordering::Relaxed);
        let ts = v >> 32;
        if ts == 0 || now_ms().saturating_sub(ts) > max_age_ms {
            return None;
        }
        Some((v & 0xffff_ffff) as f64 / 100.0)
    }
}

/// Auto-exposure mean (0..255, target 64) below which the image counts as getting dark ...
pub const SCENE_DARK_FROM: f64 = 32.0;
/// ... and at which as fully dark (session 2026-10-16 18:18 ended at means 9-15 with exposure and gain capped).
pub const SCENE_DARK_FULL: f64 = 12.0;

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
    /// Automatic fusion weights (`PIMAX_FUSE_AUTO`): the tracker's velocity noise is estimated online (running
    /// RMS of tracker velocity - IMU-propagated velocity) and, with a dark camera image ([`Predictor::scene_dark`]),
    /// turns a noisy tracker down where its noise shows, at rest and in slow motion: velocity weight
    /// `fuse_vel_w` -> 0.2 x it (0.5 -> 0.1), alphas and beta -> half. Fast motion keeps following the tracker
    /// (weights rise to the configured alphas and velocity weight 1 as before: lowering them there only added
    /// lag, 2026-10-16 replays). A clean tracker keeps the configured values.
    pub fuse_auto: bool,
    /// Velocity residual RMS (m/s) at and below which the tracker counts as clean ...
    pub noise_lo: f64,
    /// ... and at and above which as fully noisy.
    pub noise_hi: f64,
    /// Time constant of the running residual estimate (s).
    pub noise_tau_s: f64,
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
            fuse_auto: false,
            noise_lo: 0.05,
            noise_hi: 0.15,
            noise_tau_s: 2.0,
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
    /// Running mean square of (tracker velocity - IMU-propagated velocity), m²/s² ([`PredictorParams::fuse_auto`]).
    pub vel_noise_ms: f64,
    /// Residual samples taken.
    pub vel_noise_n: u64,
    /// Camera image darkness 0 (fine) .. 1 (too dark to track well), set by the caller (auto-exposure mean).
    pub scene_dark: f64,
    /// The tracker's own newest state (before fusion), for the velocity noise estimate.
    last_raw: Option<BodyState>,
}

/// Window statistics of the buffered IMU (per-axis means and standard deviations).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ImuWindow {
    /// Samples.
    pub n: usize,
    /// Mean gyro (rad/s, raw).
    pub gyro_mean: Vec3,
    /// Gyro standard deviation per axis (rad/s).
    pub gyro_std: Vec3,
    /// Mean accelerometer (m/s²).
    pub accel_mean: Vec3,
    /// Accelerometer standard deviation per axis (m/s²).
    pub accel_std: Vec3,
    /// Largest bias-corrected angular rate in the window (rad/s).
    pub gyro_max: f64,
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
            vel_noise_ms: 0.0,
            vel_noise_n: 0,
            scene_dark: 0.0,
            last_raw: None,
        }
    }

    /// Tracker noise level 0 (clean: the configured weights) .. 1 (noisy: lowest weights), from the velocity
    /// residual and the image darkness; 0 with `fuse_auto` off.
    pub fn noise_level(&self) -> f64 {
        if !self.p.fuse_auto {
            return 0.0;
        }
        let vel = if self.vel_noise_n < 5 || self.p.noise_hi <= self.p.noise_lo {
            0.0
        } else {
            ((self.vel_noise_ms.sqrt() - self.p.noise_lo) / (self.p.noise_hi - self.p.noise_lo)).clamp(0.0, 1.0)
        };
        vel.max(self.scene_dark.clamp(0.0, 1.0))
    }

    /// Effective fusion weights `[alpha, alpha_y, beta, vel_w, fast-motion vel_w]` at the current noise level
    /// and head speed fraction `fast` (0 = at rest .. 1 = at `fuse_fast_speed`): the position pull is lowered
    /// only where its steps would show (at rest), so fast motion keeps following the tracker without lag.
    pub fn fuse_weights(&self, fast: f64) -> [f64; 5] {
        let n = self.noise_level();
        let k = 1.0 - 0.5 * n * (1.0 - fast.clamp(0.0, 1.0));
        [
            self.p.fuse_alpha * k,
            self.p.fuse_alpha_y * k,
            self.p.fuse_beta * k,
            self.p.fuse_vel_w * (1.0 - 0.8 * n),
            1.0,
        ]
    }

    /// Statistics of the newest `window_ns` of IMU samples; `None` until the window is covered.
    pub fn imu_window(&self, window_ns: i64) -> Option<ImuWindow> {
        let last = self.imu.back()?;
        let from = last.ts_ns - window_ns;
        if self.imu.front().is_none_or(|f| f.ts_ns > from + window_ns / 10) {
            return None; // window not covered yet
        }
        let (mut n, mut sg, mut sg2, mut sa, mut sa2) = (0usize, [0.0; 3], [0.0; 3], [0.0; 3], [0.0; 3]);
        let mut gmax: f64 = 0.0;
        for s in self.imu.iter().rev().take_while(|s| s.ts_ns >= from) {
            n += 1;
            for k in 0..3 {
                sg[k] += s.gyro[k];
                sg2[k] += s.gyro[k] * s.gyro[k];
                sa[k] += s.accel[k];
                sa2[k] += s.accel[k] * s.accel[k];
            }
            gmax = gmax.max(geom::norm(geom::sub(s.gyro, self.gyro_bias)));
        }
        if n == 0 {
            return None;
        }
        let nf = n as f64;
        let sd = |s: f64, s2: f64| (s2 / nf - (s / nf) * (s / nf)).max(0.0).sqrt();
        Some(ImuWindow {
            n,
            gyro_mean: geom::scale(sg, 1.0 / nf),
            gyro_std: [sd(sg[0], sg2[0]), sd(sg[1], sg2[1]), sd(sg[2], sg2[2])],
            accel_mean: geom::scale(sa, 1.0 / nf),
            accel_std: [sd(sa[0], sa2[0]), sd(sa[1], sa2[1]), sd(sa[2], sa2[2])],
            gyro_max: gmax,
        })
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
        match self.imu_window(window_ns) {
            Some(w) if w.n >= 10 => (0..3).all(|k| w.gyro_std[k] < gyro_std && w.accel_std[k] < accel_std),
            _ => false,
        }
    }

    /// Newest IMU timestamp.
    pub fn last_imu_ns(&self) -> Option<i64> {
        self.imu.back().map(|s| s.ts_ns)
    }

    /// Drop the tracker poses (tracker reset: new world).
    pub fn clear_vio(&mut self) {
        self.vio.clear();
        self.last_raw = None;
    }

    /// Tracker velocity noise: the tracker's own previous state (not the fused one: the estimate must not depend
    /// on the weights it sets) carried to this pose by the IMU predicts this velocity; over 40 ms the IMU is
    /// accurate, so the running RMS of the difference is the tracker's velocity noise (x sqrt 2).
    fn learn_vel_noise(&mut self, raw: &BodyState) {
        let Some(prev) = self.last_raw else {
            return;
        };
        let dt = (raw.ts_ns - prev.ts_ns) as f64 / 1e9;
        if !(0.005..0.15).contains(&dt) || self.imu.back().is_none_or(|s| s.ts_ns < raw.ts_ns) {
            return;
        }
        let (pr, _) = self.propagate(&prev, raw.ts_ns);
        let dv = geom::norm(geom::sub(raw.v, pr.v));
        let k = (dt / self.p.noise_tau_s.max(1e-3)).min(1.0);
        if self.vel_noise_n == 0 {
            self.vel_noise_ms = dv * dv;
        } else {
            self.vel_noise_ms += (dv * dv - self.vel_noise_ms) * k;
        }
        self.vel_noise_n += 1;
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
        if self.p.fuse_auto && !self.fuse_hold && !self.lying {
            self.learn_vel_noise(&st);
        }
        self.last_raw = (!self.fuse_hold && !self.lying).then_some(st);
        if self.p.fuse_alpha > 0.0 && !self.fuse_hold {
            if let Some(prev) = self.vio.back().copied() {
                let dt = (st.ts_ns - prev.ts_ns) as f64 / 1e9;
                let (pr, _) = self.propagate(&prev, st.ts_ns);
                let r = geom::sub(st.p, pr.p);
                if dt > 0.0 && dt < 0.25 && geom::norm(r) < self.p.fuse_reset_m {
                    let f = if self.p.fuse_fast_speed > 0.0 {
                        (geom::norm(pr.v) / self.p.fuse_fast_speed).min(1.0)
                    } else {
                        0.0
                    };
                    let [a, ay, b, mut w, w_fast] = self.fuse_weights(f);
                    let b = b / dt;
                    w += (w_fast - w).max(0.0) * f;
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
    /// Window (ns) over which the still state is entered: the headset has to be desk-quiet this long. What tells
    /// a worn head from a desk is the level (session 2026-10-16 18:18: worn rest >= 0.3 deg/s gyro std in every
    /// 0.5 s window; on the desk 0.05-0.06 deg/s, the bare sensor noise; settling after a put-down 0.07-0.15),
    /// not the length: a landmark-free tracker ran away at > 1 m/s within 0.5 s of a put-down (21:30 session),
    /// so a longer window only lets that drift in (the hold anchors at the start of the window either way).
    pub still_enter_window_ns: i64,
    /// Window (ns) the exit test looks at (short: a pickup ends the still state at once) ...
    pub still_window_ns: i64,
    /// ... per-axis standard deviation limits (rad/s, m/s²) to enter the still state (over
    /// `still_enter_window_ns`) ...
    pub still_gyro_std: f64,
    /// See `still_gyro_std`.
    pub still_accel_std: f64,
    /// ... and the larger ones that end it (hysteresis: a cushion or desk with a fan nearby flickered around a
    /// single 0.03 m/s² limit in the 16:24 live run, and every flicker re-anchored the hold on the drifted
    /// tracker). Worn walks never go below 0.027 rad/s.
    pub still_exit_gyro_std: f64,
    /// See `still_exit_gyro_std`.
    pub still_exit_accel_std: f64,
    /// ... and the mean gyro must be within this (rad/s) of the learned gyro bias (a slow steady head turn is
    /// flat but not still; LESSONS row 36). Used once the bias has been learned for a while; 0 = off.
    pub still_bias_tol: f64,
    /// Gravity tilt correction (`PIMAX_HEAD_TILT`): pitch and roll of the output are pulled slowly towards the
    /// accelerometer's gravity (yaw and position stay the tracker's), with this time constant (s; 0 = off) ...
    pub tilt_tau_s: f64,
    /// ... never faster than this (rad/s) ...
    pub tilt_rate: f64,
    /// ... and only while |accel| is within this of g (m/s²) ...
    pub tilt_g_tol: f64,
    /// ... and the head turns slower than this (rad/s).
    pub tilt_gyro_max: f64,
    /// Largest tilt correction (rad); a larger measured difference is not gravity (skipped).
    pub tilt_max: f64,
    /// Corrections whose step reaches this (m) are counted and queued for the log ([`HeadTracker::take_big`]).
    pub big_corr_m: f64,
    /// Fused position: `[alpha, alpha vertical, beta, tracker velocity weight, fast speed]`
    /// ([`PredictorParams::fuse_alpha`]); alpha 0 = off. These are the values for a clean tracker ...
    pub fuse: [f64; 5],
    /// ... lowered automatically for a noisy or blind one ([`PredictorParams::fuse_auto`]).
    pub fuse_auto: bool,
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
            still_enter_window_ns: 500_000_000,
            still_window_ns: 500_000_000,
            still_gyro_std: 0.0035,
            still_accel_std: 0.03,
            still_exit_gyro_std: 0.012,
            still_exit_accel_std: 0.15,
            still_bias_tol: 0.005,
            tilt_tau_s: 1.5,
            tilt_rate: 0.5f64.to_radians(),
            tilt_g_tol: 0.3,
            tilt_gyro_max: 0.15,
            tilt_max: 5f64.to_radians(),
            big_corr_m: 0.02,
            fuse: [0.15, 0.075, 0.06, 0.5, 0.5],
            fuse_auto: true,
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
    /// `PIMAX_SLAM_BIG_CORR_MM` (log threshold), `PIMAX_FUSE_AUTO` (0 = fixed fusion weights),
    /// `PIMAX_HEAD_TILT` (0 = no gravity tilt correction, else its time constant in ms) overrides; for A/B only,
    /// the defaults are automatic.
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
        if let Some(v) = env_f64("PIMAX_FUSE_AUTO") {
            p.fuse_auto = v != 0.0;
        }
        if let Some(v) = env_f64("PIMAX_HEAD_TILT") {
            p.tilt_tau_s = if v <= 0.0 { 0.0 } else { v.clamp(100.0, 60_000.0) / 1000.0 };
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

    /// One log line naming the automatic output stages (the build-marker strings `PIMAX_HEAD_TILT` and
    /// `PIMAX_FUSE_AUTO` are in it).
    pub fn describe(&self) -> String {
        let tilt = if self.tilt_tau_s > 0.0 {
            format!(
                "on (tau {:.1} s, <= {:.2} deg/s)",
                self.tilt_tau_s,
                self.tilt_rate.to_degrees()
            )
        } else {
            "off".to_string()
        };
        let auto = if self.fuse_auto && self.fuse[0] > 0.0 {
            format!(
                "on (velocity weight {:.2}..{:.2})",
                self.fuse[3] * 0.2,
                self.fuse[3]
            )
        } else {
            "off".to_string()
        };
        format!(
            "6DoF output: PIMAX_HEAD_TILT gravity tilt correction {tilt}; PIMAX_FUSE_AUTO fusion weights {auto}; \
             still = desk-quiet ({:.2} deg/s, {:.1} s)",
            self.still_gyro_std.to_degrees(),
            self.still_enter_window_ns as f64 / 1e9
        )
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
    /// Gravity tilt correction (world frame, horizontal axis: pitch/roll only), applied to the tracked output.
    tilt: Quat,
    tilt_ns: Option<i64>,
    tilt_reset: bool,
    /// Recent raw tracker head positions (pose time, tracker world) for the still anchor.
    head_hist: VecDeque<(i64, Vec3)>,
    /// Tilt correction updates applied / skipped by the gates.
    pub tilt_updates: u64,
    /// See `tilt_updates`.
    pub tilt_skipped: u64,
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
                fuse_auto: p.fuse_auto,
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
            tilt: QUAT_ID,
            tilt_ns: None,
            tilt_reset: false,
            head_hist: VecDeque::new(),
            tilt_updates: 0,
            tilt_skipped: 0,
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
        let was_still = self.still;
        self.still = self.p.still.hold && self.still_test();
        let still = self.still;
        if still && !was_still {
            // Entered after `still_enter_window_ns` of quiet: hold the head where it was when that began.
            let from = self.pred.last_imu_ns().unwrap_or(0) - self.p.still_enter_window_ns;
            if let Some(&(_, hs)) = self.head_hist.iter().rev().find(|(t, _)| *t <= from) {
                self.sup.set_still_anchor(hs);
            }
        }
        self.sup.set_still(still);
        self.pred.lying = still;
        let ts = s.map(|v| TrackSample {
            ts_ns: v.ts_ns.max(0) as u64,
            pos: v.pos,
            q: v.q,
            q3dof: v.q3dof,
        });
        self.sup.set_tilt(self.tilt);
        let o = self.sup.update(now_ns.max(0) as u64, ts);
        if o.events & events::RESET_WANTED != 0 {
            self.pred.clear_vio();
            self.head_hist.clear();
            // A fresh tracker world starts gravity aligned (Basalt initialises its attitude from the
            // accelerometer): its correction starts from zero when it takes over. Until then the held attitude
            // carries the old correction (and the last tracked output is still evaluated with it).
            self.tilt_reset = true;
        }
        if self.tilt_reset && o.state == TrackState::Tracking {
            self.tilt_reset = false;
            self.tilt = QUAT_ID;
            self.tilt_ns = None;
        }
        if let Some(v) = s {
            if self.head_hist.back().is_none_or(|(t, _)| v.ts_ns > *t) {
                self.head_hist
                    .push_back((v.ts_ns, geom::add(v.pos, qrot(qnorm(v.q), self.head_in_imu))));
                while self.head_hist.len() > 150 {
                    self.head_hist.pop_front();
                }
            }
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
            if tracking_before && o.state == TrackState::Tracking {
                self.out = o.clone();
                self.update_tilt(&v);
            }
        }
        self.out = o.clone();
        o
    }

    /// Still state with hysteresis: entering needs `still_enter_window_ns` of desk-level quiet (and a mean gyro
    /// at the learned bias), leaving takes a short window above the larger exit limits.
    fn still_test(&self) -> bool {
        if self.still {
            return self.pred.imu_still(
                self.p.still_window_ns,
                self.p.still_exit_gyro_std,
                self.p.still_exit_accel_std,
            );
        }
        let Some(w) = self.pred.imu_window(self.p.still_enter_window_ns) else {
            return false;
        };
        if w.n < 10
            || !(0..3).all(|k| w.gyro_std[k] < self.p.still_gyro_std && w.accel_std[k] < self.p.still_accel_std)
        {
            return false;
        }
        // 250 tracker poses = 10 s of learning: before that the bias estimate may still be far off.
        if self.p.still_bias_tol > 0.0 && self.pred.bias_samples >= 250 {
            let off = geom::norm(geom::sub(w.gyro_mean, self.pred.gyro_bias));
            if off > self.p.still_bias_tol {
                return false;
            }
        }
        true
    }

    /// One step of the gravity tilt correction at a tracker pose: the accelerometer (last 20 ms, when it reads
    /// about 1 g and the head turns slowly) says where up is in the IMU frame; the output attitude
    /// (tilt * world_from_slam * tracker q) puts that somewhere near world up. The difference, a rotation about a
    /// horizontal axis, is taken in with time constant `tilt_tau_s` and at most `tilt_rate`. Yaw and position
    /// are not touched.
    fn update_tilt(&mut self, v: &VioSample) {
        if self.p.tilt_tau_s <= 0.0 {
            return;
        }
        let dt = match self.tilt_ns {
            Some(t0) if v.ts_ns > t0 => ((v.ts_ns - t0) as f64 / 1e9).min(0.2),
            Some(_) => return,
            None => 0.0,
        };
        self.tilt_ns = Some(v.ts_ns);
        let Some(w) = self.pred.imu_window(20_000_000) else {
            return;
        };
        let an = geom::norm(w.accel_mean);
        if w.n < 5 || (an - GRAVITY).abs() > self.p.tilt_g_tol || w.gyro_max > self.p.tilt_gyro_max {
            self.tilt_skipped += 1;
            return;
        }
        let o = qmul(self.tilt, qmul(self.out.world_from_slam.1, qnorm(v.q)));
        let up_w = qrot(o, geom::scale(w.accel_mean, 1.0 / an));
        let e = geom::cross(up_w, [0.0, 1.0, 0.0]);
        let s = geom::norm(e);
        let ang = s.atan2(up_w[1]);
        if ang > self.p.tilt_max {
            self.tilt_skipped += 1;
            return;
        }
        if s < 1e-9 {
            return;
        }
        let step = (ang * (1.0 - (-dt / self.p.tilt_tau_s).exp())).min(self.p.tilt_rate * dt);
        let next = qnorm(qmul(qexp(geom::scale(e, step / s)), self.tilt));
        // Keep it a pure swing about a horizontal axis (drop any twist about world up: yaw is the tracker's).
        let twist = qnorm([0.0, next[1], 0.0, next[3]]);
        let mut t = qnorm(qmul(next, qconj(twist)));
        let mag = geom::norm(qlog(t));
        if mag > self.p.tilt_max {
            t = qexp(geom::scale(qlog(t), self.p.tilt_max / mag));
        }
        self.tilt = t;
        self.tilt_updates += 1;
    }

    /// The headset counted as lying still at the last update (desk-quiet; never a worn head).
    pub fn is_still(&self) -> bool {
        self.still
    }

    /// Current gravity tilt correction (world-frame rotation applied to the tracked output).
    pub fn tilt(&self) -> Quat {
        self.tilt
    }

    /// Camera image brightness (auto-exposure trimmed mean 0..255 of the darker pair; `None` = unknown): a
    /// dark image lowers the fusion weights like a noisy tracker velocity does.
    pub fn set_scene_mean(&mut self, mean: Option<f64>) {
        self.pred.scene_dark = match mean {
            Some(m) if m.is_finite() => {
                ((SCENE_DARK_FROM - m) / (SCENE_DARK_FROM - SCENE_DARK_FULL)).clamp(0.0, 1.0)
            }
            _ => 0.0,
        };
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
                    q: qnorm(qmul(self.tilt, qmul(c.w_q, st.q))),
                    lin_vel: qrot(c.w_q, v_head),
                    ang_vel: qrot(self.tilt, qrot(c.w_q, w_slam)),
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
        let mut vl = self.p.v_min + self.p.k_v * self.speed;
        let mut wl = self.p.w_min + self.p.k_w * self.ang_speed;
        let mut tau = self.p.tau_pos_s;
        let mut tau_r = self.p.tau_rot_s;
        if self.p.tau_rest_s > tau && self.p.rest_speed > 0.0 {
            let rest = (1.0 - self.speed / self.p.rest_speed).clamp(0.0, 1.0);
            tau += (self.p.tau_rest_s - tau) * rest;
        }
        if self.still {
            // Desk-quiet: nobody wears it (a worn head is never this quiet), so a correction (the hold going
            // back to where the quiet began, a re-zero takeover) is taken in at once instead of sliding on
            // into the moment it is picked up again.
            tau = tau.min(0.05);
            tau_r = tau_r.min(0.05);
            vl = vl.max(5.0);
            wl = wl.max(1.0);
        }
        self.corr_p = shrink(self.corr_p, tau, vl);
        self.corr_r = shrink(self.corr_r, tau_r, wl);
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
        assert!(!feed(&mut h, 0, 300, 0.01), "0.3 s of desk quiet is not enough yet");
        assert!(feed(&mut h, 300, 700, 0.01), "desk quiet enters");
        assert!(feed(&mut h, 1000, 600, 0.1), "0.1 m/s^2 vibration keeps it");
        assert!(!feed(&mut h, 1600, 600, 0.3), "real motion ends it");
        assert!(!feed(&mut h, 2200, 2500, 0.1), "0.1 m/s^2 does not enter");
    }

    /// Deterministic standard normal (Irwin-Hall from an LCG).
    fn gauss(seed: &mut u64) -> f64 {
        let mut s = 0.0;
        for _ in 0..12 {
            *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            s += (*seed >> 11) as f64 / (1u64 << 53) as f64;
        }
        s - 6.0
    }

    /// IMU at rest for `secs` from `t0`: bias + white noise (rad/s, m/s² sd per axis) + a breathing / pulse sway
    /// (rad/s amplitude), the way a worn head sits.
    fn rest_trace(t0: f64, secs: f64, noise_g: f64, noise_a: f64, sway: f64, seed: &mut u64) -> Vec<ImuSample> {
        let bias = [-0.0043, 0.0138, -0.0059]; // the Dream Air's ~0.85 deg/s
        let n = (secs * 1000.0) as i64;
        (0..n)
            .map(|i| {
                let t = t0 + i as f64 * 0.001;
                let sw = sway
                    * ((2.0 * std::f64::consts::PI * 0.25 * t).sin()
                        + 0.4 * (2.0 * std::f64::consts::PI * 1.2 * t).sin());
                ImuSample {
                    ts_ns: (t * 1e9).round() as i64,
                    gyro: [
                        bias[0] + sw + noise_g * gauss(seed),
                        bias[1] + 0.5 * sw + noise_g * gauss(seed),
                        bias[2] + noise_g * gauss(seed),
                    ],
                    accel: [
                        noise_a * gauss(seed),
                        GRAVITY + noise_a * gauss(seed),
                        noise_a * gauss(seed),
                    ],
                }
            })
            .collect()
    }

    fn still_after(p: HeadParams, trace: &[ImuSample], bias: Option<Vec3>) -> (bool, u64) {
        let mut h = HeadTracker::new(SupervisorParams::default(), p, [0.0; 3]);
        if let Some(b) = bias {
            h.pred.gyro_bias = b;
            h.pred.bias_samples = 1000;
        }
        let mut entered = 0;
        for (i, s) in trace.iter().enumerate() {
            h.push_imu(*s);
            if i % 11 == 0 {
                h.update(s.ts_ns, None);
                entered += h.still as u64;
            }
        }
        (h.still, entered)
    }

    #[test]
    fn worn_head_at_rest_never_counts_as_still_a_desk_does() {
        let mut seed = 7;
        let old = HeadParams {
            still_enter_window_ns: 500_000_000,
            still_gyro_std: 0.006,
            still_accel_std: 0.06,
            still_bias_tol: 0.0,
            ..HeadParams::default()
        };
        // Worn, sitting still: 0.3 deg/s gyro std (the quietest worn 0.5 s windows of the 18:18 session),
        // 0.02 m/s^2. (Once in, the exit limit 0.69 deg/s would keep it: the entry has to be right.)
        let worn = rest_trace(0.0, 8.0, 0.0053, 0.02, 0.0, &mut seed);
        let (_, n_old) = still_after(old.clone(), &worn, None);
        let (st, n_new) = still_after(HeadParams::default(), &worn, None);
        assert!(n_old > 0, "the old 0.34 deg/s gate took a worn head for still ({n_old} {n_new})");
        let _ = &old;
        assert!(!st && n_new == 0, "worn rest entered the still state {n_new} times");
        // On the desk: bare sensor noise (0.057 deg/s, 0.012 m/s^2) -> still after 0.5 s.
        let desk = rest_trace(0.0, 4.0, 0.001, 0.012, 0.0, &mut seed);
        let (st, _) = still_after(HeadParams::default(), &desk[..400], None);
        assert!(!st, "not before 0.5 s");
        let (st, _) = still_after(HeadParams::default(), &desk, None);
        assert!(st, "desk enters");
        // Flat but turning slowly (0.4 deg/s over the bias): not still once the bias is known.
        let mut turn = desk.clone();
        for s in turn.iter_mut() {
            s.gyro[1] += 0.007;
        }
        let bias = [-0.0043, 0.0138, -0.0059];
        assert!(!still_after(HeadParams::default(), &turn, Some(bias)).0, "slow steady turn");
        assert!(still_after(HeadParams::default(), &desk, Some(bias)).0, "desk with a known bias");
    }

    /// Head in one place, pitched down 0.2 rad; `moving`: slowly looking around in yaw (0.05 rad/s plus a
    /// 0.06 rad/s wobble, so it is not still), else lying still. The tracker attitude carries the tilt error and
    /// world offset `err(t, reset_seen)` gives. Per-query outputs and the tilt after each tracker pose.
    struct TiltRun {
        out: Vec<(f64, Rel)>,
        tilts: Vec<Quat>,
        h: HeadTracker,
    }

    fn tilt_true_q(t: f64, moving: bool) -> Quat {
        let yaw = if moving { 0.05 * t + 0.02 * (3.0 * t).sin() } else { 0.0 };
        qmul(qyaw(yaw), [(0.1f64).sin(), 0.0, 0.0, (0.1f64).cos()])
    }

    fn tilt_yaw_rate(t: f64, moving: bool) -> f64 {
        if moving {
            0.05 + 0.06 * (3.0 * t).cos()
        } else {
            0.0
        }
    }

    fn run_tilt(
        p: HeadParams,
        secs: f64,
        moving: bool,
        err: &mut dyn FnMut(f64, bool) -> (Quat, Vec3),
    ) -> TiltRun {
        let sp = SupervisorParams {
            acquire_poses: 3,
            ..Default::default()
        };
        let mut h = HeadTracker::new(sp, p, [0.0, -0.02, 0.07]);
        let mut seed = 11u64;
        let (mut out, mut tilts) = (Vec::new(), Vec::new());
        let mut reset = false;
        let mut i = 0i64;
        while (i as f64) * 0.001 < secs {
            let t = i as f64 * 0.001;
            let now = i * 1_000_000;
            let q = tilt_true_q(t, moving);
            let wr = [0.0, tilt_yaw_rate(t, moving), 0.0];
            h.push_imu(ImuSample {
                ts_ns: now,
                gyro: geom::add(qrot(qconj(q), wr), [0.0004 * gauss(&mut seed), 0.0, 0.0]),
                accel: geom::add(qrot(qconj(q), [0.0, GRAVITY, 0.0]), [0.01 * gauss(&mut seed), 0.0, 0.0]),
            });
            let mut vio = None;
            if i % 40 == 0 && i > 0 {
                let (e, off) = err(t, reset);
                vio = Some(VioSample {
                    ts_ns: now,
                    pos: off,
                    q: qmul(e, q),
                    vel: [0.0; 3],
                    q3dof: q,
                });
            }
            let o = h.update(now, vio);
            if o.events & events::RESET_WANTED != 0 {
                reset = true;
            }
            if vio.is_some() {
                tilts.push(h.tilt());
            }
            if i % 11 == 0 {
                let mut r = rel3(q);
                r.ang_vel = wr;
                out.push((t, h.head(now, now, &r)));
            }
            i += 1;
        }
        TiltRun { out, tilts, h }
    }

    /// Angle (deg) between the output's up and the true up.
    fn up_err_deg(t: f64, r: &Rel, moving: bool) -> f64 {
        let up_true = qrot(qconj(tilt_true_q(t, moving)), [0.0, 1.0, 0.0]);
        let up_out = qrot(qconj(r.q), [0.0, 1.0, 0.0]);
        geom::dot(up_true, up_out).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn tilted_tracker_attitude_converges_to_gravity_yaw_untouched_rate_limited() {
        // 1.5 deg pitch + 0.8 deg roll of tracker attitude error (what Basalt showed in dark runs).
        let err = qmul(
            qexp([1.5f64.to_radians(), 0.0, 0.0]),
            qexp([0.0, 0.0, 0.8f64.to_radians()]),
        );
        let mut e1 = |_: f64, _: bool| (err, [0.3, 1.6, -0.2]);
        let on = run_tilt(HeadParams::default(), 12.0, true, &mut e1);
        let mut e2 = |_: f64, _: bool| (err, [0.3, 1.6, -0.2]);
        let off = run_tilt(
            HeadParams {
                tilt_tau_s: 0.0,
                ..HeadParams::default()
            },
            12.0,
            true,
            &mut e2,
        );
        let tracked = |r: &Rel| r.flags & flags::POSITION_TRACKED != 0;
        let t_first = on.out.iter().find(|(_, r)| tracked(r)).unwrap().0;
        let (t_last, last) = *on.out.last().unwrap();
        // The acquisition blends from the 3DoF attitude: compare at 1 s, when that has decayed.
        let (t1, r1) = *off.out.iter().find(|(t, r)| *t > t_first + 1.0 && tracked(r)).unwrap();
        let e_first = up_err_deg(t1, &r1, true);
        let e_last = up_err_deg(t_last, &last, true);
        let e_off = up_err_deg(t_last, &off.out.last().unwrap().1, true);
        eprintln!(
            "tilt error: start {e_first:.3} deg, after 12 s {e_last:.3} deg (no correction {e_off:.3} deg), {} updates, {} skipped",
            on.h.tilt_updates, on.h.tilt_skipped
        );
        assert_eq!(on.h.sup.resets, 0);
        assert!(e_first > 1.4 && e_off > 1.4, "{e_first} {e_off}");
        assert!(e_last < 0.1, "converged to {e_last}");
        // Yaw and position are the tracker's.
        for ((t, a), (_, b)) in on.out.iter().zip(off.out.iter()) {
            if !tracked(a) {
                continue;
            }
            // The world-frame difference is a rotation about a horizontal axis: no turn about world up.
            let d = qlog(qmul(a.q, qconj(b.q)));
            assert!(d[1].abs().to_degrees() < 0.01, "yaw moved {} deg at {t}", d[1].to_degrees());
            assert!(geom::norm(geom::sub(a.pos, b.pos)) < 1e-9, "position moved at {t}");
        }
        // Never faster than 0.5 deg/s: <= 0.02 deg per 40 ms pose.
        let max_step = on
            .tilts
            .windows(2)
            .map(|w| qangle_deg(qmul(w[1], qconj(w[0]))))
            .fold(0.0, f64::max);
        assert!(max_step <= 0.0201, "tilt step {max_step} deg per pose");
        assert!(on.h.tilt_updates > 200);
        // Output frame to frame: no jump beyond the true motion.
        for w in on.out.windows(2) {
            let ((t0, a), (t1, b)) = (&w[0], &w[1]);
            if *t0 > t_first + 0.5 && tracked(a) {
                let truth = qangle_deg(qmul(qconj(tilt_true_q(*t0, true)), tilt_true_q(*t1, true)));
                let extra = qangle_deg(qmul(qconj(a.q), b.q)) - truth;
                assert!(extra < 0.03, "jump {extra} deg at {t0}");
            }
        }
    }

    #[test]
    fn tilt_correction_is_held_through_a_still_rezero() {
        // Tracker tilted 1.2 deg and drifting (no landmarks) while the headset lies still: it is re-zeroed; the
        // fresh tracker world starts gravity aligned. The output attitude must not swing at either end.
        let err = qexp([1.2f64.to_radians(), 0.0, 0.0]);
        let mut e = |t: f64, reset: bool| {
            if reset {
                (QUAT_ID, [5.0, 0.0, 5.0])
            } else {
                let d = if t > 8.0 { 0.5 * 0.07 * (t - 8.0) * (t - 8.0) } else { 0.0 };
                (err, [d, 0.0, 0.0])
            }
        };
        let r = run_tilt(HeadParams::default(), 14.0, false, &mut e);
        assert_eq!(r.h.sup.still_resets, 1, "one re-zero");
        assert_eq!(r.h.sup.losses, 0);
        let (t_last, last) = *r.out.last().unwrap();
        let e_last = up_err_deg(t_last, &last, false);
        let mut max_jump: f64 = 0.0;
        for w in r.out.windows(2) {
            let ((t0, a), (t1, b)) = (&w[0], &w[1]);
            if *t0 < 2.0 {
                continue;
            }
            let truth = qangle_deg(qmul(qconj(tilt_true_q(*t0, false)), tilt_true_q(*t1, false)));
            max_jump = max_jump.max(qangle_deg(qmul(qconj(a.q), b.q)) - truth);
        }
        eprintln!("re-zero with tilt: max extra rotation per frame {max_jump:.4} deg, end tilt error {e_last:.3} deg");
        assert!(max_jump < 0.05, "attitude jump {max_jump} deg");
        assert!(e_last < 0.15, "end tilt error {e_last}");
    }

    #[test]
    fn noisy_tracker_velocity_lowers_the_fusion_weights() {
        let p = HeadParams::default();
        let mk = || {
            Predictor::new(PredictorParams {
                fuse_alpha: p.fuse[0],
                fuse_alpha_y: p.fuse[1],
                fuse_beta: p.fuse[2],
                fuse_vel_w: p.fuse[3],
                fuse_fast_speed: p.fuse[4],
                fuse_auto: true,
                ..PredictorParams::default()
            })
        };
        let run = |pr: &mut Predictor, vel_noise: f64| {
            let mut seed = 3u64;
            let mut next_vio = 0.1;
            let mut t = 0.0;
            while t < 10.0 {
                pr.push_imu(imu_at(t, [0.0; 3]));
                if t >= next_vio {
                    let (p, q, v, _, _) = truth(t);
                    let n = [gauss(&mut seed), gauss(&mut seed), gauss(&mut seed)];
                    pr.push_vio(
                        BodyState {
                            ts_ns: (t * 1e9).round() as i64,
                            p,
                            q,
                            v: geom::add(v, geom::scale(n, vel_noise)),
                        },
                        false,
                    );
                    next_vio += 0.04;
                }
                t += 0.001;
            }
        };
        let mut clean = mk();
        run(&mut clean, 0.01);
        let mut noisy = mk();
        run(&mut noisy, 0.25);
        let (wc, wn) = (clean.fuse_weights(0.0), noisy.fuse_weights(0.0));
        eprintln!(
            "velocity residual rms clean {:.3} noisy {:.3} m/s -> weights {wc:?} / {wn:?}",
            clean.vel_noise_ms.sqrt(),
            noisy.vel_noise_ms.sqrt()
        );
        assert!(clean.noise_level() < 0.1, "clean level {}", clean.noise_level());
        assert!((wc[3] - 0.5).abs() < 0.05 && (wc[0] - 0.15).abs() < 0.01);
        assert!(noisy.noise_level() > 0.9, "noisy level {}", noisy.noise_level());
        assert!((wn[3] - 0.1).abs() < 0.02 && wn[0] < 0.08 && wn[4] == 1.0);
        // In fast motion the position pull stays (no lag), only the velocity weight is lowered.
        assert!((noisy.fuse_weights(1.0)[0] - 0.15).abs() < 1e-9);
        // A dark image lowers them as well, even with a clean velocity.
        let mut h = HeadTracker::new(SupervisorParams::default(), HeadParams::default(), [0.0; 3]);
        h.pred = clean.clone();
        h.set_scene_mean(Some(40.0));
        assert!(h.pred.fuse_weights(0.0)[3] > 0.45);
        h.set_scene_mean(Some(12.0));
        assert!((h.pred.fuse_weights(0.0)[3] - 0.1).abs() < 0.01);
        h.set_scene_mean(None);
        assert!(h.pred.fuse_weights(0.0)[3] > 0.45);
        // Auto off = the configured weights whatever the noise.
        noisy.p.fuse_auto = false;
        assert_eq!(noisy.fuse_weights(0.0)[3], 0.5);
    }

    #[test]
    fn noisy_tracker_velocity_jitters_less_with_auto_weights() {
        // The test walk (0.3 m/s) counted as slow motion (fast speed 2 m/s): where auto lowers the weights.
        let head = [0.0, -0.02, 0.07];
        let mut res = Vec::new();
        for auto in [false, true] {
            let mut sim = Sim::new(HeadParams {
                fuse_auto: auto,
                fuse: [0.15, 0.075, 0.06, 0.5, 2.0],
                ..Default::default()
            });
            let mut seed = 5u64;
            let mut vio = |t: f64| {
                let mut s = slam_of(t, 0.4, [0.5, 0.0, -1.0]);
                for k in 0..3 {
                    s.2[k] += 0.3 * gauss(&mut seed);
                }
                Some(s)
            };
            let mut out = Vec::new();
            sim.run(0.0, 10.0, 0.002, &mut vio, &mut out);
            res.push(errors(&out, 2.0, head));
        }
        eprintln!("vel noise 0.3 m/s: fixed (max err, jitter) {:?}, auto {:?}", res[0], res[1]);
        assert!(res[1].2 < res[0].2, "jitter fixed {} auto {}", res[0].2, res[1].2);
    }

    #[test]
    fn describe_names_the_build_markers() {
        let d = HeadParams::default().describe();
        assert!(d.contains("PIMAX_HEAD_TILT") && d.contains("PIMAX_FUSE_AUTO"), "{d}");
        assert!(d.contains("on (tau 1.5 s") && d.contains("0.20 deg/s, 0.5 s"), "{d}");
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

    /// Old output behaviour (before the gravity tilt correction, the automatic fusion weights and the long
    /// still entry) for A/B replays.
    fn params_before() -> HeadParams {
        HeadParams {
            tilt_tau_s: 0.0,
            fuse_auto: false,
            still_enter_window_ns: 500_000_000,
            still_gyro_std: 0.006,
            still_accel_std: 0.06,
            still_bias_tol: 0.0,
            ..HeadParams::default()
        }
    }

    /// Replay a session's `head-imu.csv` (`PIMAX_HEAD_IMU_LOG` format) through the output, before and after:
    /// `PIMAX_REPLAY_HEAD_IMU=<file> cargo test --lib -p pimax-cam replay_head_imu_log -- --ignored --nocapture`.
    /// The log has no tracker velocity: a central difference of the tracker positions stands in for it, and a
    /// re-zero cannot restart the tracker, so the same world carries on. CPU only.
    #[test]
    #[ignore]
    fn replay_head_imu_log() {
        let Ok(path) = std::env::var("PIMAX_REPLAY_HEAD_IMU") else {
            eprintln!("PIMAX_REPLAY_HEAD_IMU not set");
            return;
        };
        let text = std::fs::read_to_string(&path).expect("read log");
        enum L {
            I(ImuSample),
            V(i64, Vec3, Quat),
        }
        let mut lines = Vec::new();
        for l in text.lines() {
            let f: Vec<&str> = l.split(',').collect();
            let num = |i: usize| f.get(i).and_then(|x| x.trim().parse::<f64>().ok());
            match f.first() {
                Some(&"I") if f.len() == 8 => {
                    let (Some(ts), Some(ax), Some(ay), Some(az), Some(gx), Some(gy), Some(gz)) =
                        (f[1].parse::<i64>().ok(), num(2), num(3), num(4), num(5), num(6), num(7))
                    else {
                        continue;
                    };
                    lines.push(L::I(ImuSample {
                        ts_ns: ts,
                        accel: [ax, ay, az],
                        gyro: [gx, gy, gz],
                    }));
                }
                Some(&"V") if f.len() == 9 => {
                    let v: Vec<f64> = (2..9).filter_map(num).collect();
                    if let (Ok(ts), 7) = (f[1].parse::<i64>(), v.len()) {
                        lines.push(L::V(ts, [v[0], v[1], v[2]], [v[3], v[4], v[5], v[6]]));
                    }
                }
                _ => {}
            }
        }
        let vios: Vec<(i64, Vec3)> = lines
            .iter()
            .filter_map(|l| match l {
                L::V(t, p, _) => Some((*t, *p)),
                _ => None,
            })
            .collect();
        let vel_at = |k: usize| -> Vec3 {
            if k == 0 || k + 1 >= vios.len() {
                return [0.0; 3];
            }
            let dt = (vios[k + 1].0 - vios[k - 1].0) as f64 / 1e9;
            if !(0.01..0.15).contains(&dt) {
                return [0.0; 3];
            }
            geom::scale(geom::sub(vios[k + 1].1, vios[k - 1].1), 1.0 / dt)
        };
        let pct = |v: &mut Vec<f64>, q: f64| -> f64 {
            if v.is_empty() {
                return f64::NAN;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[((v.len() - 1) as f64 * q).round() as usize]
        };
        let head_in_imu = [0.049, -0.015, 0.030];
        let variants = [
            ("before", params_before()),
            ("after", HeadParams::default()),
            (
                "after, fixed weights",
                HeadParams {
                    fuse_auto: false,
                    ..HeadParams::default()
                },
            ),
            (
                "after, no tilt",
                HeadParams {
                    tilt_tau_s: 0.0,
                    ..HeadParams::default()
                },
            ),
            (
                "after, still entry over 2 s",
                HeadParams {
                    still_enter_window_ns: 2_000_000_000,
                    ..HeadParams::default()
                },
            ),
            (
                "before + new still gate only",
                HeadParams {
                    tilt_tau_s: 0.0,
                    fuse_auto: false,
                    ..HeadParams::default()
                },
            ),
        ];
        for (name, p) in variants {
            let mut h = HeadTracker::new(SupervisorParams::default(), p, head_in_imu);
            let (mut now, mut k, mut n_imu) = (0i64, 0usize, 0u64);
            // The 3DoF attitude: the gyro integrated from the first tracker attitude (the supervisor checks the
            // tracker's rotation against it, as live with Monado's 3DoF filter).
            let mut q3: Option<Quat> = None;
            let mut q3_hist: VecDeque<(i64, Quat)> = VecDeque::new();
            let mut last_imu: Option<i64> = None;
            let (mut tilt_err, mut dev_fast, mut dev_all) = (Vec::new(), Vec::new(), Vec::new());
            let (mut still_on, mut still_entries, mut still_ticks, mut ticks) = (false, 0u64, 0u64, 0u64);
            let mut w_max = Vec::new();
            for l in &lines {
                match l {
                    L::I(s) => {
                        h.push_imu(*s);
                        if let (Some(q), Some(t0)) = (q3, last_imu) {
                            let dt = ((s.ts_ns - t0) as f64 / 1e9).clamp(0.0, 0.01);
                            let w = geom::sub(s.gyro, h.pred.gyro_bias);
                            let qn = qnorm(qmul(q, qexp(geom::scale(w, dt))));
                            q3 = Some(qn);
                            q3_hist.push_back((s.ts_ns, qn));
                            if q3_hist.len() > 500 {
                                q3_hist.pop_front();
                            }
                        }
                        last_imu = Some(s.ts_ns);
                        now = now.max(s.ts_ns);
                        n_imu += 1;
                        if n_imu % 11 == 0 {
                            h.update(now, None);
                            ticks += 1;
                            if h.still {
                                still_ticks += 1;
                                if !still_on {
                                    still_entries += 1;
                                }
                            }
                            still_on = h.still;
                            let r = h.head(now, now, &rel3(q3.unwrap_or(QUAT_ID)));
                            if h.output().state == TrackState::Tracking {
                                if let Some(w) = h.pred.imu_window(200_000_000) {
                                    let an = geom::norm(w.accel_mean);
                                    if w.gyro_max < 0.05 && (an - GRAVITY).abs() < 0.3 {
                                        let up = qrot(qconj(r.q), [0.0, 1.0, 0.0]);
                                        let c = geom::dot(up, geom::scale(w.accel_mean, 1.0 / an));
                                        tilt_err.push(c.clamp(-1.0, 1.0).acos().to_degrees());
                                    }
                                }
                            }
                        }
                    }
                    L::V(ts, pos, q) => {
                        if q3.is_none() {
                            q3 = Some(qnorm(*q));
                        }
                        let q3_at = q3_hist
                            .iter()
                            .rev()
                            .find(|(t, _)| *t <= *ts)
                            .map(|(_, q)| *q)
                            .or(q3)
                            .unwrap();
                        let v = VioSample {
                            ts_ns: *ts,
                            pos: *pos,
                            q: *q,
                            vel: vel_at(k),
                            q3dof: q3_at,
                        };
                        k += 1;
                        h.update(now, Some(v));
                        if h.output().state == TrackState::Tracking {
                            let r = h.head(now, *ts, &rel3(q3_at));
                            let d = geom::norm(geom::sub(r.pos, h.head_world(*pos, *q)));
                            dev_all.push(d * 1000.0);
                            if geom::norm(v.vel) > 0.5 {
                                dev_fast.push(d * 1000.0);
                            }
                            w_max.push(h.pred.fuse_weights(0.0)[3]);
                        }
                    }
                }
            }
            let t0 = lines
                .iter()
                .find_map(|l| match l {
                    L::I(s) => Some(s.ts_ns),
                    _ => None,
                })
                .unwrap_or(0);
            while let Some(e) = h.take_big() {
                eprintln!(
                    "  {name}: correction step {:.0} mm at {:.2} s (speed {:.2} m/s, {})",
                    geom::norm(e.step) * 1000.0,
                    (e.ts_ns - t0) as f64 / 1e9,
                    e.speed,
                    if e.pose_only { "tracker pose" } else { "supervisor" }
                );
            }
            let c = h.corr;
            eprintln!(
                "{name}: {} IMU, {} tracker poses; tilt err vs gravity (still, deg) p50 {:.2} p90 {:.2} max {:.2} (n {}); \
                 tilt updates {} skipped {}; still entries {still_entries}, still {:.1} % of the time, still \
                 re-zeros {}; corrections blended {} snapped {} >=20 mm {} (vertical {}), max {:.0} mm / {:.2} deg; \
                 |output - tracker| all p50 {:.1} p90 {:.1} mm, at > 0.5 m/s p50 {:.1} p90 {:.1} max {:.1} mm (n {}); \
                 velocity weight p10 {:.2} p50 {:.2} p90 {:.2}; velocity residual rms {:.3} m/s",
                n_imu,
                vios.len(),
                pct(&mut tilt_err.clone(), 0.5),
                pct(&mut tilt_err.clone(), 0.9),
                pct(&mut tilt_err.clone(), 1.0),
                tilt_err.len(),
                h.tilt_updates,
                h.tilt_skipped,
                100.0 * still_ticks as f64 / ticks.max(1) as f64,
                h.sup.still_resets,
                c.blended,
                c.snapped,
                c.big,
                c.big_vertical,
                c.max_pos * 1000.0,
                c.max_rot.to_degrees(),
                pct(&mut dev_all.clone(), 0.5),
                pct(&mut dev_all.clone(), 0.9),
                pct(&mut dev_fast.clone(), 0.5),
                pct(&mut dev_fast.clone(), 0.9),
                pct(&mut dev_fast.clone(), 1.0),
                dev_fast.len(),
                pct(&mut w_max.clone(), 0.1),
                pct(&mut w_max.clone(), 0.5),
                pct(&mut w_max.clone(), 0.9),
                h.pred.vel_noise_ms.sqrt(),
            );
        }
    }
}
