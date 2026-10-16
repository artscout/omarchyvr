//! Controller pose fusion for optical (LED) tracking: what to show when the cameras lose the controller, and the
//! controller gyro calibration learned from the optical poses.
//!
//! Inputs, all on the host `CLOCK_MONOTONIC` (ns):
//! * every controller report (raw 64 bytes, EP 0x82): one or two gyro samples (big-endian i16, unit unknown), stamped
//!   with the device microsecond clock;
//! * every optical fix from `pimax-ctrltrack live`: `world_from_aim` at the LED frame time (the driver already
//!   composed it with the head pose) plus the head pose at that time.
//!
//! Output for a time `t` and the head pose at `t` ([`Fuser::get`]):
//! * fresh optical (age <= `extrap_s`): last fix, position extrapolated with the last velocity (decaying,
//!   `p + v*tau*(1-exp(-age/tau))`), orientation rotated by the integrated gyro when calibrated;
//! * out of view (age <= `lost_s`): an arm model: the elbow (controller minus the forearm vector `forearm`, aim
//!   frame) keeps its offset in a body frame that follows the head position and half of the head yaw, and the
//!   controller swings around it with the gyro orientation (world); without a gyro calibration the controller itself
//!   is held, orientation head-yaw relative;
//! * older: `None` (the driver shows its rest pose).
//!
//! On reacquire after a gap the output blends from the prediction to the optical pose over `blend_s` (longer for a big
//! correction: at most 0.5 m/s and 120 deg/s, up to 0.5 s); optical fixes
//! that jump too far from the gyro prediction are dropped (three in a row are accepted, so a bad prediction cannot
//! lock tracking out).
//!
//! Gyro calibration: for pairs of optical fixes about 120 ms apart on one continuous track, the body-frame angular
//! velocity from the optical rotation, `y = log(R_a^T R_b) / dt` (aim axes, rad/s), is regressed on the mean raw gyro
//! over the same interval: `y = C x + d` (least squares, 12 unknowns), for several camera-to-IMU time offsets; the
//! best offset wins. `C` = scale * axis mapping (rad/s per LSB, raw axes -> aim axes), `bias = -C^-1 d` (LSB). The fit
//! runs online and is saved to `~/.config/omarchyvr/ctrl-imu.json` per hand; a saved fit is loaded at start, else the
//! built-in one ([`default_calib`]), so the gyro is used from the first second (`PIMAX_CTRL_TRACK_IMU=0` turns it
//! off). Masked-replay numbers: `pimax-ctrltrack occlusion`, analysis/linux-recordings-20261016.md.

#![allow(
    clippy::needless_range_loop,
    clippy::unnecessary_map_or,
    clippy::manual_is_multiple_of
)]

use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};

pub type V3 = [f64; 3];
/// Quaternion x, y, z, w.
pub type Q = [f64; 4];
pub type M3 = [[f64; 3]; 3];

// ---------- small math ----------

pub fn qmul(a: Q, b: Q) -> Q {
    [
        a[3] * b[0] + a[0] * b[3] + a[1] * b[2] - a[2] * b[1],
        a[3] * b[1] - a[0] * b[2] + a[1] * b[3] + a[2] * b[0],
        a[3] * b[2] + a[0] * b[1] - a[1] * b[0] + a[2] * b[3],
        a[3] * b[3] - a[0] * b[0] - a[1] * b[1] - a[2] * b[2],
    ]
}
pub fn qconj(a: Q) -> Q {
    [-a[0], -a[1], -a[2], a[3]]
}
pub fn qnorm(a: Q) -> Q {
    let n = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2] + a[3] * a[3]).sqrt();
    if n < 1e-12 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    [a[0] / n, a[1] / n, a[2] / n, a[3] / n]
}
pub fn qrot(q: Q, v: V3) -> V3 {
    let p = qmul(qmul(q, [v[0], v[1], v[2], 0.0]), qconj(q));
    [p[0], p[1], p[2]]
}
pub fn qexp(w: V3) -> Q {
    let a = (w[0] * w[0] + w[1] * w[1] + w[2] * w[2]).sqrt();
    if a < 1e-12 {
        return qnorm([w[0] / 2.0, w[1] / 2.0, w[2] / 2.0, 1.0]);
    }
    let s = (a / 2.0).sin() / a;
    [w[0] * s, w[1] * s, w[2] * s, (a / 2.0).cos()]
}
/// Rotation vector of `q` (shortest way).
pub fn qlog(q: Q) -> V3 {
    let q = if q[3] < 0.0 {
        [-q[0], -q[1], -q[2], -q[3]]
    } else {
        q
    };
    let s = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt();
    if s < 1e-12 {
        return [2.0 * q[0], 2.0 * q[1], 2.0 * q[2]];
    }
    let a = 2.0 * s.atan2(q[3]);
    [q[0] / s * a, q[1] / s * a, q[2] / s * a]
}
pub fn qangle(a: Q, b: Q) -> f64 {
    let v = qlog(qmul(qconj(a), b));
    (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
}
fn add(a: V3, b: V3) -> V3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}
fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}
fn scale(a: V3, s: f64) -> V3 {
    [a[0] * s, a[1] * s, a[2] * s]
}
fn norm(a: V3) -> f64 {
    (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt()
}
fn mv(m: &M3, v: V3) -> V3 {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}
fn det3(m: &M3) -> f64 {
    m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0])
}
fn inv3(m: &M3) -> Option<M3> {
    let d = det3(m);
    if d.abs() < 1e-300 {
        return None;
    }
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    Some([
        [c(1, 1, 2, 2) / d, -c(0, 1, 2, 2) / d, c(0, 1, 1, 2) / d],
        [-c(1, 0, 2, 2) / d, c(0, 0, 2, 2) / d, -c(0, 0, 1, 2) / d],
        [c(1, 0, 2, 1) / d, -c(0, 0, 2, 1) / d, c(0, 0, 1, 1) / d],
    ])
}
/// Nearest orthogonal matrix (polar decomposition, Newton iteration).
fn polar(m: &M3) -> M3 {
    let mut x = *m;
    for _ in 0..30 {
        let Some(inv) = inv3(&x) else { break };
        let mut n = [[0.0; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                n[i][j] = 0.5 * (x[i][j] + inv[j][i]);
            }
        }
        x = n;
    }
    x
}
/// Yaw-only part of a head orientation (about +Y, OpenXR axes).
pub fn yaw_only(q: Q) -> Q {
    let f = qrot(q, [0.0, 0.0, -1.0]);
    if f[0].abs() + f[2].abs() < 1e-6 {
        return [0.0, 0.0, 0.0, 1.0];
    }
    let yaw = (-f[0]).atan2(-f[2]);
    [0.0, (yaw / 2.0).sin(), 0.0, (yaw / 2.0).cos()]
}

/// Rigid pose (orientation x y z w, position).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct P {
    pub q: Q,
    pub p: V3,
}
impl P {
    pub const ID: P = P {
        q: [0.0, 0.0, 0.0, 1.0],
        p: [0.0; 3],
    };
    pub fn mul(&self, o: &P) -> P {
        P {
            q: qnorm(qmul(self.q, o.q)),
            p: add(qrot(self.q, o.p), self.p),
        }
    }
    pub fn inv(&self) -> P {
        let qi = qconj(self.q);
        P {
            q: qi,
            p: scale(qrot(qi, self.p), -1.0),
        }
    }
}

// ---------- report decoding ----------

/// Gyro samples of one raw controller report: (device µs, raw gyro). The second sample (bytes 30..41) is all zero
/// when the report holds only one; it is taken as 2 ms after the first.
/// Accelerometer of the same samples (bytes 16..21 and 30..35, big-endian, about 1024 LSB/g).
pub fn report_accel(raw: &[u8; 64]) -> [[i16; 3]; 2] {
    let be = |o: usize| i16::from_be_bytes([raw[o], raw[o + 1]]);
    [[be(16), be(18), be(20)], [be(30), be(32), be(34)]]
}

pub fn report_gyro(raw: &[u8; 64]) -> ([u32; 2], [[i16; 3]; 2], usize) {
    let be = |o: usize| i16::from_be_bytes([raw[o], raw[o + 1]]);
    let ts = u32::from_le_bytes([raw[8], raw[9], raw[10], raw[11]]);
    let g0 = [be(22), be(24), be(26)];
    let g1 = [be(36), be(38), be(40)];
    let second = raw[30..42].iter().any(|&b| b != 0);
    (
        [ts, ts.wrapping_add(2000)],
        [g0, g1],
        if second { 2 } else { 1 },
    )
}

// ---------- calibration ----------

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Calib {
    /// raw gyro LSB -> aim-frame rad/s.
    pub c: M3,
    /// Gyro bias in LSB.
    pub bias: V3,
    /// IMU time = optical time + offset (ms) for the best fit.
    pub time_offset_ms: f64,
    pub pairs: u32,
    pub moving: u32,
    pub r2: f64,
}

impl Calib {
    pub fn scale_dps_per_lsb(&self) -> f64 {
        det3(&self.c).abs().cbrt().to_degrees()
    }
    /// Nearest signed axis permutation, e.g. "+x -z +y" = aim x from raw +x, aim y from raw -z, ...
    pub fn axes(&self) -> String {
        let a = polar(&self.c);
        let mut out = Vec::new();
        for row in a.iter() {
            let (j, v) = (0..3)
                .map(|j| (j, row[j]))
                .max_by(|x, y| x.1.abs().partial_cmp(&y.1.abs()).unwrap())
                .unwrap();
            out.push(format!(
                "{}{}",
                if v >= 0.0 { "+" } else { "-" },
                ["x", "y", "z"][j]
            ));
        }
        out.join(" ")
    }
    /// Rotation error of the axis mapping vs the nearest signed permutation (deg).
    pub fn axis_misalignment_deg(&self) -> f64 {
        let a = polar(&self.c);
        let mut worst: f64 = 0.0;
        for row in a.iter() {
            let m = row.iter().fold(0.0f64, |m, v| m.max(v.abs()));
            worst = worst.max(m.clamp(-1.0, 1.0).acos().to_degrees());
        }
        worst
    }
    pub fn omega(&self, raw: V3) -> V3 {
        mv(&self.c, sub(raw, self.bias))
    }
    /// The rotation part of `c` (raw IMU axes -> aim axes), also used for the accelerometer (same chip).
    pub fn axis_rotation(&self) -> M3 {
        polar(&self.c)
    }
    /// Accelerometer sample (raw LSB, 1024 per g) as specific force in the aim frame (m/s²).
    pub fn accel(&self, raw: V3) -> V3 {
        scale(mv(&self.axis_rotation(), raw), ACCEL_G / ACCEL_LSB_PER_G)
    }
}

/// Accelerometer scale (verified at rest, analysis/controller-6dof.md) and standard gravity.
pub const ACCEL_LSB_PER_G: f64 = 1024.0;
pub const ACCEL_G: f64 = 9.80665;

/// Time offsets tried by the fit (IMU interval = optical interval + offset).
pub const FIT_OFFSETS_MS: [f64; 9] = [-64.0, -48.0, -32.0, -16.0, 0.0, 16.0, 32.0, 48.0, 64.0];

