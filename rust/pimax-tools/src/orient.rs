//! Mahony orientation filter and IMU calibration, a line-by-line port of `linux/pimax_imu_orient.py`.
//! The unit tests compare against quaternions produced by the Python implementation on the real p83 capture.
use pimax_proto::Report;

/// Accelerometer LSB per g.
pub const ACC_LSB: f64 = 2048.0;
/// Gyro LSB per deg/s.
pub const GYR_LSB: f64 = 16.4;
const G: f64 = 9.80665;
const D2R: f64 = std::f64::consts::PI / 180.0;

/// Factory calibration terms from `headset-calibration.xml`.
#[derive(Debug, Clone, PartialEq)]
pub struct Calib {
    /// Accelerometer bias (m/s^2).
    pub a_bias: [f64; 3],
    /// Gyro bias (rad/s).
    pub w_bias: [f64; 3],
    /// Accelerometer scale error.
    pub ka: [f64; 3],
    /// Gyro scale error.
    pub kg: [f64; 3],
}

fn attr(text: &str, key: &str) -> Option<[f64; 3]> {
    let pat = format!("{key}=\"");
    let i = text.find(&pat)? + pat.len();
    let v: Vec<f64> = text[i..i + text[i..].find('"')?]
        .split_whitespace()
        .map(|x| x.parse().ok())
        .collect::<Option<_>>()?;
    Some([*v.first()?, *v.get(1)?, *v.get(2)?])
}

/// Parse `aBias`, `wBias`, `ka`, `kg` (first occurrence of each attribute).
pub fn parse_calib(xml: &str) -> Option<Calib> {
    Some(Calib {
        a_bias: attr(xml, "aBias")?,
        w_bias: attr(xml, "wBias")?,
        ka: attr(xml, "ka")?,
        kg: attr(xml, "kg")?,
    })
}

/// Apply scale and bias (sensor frame).
pub fn correct(acc: [f64; 3], gyr: [f64; 3], cal: Option<&Calib>) -> ([f64; 3], [f64; 3]) {
    let Some(c) = cal else { return (acc, gyr) };
    let mut a = [0.0; 3];
    let mut w = [0.0; 3];
    for i in 0..3 {
        a[i] = (acc[i] * G - c.a_bias[i]) / (1.0 + c.ka[i]) / G;
        w[i] = (gyr[i] * D2R - c.w_bias[i]) / (1.0 + c.kg[i]) / D2R;
    }
    (a, w)
}

/// Sensor frame to OpenXR-like frame. Mode 0 = identity, mode 1 = 180 degrees about Y (X and Z negated);
/// mode 1 is what the Monado driver uses (verified with a nose-down tilt test, analysis/imu-orientation.md).
pub fn axis_map(mode: i32, v: [f64; 3]) -> [f64; 3] {
    if mode == 1 {
        [-v[0], v[1], -v[2]]
    } else {
        v
    }
}

/// One IMU sample with an unwrapped microsecond clock.
#[derive(Debug, Clone, Copy)]
pub struct Sample {
    /// Unwrapped device time, microseconds.
    pub t_us: u64,
    /// g, sensor frame, uncorrected.
    pub acc: [f64; 3],
    /// deg/s, sensor frame, uncorrected.
    pub gyro: [f64; 3],
}

/// Turns IMU reports into samples, handling the 32-bit timestamp wrap (every 71.6 min).
#[derive(Default)]
pub struct Unwrap {
    wraps: u64,
    last: Option<u32>,
}

impl Unwrap {
    /// Samples of a report, or `None` if it is not an IMU report.
    pub fn push(&mut self, d: &Report) -> Option<[Sample; 2]> {
        if d[0] != 0x53 || d[1] != 0x31 {
            return None;
        }
        let ts = u32::from_le_bytes([d[4], d[5], d[6], d[7]]);
        if let Some(l) = self.last {
            if (ts as i64) < l as i64 - (1i64 << 31) {
                self.wraps += 1;
            }
        }
        self.last = Some(ts);
        let mut out = [Sample {
            t_us: 0,
            acc: [0.0; 3],
            gyro: [0.0; 3],
        }; 2];
        for (k, o) in out.iter_mut().enumerate() {
            let b = 12 + 16 * k + 2;
            let v = |j: usize| f64::from(i16::from_be_bytes([d[b + 2 * j], d[b + 2 * j + 1]]));
            o.t_us = u64::from(ts) + (self.wraps << 32) + 1000 * k as u64;
            o.acc = [v(0) / ACC_LSB, v(1) / ACC_LSB, v(2) / ACC_LSB];
            o.gyro = [v(3) / GYR_LSB, v(4) / GYR_LSB, v(5) / GYR_LSB];
        }
        Some(out)
    }
}

