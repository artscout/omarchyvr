//! Head tracking on a "Record tracking" take (`~/.local/share/pimax/tracking-recordings/<take>/`) without the head
//! IMU: what the live tracker did (the take's `monado.log` slice) and what the cameras saw at speed
//! (`cameras.pxcam`).
//!
//! * Live tracker: 6DoF acquisitions / losses / re-zeros during the take, corrections >= 20 mm (with the head
//!   speed Monado logged), auto-exposure lines.
//! * Cameras, every tracking frame of camera 0 (and 1 when 0 has too little): head rotation between consecutive
//!   tracking frames (40 ms) from ORB matches turned into bearings (KB4) and a RANSAC rotation-only fit. That gives
//!   the head speed over the whole take (the take has no head IMU), and how many features survive at each speed:
//!   the inliers are what an optical-flow front end can still track. Per frame also the exposure, the brightness and
//!   a sharpness measure, so motion blur and darkness at speed show up next to each other.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::camera::Camera;
use pimax_cam::geom::{self, Vec3};
use pimax_cam::kb4::Kb4;
use pimax_cam::source::ReplayOptions;
use pimax_room::orb::{self, Image, OrbParams};
use std::path::Path;

/// Exposure Pimax Play used on Windows the whole session (µs, `analysis/tracking-vs-windows.md`).
pub const PIMAX_EXPOSURE_US: f64 = 10410.0;

/// One tracking frame of the take.
#[derive(Clone, Debug, Default)]
pub struct FrameRow {
    /// Device time (s from the first frame).
    pub t: f64,
    /// Exposure of camera 0 (µs) and gain code.
    pub exposure: u16,
    /// See `exposure`.
    pub gain: u16,
    /// Mean brightness of cameras 0 and 1 (0..255).
    pub mean: f64,
    /// Mean absolute gradient divided by the mean brightness (contrast-normalised sharpness; drops with blur).
    pub sharp: f64,
    /// ORB features found in camera 0.
    pub features: usize,
    /// Head rotation speed since the previous tracking frame (deg/s), None when the fit failed.
    pub speed: Option<f64>,
    /// Matches consistent with that rotation (what a tracker can still follow).
    pub inliers: usize,
}

/// What the live tracker logged during the take.
#[derive(Clone, Debug, Default)]
pub struct LogSummary {
    /// (acquired, lost) at the first and the last `slam:` line.
    pub first: Option<(u64, u64)>,
    /// See `first`.
    pub last: Option<(u64, u64)>,
    /// `tracker re-zeroed while still` lines.
    pub rezeros: usize,
    /// `SqrtKeypointVioEstimator restarted` lines.
    pub restarts: usize,
    /// `6DoF: lost` lines.
    pub lost_lines: Vec<String>,
    /// Corrections >= 20 mm: (mm, head deg/s).
    pub corrections: Vec<(f64, f64)>,
    /// Largest correction in the 5 s `6DoF output:` lines (mm).
    pub max_corr_mm: f64,
    /// AE lines: (lower exposure, lower mean).
    pub ae: Vec<(f64, f64)>,
}

fn after<'a>(l: &'a str, key: &str) -> Option<&'a str> {
    l.find(key).map(|i| &l[i + key.len()..])
}