/// Least-squares accumulator for `y = C x + d`.
#[derive(Clone, Debug, Default)]
pub struct Acc {
    a: [[f64; 4]; 4],
    b: [[f64; 3]; 4],
    yy: f64,
    ysum: V3,
    n: u32,
    moving: u32,
}

impl Acc {
    pub fn add(&mut self, x: V3, y: V3) {
        let xe = [x[0], x[1], x[2], 1.0];
        for i in 0..4 {
            for j in 0..4 {
                self.a[i][j] += xe[i] * xe[j];
            }
            for j in 0..3 {
                self.b[i][j] += xe[i] * y[j];
            }
        }
        self.yy += y[0] * y[0] + y[1] * y[1] + y[2] * y[2];
        self.ysum = add(self.ysum, y);
        self.n += 1;
        if norm(y) > 0.6 {
            self.moving += 1;
        }
    }

    /// (C, d, r2) or None when singular.
    pub fn solve(&self) -> Option<(M3, V3, f64)> {
        if self.n < 12 {
            return None;
        }
        // Gauss-Jordan on A W = B (4x4, 3 right-hand sides)
        let mut a = self.a;
        let mut b = self.b;
        for c in 0..4 {
            let p = (c..4).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
            if a[p][c].abs() < 1e-9 {
                return None;
            }
            a.swap(c, p);
            b.swap(c, p);
            for r in 0..4 {
                if r != c {
                    let f = a[r][c] / a[c][c];
                    for k in 0..4 {
                        a[r][k] -= f * a[c][k];
                    }
                    for k in 0..3 {
                        b[r][k] -= f * b[c][k];
                    }
                }
            }
        }
        let mut w = [[0.0; 3]; 4];
        for r in 0..4 {
            for k in 0..3 {
                w[r][k] = b[r][k] / a[r][r];
            }
        }
        // residual: yy - 2 tr(W^T B) + tr(W^T A W), with the original A and B
        let mut wtb = 0.0;
        let mut wtaw = 0.0;
        for k in 0..3 {
            for i in 0..4 {
                wtb += w[i][k] * self.b[i][k];
                for j in 0..4 {
                    wtaw += w[i][k] * self.a[i][j] * w[j][k];
                }
            }
        }
        let ssres = (self.yy - 2.0 * wtb + wtaw).max(0.0);
        let n = self.n as f64;
        let sstot =
            self.yy - (self.ysum[0].powi(2) + self.ysum[1].powi(2) + self.ysum[2].powi(2)) / n;
        let r2 = if sstot > 1e-12 {
            1.0 - ssres / sstot
        } else {
            0.0
        };
        let c = [
            [w[0][0], w[1][0], w[2][0]],
            [w[0][1], w[1][1], w[2][1]],
            [w[0][2], w[1][2], w[2][2]],
        ];
        Some((c, [w[3][0], w[3][1], w[3][2]], r2))
    }
}

/// The online fit: one accumulator per time offset.
#[derive(Clone, Debug, Default)]
pub struct Fit {
    /// (mean raw gyro per time offset, optical body angular velocity), newest last.
    pub pairs: VecDeque<([V3; FIT_OFFSETS_MS.len()], V3)>,
}

/// Pairs kept for the fit (about 10 min of tracking at 4-5 pairs/s).
pub const FIT_MAX_PAIRS: usize = 3000;

impl Fit {
    pub fn new() -> Fit {
        Fit {
            pairs: VecDeque::new(),
        }
    }
    pub fn pairs(&self) -> u32 {
        self.pairs.len() as u32
    }
    pub fn add(&mut self, xs: [V3; FIT_OFFSETS_MS.len()], y: V3) {
        self.pairs.push_back((xs, y));
        while self.pairs.len() > FIT_MAX_PAIRS {
            self.pairs.pop_front();
        }
    }

    /// Robust fit for one time offset: least squares, then four rounds that drop the pairs whose residual is above
    /// the 85th percentile (at least 0.3 rad/s). Optical orientations from few LEDs are sometimes far off (flips,
    /// spins of 20+ rad/s); plain least squares on the live data gave R2 0.3, this gives 0.99. Returns
    /// (C, d, R2 of the kept pairs, kept, moving kept).
    pub fn robust(&self, k: usize) -> Option<(M3, V3, f64, u32, u32)> {
        let n = self.pairs.len();
        let mut keep = vec![true; n];
        let solve = |keep: &[bool]| {
            let mut a = Acc::default();
            for (i, (xs, y)) in self.pairs.iter().enumerate() {
                if keep[i] {
                    a.add(xs[k], *y);
                }
            }
            a.solve().map(|(c, d, r2)| (c, d, r2, a.n, a.moving))
        };
        let mut cur = solve(&keep)?;
        for _ in 0..4 {
            let res: Vec<f64> = self
                .pairs
                .iter()
                .map(|(xs, y)| norm(sub(*y, add(mv(&cur.0, xs[k]), cur.1))))
                .collect();
            let mut kept: Vec<f64> = res
                .iter()
                .zip(&keep)
                .filter(|(_, k)| **k)
                .map(|(r, _)| *r)
                .collect();
            if kept.len() < 24 {
                break;
            }
            kept.sort_by(|a, b| a.partial_cmp(b).unwrap());
            let thr = kept[(kept.len() as f64 * 0.85) as usize].max(0.3);
            for i in 0..n {
                keep[i] = keep[i] && res[i] <= thr;
            }
            cur = solve(&keep)?;
        }
        Some(cur)
    }

    /// Best calibration over the time offsets, if the data supports one.
    pub fn best(&self, min_pairs: u32, min_moving: u32, min_r2: f64) -> Option<Calib> {
        let mut best: Option<Calib> = None;
        for k in 0..FIT_OFFSETS_MS.len() {
            let Some((c, d, r2, kept, moving)) = self.robust(k) else {
                continue;
            };
            if kept < min_pairs || moving < min_moving || r2 < min_r2 || det3(&c).abs() < 1e-18 {
                continue;
            }
            let Some(ci) = inv3(&c) else { continue };
            let bias = scale(mv(&ci, d), -1.0);
            let cal = Calib {
                c,
                bias,
                time_offset_ms: FIT_OFFSETS_MS[k],
                pairs: kept,
                moving,
                r2,
            };
            if best.as_ref().map_or(true, |b| cal.r2 > b.r2) {
                best = Some(cal);
            }
        }
        best
    }
}

/// Built-in calibration (raw LSB -> aim rad/s) used when `~/.config/omarchyvr/ctrl-imu.json` has no entry for a hand,
/// so the gyro is on from the first second on a new install. Fitted on optical poses (online fit of the 2026-10-14/15
/// sessions, checked on the 2026-10-16 takes: same scale 0.122 deg/s per LSB, same axes, misalignment 8-11 deg and
/// time offset 0-8 ms in every log, analysis/linux-recordings-20261016.md). The bias is re-learned at rest.
pub fn default_calib(hand: usize) -> Calib {
    let c = if hand == 0 {
        [
            [2.67478985236365e-4, 1.6795515395578024e-4, -2.109750888158635e-3],
            [-2.120013326287931e-3, -6.379925495236746e-5, -3.9738136017879584e-4],
            [1.0217973941656887e-5, 2.0903105813178473e-3, 1.7519832998645835e-4],
        ]
    } else {
        [
            [2.9903520979486335e-4, 4.4348525459939994e-5, 2.093408299192325e-3],
            [2.1180422764101904e-3, -6.652499863749047e-5, -2.916982133170233e-4],
            [5.9122966786586084e-5, 2.1070131931618533e-3, -4.658550218077466e-5],
        ]
    };
    Calib {
        c,
        bias: [0.0; 3],
        time_offset_ms: 0.0,
        pairs: 0,
        moving: 0,
        r2: 0.98,
    }
}

/// The saved calibration of `hand`, else the built-in one (unless PIMAX_CTRL_IMU_DEFAULT=0).
pub fn calib_or_default(path: &Path, hand: usize) -> Option<Calib> {
    load_calib(path, hand).or_else(|| {
        let off = std::env::var("PIMAX_CTRL_IMU_DEFAULT").is_ok_and(|v| v.trim() == "0");
        (!off).then(|| default_calib(hand))
    })
}

// ---------- calibration file ----------

pub fn calib_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_CTRL_IMU_CALIB").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let h = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    h.join(".config/omarchyvr/ctrl-imu.json")
}

fn hand_name(hand: usize) -> &'static str {
    if hand == 0 {
        "left"
    } else {
        "right"
    }
}

fn nums(s: &str) -> Vec<f64> {
    s.split(|c: char| c == ',' || c == '[' || c == ']' || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .filter_map(|t| t.parse().ok())
        .collect()
}

/// Value text after `"key":` inside `obj` (up to the matching `]` for arrays, else up to `,` / `}`).
fn field<'a>(obj: &'a str, key: &str) -> Option<&'a str> {
    let i = obj.find(&format!("\"{key}\""))?;
    let rest = &obj[i + key.len() + 2..];
    let rest = &rest[rest.find(':')? + 1..];
    let rest = rest.trim_start();
    if rest.starts_with('[') {
        Some(&rest[..rest.find(']')? + 1])
    } else {
        let e = rest.find([',', '}', '\n']).unwrap_or(rest.len());
        Some(rest[..e].trim())
    }
}

fn hand_obj(text: &str, hand: usize) -> Option<&str> {
    let i = text.find(&format!("\"{}\"", hand_name(hand)))?;
    let rest = &text[i..];
    let s = rest.find('{')?;
    let e = rest[s..].find('}')?;
    Some(&rest[s..s + e + 1])
}

/// Read one hand's calibration from the JSON file.
pub fn load_calib(path: &Path, hand: usize) -> Option<Calib> {
    let t = std::fs::read_to_string(path).ok()?;
    if !t.contains("pimax-ctrl-imu") {
        return None;
    }
    let o = hand_obj(&t, hand)?;
    let c = nums(field(o, "c")?);
    let b = nums(field(o, "bias_lsb")?);
    if c.len() != 9 || b.len() != 3 {
        return None;
    }
    let f = |k: &str| field(o, k).and_then(|v| v.parse::<f64>().ok());
    Some(Calib {
        c: [[c[0], c[1], c[2]], [c[3], c[4], c[5]], [c[6], c[7], c[8]]],
        bias: [b[0], b[1], b[2]],
        time_offset_ms: f("time_offset_ms").unwrap_or(0.0),
        pairs: f("pairs").unwrap_or(0.0) as u32,
        moving: f("moving").unwrap_or(0.0) as u32,
        r2: f("r2").unwrap_or(0.0),
    })
}

fn hand_json(c: &Calib) -> String {
    let fl: Vec<String> = c.c.iter().flatten().map(|v| format!("{v:e}")).collect();
    format!(
        "{{ \"c\": [{}], \"bias_lsb\": [{:.3}, {:.3}, {:.3}], \"time_offset_ms\": {}, \"pairs\": {}, \"moving\": {}, \"r2\": {:.4}, \"scale_dps_per_lsb\": {:.5}, \"axes\": \"{}\", \"axis_misalignment_deg\": {:.2} }}",
        fl.join(", "),
        c.bias[0],
        c.bias[1],
        c.bias[2],
        c.time_offset_ms,
        c.pairs,
        c.moving,
        c.r2,
        c.scale_dps_per_lsb(),
        c.axes(),
        c.axis_misalignment_deg()
    )
}

