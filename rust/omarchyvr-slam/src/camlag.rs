//! Camera-IMU time offset, gyro bias and gyro gain from a EuRoC recording alone (no tracker, no reference).
//!
//! Each pair of consecutive frames of one camera gives the head rotation between them (ORB matches turned into
//! bearings with the factory KB4 model, RANSAC rotation-only fit, mapped into the body frame with `T_imu_cam`).
//! The gyro integrated over the same interval, shifted by a lag `L`, must give the same rotation vector. For every
//! `L` in the scan the gyro bias is solved in closed form (least squares), the `L` with the smallest residual wins.
//! `L` is in Basalt's convention: `t_imu = t_cam + L` (`cam_time_offset_ns`).
//!
//! The gain (image rotation / gyro rotation) is not the gyro scale: the cameras sit about 15 cm in front of the
//! neck, so a head turn also moves them sideways and near points add parallax in the same direction. On the
//! Crystal Super recording of 2026-10-18 the gain was 1.12 for features near the image centre and 1.05 near the
//! edge (where that motion runs along the line of sight), while gravity seen by the accelerometer put the gyro
//! scale at 1.00-1.03. Lag and bias are not affected (the parallax is in phase with the rotation).

use crate::euroc::{Dataset, ImuRow};
use crate::takehead::ransac_rotation_q;
use omarchyvr_room::orb::{self, Descriptor, Image, OrbParams};
use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Mat3, Vec3};
use pimax_cam::kb4::Kb4;
use pimax_cam::slam;
use std::fs::File;
use std::path::Path;

/// Head rotation between two frames, seen by the camera.
#[derive(Clone, Debug, PartialEq)]
pub struct PairRot {
    /// Frame timestamps (ns, recording clock).
    pub t0: i64,
    /// See `t0`.
    pub t1: i64,
    /// Rotation vector of the body from frame 0 to frame 1, in the body frame at frame 0 (rad).
    pub theta: Vec3,
    /// RANSAC inliers.
    pub inliers: usize,
}

/// Rotation vector of a unit quaternion (x y z w).
pub fn quat_log(q: [f64; 4]) -> Vec3 {
    // q and -q are the same rotation: take the one with w >= 0 (angle up to pi)
    let q = if q[3] < 0.0 { q.map(|x| -x) } else { q };
    let v = [q[0], q[1], q[2]];
    let s = geom::norm(v);
    if s < 1e-12 {
        return geom::scale(v, 2.0);
    }
    geom::scale(v, 2.0 * s.atan2(q[3]) / s)
}

/// Body rotation from the camera's bearing fit `b ~ q a` (a in frame 0, b in frame 1 of a static scene):
/// the camera turned by `q^-1`, mapped into the body frame by `r_bc` (body_from_cam rotation).
pub fn body_rotation(q: [f64; 4], r_bc: &Mat3) -> Vec3 {
    geom::mat_vec(r_bc, geom::scale(quat_log(q), -1.0))
}

fn read_png(path: &Path, buf: &mut Vec<u8>) -> Result<(), String> {
    let f = File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = png::Decoder::new(std::io::BufReader::new(f))
        .read_info()
        .map_err(|e| format!("{}: {e}", path.display()))?;
    buf.resize(r.output_buffer_size().unwrap_or(0), 0);
    let info = r
        .next_frame(buf)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if info.width != 640 || info.height != 480 || info.color_type != png::ColorType::Grayscale {
        return Err(format!(
            "{}: {}x{} {:?}, want 640x480 grayscale",
            path.display(),
            info.width,
            info.height,
            info.color_type
        ));
    }
    buf.truncate(info.buffer_size());
    Ok(())
}

struct Feat {
    bearings: Vec<Option<Vec3>>,
    desc: Vec<Descriptor>,
}