fn num_at(s: &str) -> Option<f64> {
    let e = s
        .find(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-'))
        .unwrap_or(s.len());
    s[..e].parse().ok()
}

/// Parse the take's `monado.log` slice.
pub fn parse_log(text: &str) -> LogSummary {
    let mut s = LogSummary::default();
    for l in text.lines() {
        if l.contains("] slam:") {
            // "...; 1 acquired, 0 lost, ..."
            if let (Some(a), Some(b)) = (l.find(" acquired, "), l.find(" lost")) {
                let pre = &l[..a];
                let acq = pre.rsplit(' ').next().and_then(num_at);
                let lost = num_at(&l[a + 11..b]);
                if let (Some(a), Some(b)) = (acq, lost) {
                    let v = (a as u64, b as u64);
                    s.first.get_or_insert(v);
                    s.last = Some(v);
                }
            }
        } else if l.contains("re-zeroed while still (position held") {
            s.rezeros += 1;
        } else if l.contains("SqrtKeypointVioEstimator restarted") {
            s.restarts += 1;
        } else if l.contains("6DoF: lost") {
            s.lost_lines.push(l.trim().to_string());
        } else if let Some(r) = after(l, "6DoF correction: ") {
            let mm = num_at(r).unwrap_or(0.0);
            let dps = after(l, " m/s ").and_then(num_at).unwrap_or(f64::NAN);
            s.corrections.push((mm, dps));
        } else if let Some(r) = after(l, "(max ") {
            if l.contains("6DoF output:") {
                s.max_corr_mm = s.max_corr_mm.max(num_at(r).unwrap_or(0.0));
            }
        } else if let Some(r) = after(l, "AE on: lower ") {
            let e = num_at(r).unwrap_or(f64::NAN);
            let m = after(r, "mean ").and_then(num_at).unwrap_or(f64::NAN);
            s.ae.push((e, m));
        }
    }
    s
}

/// Rotation (unit quaternion x y z w) that best maps `a[i]` onto `b[i]` (Horn, no translation).
fn horn(pairs: &[(Vec3, Vec3)]) -> [f64; 4] {
    let mut sm = [[0.0; 3]; 3];
    for (a, b) in pairs {
        for i in 0..3 {
            for j in 0..3 {
                sm[i][j] += a[i] * b[j];
            }
        }
    }
    let (sxx, sxy, sxz) = (sm[0][0], sm[0][1], sm[0][2]);
    let (syx, syy, syz) = (sm[1][0], sm[1][1], sm[1][2]);
    let (szx, szy, szz) = (sm[2][0], sm[2][1], sm[2][2]);
    // order w x y z
    let n = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let shift = pairs.len() as f64 * 3.0;
    let mut v = [1.0, 0.0, 0.0, 0.0];
    for _ in 0..200 {
        let mut w = [0.0; 4];
        for i in 0..4 {
            for j in 0..4 {
                w[i] += (n[i][j] + if i == j { shift } else { 0.0 }) * v[j];
            }
        }
        let l = (w.iter().map(|x| x * x).sum::<f64>()).sqrt().max(1e-30);
        v = [w[0] / l, w[1] / l, w[2] / l, w[3] / l];
    }
    [v[1], v[2], v[3], v[0]]
}

fn qrot(q: [f64; 4], v: Vec3) -> Vec3 {
    let u = [q[0], q[1], q[2]];
    let t = geom::scale(geom::cross(u, v), 2.0);
    geom::add(geom::add(v, geom::scale(t, q[3])), geom::cross(u, t))
}

fn q_angle_deg(q: [f64; 4]) -> f64 {
    2.0 * q[3].abs().clamp(0.0, 1.0).acos().to_degrees()
}

/// Simple deterministic generator for RANSAC.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self, n: usize) -> usize {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as usize) % n.max(1)
    }
}

/// Rotation between two bearing sets (matched): (angle deg, inliers). Inliers within `thr_deg`.
pub fn ransac_rotation(pairs: &[(Vec3, Vec3)], thr_deg: f64) -> Option<(f64, usize)> {
    if pairs.len() < 6 {
        return None;
    }
    let cthr = thr_deg.to_radians().cos();
    let mut rng = Lcg(12345);
    let mut best: (usize, [f64; 4]) = (0, [0.0, 0.0, 0.0, 1.0]);
    let count = |q: [f64; 4]| {
        pairs
            .iter()
            .filter(|(a, b)| geom::dot(qrot(q, *a), *b) > cthr)
            .count()
    };
    for _ in 0..200 {
        let i = rng.next(pairs.len());
        let j = rng.next(pairs.len());
        if i == j || geom::dot(pairs[i].0, pairs[j].0) > 0.995 {
            continue;
        }
        let q = horn(&[pairs[i], pairs[j]]);
        let n = count(q);
        if n > best.0 {
            best = (n, q);
        }
    }
    if best.0 < 6 {
        return None;
    }
    // refine on the inliers
    let inl: Vec<(Vec3, Vec3)> = pairs
        .iter()
        .filter(|(a, b)| geom::dot(qrot(best.1, *a), *b) > cthr)
        .copied()
        .collect();
    let q = horn(&inl);
    Some((q_angle_deg(q), count(q)))
}