/// Write one hand's calibration, keeping the other hand's entry (atomic rename).
pub fn save_calib(path: &Path, hand: usize, c: &Calib) -> std::io::Result<()> {
    let other = 1 - hand;
    let old = std::fs::read_to_string(path).unwrap_or_default();
    let keep = if old.contains("pimax-ctrl-imu") {
        hand_obj(&old, other).map(|s| s.to_string())
    } else {
        None
    };
    let mut entries = [None, None];
    entries[hand] = Some(hand_json(c));
    entries[other] = keep;
    let mut s = String::from("{\n  \"format\": \"pimax-ctrl-imu\",\n  \"version\": 1,\n  \"note\": \"controller gyro calibration fitted on optical poses (pimax-hands fuse.rs): aim-frame rad/s = c * (raw - bias_lsb)\"");
    for (h, e) in entries.iter().enumerate() {
        if let Some(e) = e {
            s += &format!(",\n  \"{}\": {}", hand_name(h), e);
        }
    }
    s += "\n}\n";
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("json.tmp{}", std::process::id()));
    std::fs::write(&tmp, s)?;
    std::fs::rename(tmp, path)
}

// ---------- the fuser ----------

#[derive(Clone, Copy, Debug)]
pub struct Config {
    /// Position extrapolation window after the last fix (s).
    pub extrap_s: f64,
    /// Decay constant of the extrapolation (s).
    pub extrap_tau: f64,
    /// Cap of the extrapolation velocity (m/s).
    pub max_vel: f64,
    /// Out of view this long -> None (rest pose).
    pub lost_s: f64,
    /// Reacquire blend duration (s).
    pub blend_s: f64,
    /// A fix after this gap is a reacquire (s).
    pub gap_s: f64,
    /// Outlier gate: orientation jump vs the gyro prediction (deg).
    pub max_jump_deg: f64,
    /// Outlier gate: position jump vs the extrapolation (m, plus 3 m/s * age).
    pub max_jump_m: f64,
    /// Use the gyro: Some(true/false) forced, None = when a calibration exists.
    pub use_imu: Option<bool>,
    /// Allow the online fit to replace / create the calibration.
    pub online_fit: bool,
    /// Body model (rule based): shoulder of the LEFT hand in the head-yaw frame (m; the right one is mirrored).
    pub shoulder: V3,
    /// Arm reach from the shoulder (m).
    pub reach: f64,
    /// How far a hand may cross the head centre to the other side (m).
    pub cross: f64,
    /// Two hands are never closer than this (m).
    pub min_sep: f64,
    /// Filter the optical poses (alpha-beta position + gyro-propagated orientation, gains by LED count, stronger at
    /// rest) instead of showing each fix as is.
    pub filter: bool,
    /// Out of view and the gyro says the controller is still (lying on the desk): hold it in the world instead of
    /// relative to the head.
    pub still_lock: bool,
    /// Forearm model out of view (with the gyro): the controller minus the elbow, in the aim frame (m). The elbow is
    /// held (head-yaw frame, or the world when still) and the controller swings around it with the gyro orientation,
    /// so a wrist or forearm turn out of view also moves the position. None = the controller itself is held
    /// (before 2026-10-16). Fitted on optical poses (`pimax-ctrltrack occlusion --fit-forearm`).
    pub forearm: Option<V3>,
    /// Out of view: carry the motion the controller had with the accelerometer (gravity from the last 0.5 s in
    /// view removed, velocity leaking with `dr_tau_s`), added to the hold. 0 = off.
    pub dr_tau_s: f64,
    /// Cap of that carried displacement (m).
    pub dr_max_m: f64,
    /// Re-acquire: a big correction slides at most this fast (m/s; the blend gets longer, up to `blend_max_s`).
    /// 0 = always `blend_s`.
    pub blend_mps: f64,
    /// Same for the orientation (deg/s).
    pub blend_dps: f64,
    pub blend_max_s: f64,
    /// Ease in and out (smoothstep) instead of a linear blend: no velocity jump at either end.
    pub blend_ease: bool,
    /// Out of view, how much of the head's yaw / position change the held controller follows (1 = all).
    pub follow_yaw: f64,
    pub follow_pos: f64,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            extrap_s: 0.08,
            extrap_tau: 0.06,
            max_vel: 1.5,
            lost_s: 4.0,
            blend_s: 0.15,
            gap_s: 0.1,
            max_jump_deg: 35.0,
            max_jump_m: 0.15,
            use_imu: None,
            online_fit: true,
            shoulder: [-0.18, -0.25, 0.10],
            reach: 0.75,
            cross: 0.30,
            min_sep: 0.08,
            filter: true,
            still_lock: true,
            forearm: Some(FOREARM_LEFT),
            dr_tau_s: 0.0,
            dr_max_m: 0.25,
            blend_mps: 0.5,
            blend_dps: 120.0,
            blend_max_s: 0.5,
            blend_ease: false,
            follow_yaw: 0.5,
            follow_pos: 1.0,
        }
    }
}

/// Default forearm vector of the LEFT controller (aim frame, controller minus elbow; the right one is mirrored in x).
pub const FOREARM_LEFT: V3 = [0.01, 0.085, -0.09];

impl Config {
    /// Defaults plus the environment: PIMAX_CTRL_TRACK_IMU (0/1), PIMAX_CTRL_TRACK_LOST_S, PIMAX_CTRL_TRACK_EXTRAP_MS,
    /// PIMAX_CTRL_TRACK_BLEND_MS, PIMAX_CTRL_IMU_FIT (0 = no online fit), PIMAX_CTRL_TRACK_FOREARM (0 = off, or
    /// x,y,z m), PIMAX_CTRL_TRACK_FOLLOW_YAW (0..1, 1 = the old full head-yaw hold), PIMAX_CTRL_TRACK_BLEND_MPS
    /// (0 = fixed-length re-acquire blend), PIMAX_CTRL_TRACK_DR_MS (accelerometer carry, 0 = off; measured worse).
    pub fn from_env() -> Config {
        let mut c = Config::default();
        let num = |k: &str| {
            std::env::var(k)
                .ok()
                .and_then(|v| v.trim().parse::<f64>().ok())
        };
        if let Ok(v) = std::env::var("PIMAX_CTRL_TRACK_IMU") {
            match v.trim() {
                "0" | "off" | "false" => c.use_imu = Some(false),
                "1" | "on" | "true" => c.use_imu = Some(true),
                _ => {}
            }
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_LOST_S").filter(|v| *v > 0.0) {
            c.lost_s = v;
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_EXTRAP_MS").filter(|v| *v >= 0.0) {
            c.extrap_s = v / 1000.0;
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_BLEND_MS").filter(|v| *v >= 0.0) {
            c.blend_s = v / 1000.0;
        }
        if num("PIMAX_CTRL_IMU_FIT") == Some(0.0) {
            c.online_fit = false;
        }
        if num("PIMAX_CTRL_TRACK_FILTER") == Some(0.0) {
            c.filter = false;
        }
        if num("PIMAX_CTRL_TRACK_STILL_LOCK") == Some(0.0) {
            c.still_lock = false;
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_FOLLOW_YAW").filter(|v| (0.0..=1.0).contains(v)) {
            c.follow_yaw = v;
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_BLEND_MPS").filter(|v| *v >= 0.0) {
            c.blend_mps = v;
            if v == 0.0 {
                c.blend_dps = 0.0;
            }
        }
        if let Some(v) = num("PIMAX_CTRL_TRACK_DR_MS").filter(|v| *v >= 0.0) {
            c.dr_tau_s = v / 1000.0;
        }
        if let Ok(v) = std::env::var("PIMAX_CTRL_TRACK_FOREARM") {
            let n = nums(&v);
            match n.len() {
                1 if n[0] == 0.0 => c.forearm = None,
                3 => c.forearm = Some([n[0], n[1], n[2]]),
                _ => {}
            }
        }
        c
    }
}

/// What [`Fuser::get`] returned.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// No pose (never seen, or out of view longer than `lost_s`): use the rest pose.
    None = 0,
    /// Fresh optical (+ extrapolation).
    Optical = 1,
    /// Out of view: arm-model hold relative to the head.
    Hold = 2,
}

#[derive(Clone, Copy, Debug)]
struct Fix {
    t: u64,
    w: P, // world_from_aim
    leds: u32,
    seg: u32,
}

#[derive(Clone, Copy, Debug)]
struct Dr {
    fix_t: u64,
    g: V3,
    t: u64,
    v: V3,
    d: V3,
    d_anchor: Option<V3>,
}

#[derive(Clone, Copy, Debug)]
struct Blend {
    /// Set by the first display request after the fix (a fix is 40-80 ms in the past by then).
    t0: Option<u64>,
    /// Duration (s): `blend_s`, longer for a big correction (`blend_mps`, `blend_dps`).
    dur: f64,
    dp: V3,
    dq: Q, // predicted = dq * optical (world)
}

const RING_NS: u64 = 3_000_000_000;

pub struct Fuser {
    pub hand: usize,
    pub cfg: Config,
    pub calib: Option<Calib>,
    calib_from_file: bool,
    // imu
    dev_last: Option<u32>,
    dev_us: u64,
    clk_off: Option<i64>, // host_ns - dev_us*1000 (lower envelope)
    clk_t: u64,
    raw: VecDeque<(u64, V3)>,
    /// Accelerometer samples (raw LSB), same clock.
    acc: VecDeque<(u64, V3)>,
    /// Accelerometer dead reckoning since the last fix: (fix time, gravity in the world, time, velocity,
    /// displacement, displacement when the hold was anchored).
    dr: Option<Dr>,
    q_ring: VecDeque<(u64, Q)>,
    q_imu: Q,
    rest_n: u32,
    rest_sum: V3,
    // optical
    last: Option<Fix>,
    hist: VecDeque<Fix>,
    seg: u32,
    vel: V3,
    /// Out-of-view anchor: the aim pose in the head-yaw frame (false) or in the world (true, controller still).
    hold: Option<(P, bool)>,
    /// Head yaw frame when the hold was anchored (partial follow).
    hold_yaw: Option<P>,
    blend: Option<Blend>,
    rejects: u32,
    pending: VecDeque<(Fix, Fix)>,
    pub fit: Fit,
    fit_dirty: bool,
    last_save: u64,
    pub stats: Stats,
    log: Option<Log>,
    last_p_log: u64,
    /// Latest time a pose was asked for (display requests).
    last_query_t: u64,
    /// LED count of the fix being judged.
    last_leds: u32,
}

struct Log {
    w: std::io::BufWriter<std::fs::File>,
    span_ns: u64,
    end: Option<u64>,
    /// After `span_ns` the log pauses; it writes again while `$XDG_RUNTIME_DIR/omarchyvr-imu-log.on` exists (a tracking
    /// take is being recorded). (last check time, flag present)
    flag: (u64, bool),
}

#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub imu_samples: u64,
    pub fixes: u64,
    pub rejected: u64,
    /// Dropped for the body model: out of reach, wrong side, on top of the other hand, stale.
    pub rejected_body: u64,
    pub reacquires: u64,
    pub fit_updates: u64,
}