fn features(buf: &[u8], k: &Kb4, p: &OrbParams, radius: (f64, f64)) -> Feat {
    let Some(img) = Image::new(640, 480, buf) else {
        return Feat {
            bearings: Vec::new(),
            desc: Vec::new(),
        };
    };
    let (cx, cy) = (k.cx, k.cy);
    let keep = |x: f32, y: f32| {
        let r = ((x as f64 - cx).powi(2) + (y as f64 - cy).powi(2)).sqrt();
        r >= radius.0 && r < radius.1
    };
    let f = orb::extract(&img, Some(&keep), p);
    Feat {
        bearings: f
            .iter()
            .map(|(kp, _)| k.unproject(kp.x as f64, kp.y as f64).map(geom::normalize))
            .collect(),
        desc: f.iter().map(|(_, d)| *d).collect(),
    }
}

/// Camera rotations of every `every`-th consecutive frame pair of camera `cam`, from features whose distance to
/// the principal point is in `radius` (pixels, [min, max)).
pub fn image_rotations(
    ds: &Dataset,
    cal: &DeviceCalib,
    cam: usize,
    every: usize,
    radius: (f64, f64),
) -> Result<Vec<PairRot>, String> {
    let frames = ds.cams.get(cam).ok_or(format!("no cam{cam} in the recording"))?;
    let k = &cal
        .cameras
        .get(cam)
        .ok_or(format!("no camera {cam} in the calibration"))?
        .intr;
    let r_bc = slam::t_imu_cam(cal, cam)
        .ok_or("no camera-IMU extrinsic in the calibration")?
        .r;
    let p = OrbParams {
        n_features: 600,
        ..OrbParams::default()
    };
    let dir = ds.root.join(format!("mav0/cam{cam}/data"));
    let mut buf = Vec::new();
    let mut prev: Option<(usize, Feat)> = None;
    let mut out = Vec::new();
    let every = every.max(1);
    for i in (0..frames.len().saturating_sub(1)).step_by(every) {
        let a = match prev.take() {
            Some((j, f)) if j == i => f,
            _ => {
                read_png(&dir.join(&frames[i].1), &mut buf)?;
                features(&buf, k, &p, radius)
            }
        };
        read_png(&dir.join(&frames[i + 1].1), &mut buf)?;
        let b = features(&buf, k, &p, radius);
        let pairs: Vec<(Vec3, Vec3)> = orb::match_bf(&a.desc, &b.desc, 64, 0.85)
            .iter()
            .filter_map(|&(x, y, _)| Some((a.bearings[x]?, b.bearings[y]?)))
            .collect();
        if let Some((q, n)) = ransac_rotation_q(&pairs, 1.0) {
            out.push(PairRot {
                t0: frames[i].0,
                t1: frames[i + 1].0,
                theta: body_rotation(q, &r_bc),
                inliers: n,
            });
        }
        prev = Some((i + 1, b));
    }
    Ok(out)
}

/// Running integral of the gyro (trapezoid), sampled at the IMU timestamps.
pub struct GyroIntegral {
    ts: Vec<f64>,
    c: Vec<Vec3>,
}

impl GyroIntegral {
    /// Build from IMU rows (sorted by time).
    pub fn new(imu: &[ImuRow]) -> GyroIntegral {
        let mut ts = Vec::with_capacity(imu.len());
        let mut c = Vec::with_capacity(imu.len());
        let mut acc = [0.0; 3];
        for (i, r) in imu.iter().enumerate() {
            if i > 0 {
                let dt = (r.ts - imu[i - 1].ts) as f64 * 1e-9;
                for k in 0..3 {
                    acc[k] += 0.5 * (r.gyro[k] + imu[i - 1].gyro[k]) * dt;
                }
            }
            ts.push(r.ts as f64);
            c.push(acc);
        }
        GyroIntegral { ts, c }
    }

    /// Integral from the first sample to `t` (ns), linear between samples; `None` outside the recording.
    pub fn at(&self, t: f64) -> Option<Vec3> {
        if self.ts.len() < 2 || t < self.ts[0] || t > *self.ts.last()? {
            return None;
        }
        let i = self.ts.partition_point(|&x| x <= t).clamp(1, self.ts.len() - 1);
        let (t0, t1) = (self.ts[i - 1], self.ts[i]);
        let f = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
        let (a, b) = (self.c[i - 1], self.c[i]);
        Some([0, 1, 2].map(|k| a[k] + (b[k] - a[k]) * f))
    }