struct Feat {
    bearings: Vec<Option<Vec3>>,
    desc: Vec<orb::Descriptor>,
}

fn features(buf: &[u8], k: &Kb4, p: &OrbParams) -> Feat {
    let Some(img) = Image::new(640, 480, buf) else {
        return Feat {
            bearings: Vec::new(),
            desc: Vec::new(),
        };
    };
    let f = orb::extract(&img, None, p);
    Feat {
        bearings: f
            .iter()
            .map(|(kp, _)| k.unproject(kp.x as f64, kp.y as f64).map(geom::normalize))
            .collect(),
        desc: f.iter().map(|(_, d)| *d).collect(),
    }
}

fn rotation_between(a: &Feat, b: &Feat) -> Option<(f64, usize)> {
    let m = orb::match_bf(&a.desc, &b.desc, 64, 0.85);
    let pairs: Vec<(Vec3, Vec3)> = m
        .iter()
        .filter_map(|&(i, j, _)| Some((a.bearings[i]?, b.bearings[j]?)))
        .collect();
    ransac_rotation(&pairs, 1.0)
}

fn brightness(buf: &[u8]) -> (f64, f64) {
    let (w, h) = (640usize, 480usize);
    let (mut sum, mut grad, mut n) = (0.0, 0.0, 0.0);
    for y in (1..h).step_by(2) {
        for x in (1..w).step_by(2) {
            let v = buf[y * w + x] as f64;
            sum += v;
            grad += (v - buf[y * w + x - 1] as f64).abs() + (v - buf[(y - 1) * w + x] as f64).abs();
            n += 1.0;
        }
    }
    let mean = sum / n;
    (mean, grad / n / mean.max(1.0))
}