/// Quaternion `q = (w, x, y, z)` = sensor to world, world up = +Y; accelerometer-gated tilt correction.
pub struct Mahony {
    /// Orientation.
    pub q: [f64; 4],
    kp: f64,
    ki: f64,
    ei: [f64; 3],
}

impl Mahony {
    /// New filter.
    pub fn new(kp: f64, ki: f64) -> Self {
        Self {
            q: [1.0, 0.0, 0.0, 0.0],
            kp,
            ki,
            ei: [0.0; 3],
        }
    }

    /// Seed the tilt from the accelerometer so convergence is instant.
    pub fn seed(&mut self, ac: [f64; 3]) {
        let n = (ac.iter().map(|x| x * x).sum::<f64>()).sqrt();
        let n = if n == 0.0 { 1.0 } else { n };
        let (ux, uy, uz) = (ac[0] / n, ac[1] / n, ac[2] / n);
        let w = 1.0 + uy;
        let (qx, qz) = (-uz, ux);
        let m = (w * w + qx * qx + qz * qz).sqrt();
        let m = if m == 0.0 { 1.0 } else { m };
        self.q = [w / m, qx / m, 0.0, qz / m];
    }

    /// One step: `g` in deg/s, `a` in g, `dt` in seconds.
    pub fn update(&mut self, g: [f64; 3], a: [f64; 3], dt: f64) {
        let [q0, q1, q2, q3] = self.q;
        let n = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let (mut ex, mut ey, mut ez) = (0.0, 0.0, 0.0);
        if 0.7 < n && n < 1.3 && g.iter().map(|x| x.abs()).sum::<f64>() < 150.0 {
            let (ax, ay, az) = (a[0] / n, a[1] / n, a[2] / n);
            let vx = 2.0 * (q1 * q2 + q0 * q3);
            let vy = q0 * q0 - q1 * q1 + q2 * q2 - q3 * q3;
            let vz = 2.0 * (q2 * q3 - q0 * q1);
            ex = ay * vz - az * vy;
            ey = az * vx - ax * vz;
            ez = ax * vy - ay * vx;
        }
        if self.ki != 0.0 {
            for (i, e) in [ex, ey, ez].into_iter().enumerate() {
                self.ei[i] += self.ki * e * dt;
            }
        }
        let gx = g[0] * D2R + self.kp * ex + self.ei[0];
        let gy = g[1] * D2R + self.kp * ey + self.ei[1];
        let gz = g[2] * D2R + self.kp * ez + self.ei[2];
        let h = 0.5 * dt;
        let q = [
            q0 + h * (-q1 * gx - q2 * gy - q3 * gz),
            q1 + h * (q0 * gx + q2 * gz - q3 * gy),
            q2 + h * (q0 * gy - q1 * gz + q3 * gx),
            q3 + h * (q0 * gz + q1 * gy - q2 * gx),
        ];
        let m = q.iter().map(|x| x * x).sum::<f64>().sqrt();
        self.q = [q[0] / m, q[1] / m, q[2] / m, q[3] / m];
    }

    /// `(yaw, pitch, roll)` in degrees.
    pub fn euler(&self) -> (f64, f64, f64) {
        let [w, x, y, z] = self.q;
        let r = [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ];
        let u = [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ];
        let b = [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ];
        let f = [-b[0], -b[1], -b[2]];
        let yaw = (-f[0]).atan2(-f[2]).to_degrees();
        let pitch = f[1].clamp(-1.0, 1.0).asin().to_degrees();
        let roll = (-r[1]).atan2(u[1]).to_degrees();
        (yaw, pitch, roll)
    }
}