    /// Gyro rotation vector over [t0, t1] (ns), small-angle (sum of rates).
    pub fn between(&self, t0: f64, t1: f64) -> Option<Vec3> {
        Some(geom::sub(self.at(t1)?, self.at(t0)?))
    }
}

/// Result of the lag scan.
#[derive(Clone, Debug)]
pub struct LagFit {
    /// Best lag (ms), `t_imu = t_cam + lag`.
    pub lag_ms: f64,
    /// Gyro bias at the best lag (rad/s, body frame).
    pub bias: Vec3,
    /// Image rotation / bias-corrected gyro rotation. Above 1 in a room from parallax (module doc), not a gyro scale.
    pub gain: f64,
    /// The same per body axis.
    pub gain_axis: Vec3,
    /// RMS residual per pair at the best lag (deg).
    pub rms_deg: f64,
    /// Pairs used.
    pub n: usize,
    /// (lag ms, RMS deg) of the scan.
    pub curve: Vec<(f64, f64)>,
}

fn residual(pairs: &[&PairRot], g: &GyroIntegral, lag_ns: f64) -> Option<(f64, Vec3, usize)> {
    let mut rows = Vec::with_capacity(pairs.len());
    for p in pairs {
        let gi = g.between(p.t0 as f64 + lag_ns, p.t1 as f64 + lag_ns)?;
        rows.push((p.theta, gi, (p.t1 - p.t0) as f64 * 1e-9));
    }
    // theta = G - b dt  ->  b = sum dt (G - theta) / sum dt^2
    let (mut num, mut den) = ([0.0; 3], 0.0);
    for (th, gi, dt) in &rows {
        for k in 0..3 {
            num[k] += dt * (gi[k] - th[k]);
        }
        den += dt * dt;
    }
    let b = num.map(|x| x / den.max(1e-30));
    let mut cost = 0.0;
    for (th, gi, dt) in &rows {
        let r = geom::sub(*th, geom::sub(*gi, geom::scale(b, *dt)));
        cost += geom::dot(r, r);
    }
    Some((cost, b, rows.len()))
}

/// Scan lags in `[-range_ms, range_ms]` (step `step_ms`), drop pairs with few inliers, one outlier pass.
pub fn fit_lag(pairs: &[PairRot], imu: &[ImuRow], range_ms: f64, step_ms: f64) -> Option<LagFit> {
    let g = GyroIntegral::new(imu);
    let mut use_: Vec<&PairRot> = pairs.iter().filter(|p| p.inliers >= 20).collect();
    let mut best;
    let mut curve = Vec::new();
    let mut pass = 0;
    loop {
        best = (f64::MAX, 0.0, [0.0; 3]);
        curve.clear();
        let n = (range_ms / step_ms).round() as i64;
        for i in -n..=n {
            let lag = i as f64 * step_ms;
            if let Some((c, b, m)) = residual(&use_, &g, lag * 1e6) {
                curve.push((lag, (c / m.max(1) as f64).sqrt().to_degrees()));
                if c < best.0 {
                    best = (c, lag, b);
                }
            }
        }
        if pass == 0 {
            // drop pairs whose residual at the first optimum is above 3x the median (bad fits, moving objects)
            let mut res: Vec<(f64, &PairRot)> = use_
                .iter()
                .filter_map(|p| {
                    let gi = g.between(p.t0 as f64 + best.1 * 1e6, p.t1 as f64 + best.1 * 1e6)?;
                    let dt = (p.t1 - p.t0) as f64 * 1e-9;
                    Some((geom::norm(geom::sub(p.theta, geom::sub(gi, geom::scale(best.2, dt)))), *p))
                })
                .collect();
            let mut v: Vec<f64> = res.iter().map(|r| r.0).collect();
            v.sort_by(f64::total_cmp);
            let med = *v.get(v.len() / 2)?;
            res.retain(|r| r.0 <= 3.0 * med.max(1e-6));
            use_ = res.into_iter().map(|r| r.1).collect();
        }
        pass += 1;
        if pass == 2 {
            break;
        }
    }
    // parabola through the best scan point and its neighbours
    let i = curve.iter().position(|c| c.1 == best_rms(&curve))?;
    let mut lag = curve[i].0;
    if i > 0 && i + 1 < curve.len() {
        let (y0, y1, y2) = (curve[i - 1].1.powi(2), curve[i].1.powi(2), curve[i + 1].1.powi(2));
        let d = y0 - 2.0 * y1 + y2;
        if d > 0.0 {
            lag += 0.5 * (y0 - y2) / d * step_ms;
        }
    }
    let (c, b, m) = residual(&use_, &g, lag * 1e6)?;
    let (mut num, mut den) = (0.0, 0.0);
    let (mut na, mut da) = ([0.0; 3], [0.0; 3]);
    for p in &use_ {
        let gi = g.between(p.t0 as f64 + lag * 1e6, p.t1 as f64 + lag * 1e6)?;
        let gc = geom::sub(gi, geom::scale(b, (p.t1 - p.t0) as f64 * 1e-9));
        num += geom::dot(p.theta, gc);
        den += geom::dot(gc, gc);
        for k in 0..3 {
            na[k] += p.theta[k] * gc[k];
            da[k] += gc[k] * gc[k];
        }
    }
    Some(LagFit {
        lag_ms: lag,
        bias: b,
        gain: num / den.max(1e-30),
        gain_axis: [0, 1, 2].map(|k| na[k] / da[k].max(1e-30)),
        rms_deg: (c / m.max(1) as f64).sqrt().to_degrees(),
        n: m,
        curve,
    })
}