impl Fuser {
    pub fn new(hand: usize, cfg: Config, calib: Option<Calib>) -> Fuser {
        Fuser {
            hand,
            cfg,
            calib_from_file: calib.is_some(),
            calib,
            dev_last: None,
            dev_us: 0,
            clk_off: None,
            clk_t: 0,
            raw: VecDeque::new(),
            q_ring: VecDeque::new(),
            q_imu: [0.0, 0.0, 0.0, 1.0],
            acc: VecDeque::new(),
            dr: None,
            rest_n: 0,
            rest_sum: [0.0; 3],
            last: None,
            hist: VecDeque::new(),
            seg: 0,
            vel: [0.0; 3],
            hold: None,
            hold_yaw: None,
            blend: None,
            rejects: 0,
            pending: VecDeque::new(),
            fit: Fit::new(),
            fit_dirty: false,
            last_save: 0,
            stats: Stats::default(),
            log: None,
            last_p_log: 0,
            last_query_t: 0,
            last_leds: 0,
        }
    }

    /// Log every gyro sample and optical fix to `path` for `minutes` (offline fit: `pimax-ctrltrack imufit`).
    pub fn start_log(&mut self, path: &Path, minutes: f64) -> std::io::Result<()> {
        let f = std::fs::File::create(path)?;
        let mut w = std::io::BufWriter::with_capacity(1 << 16, f);
        writeln!(w, "# pimax-ctrlfuse log v1 hand={}", hand_name(self.hand))?;
        writeln!(w, "# I,host_ns,dev_us,gx,gy,gz,ax,ay,az (raw LSB) | O,frame_ns,px,py,pz,qx,qy,qz,qw (world_from_aim),hpx,hpy,hpz,hqx,hqy,hqz,hqw (world_from_head),rms_px,leds,verdict | P,t_ns,state,px,py,pz,qx,qy,qz,qw (shown aim pose),hpx,hpy,hpz,hqx,hqy,hqz,hqw (head),other_px,other_py,other_pz (50 Hz)")?;
        self.log = Some(Log {
            w,
            span_ns: (minutes * 60e9) as u64,
            end: None,
            flag: (0, false),
        });
        Ok(())
    }

    fn log_line(&mut self, t: u64, line: std::fmt::Arguments) {
        let Some(l) = &mut self.log else { return };
        let end = *l.end.get_or_insert(t.saturating_add(l.span_ns));
        if t > end {
            if t.saturating_sub(l.flag.0) > 1_000_000_000 {
                let on = std::env::var_os("XDG_RUNTIME_DIR")
                    .filter(|d| !d.is_empty())
                    .is_some_and(|d| Path::new(&d).join("omarchyvr-imu-log.on").exists());
                if l.flag.1 && !on {
                    let _ = l.w.flush();
                }
                l.flag = (t, on);
            }
            if !l.flag.1 {
                return;
            }
        }
        let _ = l.w.write_fmt(line);
        let _ = l.w.write_all(b"\n");
    }

    pub fn gyro_active(&self) -> bool {
        match self.cfg.use_imu {
            Some(false) => false,
            Some(true) => true,
            None => self.calib.is_some(),
        }
    }

    fn active_calib(&self) -> Option<Calib> {
        if !self.gyro_active() {
            return None;
        }
        self.calib.or_else(|| {
            // forced on without a fit: Monado's old guess (16.4 LSB per deg/s, raw axes = aim axes)
            let s = (1.0 / 16.4f64).to_radians();
            Some(Calib {
                c: [[s, 0.0, 0.0], [0.0, s, 0.0], [0.0, 0.0, s]],
                bias: [0.0; 3],
                time_offset_ms: 0.0,
                pairs: 0,
                moving: 0,
                r2: 0.0,
            })
        })
    }

    /// One raw controller report received at `host_ns`.
    pub fn report(&mut self, host_ns: u64, raw: &[u8; 64]) {
        let (ts, g, n) = report_gyro(raw);
        let a = report_accel(raw);
        for k in 0..n {
            self.sample_acc(host_ns, ts[k], g[k], Some(a[k]));
        }
    }

    /// One gyro sample: device µs stamp, raw LSB.
    pub fn sample(&mut self, host_ns: u64, dev_ts: u32, g: [i16; 3]) {
        self.sample_acc(host_ns, dev_ts, g, None);
    }

    /// One gyro sample with the accelerometer (logged for later motion models).
    pub fn sample_acc(&mut self, host_ns: u64, dev_ts: u32, g: [i16; 3], acc: Option<[i16; 3]>) {
        // unwrap the u32 µs device clock
        let dt_us = match self.dev_last {
            None => 0u64,
            Some(l) => {
                let d = dev_ts.wrapping_sub(l);
                if d > 0x8000_0000 {
                    return; // older than the last sample (reordered): ignore
                }
                d as u64
            }
        };
        self.dev_last = Some(dev_ts);
        self.dev_us += dt_us;
        // host time of the sample: lower envelope of host - device (USB delays only add), drifting up 50 µs/s
        let off = host_ns as i64 - (self.dev_us * 1000) as i64;
        let est = match self.clk_off {
            None => off,
            Some(o) => {
                let age_s = host_ns.saturating_sub(self.clk_t) as f64 * 1e-9;
                (o + (age_s * 50_000.0) as i64).min(off)
            }
        };
        self.clk_off = Some(est);
        self.clk_t = host_ns;
        let t = ((self.dev_us * 1000) as i64 + est).max(0) as u64;
        let x = [g[0] as f64, g[1] as f64, g[2] as f64];
        self.stats.imu_samples += 1;
        if self.log.is_some() {
            match acc {
                Some(a) => self.log_line(
                    t,
                    format_args!(
                        "I,{t},{dev_ts},{},{},{},{},{},{}",
                        g[0], g[1], g[2], a[0], a[1], a[2]
                    ),
                ),
                None => self.log_line(t, format_args!("I,{t},{dev_ts},{},{},{}", g[0], g[1], g[2])),
            }
        }
        self.raw.push_back((t, x));
        if let Some(a) = acc {
            self.acc.push_back((t, [a[0] as f64, a[1] as f64, a[2] as f64]));
            while self
                .acc
                .front()
                .is_some_and(|(t0, _)| t.saturating_sub(*t0) > RING_NS)
            {
                self.acc.pop_front();
            }
        }
        while self
            .raw
            .front()
            .is_some_and(|(t0, _)| t.saturating_sub(*t0) > RING_NS)
        {
            self.raw.pop_front();
        }
        // bias at rest: 0.5 s window with every axis within +-6 LSB of its mean
        self.update_rest_bias();
        if !self.pending.is_empty() {
            self.process_pending();
        }
        // integrate
        if let Some(c) = self.active_calib() {
            let dt = (dt_us as f64 * 1e-6).clamp(0.0, 0.02);
            let w = c.omega(x);
            self.q_imu = qnorm(qmul(self.q_imu, qexp(scale(w, dt))));
        }
        self.q_ring.push_back((t, self.q_imu));
        while self
            .q_ring
            .front()
            .is_some_and(|(t0, _)| t.saturating_sub(*t0) > RING_NS)
        {
            self.q_ring.pop_front();
        }
    }

    fn update_rest_bias(&mut self) {
        let Some(&(t, _)) = self.raw.back() else {
            return;
        };
        let win: Vec<V3> = self
            .raw
            .iter()
            .rev()
            .take_while(|(t0, _)| t - t0 < 500_000_000)
            .map(|(_, x)| *x)
            .collect();
        if win.len() < 100 {
            return;
        }
        let n = win.len() as f64;
        let mean = scale(win.iter().fold([0.0; 3], |a, x| add(a, *x)), 1.0 / n);
        // a steady turn is just as flat: only a mean near the known bias is rest (real biases are within ~15 LSB;
        // 40 LSB = about 5 deg/s)
        let near_bias = self
            .calib
            .map_or(true, |c| (0..3).all(|k| (mean[k] - c.bias[k]).abs() <= 40.0));
        if near_bias
            && win
                .iter()
                .all(|x| (0..3).all(|k| (x[k] - mean[k]).abs() <= 6.0))
        {
            self.rest_n += 1;
            self.rest_sum = add(self.rest_sum, mean);
            if self.rest_n % 50 == 0 {
                if let Some(c) = &mut self.calib {
                    // slow blend of the measured rest bias into the calibration
                    let m = scale(self.rest_sum, 1.0 / self.rest_n as f64);
                    c.bias = add(scale(c.bias, 0.7), scale(m, 0.3));
                }
                self.rest_n = 0;
                self.rest_sum = [0.0; 3];
            }
        }
    }

    /// Integrated gyro orientation at `t` (interpolated in the ring).
    fn q_at(&self, t: u64) -> Option<Q> {
        let ring = &self.q_ring;
        if ring.is_empty() {
            return None;
        }
        if t >= ring.back().unwrap().0 {
            return Some(ring.back().unwrap().1);
        }
        if t <= ring.front().unwrap().0 {
            return Some(ring.front().unwrap().1);
        }
        // binary search
        let idx = ring.partition_point(|(t0, _)| *t0 <= t);
        let (t0, q0) = ring[idx - 1];
        let (t1, q1) = ring[idx];
        let f = if t1 > t0 {
            (t - t0) as f64 / (t1 - t0) as f64
        } else {
            0.0
        };
        // small step: linear + normalise
        let q1 = if q0[0] * q1[0] + q0[1] * q1[1] + q0[2] * q1[2] + q0[3] * q1[3] < 0.0 {
            [-q1[0], -q1[1], -q1[2], -q1[3]]
        } else {
            q1
        };
        Some(qnorm([
            q0[0] + (q1[0] - q0[0]) * f,
            q0[1] + (q1[1] - q0[1]) * f,
            q0[2] + (q1[2] - q0[2]) * f,
            q0[3] + (q1[3] - q0[3]) * f,
        ]))
    }

    /// Gyro rotation (aim frame) from `t0` to `t1`, if the gyro is in use and covers it.
    fn gyro_delta(&self, t0: u64, t1: u64) -> Option<Q> {
        let c = self.active_calib()?;
        let off = (c.time_offset_ms * 1e6) as i64;
        let a = self.q_at((t0 as i64 + off).max(0) as u64)?;
        let b = self.q_at((t1 as i64 + off).max(0) as u64)?;
        Some(qnorm(qmul(qconj(a), b)))
    }

    /// This hand's shoulder in the world, from the head pose (position + yaw).
    pub fn shoulder(&self, head: &P) -> V3 {
        let mut o = self.cfg.shoulder;
        if self.hand == 1 {
            o[0] = -o[0];
        }
        add(head.p, qrot(yaw_only(head.q), o))
    }