/// Read every tracking frame of the take's `cameras.pxcam` (every `every`-th pair is fitted).
pub fn frames(pxcam: &Path, calib: &DeviceCalib, every: usize) -> Result<Vec<FrameRow>, String> {
    let mut cam = Camera::open(&pxcam.to_string_lossy(), 0, ReplayOptions::default())
        .map_err(|e| format!("open {}: {e}", pxcam.display()))?;
    let k0 = calib
        .cameras
        .first()
        .ok_or("no camera 0 in the calibration")?
        .intr
        .clone();
    let k1 = calib
        .cameras
        .get(1)
        .ok_or("no camera 1 in the calibration")?
        .intr
        .clone();
    let p = OrbParams {
        n_features: 600,
        ..OrbParams::default()
    };
    let mut b0 = vec![0u8; 640 * 480];
    let mut b1 = vec![0u8; 640 * 480];
    let mut prev: Option<(u64, Feat, Feat)> = None;
    let mut out = Vec::new();
    let mut t_first = None;
    let mut n = 0usize;
    loop {
        let info = match cam.next_frame(100).map_err(|e| e.to_string())? {
            Some(i) => i,
            None if cam.source().finished() => break,
            None => continue,
        };
        if info.kind != 1 {
            continue;
        }
        n += 1;
        cam.copy_tile(0, &mut b0, 640).map_err(|e| e.to_string())?;
        cam.copy_tile(1, &mut b1, 640).map_err(|e| e.to_string())?;
        let t0 = *t_first.get_or_insert(info.dev_ts_us);
        let (m0, s0) = brightness(&b0);
        let (m1, s1) = brightness(&b1);
        let mut row = FrameRow {
            t: (info.dev_ts_us - t0) as f64 * 1e-6,
            exposure: info.exposure[0],
            gain: info.gain[0],
            mean: 0.5 * (m0 + m1),
            sharp: 0.5 * (s0 + s1),
            ..Default::default()
        };
        if every > 1 && !n.is_multiple_of(every) {
            prev = None;
            out.push(row);
            continue;
        }
        let f0 = features(&b0, &k0, &p);
        let f1 = features(&b1, &k1, &p);
        row.features = f0.desc.len();
        if let Some((tp, a0, a1)) = &prev {
            let dt = (info.dev_ts_us - tp) as f64 * 1e-6;
            if dt > 0.0 && dt < 0.06 {
                let r0 = rotation_between(a0, &f0);
                let r1 = rotation_between(a1, &f1);
                let best = match (r0, r1) {
                    (Some(a), Some(b)) => Some(if a.1 >= b.1 { a } else { b }),
                    (a, b) => a.or(b),
                };
                if let Some((ang, inl)) = best {
                    row.speed = Some(ang / dt);
                    row.inliers = inl;
                }
            }
        }
        prev = Some((info.dev_ts_us, f0, f1));
        out.push(row);
    }
    Ok(out)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

/// The report: live tracker, then the camera table binned by head speed.
pub fn report(dir: &Path, log: &LogSummary, rows: &[FrameRow], px_per_deg: f64) -> String {
    let mut s = format!("take {}\n", dir.display());
    s += "live tracker (monado.log slice):\n";
    if let (Some(a), Some(b)) = (log.first, log.last) {
        s += &format!(
            "  6DoF acquisitions {} -> {}, losses {} -> {} (during the take: {} lost)\n",
            a.0,
            b.0,
            a.1,
            b.1,
            b.1 - a.1
        );
    }
    s += &format!(
        "  re-zeros while still {}, Basalt restarts {}, '6DoF: lost' lines {}\n",
        log.rezeros,
        log.restarts,
        log.lost_lines.len()
    );
    s += &format!(
        "  corrections >= 20 mm: {} {:?}; largest correction in the 5 s stats {:.1} mm\n",
        log.corrections.len(),
        log.corrections
            .iter()
            .map(|(mm, d)| format!("{mm:.0} mm at {d:.0} deg/s"))
            .collect::<Vec<_>>(),
        log.max_corr_mm
    );
    if !log.ae.is_empty() {
        let mut e: Vec<f64> = log.ae.iter().map(|a| a.0).collect();
        let mut m: Vec<f64> = log.ae.iter().map(|a| a.1).collect();
        s += &format!(
            "  AE (lower pair, 5 s lines): exposure p10/p50/p90 {:.0}/{:.0}/{:.0} us, mean brightness p10/p50/p90 {:.0}/{:.0}/{:.0} (target 64)\n",
            pct(&mut e.clone(), 0.1),
            pct(&mut e.clone(), 0.5),
            pct(&mut e, 0.9),
            pct(&mut m.clone(), 0.1),
            pct(&mut m.clone(), 0.5),
            pct(&mut m, 0.9)
        );
    }
    let fitted: Vec<&FrameRow> = rows.iter().filter(|r| r.speed.is_some()).collect();
    let mut sp: Vec<f64> = fitted.iter().filter_map(|r| r.speed).collect();
    let attempted = rows.iter().filter(|r| r.features > 0).count();
    s += &format!(
        "cameras: {} tracking frames over {:.1} s, rotation fitted for {} of {} frame pairs; head speed p50/p90/p99/max {:.0}/{:.0}/{:.0}/{:.0} deg/s, {:.1} s above 200 deg/s, {:.1} s above 300 deg/s\n",
        rows.len(),
        rows.last().map_or(0.0, |r| r.t),
        fitted.len(),
        attempted.saturating_sub(1),
        pct(&mut sp.clone(), 0.5),
        pct(&mut sp.clone(), 0.9),
        pct(&mut sp.clone(), 0.99),
        pct(&mut sp, 1.0),
        fitted.iter().filter(|r| r.speed.unwrap() > 200.0).count() as f64 * 0.04,
        fitted.iter().filter(|r| r.speed.unwrap() > 300.0).count() as f64 * 0.04,
    );
    s += &format!(
        "  {:<13} {:>6} {:>13} {:>10} {:>11} {:>9} {:>10} {:>13} {:>16}\n",
        "head speed",
        "frames",
        "inliers p50",
        "< 20 inl.",
        "exposure",
        "bright",
        "sharp",
        "blur ours px",
        "blur Pimax px"
    );
    let bins = [0.0, 50.0, 150.0, 300.0, 450.0, f64::INFINITY];
    for k in 0..bins.len() - 1 {
        let b: Vec<&&FrameRow> = fitted
            .iter()
            .filter(|r| (bins[k]..bins[k + 1]).contains(&r.speed.unwrap()))
            .collect();
        if b.is_empty() {
            continue;
        }
        let mut inl: Vec<f64> = b.iter().map(|r| r.inliers as f64).collect();
        let weak = b.iter().filter(|r| r.inliers < 20).count();
        let mut ex: Vec<f64> = b.iter().map(|r| r.exposure as f64).collect();
        let mut br: Vec<f64> = b.iter().map(|r| r.mean).collect();
        let mut sh: Vec<f64> = b.iter().map(|r| r.sharp).collect();
        let mut v: Vec<f64> = b.iter().map(|r| r.speed.unwrap()).collect();
        let vm = pct(&mut v, 0.5);
        let e = pct(&mut ex, 0.5);
        let label = if bins[k + 1].is_finite() {
            format!("{:.0}-{:.0}", bins[k], bins[k + 1])
        } else {
            format!("> {:.0}", bins[k])
        };
        s += &format!(
            "  {:<13} {:>6} {:>13.0} {:>9.0}% {:>8.0} us {:>9.0} {:>10.3} {:>13.1} {:>16.1}\n",
            label,
            b.len(),
            pct(&mut inl, 0.5),
            100.0 * weak as f64 / b.len() as f64,
            e,
            pct(&mut br, 0.5),
            pct(&mut sh, 0.5),
            vm * e * 1e-6 * px_per_deg,
            vm * PIMAX_EXPOSURE_US * 1e-6 * px_per_deg
        );
    }
    let fails = rows
        .windows(2)
        .filter(|w| w[1].features > 0 && w[0].features > 0 && w[1].speed.is_none())
        .count();
    s += &format!("  frame pairs where no rotation could be fitted (too few matches): {fails}\n");
    s
}

/// `pimax-slam take-head DIR [--every N] [--csv OUT]`.
pub fn run(dir: &Path, every: usize, csv: Option<&Path>) -> Result<String, String> {
    let log = std::fs::read_to_string(dir.join("monado.log")).unwrap_or_default();
    let ls = parse_log(&log);
    let (calib, _) = DeviceCalib::load_default(None).map_err(|e| e.to_string())?;
    let rows = frames(&dir.join("cameras.pxcam"), &calib, every)?;
    if let Some(p) = csv {
        let mut t = String::from("t_s,exposure_us,gain,mean,sharp,features,speed_dps,inliers\n");
        for r in &rows {
            t += &format!(
                "{:.3},{},{},{:.1},{:.4},{},{},{}\n",
                r.t,
                r.exposure,
                r.gain,
                r.mean,
                r.sharp,
                r.features,
                r.speed.map_or(String::new(), |v| format!("{v:.1}")),
                r.inliers
            );
        }
        std::fs::write(p, t).map_err(|e| format!("{}: {e}", p.display()))?;
    }
    let fx = calib.cameras[0].intr.fx;
    Ok(report(dir, &ls, &rows, fx * std::f64::consts::PI / 180.0))
}

/// Camera device clock (µs) -> the IMU log's clock (ns): `mono = dev_us * 1000 + offset`. The tracker pose times
/// in the log are camera frame times on the IMU's clock, so the offset is the one that puts most of them on a
/// tracking frame (within 1 ms), refined by their median residual. Frames are periodic, so a whole frame period
/// fits as well: `coarse` (host arrival - device time, a few ms late) picks the period; candidates are searched
/// within 20 ms of it. Returns (offset ns, poses matched).
pub fn clock_offset(frames_us: &[u64], vio_ts: &[i64], coarse: i64) -> Option<(i64, usize)> {
    if frames_us.is_empty() || vio_ts.len() < 10 {
        return None;
    }
    let fr: Vec<i64> = frames_us.iter().map(|&d| d as i64 * 1000).collect();
    let near = |t: i64| -> i64 {
        let i = fr.partition_point(|&f| f < t);
        let mut best = i64::MAX;
        for k in [i.saturating_sub(1), i.min(fr.len() - 1)] {
            if (t - fr[k]).abs() < best.abs() {
                best = t - fr[k];
            }
        }
        best
    };
    let count = |off: i64| {
        vio_ts
            .iter()
            .filter(|&&v| near(v - off).abs() < 1_000_000)
            .count()
    };
    // candidates: the first poses against every frame
    let mut best = (0usize, 0i64);
    for &v in vio_ts.iter().take(5) {
        for &f in &fr {
            let off = v - f;
            if (off - coarse).abs() > 20_000_000 {
                continue;
            }
            let n = count(off);
            if n > best.0 {
                best = (n, off);
            }
        }
    }
    if best.0 < vio_ts.len() / 2 {
        return None;
    }
    let mut res: Vec<i64> = vio_ts
        .iter()
        .map(|&v| near(v - best.1))
        .filter(|r| r.abs() < 1_000_000)
        .collect();
    res.sort_unstable();
    Some((best.1 + res[res.len() / 2], best.0))
}

/// `pimax-slam take-euroc TAKE [OUT]`: the take's cameras (0, 1) + head IMU log as a EuRoC dataset for
/// `run_slam.sh replay` / `bench-basalt` (default OUT = TAKE/euroc_take). Returns a one-line summary.
pub fn to_euroc(dir: &Path, out: Option<&Path>) -> Result<String, String> {
    let log = dir.join("head-imu.csv");
    let text = std::fs::read_to_string(&log).map_err(|e| {
        format!(
            "{}: {e} (takes recorded before 2026-10-16 have no head IMU; record a new take with a 6DoF session)",
            log.display()
        )
    })?;
    let h = pimax_cam::imulog::parse(&text);
    if h.imu.len() < 1000 {
        return Err(format!(
            "{}: only {} IMU samples",
            log.display(),
            h.imu.len()
        ));
    }
    let out = out
        .map(Path::to_path_buf)
        .unwrap_or_else(|| dir.join("euroc_take"));
    // pass 1: tracking frame device times and host arrival times (raw records)
    let pxcam = dir.join("cameras.pxcam");
    let mut dev = Vec::new();
    let mut late = Vec::new();
    let mut rd = pimax_cam::record::Reader::open(&pxcam)
        .map_err(|e| format!("open {}: {e}", pxcam.display()))?;
    while let Some(rec) = rd.next_record().map_err(|e| e.to_string())? {
        let Some(m) = rec
            .data
            .get(..pimax_cam::frame::META_LEN)
            .and_then(pimax_cam::frame::Meta::parse)
        else {
            continue;
        };
        if m.kind() == pimax_cam::frame::FrameKind::Tracking {
            dev.push(m.ts_us);
            // host arrival (the broker's CLOCK_MONOTONIC, same clock as the session) minus device time
            late.push(rec.host_ts_ns as i64 - m.ts_us as i64 * 1000);
        }
    }
    late.sort_unstable();
    let coarse = late.first().copied().unwrap_or(0); // lower envelope: least USB delay
    let (off, matched) = clock_offset(&dev, &h.vio_ts, coarse).ok_or(
        "the tracker pose times in head-imu.csv match no camera frames (was 6DoF tracking on during the take?)",
    )?;
    let (t_lo, t_hi) = (h.imu[0].0, h.imu[h.imu.len() - 1].0);
    // pass 2: images inside the IMU span
    let mut cam = Camera::open(&pxcam.to_string_lossy(), 0, ReplayOptions::default())
        .map_err(|e| format!("open {}: {e}", pxcam.display()))?;
    let mut lists: Vec<Vec<(i64, String)>> = vec![Vec::new(); 2];
    for k in 0..2 {
        std::fs::create_dir_all(out.join(format!("mav0/cam{k}/data")))
            .map_err(|e| e.to_string())?;
    }
    let mut buf = vec![0u8; 640 * 480];
    loop {
        let info = match cam.next_frame(100).map_err(|e| e.to_string())? {
            Some(i) => i,
            None if cam.source().finished() => break,
            None => continue,
        };
        if info.kind != 1 {
            continue;
        }
        let ts = info.dev_ts_us as i64 * 1000 + off;
        if ts < t_lo + 100_000_000 || ts > t_hi - 100_000_000 {
            continue;
        }
        for (k, list) in lists.iter_mut().enumerate() {
            cam.copy_tile(k, &mut buf, 640).map_err(|e| e.to_string())?;
            let name = format!("{ts}.png");
            pimax_cam::png::write_grey(
                &out.join(format!("mav0/cam{k}/data/{name}")),
                640,
                480,
                &buf,
            )
            .map_err(|e| e.to_string())?;
            list.push((ts, name));
        }
    }
    let imu: Vec<crate::euroc::ImuRow> = h
        .imu
        .iter()
        .map(|&(ts, a, g)| crate::euroc::ImuRow {
            ts,
            gyro: g,
            accel: a,
        })
        .collect();
    crate::euroc::write_csvs(&out, &imu, &lists).map_err(|e| e.to_string())?;
    Ok(format!(
        "{}: {} frame sets + {} IMU samples (camera clock offset {:.3} ms, {matched} of {} tracker poses on a frame)",
        out.display(),
        lists[0].len(),
        imu.len(),
        off as f64 / 1e6,
        h.vio_ts.len()
    ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn camera_clock_offset_from_tracker_pose_times() {
        let frames: Vec<u64> = (0..500).map(|k| 1_000_000 + k * 40_000 + (k % 3)).collect();
        let off = 123_456_789_012i64;
        // poses for most frames (some skipped), 0.3 ms of jitter
        let vio: Vec<i64> = frames
            .iter()
            .enumerate()
            .filter(|(k, _)| k % 7 != 0)
            .map(|(k, &f)| f as i64 * 1000 + off + ((k as i64 % 5) - 2) * 150_000)
            .collect();
        // the host saw the frames 2-4 ms after their device time
        let (got, n) = super::clock_offset(&frames, &vio, off + 3_000_000).unwrap();
        assert!((got - off).abs() < 200_000, "{got} vs {off}");
        assert_eq!(n, vio.len());
    }

    use super::*;

    #[test]
    fn ransac_finds_a_known_rotation_among_outliers() {
        let ang = 20f64.to_radians();
        let q = [0.0, (ang / 2.0).sin(), 0.0, (ang / 2.0).cos()];
        let mut rng = Lcg(3);
        let mut pairs = Vec::new();
        for i in 0..80 {
            let a = geom::normalize([
                (rng.next(1000) as f64 / 500.0) - 1.0,
                (rng.next(1000) as f64 / 500.0) - 1.0,
                1.0,
            ]);
            let b = if i % 4 == 0 {
                geom::normalize([(rng.next(1000) as f64 / 500.0) - 1.0, 0.3, 1.0])
            } else {
                qrot(q, a)
            };
            pairs.push((a, b));
        }
        let (deg, inl) = ransac_rotation(&pairs, 1.0).unwrap();
        assert!((deg - 20.0).abs() < 0.1, "{deg}");
        assert!(inl >= 60, "{inl}");
    }

    #[test]
    fn log_slice_is_summarised() {
        let t = " INFO [update_stats] slam: 8533 frame sets, ...; 6DoF tracking; 1 acquired, 0 lost, tracking\n\
 INFO [update_stats] 6DoF output: gyro bias (0.3 0.7 -0.4) deg/s, correction 3.0 mm / 0.01 deg (max 51.8 mm / 1.43 deg), 8773 blended, 0 snapped\n\
 INFO [update_stats] 6DoF correction: 23 mm (horizontal: vertical 1, horizontal 22), pending 21 mm / 0.17 deg, head 0.95 m/s 430 deg/s, tracker pose, t 1766.101 s\n\
 INFO [update_stats] AE on: lower 511/13 mean 27, upper 441/13 mean 31; frame 559/13; 2776 changes, 3408 sent\n\
 INFO [update_stats] slam: 9914 frame sets, ...; 6DoF tracking; 1 acquired, 2 lost, tracking\n";
        let s = parse_log(t);
        assert_eq!(s.first, Some((1, 0)));
        assert_eq!(s.last, Some((1, 2)));
        assert_eq!(s.corrections, vec![(23.0, 430.0)]);
        assert!((s.max_corr_mm - 51.8).abs() < 1e-9);
        assert_eq!(s.ae, vec![(511.0, 27.0)]);
    }
}