fn best_rms(curve: &[(f64, f64)]) -> f64 {
    curve.iter().map(|c| c.1).fold(f64::MAX, f64::min)
}

/// The whole command: rotations of camera `cam`, the lag scan, a text report.
pub fn run(
    ds: &Dataset,
    cal: &DeviceCalib,
    cam: usize,
    every: usize,
    range_ms: f64,
    radius: (f64, f64),
) -> Result<String, String> {
    let pairs = image_rotations(ds, cal, cam, every, radius)?;
    let fit = fit_lag(&pairs, &ds.imu, range_ms, 0.25).ok_or("not enough frame pairs with a rotation fit")?;
    let mut s = format!(
        "camera {cam}: {} frame pairs with a rotation fit, {} used\n\
         camera-IMU lag   {:+.2} ms (t_imu = t_cam + lag; Basalt cam_time_offset_ns {:+.0})\n\
         gyro bias        ({:+.4} {:+.4} {:+.4}) rad/s = ({:+.3} {:+.3} {:+.3}) deg/s, body frame\n\
         gain image/gyro  {:.4} (body x {:.4}, y {:.4}, z {:.4}; includes parallax, not a gyro scale)\n\
         residual         {:.3} deg RMS per pair\n",
        pairs.len(),
        fit.n,
        fit.lag_ms,
        fit.lag_ms * 1e6,
        fit.bias[0],
        fit.bias[1],
        fit.bias[2],
        fit.bias[0].to_degrees(),
        fit.bias[1].to_degrees(),
        fit.bias[2].to_degrees(),
        fit.gain,
        fit.gain_axis[0],
        fit.gain_axis[1],
        fit.gain_axis[2],
        fit.rms_deg
    );
    s.push_str("scan (lag ms: RMS deg):");
    for (lag, r) in fit.curve.iter().filter(|c| (c.0 / 2.0).fract() == 0.0) {
        s.push_str(&format!(" {lag:+.0}:{r:.3}"));
    }
    s.push('\n');
    Ok(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn quat_from_rotvec(w: Vec3) -> [f64; 4] {
        let a = geom::norm(w);
        if a < 1e-15 {
            return [0.0, 0.0, 0.0, 1.0];
        }
        let s = (a / 2.0).sin() / a;
        [w[0] * s, w[1] * s, w[2] * s, (a / 2.0).cos()]
    }

    #[test]
    fn quat_log_inverts_rotvec() {
        let w = [0.1, -0.2, 0.05];
        let l = quat_log(quat_from_rotvec(w));
        assert!(geom::norm(geom::sub(l, w)) < 1e-12);
        let neg = quat_from_rotvec(w).map(|x| -x);
        assert!(geom::norm(geom::sub(quat_log(neg), w)) < 1e-12);
    }

    #[test]
    fn camera_turn_is_the_inverse_of_the_bearing_fit() {
        // camera turns by +0.1 rad about its y: a static point's bearing turns the other way
        let turn = geom::rodrigues([0.0, 0.1, 0.0]);
        let pts = [[0.3, 0.1, 1.0], [-0.4, 0.2, 1.0], [0.1, -0.3, 1.0], [0.5, 0.4, 1.0], [-0.2, -0.5, 1.0], [0.0, 0.0, 1.0], [0.6, -0.1, 1.0]];
        let pairs: Vec<(Vec3, Vec3)> = pts
            .iter()
            .map(|&p| {
                let a = geom::normalize(p);
                (a, geom::mat_vec(&geom::transpose(&turn), a))
            })
            .collect();
        let (q, n) = ransac_rotation_q(&pairs, 0.5).unwrap();
        assert_eq!(n, pts.len());
        let id = geom::rodrigues([0.0, 0.0, 0.0]);
        let th = body_rotation(q, &id);
        assert!(geom::norm(geom::sub(th, [0.0, 0.1, 0.0])) < 1e-6, "{th:?}");
    }

    fn imu_const(rate: Vec3, n: usize, dt_ns: i64) -> Vec<ImuRow> {
        (0..n)
            .map(|i| ImuRow {
                ts: i as i64 * dt_ns,
                gyro: rate,
                accel: [0.0, 9.81, 0.0],
            })
            .collect()
    }

    #[test]
    fn gyro_integral_is_rate_times_time() {
        let g = GyroIntegral::new(&imu_const([0.5, 0.0, -1.0], 1001, 1_000_000));
        let r = g.between(100e6, 300.5e6).unwrap();
        assert!((r[0] - 0.10025).abs() < 1e-9 && (r[2] + 0.2005).abs() < 1e-9, "{r:?}");
        assert!(g.at(-1.0).is_none());
    }

    #[test]
    fn lag_and_bias_recovered_on_synthetic_motion() {
        // head yaw and pitch rates, frame times 7 ms off the IMU clock (t_imu = t_cam + 7 ms), gyro bias on z
        let dt = 1_000_000i64;
        let lag_ns = 7_000_000.0;
        let bias = [0.0, 0.0, 0.02];
        let rate = |t: f64| [0.3 * (2.0 * std::f64::consts::PI * 1.3 * t).sin(), 0.8 * (2.0 * std::f64::consts::PI * 0.7 * t).sin(), 0.0];
        let imu: Vec<ImuRow> = (0..20_000)
            .map(|i| {
                let t = i as f64 * 1e-3;
                let w = rate(t);
                ImuRow {
                    ts: i * dt,
                    gyro: geom::add(w, bias),
                    accel: [0.0, 9.81, 0.0],
                }
            })
            .collect();
        let truth = GyroIntegral::new(
            &(0..20_000)
                .map(|i| ImuRow {
                    ts: i * dt,
                    gyro: rate(i as f64 * 1e-3),
                    accel: [0.0; 3],
                })
                .collect::<Vec<_>>(),
        );
        let pairs: Vec<PairRot> = (1..500)
            .map(|k| {
                let (t0, t1) = (k as i64 * 33_333_333, (k + 1) as i64 * 33_333_333);
                PairRot {
                    t0,
                    t1,
                    theta: truth.between(t0 as f64 + lag_ns, t1 as f64 + lag_ns).unwrap(),
                    inliers: 100,
                }
            })
            .collect();
        let f = fit_lag(&pairs, &imu, 20.0, 0.25).unwrap();
        assert!((f.lag_ms - 7.0).abs() < 0.1, "{f:?}");
        assert!(geom::norm(geom::sub(f.bias, bias)) < 1e-4, "{f:?}");
        assert!((f.gain - 1.0).abs() < 1e-3, "{f:?}");
    }
}