    /// Why a world position cannot be this hand (None = plausible).
    pub fn implausible(&self, p: V3, head: &P) -> Option<&'static str> {
        if norm(sub(p, self.shoulder(head))) > self.cfg.reach + 0.05 {
            return Some("reach");
        }
        let local = qrot(qconj(yaw_only(head.q)), sub(p, head.p));
        if (self.hand == 0 && local[0] > self.cfg.cross)
            || (self.hand == 1 && local[0] < -self.cfg.cross)
        {
            return Some("side");
        }
        None
    }

    /// Keep a predicted position inside the reach sphere and away from the other hand.
    fn constrain(&self, mut p: V3, head: &P, other: Option<V3>) -> V3 {
        let sh = self.shoulder(head);
        let d = sub(p, sh);
        let n = norm(d);
        if n > self.cfg.reach {
            p = add(sh, scale(d, self.cfg.reach / n));
        }
        if let Some(o) = other {
            let d = sub(p, o);
            let n = norm(d);
            if n < self.cfg.min_sep {
                // push away from the other hand, towards this hand's side if they coincide
                let dir = if n > 1e-6 {
                    scale(d, 1.0 / n)
                } else {
                    qrot(
                        yaw_only(head.q),
                        [if self.hand == 0 { -1.0 } else { 1.0 }, 0.0, 0.0],
                    )
                };
                p = add(o, scale(dir, self.cfg.min_sep));
            }
        }
        p
    }

    /// Position of the last fix if it is at most `max_age_s` older than `t` (for the other hand's checks).
    pub fn fresh_fix(&self, t: u64, max_age_s: f64) -> Option<V3> {
        let f = self.last?;
        let age = (t as i64 - f.t as i64).abs() as f64 * 1e-9;
        (age <= max_age_s).then_some(f.w.p)
    }

    /// Unconstrained prediction of the position at `t` (for the other hand's separation rule).
    pub fn peek(&mut self, t: u64, head: &P) -> Option<V3> {
        let (s, p) = self.predict(t, head);
        (s != State::None).then_some(p.p)
    }

    /// The gyro says the controller is not rotating (mean |w| < 0.2 rad/s over the last 0.3 s). Without a gyro
    /// calibration: every raw axis within 40 LSB.
    pub fn gyro_still(&self, t: u64) -> bool {
        let t0 = t.saturating_sub(300_000_000);
        let mut n = 0usize;
        let mut sum = 0.0;
        let mut maxraw: f64 = 0.0;
        for (ts, x) in self.raw.iter().rev() {
            if *ts > t {
                continue;
            }
            if *ts < t0 {
                break;
            }
            n += 1;
            match &self.calib {
                Some(c) => sum += norm(c.omega(*x)),
                None => maxraw = maxraw.max(x[0].abs()).max(x[1].abs()).max(x[2].abs()),
            }
        }
        if n < 50 {
            return false;
        }
        match self.calib {
            Some(_) => sum / (n as f64) < 0.2,
            None => maxraw < 40.0,
        }
    }

    /// Position gain for a fix with `leds` LEDs: few-LED solves jitter by centimetres (live: 5-6 LEDs 7-34 mm
    /// fix-to-fix at rest, 10+ LEDs 0.4-0.6 mm).
    fn gain(leds: u32, rms_px: f32) -> f64 {
        let a: f64 = match leds {
            0..=6 => 0.2,
            7..=9 => 0.5,
            _ => 0.85,
        };
        if rms_px > 1.5 {
            a * 0.5
        } else {
            a
        }
    }

    /// Alpha-beta update of the last filtered pose `f` with the fix `w` (same track, `dt` > 0). Returns the
    /// filtered pose; updates the velocity.
    fn filter_update(&mut self, f: &Fix, t: u64, w: P, leds: u32, rms_px: f32) -> P {
        let dt = (t - f.t) as f64 * 1e-9;
        let still = self.gyro_still(t) && norm(self.vel) < 0.1;
        let mut a = Self::gain(leds, rms_px);
        if still {
            a *= 0.35; // adaptive: smooth hard at rest, follow quickly when moving (One-Euro idea)
        }
        let pred_p = add(f.w.p, scale(self.vel, dt));
        let dq = self.gyro_delta(f.t, t);
        let pred_q = match dq {
            Some(d) => qnorm(qmul(f.w.q, d)),
            None => f.w.q,
        };
        let e = sub(w.p, pred_p);
        let p = add(pred_p, scale(e, a));
        let beta = 0.5 * a * a;
        let vmax = self.cfg.max_vel;
        let mut v = add(self.vel, scale(e, beta / dt.max(1e-3)));
        if norm(v) > vmax {
            v = scale(v, vmax / norm(v));
        }
        if still {
            v = scale(v, 0.5);
        }
        self.vel = v;
        // orientation: the gyro carries it, the fix pulls it (less when the gyro is in use)
        let k = if dq.is_some() { 0.6 * a } else { a };
        let corr = qlog(qmul(qconj(pred_q), w.q));
        let q = qnorm(qmul(pred_q, qexp(scale(corr, k))));
        P { q, p }
    }

    /// Prediction (no blend) at `t` with the head pose at `t`.
    fn predict(&mut self, t: u64, head: &P) -> (State, P) {
        let Some(f) = self.last else {
            return (State::None, P::ID);
        };
        let age = t.saturating_sub(f.t) as f64 * 1e-9;
        if age > self.cfg.lost_s {
            return (State::None, P::ID);
        }
        let dq = self.gyro_delta(f.t, t);
        let q = match dq {
            Some(d) => qnorm(qmul(f.w.q, d)),
            None => f.w.q,
        };
        let tau = self.cfg.extrap_tau;
        let ex = |a: f64| tau * (1.0 - (-a / tau).exp());
        if age <= self.cfg.extrap_s {
            let p = add(f.w.p, scale(self.vel, ex(age)));
            self.hold = None;
            return (State::Optical, P { q, p });
        }
        // out of view: the controller keeps its offset in the head-yaw frame from the end of the extrapolation
        let yaw_now = P {
            q: yaw_only(head.q),
            p: head.p,
        };
        // the body frame the hold follows: the head's yaw and position, or only part of them (the torso turns
        // less than the head)
        let y0 = *self.hold_yaw.get_or_insert(yaw_now);
        let yaw = if self.hold.is_none() {
            self.hold_yaw = Some(yaw_now);
            yaw_now
        } else {
            let dyaw = qlog(qmul(qconj(y0.q), yaw_now.q));
            P {
                q: qnorm(qmul(y0.q, qexp(scale(dyaw, self.cfg.follow_yaw)))),
                p: add(y0.p, scale(sub(yaw_now.p, y0.p), self.cfg.follow_pos)),
            }
        };
        let want_world = self.cfg.still_lock && self.gyro_still(t);
        // forearm model: the anchor is the elbow, the controller swings around it with the gyro orientation
        let arm = match (dq, self.cfg.forearm) {
            (Some(_), Some(mut f)) => {
                if self.hand == 1 {
                    f[0] = -f[0];
                }
                Some(f)
            }
            _ => None,
        };
        match self.hold {
            None => {
                let p_end = add(f.w.p, scale(self.vel, ex(self.cfg.extrap_s)));
                // the elbow from the orientation at the end of the extrapolation (the first query in the hold
                // may come later)
                let p_end = match arm {
                    Some(fa) => {
                        let t_end = f.t + (self.cfg.extrap_s * 1e9) as u64;
                        let q_end = self
                            .gyro_delta(f.t, t_end.min(t))
                            .map_or(q, |d| qnorm(qmul(f.w.q, d)));
                        sub(p_end, qrot(q_end, fa))
                    }
                    None => p_end,
                };
                let end = P { q, p: p_end };
                self.hold = Some(if want_world {
                    (end, true)
                } else {
                    (yaw.inv().mul(&end), false)
                });
            }
            Some((off, world)) if world != want_world => {
                // the controller started or stopped moving: re-anchor where it is now (no jump)
                let now = if world { off } else { yaw.mul(&off) };
                self.hold = Some(if want_world {
                    (now, true)
                } else {
                    (yaw.inv().mul(&now), false)
                });
            }
            _ => {}
        }
        let (off, world) = self.hold.unwrap();
        let held = if world { off } else { yaw.mul(&off) };
        let q = if dq.is_some() { q } else { held.q };
        let mut p = match arm {
            Some(fa) => add(held.p, qrot(q, fa)),
            None => held.p,
        };
        if !world {
            if let Some(d) = self.dead_reckon(&f, t) {
                p = add(p, d);
            }
        }
        (State::Hold, P { q, p })
    }

    /// Accelerometer displacement since the hold was anchored (world, m), or None (off, no accelerometer or no
    /// gyro calibration). Integrates lazily from the last call; the velocity starts at the filter's and leaks.
    fn dead_reckon(&mut self, f: &Fix, t: u64) -> Option<V3> {
        let tau = self.cfg.dr_tau_s;
        if tau <= 0.0 {
            return None;
        }
        let c = self.active_calib()?;
        let off = (c.time_offset_ms * 1e6) as i64;
        let q_at = |s: &Fuser, ts: u64| -> Option<Q> {
            let a = s.q_at((f.t as i64 + off).max(0) as u64)?;
            let b = s.q_at((ts as i64 + off).max(0) as u64)?;
            Some(qnorm(qmul(f.w.q, qmul(qconj(a), b))))
        };
        if self.dr.map_or(true, |d| d.fix_t != f.t) {
            // gravity (+ the accelerometer's own error) as seen in the world over the last 0.5 s in view
            let mut g = [0.0; 3];
            let mut n = 0.0;
            for &(ts, a) in self.acc.iter() {
                if ts + 500_000_000 < f.t || ts > f.t {
                    continue;
                }
                if let Some(q) = q_at(self, ts) {
                    g = add(g, qrot(q, c.accel(a)));
                    n += 1.0;
                }
            }
            if n < 50.0 {
                return None;
            }
            self.dr = Some(Dr {
                fix_t: f.t,
                g: scale(g, 1.0 / n),
                t: f.t,
                v: self.vel,
                d: [0.0; 3],
                d_anchor: None,
            });
        }
        let mut dr = self.dr?;
        let samples: Vec<(u64, V3)> = self
            .acc
            .iter()
            .filter(|(ts, _)| *ts > dr.t && *ts <= t)
            .copied()
            .collect();
        for (ts, a) in samples {
            let dt = (ts - dr.t) as f64 * 1e-9;
            if let Some(q) = q_at(self, ts) {
                let aw = sub(qrot(q, c.accel(a)), dr.g);
                dr.v = add(scale(dr.v, (-dt / tau).exp()), scale(aw, dt));
            }
            dr.d = add(dr.d, scale(dr.v, dt));
            let n = norm(dr.d);
            if n > self.cfg.dr_max_m {
                dr.d = scale(dr.d, self.cfg.dr_max_m / n);
            }
            dr.t = ts;
        }
        let base = *dr.d_anchor.get_or_insert(dr.d);
        self.dr = Some(dr);
        Some(sub(dr.d, base))
    }

    /// Share of the re-acquire correction still shown `a` s into a blend of `dur` s (1 -> 0).
    fn blend_weight(&self, a: f64, dur: f64) -> f64 {
        let x = (a / dur.max(1e-6)).clamp(0.0, 1.0);
        if self.cfg.blend_ease {
            1.0 - x * x * (3.0 - 2.0 * x)
        } else {
            1.0 - x
        }
    }

    /// Pose at `t` given the head pose at `t` (no other hand known).
    pub fn get(&mut self, t: u64, head: &P) -> (State, P) {
        self.get_with(t, head, None)
    }

    /// Pose at `t` given the head pose at `t` and the other hand's position: a prediction (extrapolated or held)
    /// stays inside the reach sphere and at least `min_sep` from the other hand; a fresh optical pose is shown as is.
    pub fn get_with(&mut self, t: u64, head: &P, other: Option<V3>) -> (State, P) {
        let (s, mut p) = self.predict(t, head);
        if s == State::None {
            return (s, p);
        }
        let age = self
            .last
            .map(|f| t.saturating_sub(f.t) as f64 * 1e-9)
            .unwrap_or(0.0);
        if s == State::Hold || age > 0.05 {
            p.p = self.constrain(p.p, head, other);
        }
        self.last_query_t = self.last_query_t.max(t);
        if let Some(b) = &mut self.blend {
            let t0 = *b.t0.get_or_insert(t);
            let b = *b;
            let a = t.saturating_sub(t0) as f64 * 1e-9;
            if a >= b.dur {
                self.blend = None;
            } else {
                let w = self.blend_weight(a, b.dur);
                p.p = add(p.p, scale(b.dp, w));
                p.q = qnorm(qmul(qexp(scale(qlog(b.dq), w)), p.q));
            }
        }
        if self.log.is_some() && t >= self.last_p_log + 20_000_000 {
            self.last_p_log = t;
            let o = other.unwrap_or([f64::NAN; 3]);
            self.log_line(
                t,
                format_args!(
                    "P,{t},{},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6},{:.4},{:.4},{:.4}",
                    s as u32, p.p[0], p.p[1], p.p[2], p.q[0], p.q[1], p.q[2], p.q[3], head.p[0], head.p[1], head.p[2],
                    head.q[0], head.q[1], head.q[2], head.q[3], o[0], o[1], o[2]
                ),
            );
        }
        (s, p)
    }

    /// An optical fix: world_from_aim at `t` (LED frame time), the head pose at `t` (no other hand known).
    pub fn optical(&mut self, t: u64, w: P, head: &P, rms_px: f32, leds: u32) {
        self.optical_with(t, w, head, rms_px, leds, None);
    }

    /// An optical fix plus the other hand's fresh optical position (if any). Returns the verdict: "ok" or the reason
    /// it was dropped (body model: "reach", "side", "merged", "stale"; prediction: "jump", "turn").
    pub fn optical_with(
        &mut self,
        t: u64,
        w: P,
        head: &P,
        rms_px: f32,
        leds: u32,
        other: Option<V3>,
    ) -> &'static str {
        self.last_leds = leds;
        let verdict = self.judge(t, w, head, other);
        if self.log.is_some() {
            self.log_line(
                t,
                format_args!(
                    "O,{t},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6},{rms_px:.3},{leds},{verdict}",
                    w.p[0], w.p[1], w.p[2], w.q[0], w.q[1], w.q[2], w.q[3], head.p[0], head.p[1], head.p[2], head.q[0],
                    head.q[1], head.q[2], head.q[3]
                ),
            );
        }
        match verdict {
            "ok" => self.accept(t, w, head, leds, rms_px, false),
            // orientation disagrees with the gyro on a few-LED solve (often a flipped solution): keep the position,
            // take the orientation from the gyro
            "pos-only" => {
                let (_, pred) = self.predict(t, head);
                self.accept(t, P { q: pred.q, p: w.p }, head, leds, rms_px, true)
            }
            _ => {}
        }
        verdict
    }

    /// Body-model and prediction gates. Body-model violations are never accepted; a jump or turn away from the
    /// prediction is accepted when it repeats (the prediction may be the wrong one).
    fn judge(&mut self, t: u64, w: P, head: &P, other: Option<V3>) -> &'static str {
        if let Some(&(t_imu, _)) = self.raw.back() {
            if t + 2_000_000_000 < t_imu {
                self.stats.rejected_body += 1;
                return "stale";
            }
        }
        if let Some(f) = self.last {
            if t <= f.t {
                return "old";
            }
        }
        if let Some(r) = self.implausible(w.p, head) {
            self.stats.rejected_body += 1;
            return r;
        }
        if let Some(o) = other {
            if norm(sub(o, w.p)) < self.cfg.min_sep {
                self.stats.rejected_body += 1;
                return "merged";
            }
        }
        let Some(f) = self.last else { return "ok" };
        let age = t.saturating_sub(f.t) as f64 * 1e-9;
        let (s, pred) = self.predict(t, head);
        if s == State::None {
            self.rejects = 0;
            return "ok";
        }
        let mut why = "ok";
        // the gyro is good to 2-4 deg over 1 s (live fit check), so also judge re-entries after a gap
        if self.gyro_delta(f.t, t).is_some()
            && age < self.cfg.lost_s
            && qangle(pred.q, w.q).to_degrees() > self.cfg.max_jump_deg
        {
            why = "turn";
        }
        if age <= self.cfg.extrap_s + 0.05
            && norm(sub(pred.p, w.p)) > self.cfg.max_jump_m + 3.0 * age
        {
            why = "jump";
        }
        // few-LED solves that only disagree in orientation: keep their position (live: 157 + 162 such fixes per
        // session, LED count p50 7, many flipped 120-180 deg)
        if why == "turn" && self.last_leds < 9 {
            self.stats.rejected += 1;
            return "pos-only";
        }
        // a jump is believed after 4 in a row (about 160 ms of consistent fixes), a turn after 3
        let limit = if why == "jump" { 3 } else { 2 };
        if why != "ok" && self.rejects < limit {
            self.rejects += 1;
            self.stats.rejected += 1;
            return why;
        }
        self.rejects = 0;
        "ok"
    }

    fn accept(&mut self, t: u64, w: P, head: &P, leds: u32, rms_px: f32, pos_only: bool) {
        let prev = self.last;
        if let Some(f) = prev {
            if t <= f.t {
                return;
            }
        }
        let age = prev.map(|f| t.saturating_sub(f.t) as f64 * 1e-9);
        // reacquire: blend from what was shown
        let gap = age.map_or(true, |a| a > self.cfg.gap_s);
        if let (true, Some(_)) = (gap, prev) {
            // what the last display request saw (the old prediction at about "now"), not at the frame time
            let qt = self.last_query_t.max(t);
            let (s, mut shown) = self.predict(qt, head);
            if let Some(b) = self.blend {
                if let Some(t0) = b.t0 {
                    let a = qt.saturating_sub(t0) as f64 * 1e-9;
                    if a < b.dur {
                        let wt = self.blend_weight(a, b.dur);
                        shown.p = add(shown.p, scale(b.dp, wt));
                    }
                }
            }
            if s != State::None {
                let dp = sub(shown.p, w.p);
                let dq = qnorm(qmul(shown.q, qconj(w.q)));
                let mut dur = self.cfg.blend_s;
                if self.cfg.blend_mps > 0.0 {
                    dur = dur.max(norm(dp) / self.cfg.blend_mps);
                }
                if self.cfg.blend_dps > 0.0 {
                    dur = dur.max(norm(qlog(dq)).to_degrees() / self.cfg.blend_dps);
                }
                dur = dur.min(self.cfg.blend_max_s.max(self.cfg.blend_s));
                self.blend = Some(Blend { t0: None, dur, dp, dq });
                self.stats.reacquires += 1;
            }
        }
        if gap {
            self.seg += 1;
        }
        // filter (same track) or raw
        let raw_w = w;
        let w = match (self.cfg.filter, prev, gap) {
            (true, Some(f), false) => self.filter_update(&f, t, w, leds, rms_px),
            _ => w,
        };
        // velocity from consecutive fixes (raw mode; the filter keeps its own)
        match (prev, age) {
            _ if self.cfg.filter && !gap => {}
            (Some(f), Some(a)) if a <= 0.1 && a > 0.0 => {
                let v = scale(sub(w.p, f.w.p), 1.0 / a);
                // jittery fixes give metres per second of noise: smooth over ~3 fixes and cap
                let vmax = self.cfg.max_vel;
                let v = if norm(v) > vmax {
                    scale(v, vmax / norm(v))
                } else {
                    v
                };
                self.vel = add(scale(self.vel, 0.7), scale(v, 0.3));
                if norm(self.vel) > vmax {
                    self.vel = scale(self.vel, vmax / norm(self.vel));
                }
            }
            _ => self.vel = [0.0; 3],
        }
        let fix = Fix {
            t,
            w,
            leds,
            seg: self.seg,
        };
        // the gyro fit compares the gyro with the raw optical rotation (the filtered one already contains the gyro)
        let raw_fix = Fix { w: raw_w, ..fix };
        if !pos_only {
            self.fit_pair(&raw_fix);
        }
        self.last = Some(fix);
        self.hold = None;
        self.stats.fixes += 1;
        if !pos_only {
            self.hist.push_back(raw_fix);
        }
        while self.hist.len() > 16 {
            self.hist.pop_front();
        }
    }

    /// Pair this fix with the one ~120 ms earlier on the same track and feed the fit.
    fn fit_pair(&mut self, b: &Fix) {
        // orientation from fewer LEDs is often far off
        if b.leds < 9 {
            return;
        }
        let Some(a) = self
            .hist
            .iter()
            .filter(|a| a.seg == b.seg && a.leds >= 9)
            .filter(|a| {
                let dt = b.t.saturating_sub(a.t) as f64 * 1e-9;
                (0.08..=0.2).contains(&dt)
            })
            .min_by_key(|a| (b.t.saturating_sub(a.t) as i64 - 120_000_000).abs())
            .copied()
        else {
            return;
        };
        // the IMU samples for the later time offsets have not arrived yet: evaluate the pair later
        self.pending.push_back((a, *b));
        if self.pending.len() > 64 {
            self.pending.pop_front();
        }
    }

    /// Feed queued pairs whose IMU interval (with the largest time offset) is complete.
    fn process_pending(&mut self) {
        let Some(&(t_last, _)) = self.raw.back() else {
            return;
        };
        let max_off = (FIT_OFFSETS_MS[FIT_OFFSETS_MS.len() - 1] * 1e6) as u64 + 10_000_000;
        while let Some(&(a, b)) = self.pending.front() {
            if b.t + max_off > t_last {
                break;
            }
            self.pending.pop_front();
            self.fit_eval(a, b);
        }
    }

    fn fit_eval(&mut self, a: Fix, b: Fix) {
        let dt = (b.t - a.t) as f64 * 1e-9;
        let y = scale(qlog(qmul(qconj(a.w.q), b.w.q)), 1.0 / dt);
        if norm(y) > 20.0 {
            return;
        }
        let mut xs = Vec::with_capacity(FIT_OFFSETS_MS.len());
        for off in FIT_OFFSETS_MS {
            let o = (off * 1e6) as i64;
            let t0 = (a.t as i64 + o).max(0) as u64;
            let t1 = (b.t as i64 + o).max(0) as u64;
            let s: Vec<V3> = self
                .raw
                .iter()
                .filter(|(t, _)| *t >= t0 && *t < t1)
                .map(|(_, x)| *x)
                .collect();
            // expect ~400-500 samples/s: need 60 % coverage
            if (s.len() as f64) < dt * 400.0 * 0.6 {
                return;
            }
            xs.push(scale(
                s.iter().fold([0.0; 3], |acc, x| add(acc, *x)),
                1.0 / s.len() as f64,
            ));
        }
        let mut arr = [[0.0; 3]; FIT_OFFSETS_MS.len()];
        arr.copy_from_slice(&xs);
        self.fit.add(arr, y);
        let n = self.fit.pairs();
        if self.cfg.online_fit && n % 25 == 0 {
            // kept pairs after trimming (the robust fit keeps about half of the live pairs)
            let need = if self.calib_from_file { 300 } else { 120 };
            if let Some(c) = self.fit.best(need, 40, 0.8) {
                if self
                    .calib
                    .map_or(true, |old| !self.calib_from_file || c.r2 >= old.r2 - 0.02)
                {
                    self.calib = Some(c);
                    self.calib_from_file = false;
                    self.fit_dirty = true;
                    self.stats.fit_updates += 1;
                }
            }
        }
    }

    /// Save the online fit if it changed and the last save is older than `min_interval_s` (0 = now).
    pub fn maybe_save(
        &mut self,
        now: u64,
        path: &Path,
        min_interval_s: f64,
    ) -> Option<std::io::Result<()>> {
        if !self.fit_dirty || now.saturating_sub(self.last_save) < (min_interval_s * 1e9) as u64 {
            return None;
        }
        let c = self.calib?;
        self.fit_dirty = false;
        self.last_save = now;
        Some(save_calib(path, self.hand, &c))
    }

    pub fn flush_log(&mut self) {
        if let Some(l) = &mut self.log {
            let _ = l.w.flush();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rotm(q: Q) -> M3 {
        let c = |v: V3| qrot(q, v);
        let (x, y, z) = (c([1.0, 0.0, 0.0]), c([0.0, 1.0, 0.0]), c([0.0, 0.0, 1.0]));
        [[x[0], y[0], z[0]], [x[1], y[1], z[1]], [x[2], y[2], z[2]]]
    }

    /// Simple LCG noise.
    struct Rng(u64);
    impl Rng {
        fn f(&mut self) -> f64 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 11) as f64 / (1u64 << 53) as f64) * 2.0 - 1.0
        }
    }

    fn report(ts: u32, g0: [i16; 3], g1: Option<[i16; 3]>) -> [u8; 64] {
        let mut r = [0u8; 64];
        r[0] = 0x53;
        r[1] = 0x32;
        r[2] = 0x08;
        r[3] = 2;
        r[8..12].copy_from_slice(&ts.to_le_bytes());
        for k in 0..3 {
            r[22 + 2 * k..24 + 2 * k].copy_from_slice(&g0[k].to_be_bytes());
            r[16 + 2 * k..18 + 2 * k].copy_from_slice(&1000i16.to_be_bytes()); // accel non-zero
        }
        if let Some(g1) = g1 {
            for k in 0..3 {
                r[36 + 2 * k..38 + 2 * k].copy_from_slice(&g1[k].to_be_bytes());
                r[30 + 2 * k..32 + 2 * k].copy_from_slice(&1000i16.to_be_bytes());
            }
        }
        r
    }

    /// Synthetic session: smooth rotation, gyro reports at 250 Hz x 2 samples, optical fixes at 25 Hz with noise.
    /// True mapping: aim = (raw y, -raw z, -raw x) * 1/16.4 deg/s, bias (12, -7, 3) LSB, IMU 16 ms late.
    fn synth(secs: f64, mut f: impl FnMut(u64, Option<&[u8; 64]>, Option<(P, P)>)) {
        let s = (1.0 / 16.4f64).to_radians();
        let c_true: M3 = [[0.0, s, 0.0], [0.0, 0.0, -s], [-s, 0.0, 0.0]];
        let ci = inv3(&c_true).unwrap();
        let bias = [12.0, -7.0, 3.0];
        let mut rng = Rng(7);
        let omega = |t: f64| -> V3 {
            [
                3.0 * (1.3 * t).sin() + 1.0 * (3.1 * t).cos(),
                2.5 * (0.7 * t + 1.0).sin(),
                2.0 * (2.3 * t).cos(),
            ]
        };
        let mut q: Q = [0.0, 0.0, 0.0, 1.0];
        let t0: u64 = 10_000_000_000;
        let dt = 0.002;
        let n = (secs / dt) as usize;
        let lat_ns = 16_000_000u64; // the IMU reports arrive 16 ms after the motion they measure... (time offset)
        let mut pending: Option<[i16; 3]> = None;
        for k in 0..n {
            let ts = k as f64 * dt;
            let w = omega(ts);
            q = qnorm(qmul(q, qexp(scale(w, dt))));
            let host = t0 + (ts * 1e9) as u64;
            // gyro sample for the motion at ts, stamped ts + 16 ms on the device clock
            let raw = add(mv(&ci, w), bias);
            let g = [
                (raw[0] + rng.f() * 2.0).round() as i16,
                (raw[1] + rng.f() * 2.0).round() as i16,
                (raw[2] + rng.f() * 2.0).round() as i16,
            ];
            match pending.take() {
                None => pending = Some(g),
                Some(g0) => {
                    let dev = ((ts - dt) * 1e6) as u32 + 16_000;
                    let r = report(dev, g0, Some(g));
                    f(host + lat_ns + 1_000_000, Some(&r), None);
                }
            }
            if k % 20 == 0 {
                let noise = [rng.f() * 0.02, rng.f() * 0.02, rng.f() * 0.02];
                let mut qn = qnorm(qmul(q, qexp(noise)));
                // one fix in eight is a bad solve (flipped up to 90 deg), like the live 5-LED fixes
                if (k / 20) % 8 == 3 {
                    qn = qnorm(qmul(
                        qn,
                        qexp([rng.f() * 1.5, rng.f() * 1.5, rng.f() * 1.5]),
                    ));
                }
                let w = P {
                    q: qn,
                    p: [0.1, -0.1, -0.4],
                };
                f(host, None, Some((w, P::ID)));
            }
        }
        let _ = c_true;
    }

    #[test]
    fn online_fit_recovers_scale_axes_bias_and_offset() {
        let mut fu = Fuser::new(0, Config::default(), None);
        let mut events: Vec<(u64, Option<[u8; 64]>, Option<(P, P)>)> = Vec::new();
        synth(60.0, |t, r, o| events.push((t, r.copied(), o)));
        events.sort_by_key(|e| e.0);
        for (t, r, o) in &events {
            if let Some(r) = r {
                fu.report(*t, r);
            }
            if let Some((w, h)) = o {
                fu.optical(*t, *w, h, 0.5, 10);
            }
        }
        let c = fu.calib.expect("fit");
        let s = (1.0 / 16.4f64).to_radians();
        assert!(
            (c.scale_dps_per_lsb() - 1.0 / 16.4).abs() < 0.03 / 16.4,
            "scale {}",
            c.scale_dps_per_lsb()
        );
        assert_eq!(c.axes(), "+y -z -x");
        assert!(c.axis_misalignment_deg() < 2.0);
        assert!(
            (c.bias[0] - 12.0).abs() < 5.0 && (c.bias[1] + 7.0).abs() < 5.0,
            "bias {:?}",
            c.bias
        );
        assert!(
            c.time_offset_ms == 16.0 || c.time_offset_ms == 32.0 || c.time_offset_ms == 0.0,
            "offset {}",
            c.time_offset_ms
        );
        assert!(c.r2 > 0.95, "r2 {}", c.r2);
        let _ = s;
        let _ = rotm([0.0, 0.0, 0.0, 1.0]);
    }

    #[test]
    fn save_and_load_keep_both_hands() {
        let p = std::env::temp_dir().join(format!("pimax-ctrl-imu-{}.json", std::process::id()));
        let c = Calib {
            c: [[0.001, 0.0, 0.0], [0.0, 0.0, -0.001], [0.0, 0.001, 0.0]],
            bias: [1.0, 2.0, 3.0],
            time_offset_ms: 16.0,
            pairs: 500,
            moving: 200,
            r2: 0.93,
        };
        save_calib(&p, 0, &c).unwrap();
        let mut c2 = c;
        c2.bias = [4.0, 5.0, 6.0];
        save_calib(&p, 1, &c2).unwrap();
        let l = load_calib(&p, 0).unwrap();
        let r = load_calib(&p, 1).unwrap();
        assert!((l.c[1][2] + 0.001).abs() < 1e-12 && (l.bias[2] - 3.0).abs() < 1e-9);
        assert!(
            (r.bias[0] - 4.0).abs() < 1e-9
                && r.pairs == 500
                && (r.time_offset_ms - 16.0).abs() < 1e-9
        );
        assert_eq!(l.axes(), "+x -z +y");
        let _ = std::fs::remove_file(&p);
    }

    #[test]
    fn out_of_view_extrapolates_then_holds_relative_to_the_head_then_gives_up() {
        let cfg = Config {
            use_imu: Some(false),
            lost_s: 3.0,
            follow_yaw: 1.0,
            ..Config::default()
        };
        let mut fu = Fuser::new(1, cfg, None);
        let head = P::ID;
        // moving +x at 1 m/s: fixes every 40 ms
        for k in 0..10u64 {
            let t = 1_000_000_000 + k * 40_000_000;
            fu.optical(
                t,
                P {
                    q: [0.0, 0.0, 0.0, 1.0],
                    p: [0.04 * k as f64, 0.0, -0.4],
                },
                &head,
                0.5,
                10,
            );
        }
        let tl = 1_000_000_000 + 9 * 40_000_000;
        let (s, p) = fu.get(tl + 50_000_000, &head);
        assert_eq!(s, State::Optical);
        assert!(
            p.p[0] > 0.36 + 0.02 && p.p[0] < 0.36 + 0.05,
            "extrapolated x {}",
            p.p[0]
        );
        // after the window: held; the head turns 90 deg left and steps 0.3 m: the controller comes along
        let (s, p_hold) = fu.get(tl + 300_000_000, &head);
        assert_eq!(s, State::Hold);
        let x_end = p_hold.p[0];
        let turned = P {
            q: [
                0.0,
                (std::f64::consts::FRAC_PI_4).sin(),
                0.0,
                (std::f64::consts::FRAC_PI_4).cos(),
            ],
            p: [0.0, 0.0, -0.3],
        };
        let (s, p2) = fu.get(tl + 600_000_000, &turned);
        assert_eq!(s, State::Hold);
        // offset (x_end, 0, -0.4) rotated 90 deg left about +y -> (-0.4, 0, -x_end), plus the head position
        assert!(
            (p2.p[0] + 0.4).abs() < 1e-6 && (p2.p[2] - (-x_end - 0.3)).abs() < 1e-6,
            "{:?}",
            p2.p
        );
        assert_eq!(fu.get(tl + 3_500_000_000, &head).0, State::None);
    }

    #[test]
    fn reacquire_blends_and_outliers_are_dropped() {
        let cfg = Config {
            use_imu: Some(false),
            blend_mps: 0.0,
            blend_dps: 0.0,
            ..Config::default()
        };
        let mut fu = Fuser::new(0, cfg, None);
        let head = P::ID;
        fu.optical(
            1_000_000_000,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [0.0, 0.0, -0.4],
            },
            &head,
            0.5,
            10,
        );
        fu.optical(
            1_040_000_000,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [0.0, 0.0, -0.4],
            },
            &head,
            0.5,
            10,
        );
        // 40 ms later a fix 1 m away: dropped (twice), the third is accepted
        fu.optical(
            1_080_000_000,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [1.0, 0.0, -0.4],
            },
            &head,
            0.5,
            10,
        );
        assert_eq!(
            fu.stats.rejected_body, 1,
            "1 m to the right of the left hand is out of its reach"
        );
        assert!(fu.get(1_090_000_000, &head).1.p[0].abs() < 1e-6);
        // a gap of 1 s, then the controller is 0.2 m to the right: blend over 150 ms from the first display request,
        // which comes 60 ms after the LED frame
        let t = 2_100_000_000;
        let q0 = t + 60_000_000;
        fu.optical(
            t,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [0.2, 0.0, -0.4],
            },
            &head,
            0.5,
            10,
        );
        let (_, a) = fu.get(q0, &head);
        let (_, b) = fu.get(q0 + 75_000_000, &head);
        let (_, c) = fu.get(q0 + 160_000_000, &head);
        assert!(a.p[0].abs() < 0.02, "starts at the shown pose: {}", a.p[0]);
        assert!(b.p[0] > 0.07 && b.p[0] < 0.13, "halfway: {}", b.p[0]);
        assert!((c.p[0] - 0.2).abs() < 0.01, "ends at the fix: {}", c.p[0]);
        assert_eq!(fu.stats.reacquires, 1);
    }

    #[test]
    fn body_model_never_merges_the_hands_and_keeps_a_held_hand_in_reach() {
        let cfg = Config {
            use_imu: Some(false),
            ..Config::default()
        };
        let head = P::ID;
        let mut left = Fuser::new(0, cfg, None);
        left.optical(
            1_000_000_000,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [-0.15, -0.1, -0.35],
            },
            &head,
            0.5,
            10,
        );
        // a fix for the left hand right where the right hand is: dropped as a merge, every time (no 3-in-a-row)
        let right_at = [0.12, -0.1, -0.35];
        for k in 1..6u64 {
            let v = left.optical_with(
                1_000_000_000 + k * 40_000_000,
                P {
                    q: [0.0, 0.0, 0.0, 1.0],
                    p: [0.13, -0.1, -0.35],
                },
                &head,
                0.5,
                10,
                Some(right_at),
            );
            assert_eq!(v, "merged");
        }
        // a left-hand fix far on the right side of the body: dropped as "side"
        let v = left.optical_with(
            1_300_000_000,
            P {
                q: [0.0, 0.0, 0.0, 1.0],
                p: [0.45, -0.1, -0.3],
            },
            &head,
            0.5,
            10,
            None,
        );
        assert_eq!(v, "side");
        // held out of view, the right hand comes to the held place: the held left hand is pushed 8 cm away
        let (s, p) = left.get_with(2_000_000_000, &head, Some([-0.15, -0.1, -0.35]));
        assert_eq!(s, State::Hold);
        let d = norm(sub(p.p, [-0.15, -0.1, -0.35]));
        assert!((d - 0.08).abs() < 1e-9, "separation {d}");
        assert!(p.p[0] < -0.15, "pushed to the left side: {:?}", p.p);
        // the hold never leaves the reach sphere, even when the head moves away
        let far_head = P {
            q: [0.0, 0.0, 0.0, 1.0],
            p: [0.0, 0.0, -2.0],
        };
        let (_, p) = left.get_with(2_100_000_000, &far_head, None);
        assert!(norm(sub(p.p, left.shoulder(&far_head))) <= 0.75 + 1e-9);
    }

    /// Gyro samples (raw LSB) for a rotation of `w_dps` about aim +x with the built-in left calibration.
    fn raw_for(c: &Calib, w_aim: V3) -> [i16; 3] {
        let ci = inv3(&c.c).unwrap();
        let r = add(mv(&ci, w_aim), c.bias);
        [r[0].round() as i16, r[1].round() as i16, r[2].round() as i16]
    }

    #[test]
    fn forearm_model_swings_the_held_controller_around_the_elbow() {
        let cal = default_calib(0);
        let f = [0.0, 0.0, -0.3];
        let cfg = Config { forearm: Some(f), follow_yaw: 1.0, still_lock: false, ..Config::default() };
        let mut fu = Fuser::new(0, cfg, Some(cal));
        let head = P::ID;
        let (mut t, mut dev) = (1_000_000_000u64, 0u32);
        // tracked and still for 0.5 s, then out of view while the forearm pitches up 90 deg in 1 s
        for k in 0..1000u64 {
            let w = if (1_500_000_000..2_500_000_000).contains(&t) { [std::f64::consts::FRAC_PI_2, 0.0, 0.0] } else { [0.0; 3] };
            fu.sample(t, dev, raw_for(&cal, w));
            if k % 20 == 0 && t < 1_500_000_000 {
                fu.optical(t, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.1, -0.2, -0.4] }, &head, 0.5, 12);
            }
            t += 2_000_000;
            dev += 2000;
        }
        let (s, p) = fu.get(2_600_000_000, &head);
        assert_eq!(s, State::Hold);
        // elbow at (-0.1, -0.2, -0.1); the controller now 0.3 m above it
        assert!((p.p[1] - 0.1).abs() < 0.03 && (p.p[2] + 0.1).abs() < 0.03, "{:?}", p.p);
        // without the forearm the old hold keeps the position
        let mut old = Fuser::new(0, Config { forearm: None, still_lock: false, ..cfg }, Some(cal));
        let (mut t, mut dev) = (1_000_000_000u64, 0u32);
        for k in 0..1000u64 {
            let w = if (1_500_000_000..2_500_000_000).contains(&t) { [std::f64::consts::FRAC_PI_2, 0.0, 0.0] } else { [0.0; 3] };
            old.sample(t, dev, raw_for(&cal, w));
            if k % 20 == 0 && t < 1_500_000_000 {
                old.optical(t, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.1, -0.2, -0.4] }, &head, 0.5, 12);
            }
            t += 2_000_000;
            dev += 2000;
        }
        let (_, q) = old.get(2_600_000_000, &head);
        assert!(norm(sub(q.p, [-0.1, -0.2, -0.4])) < 0.01, "{:?}", q.p);
    }

    #[test]
    fn the_hold_follows_half_the_head_yaw_by_default() {
        let cfg = Config { use_imu: Some(false), ..Config::default() };
        assert_eq!(cfg.follow_yaw, 0.5);
        let mut fu = Fuser::new(1, cfg, None);
        let head = P::ID;
        for k in 0..10u64 {
            fu.optical(1_000_000_000 + k * 40_000_000, P { q: [0.0, 0.0, 0.0, 1.0], p: [0.0, 0.0, -0.4] }, &head, 0.5, 10);
        }
        let _ = fu.get(1_700_000_000, &head);
        // head turns 90 deg left: the held controller turns 45 deg with it
        let s = std::f64::consts::FRAC_PI_4;
        let turned = P { q: [0.0, s.sin(), 0.0, s.cos()], p: [0.0; 3] };
        let (st, p) = fu.get(1_800_000_000, &turned);
        assert_eq!(st, State::Hold);
        let a = (-p.p[0]).atan2(-p.p[2]).to_degrees();
        assert!((a - 45.0).abs() < 1.0, "turned {a} deg");
    }

    #[test]
    fn a_big_reacquire_correction_slides_at_most_half_a_metre_per_second() {
        let cfg = Config { use_imu: Some(false), ..Config::default() };
        let mut fu = Fuser::new(0, cfg, None);
        let head = P::ID;
        fu.optical(1_000_000_000, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.1, 0.0, -0.4] }, &head, 0.5, 10);
        fu.optical(1_040_000_000, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.1, 0.0, -0.4] }, &head, 0.5, 10);
        let _ = fu.get(1_500_000_000, &head);
        // back in view 0.2 m further left: the correction takes 0.4 s instead of 0.15 s
        fu.optical(2_000_000_000, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.3, 0.0, -0.4] }, &head, 0.5, 10);
        let q0 = 2_050_000_000;
        let (_, a) = fu.get(q0, &head);
        let (_, b) = fu.get(q0 + 200_000_000, &head);
        let (_, c) = fu.get(q0 + 410_000_000, &head);
        assert!((a.p[0] + 0.1).abs() < 0.01, "starts at the shown pose {}", a.p[0]);
        assert!((b.p[0] + 0.2).abs() < 0.015, "halfway at 0.2 s {}", b.p[0]);
        assert!((c.p[0] + 0.3).abs() < 0.005, "ends at the fix {}", c.p[0]);
    }

    #[test]
    fn built_in_calibration_matches_the_fitted_axes_and_scale() {
        for (h, axes) in [(0, "-z -x +y"), (1, "+z +x +y")] {
            let c = default_calib(h);
            assert_eq!(c.axes(), axes);
            assert!((c.scale_dps_per_lsb() - 0.122).abs() < 0.002, "{}", c.scale_dps_per_lsb());
            assert!(c.axis_misalignment_deg() < 12.0);
        }
        // a missing file gives the built-in one
        let p = std::env::temp_dir().join(format!("pimax-ctrl-imu-none-{}.json", std::process::id()));
        assert_eq!(calib_or_default(&p, 1).map(|c| c.c), Some(default_calib(1).c));
    }

    #[test]
    fn a_flipped_few_led_solve_keeps_its_position_but_not_its_orientation() {
        // gyro forced on (Monado's guess), controller not rotating: raw gyro 0
        let cfg = Config { use_imu: Some(true), ..Config::default() };
        let mut fu = Fuser::new(0, cfg, None);
        let head = P::ID;
        let mut t = 1_000_000_000u64;
        let mut dev = 0u32;
        let mut k = 0;
        while t < 2_000_000_000 {
            fu.sample(t, dev, [0, 0, 0]);
            if k % 20 == 0 && t < 1_500_000_000 {
                fu.optical(t, P { q: [0.0, 0.0, 0.0, 1.0], p: [-0.1, -0.1, -0.35] }, &head, 0.5, 12);
            }
            t += 2_000_000;
            dev += 2000;
            k += 1;
        }
        // re-entry with a 6-LED solve flipped 180 deg about y, 2 cm to the left
        let flipped = P { q: [0.0, 1.0, 0.0, 0.0], p: [-0.12, -0.1, -0.35] };
        let v = fu.optical_with(2_000_000_000, flipped, &head, 0.8, 6, None);
        assert_eq!(v, "pos-only");
        let _ = fu.get(2_050_000_000, &head); // first display request starts the re-entry blend
        let (_, p) = fu.get(2_300_000_000, &head);
        assert!(qangle(p.q, [0.0, 0.0, 0.0, 1.0]).to_degrees() < 1.0, "orientation kept from the gyro");
        assert!((p.p[0] + 0.12).abs() < 0.01, "position from the fix: {:?}", p.p);
        // a 12-LED solve that disagrees is believed after 3 in a row
        let mut last = "";
        for i in 0..3u64 {
            last = fu.optical_with(2_440_000_000 + i * 40_000_000, flipped, &head, 0.5, 12, None);
        }
        assert_eq!(last, "ok");
    }
}
