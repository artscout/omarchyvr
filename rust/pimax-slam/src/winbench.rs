//! Offline head-tracking benchmark against Pimax's own tracker on Windows (`pimax-slam bench-windows`).
//!
//! Input: one take of a Windows capture session (`windows/capture-session.ps1`): the USBPcap capture of hub 2
//! (headset IMU at 500 reports/s = 1 kHz samples with the MCU's microsecond clock, auto-exposure commands) and
//! Pimax's own pose log `pimax-poses.csv` (LibPVRClient head pose at ~640 rows/s, `utc_filetime` = the same
//! Windows clock as the pcap timestamps).
//!
//! What the captures can and cannot give:
//! * The IMU stream is complete (every report).
//! * The camera stream is **not**: USBPcap keeps only the first ~64 KB of each ~2 MB isochronous URB (3.8 % of
//!   the bytes), so the tracking frames cannot be rebuilt and Basalt cannot run on these takes. The tracker
//!   part of our pipeline is therefore replayed with a synthetic tracker: Pimax's poses (lag removed) sampled at
//!   25 Hz, with VIO-like noise and delivery latency. Basalt's own behaviour in fast turns is measured on our
//!   Linux recordings instead ([`basalt_fast`]).
//!
//! Steps ([`run`]):
//! 1. MCU clock -> Windows host clock (lower envelope of arrival - device time, like the driver's
//!    `sample_time_ns`), sample order inside a report (smoothness test).
//! 2. Pimax's head angular velocity (from its quaternions) vs the gyro: lag (cross-correlation), head -> IMU
//!    rotation and gyro bias (Horn), gyro scale/misalignment (least squares), smoothing (first-order low-pass
//!    fit), lever arm head -> IMU and gravity in Pimax's world (accelerometer fit).
//! 3. Pimax at rest (jitter), in fast turns (peak angular speed, losses, jumps), its lag.
//! 4. Our output ([`HeadTracker`], the driver's code) on the real IMU with the synthetic tracker, queried at
//!    90 Hz for `now + predict`, scored against the lag-free Pimax trajectory: rotation / position error overall
//!    and in fast turns, effective lag.

use crate::traj::PoseRow;
use pimax_cam::geom::{self, Mat3, Vec3};
use pimax_cam::headpose::{flags, qexp, qlog, HeadParams, HeadTracker, ImuSample, Rel, VioSample};
use pimax_cam::slam::{qangle_deg, qconj, qmul, qnorm, qrot, qyaw, Quat, SupervisorParams};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

/// One IMU sample from the capture (driver body axes: `PIMAX_AXIS_MODE=1`, x and z negated).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinImu {
    /// Device time (MCU microseconds, unwrapped) in ns.
    pub dev_ns: i64,
    /// Host (Windows) arrival time of the report that carried it (ns, unix epoch).
    pub arr_ns: i64,
    /// Host time of the sample after the clock fit (ns).
    pub ts_ns: i64,
    /// Specific force (m/s²).
    pub accel: Vec3,
    /// Angular rate (rad/s).
    pub gyro: Vec3,
    /// 0 / 1: position in the report.
    pub slot: u8,
}

/// One auto-exposure command (`58 19 51`, host -> headset).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WinExp {
    /// Host time (ns).
    pub ts_ns: i64,
    /// Exposure per camera (same units as our AE command).
    pub exp: [u16; 4],
    /// Gain per camera.
    pub gain: [u16; 4],
}

/// The parts of a USBPcap capture the benchmark needs.
#[derive(Clone, Debug, Default)]
pub struct WinCapture {
    /// IMU samples (time order).
    pub imu: Vec<WinImu>,
    /// Exposure commands.
    pub exp: Vec<WinExp>,
    /// Camera isochronous bytes really on the wire vs captured (for the report).
    pub cam_bytes: (u64, u64),
    /// Host time of the first and last packet.
    pub span_ns: (i64, i64),
}

/// One row of Pimax's pose log.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PvrRow {
    /// Host time (ns, unix epoch) of the pose: `head_time` (PVR clock) mapped to the Windows clock by the lower
    /// envelope of `utc_filetime - head_time` (the row's `utc_filetime` is written after the call and jitters).
    pub ts_ns: i64,
    /// `head_time` (ns, PVR clock).
    pub pvr_ns: i64,
    /// Head status (3 = 6DoF, 1 = no position).
    pub status: i32,
    /// Position (m).
    pub p: Vec3,
    /// Orientation (head -> world, x y z w).
    pub q: Quat,
    /// Reported angular velocity.
    pub w: Vec3,
    /// Reported linear velocity.
    pub v: Vec3,
    /// `pvr_now - head_time` (s): how far ahead the logger asked (0 = "now").
    pub ahead_s: f64,
}

/// Device time u32 microseconds -> unwrapped ns.
struct Unwrap {
    last: Option<u32>,
    ext: i64,
}

impl Unwrap {
    fn push(&mut self, us: u32) -> i64 {
        match self.last {
            None => self.ext = us as i64,
            Some(l) => self.ext += us.wrapping_sub(l) as i32 as i64,
        }
        self.last = Some(us);
        self.ext * 1000
    }
}

fn be16(b: &[u8]) -> f64 {
    i16::from_be_bytes([b[0], b[1]]) as f64
}

/// Read a USBPcap capture: IMU reports (EP 0x83, `53 31 08`), AE commands (EP 0x01 `58 19 51`), camera bytes.
/// `slot1_after`: the report's second sample is 1 ms after its timestamp (libpimax) instead of before.
pub fn read_pcap(path: &Path, slot1_after: bool) -> Result<WinCapture, String> {
    let f = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut r = BufReader::with_capacity(1 << 20, f);
    let mut gh = [0u8; 24];
    r.read_exact(&mut gh)
        .map_err(|e| format!("pcap header: {e}"))?;
    let nano = match gh[..4] {
        [0xd4, 0xc3, 0xb2, 0xa1] => false,
        [0x4d, 0x3c, 0xb2, 0xa1] => true,
        _ => return Err("not a little-endian pcap".into()),
    };
    let mut cap = WinCapture::default();
    let mut uw = Unwrap { last: None, ext: 0 };
    let mut rh = [0u8; 16];
    let mut buf = Vec::new();
    let g_lsb = 9.80665 / 2048.0;
    let w_lsb = (1.0f64 / 16.4).to_radians();
    let mut first = true;
    while r.read_exact(&mut rh).is_ok() {
        let u = |i: usize| u32::from_le_bytes([rh[i], rh[i + 1], rh[i + 2], rh[i + 3]]);
        let (s, frac, cl) = (u(0) as i64, u(4) as i64, u(8) as usize);
        let ts = s * 1_000_000_000 + if nano { frac } else { frac * 1000 };
        buf.resize(cl, 0);
        if r.read_exact(&mut buf).is_err() {
            break;
        }
        if first {
            cap.span_ns.0 = ts;
            first = false;
        }
        cap.span_ns.1 = ts;
        if cl < 27 {
            continue;
        }
        let hl = u16::from_le_bytes([buf[0], buf[1]]) as usize;
        let info = buf[16];
        let ep = buf[21];
        let tt = buf[22];
        let dl = u32::from_le_bytes([buf[23], buf[24], buf[25], buf[26]]) as u64;
        if hl > buf.len() {
            continue;
        }
        let d = &buf[hl..];
        if tt == 0 && info & 1 == 1 && ep == 0x81 {
            cap.cam_bytes.0 += dl;
            cap.cam_bytes.1 += d.len() as u64;
        }
        if tt != 1 {
            continue;
        }
        if ep == 0x83
            && info & 1 == 1
            && d.len() >= 60
            && d[0] == 0x53
            && d[1] == 0x31
            && d[2] == 0x08
        {
            let dev = uw.push(u32::from_le_bytes([d[4], d[5], d[6], d[7]]));
            for i in 0..2usize {
                let o = 12 + 16 * i + 2;
                let raw_a = [be16(&d[o..]), be16(&d[o + 2..]), be16(&d[o + 4..])];
                let raw_g = [be16(&d[o + 6..]), be16(&d[o + 8..]), be16(&d[o + 10..])];
                let off = if slot1_after {
                    1_000_000 * i as i64
                } else {
                    -1_000_000 * i as i64
                };
                cap.imu.push(WinImu {
                    dev_ns: dev + off,
                    arr_ns: ts,
                    ts_ns: 0,
                    accel: [-raw_a[0] * g_lsb, raw_a[1] * g_lsb, -raw_a[2] * g_lsb],
                    gyro: [-raw_g[0] * w_lsb, raw_g[1] * w_lsb, -raw_g[2] * w_lsb],
                    slot: i as u8,
                });
            }
        } else if ep == 0x01 && info & 1 == 0 && d.len() >= 22 && d[0] == 0x58 && d[2] == 0x51 {
            let h = |i: usize| u16::from_le_bytes([d[i], d[i + 1]]);
            cap.exp.push(WinExp {
                ts_ns: ts,
                exp: [h(6), h(8), h(10), h(12)],
                gain: [h(14), h(16), h(18), h(20)],
            });
        }
    }
    cap.imu.sort_by_key(|s| s.dev_ns);
    cap.imu.dedup_by_key(|s| s.dev_ns);
    if cap.imu.len() < 1000 {
        return Err(format!(
            "only {} IMU samples in {}",
            cap.imu.len(),
            path.display()
        ));
    }
    Ok(cap)
}

/// Clock fit result.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ClockFit {
    /// host = dev + a + b * (dev - dev0).
    pub a: f64,
    /// Drift (ppm).
    pub ppm: f64,
    /// Arrival delay after the fitted clock (ms): p50, p95, p99, max (USB + host scheduling on Windows).
    pub delay_ms: [f64; 4],
}

/// Map device time to host time by the lower envelope of `arrival - device time` (per 2 s bin minimum, then a
/// straight line), as the driver does live with its running minimum. Sets `ts_ns` of every sample.
pub fn fit_clock(imu: &mut [WinImu]) -> ClockFit {
    // The newest sample of each report defines the arrival constraint.
    let dev0 = imu[0].dev_ns;
    let mut bins: Vec<(f64, f64)> = Vec::new();
    let bin_ns = 2_000_000_000i64;
    let mut cur: Option<(i64, f64, f64)> = None;
    for s in imu.iter() {
        let b = (s.dev_ns - dev0) / bin_ns;
        let off = (s.arr_ns - s.dev_ns) as f64;
        let x = (s.dev_ns - dev0) as f64;
        match cur {
            Some((cb, ref mut bx, ref mut bo)) if cb == b => {
                if off < *bo {
                    *bo = off;
                    *bx = x;
                }
            }
            _ => {
                if let Some((_, bx, bo)) = cur {
                    bins.push((bx, bo));
                }
                cur = Some((b, x, off));
            }
        }
    }
    if let Some((_, bx, bo)) = cur {
        bins.push((bx, bo));
    }
    let (a, b) = if bins.len() >= 3 {
        // Line through the bin minima, then shifted down so no sample arrives before its time.
        let n = bins.len() as f64;
        let mx = bins.iter().map(|x| x.0).sum::<f64>() / n;
        let my = bins.iter().map(|x| x.1).sum::<f64>() / n;
        let sxy: f64 = bins.iter().map(|x| (x.0 - mx) * (x.1 - my)).sum();
        let sxx: f64 = bins.iter().map(|x| (x.0 - mx) * (x.0 - mx)).sum();
        let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
        (my - b * mx, b)
    } else {
        (bins.iter().map(|x| x.1).fold(f64::INFINITY, f64::min), 0.0)
    };
    let lift = imu
        .iter()
        .map(|s| (s.arr_ns - s.dev_ns) as f64 - (a + b * (s.dev_ns - dev0) as f64))
        .fold(f64::INFINITY, f64::min);
    let a = a + lift;
    let mut delays = Vec::with_capacity(imu.len());
    for s in imu.iter_mut() {
        s.ts_ns = s.dev_ns + (a + b * (s.dev_ns - dev0) as f64) as i64;
        delays.push((s.arr_ns - s.ts_ns) as f64 / 1e6);
    }
    ClockFit {
        a,
        ppm: b * 1e6,
        delay_ms: [
            pct(&mut delays.clone(), 0.5),
            pct(&mut delays.clone(), 0.95),
            pct(&mut delays.clone(), 0.99),
            pct(&mut delays, 1.0),
        ],
    }
}

/// Parse `pimax-poses.csv` (the last line is usually cut: the writer flushes in 1 MiB blocks).
pub fn parse_pvr(text: &str) -> Result<Vec<PvrRow>, String> {
    let mut lines = text.lines();
    let head: Vec<&str> = lines.next().ok_or("empty pose log")?.split(',').collect();
    let col = |n: &str| {
        head.iter()
            .position(|h| h.trim() == n)
            .ok_or_else(|| format!("column {n} missing"))
    };
    let cft = col("utc_filetime")?;
    let (cnow, cht, cs) = (col("pvr_now")?, col("head_time")?, col("head_status")?);
    let cp = [col("head_px")?, col("head_py")?, col("head_pz")?];
    let cq = [
        col("head_qx")?,
        col("head_qy")?,
        col("head_qz")?,
        col("head_qw")?,
    ];
    let cw = [col("head_wx")?, col("head_wy")?, col("head_wz")?];
    let cv = [col("head_vx")?, col("head_vy")?, col("head_vz")?];
    let need = *[cft, cnow, cht, cs, cp[2], cq[3], cw[2], cv[2]]
        .iter()
        .max()
        .unwrap();
    let mut out = Vec::new();
    for l in lines {
        let f: Vec<&str> = l.split(',').collect();
        if f.len() <= need {
            continue;
        }
        let g = |i: usize| f[i].trim().parse::<f64>().ok();
        let Ok(ft) = f[cft].trim().parse::<i64>() else {
            continue;
        };
        let v3 = |c: [usize; 3]| -> Option<Vec3> { Some([g(c[0])?, g(c[1])?, g(c[2])?]) };
        let (Some(p), Some(w), Some(v), Some(st), Some(now), Some(ht)) =
            (v3(cp), v3(cw), v3(cv), g(cs), g(cnow), g(cht))
        else {
            continue;
        };
        let (Some(qx), Some(qy), Some(qz), Some(qw)) = (g(cq[0]), g(cq[1]), g(cq[2]), g(cq[3]))
        else {
            continue;
        };
        out.push(PvrRow {
            ts_ns: (ft - 116_444_736_000_000_000) * 100,
            pvr_ns: (ht * 1e9).round() as i64,
            status: st as i32,
            p,
            q: qnorm([qx, qy, qz, qw]),
            w,
            v,
            ahead_s: now - ht,
        });
    }
    out.sort_by_key(|r| r.pvr_ns);
    out.dedup_by_key(|r| r.pvr_ns);
    if out.len() < 100 {
        return Err(format!("only {} pose rows", out.len()));
    }
    // PVR clock -> host clock: line through the per-2 s minima of (utc - head_time), lifted to the lowest point.
    let p0 = out[0].pvr_ns;
    let mut bins: Vec<(f64, f64)> = Vec::new();
    for r in &out {
        let b = ((r.pvr_ns - p0) / 2_000_000_000) as usize;
        let x = (r.pvr_ns - p0) as f64;
        let o = (r.ts_ns - r.pvr_ns) as f64;
        if bins.len() <= b {
            bins.resize(b + 1, (f64::NAN, f64::INFINITY));
        }
        if o < bins[b].1 {
            bins[b] = (x, o);
        }
    }
    bins.retain(|b| b.0.is_finite());
    let n = bins.len() as f64;
    let mx = bins.iter().map(|x| x.0).sum::<f64>() / n;
    let my = bins.iter().map(|x| x.1).sum::<f64>() / n;
    let sxy: f64 = bins.iter().map(|x| (x.0 - mx) * (x.1 - my)).sum();
    let sxx: f64 = bins.iter().map(|x| (x.0 - mx) * (x.0 - mx)).sum();
    let b = if sxx > 0.0 { sxy / sxx } else { 0.0 };
    let a = my - b * mx;
    let lift = out
        .iter()
        .map(|r| (r.ts_ns - r.pvr_ns) as f64 - (a + b * (r.pvr_ns - p0) as f64))
        .fold(f64::INFINITY, f64::min);
    for r in out.iter_mut() {
        r.ts_ns = r.pvr_ns + (a + lift + b * (r.pvr_ns - p0) as f64) as i64;
    }
    Ok(out)
}

pub(crate) fn pct(v: &mut Vec<f64>, q: f64) -> f64 {
    v.retain(|x| x.is_finite());
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

fn mean3(v: &[Vec3]) -> Vec3 {
    let n = v.len().max(1) as f64;
    let mut m = [0.0; 3];
    for x in v {
        m = geom::add(m, geom::scale(*x, 1.0 / n));
    }
    m
}

fn skew(w: Vec3) -> Mat3 {
    [[0.0, -w[2], w[1]], [w[2], 0.0, -w[0]], [-w[1], w[0], 0.0]]
}

/// Solve `A x = b` (n x n, Gauss with partial pivoting).
#[allow(clippy::needless_range_loop)]
fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[p][c].abs() < 1e-15 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            for k in c..n {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for c in (0..n).rev() {
        let s: f64 = (c + 1..n).map(|k| a[c][k] * x[k]).sum();
        x[c] = (b[c] - s) / a[c][c];
    }
    Some(x)
}

/// Linear least squares `rows · x ≈ y` via the normal equations.
fn lstsq(rows: &[(Vec<f64>, f64)]) -> Option<Vec<f64>> {
    let n = rows.first()?.0.len();
    let mut a = vec![vec![0.0; n]; n];
    let mut b = vec![0.0; n];
    for (r, y) in rows {
        for i in 0..n {
            b[i] += r[i] * y;
            for j in 0..n {
                a[i][j] += r[i] * r[j];
            }
        }
    }
    solve(a, b)
}

/// A uniformly sampled vector series.
#[derive(Clone, Debug)]
struct Series {
    t0: i64,
    dt: i64,
    v: Vec<Option<Vec3>>,
}

impl Series {
    fn at(&self, t: i64) -> Option<Vec3> {
        let x = (t - self.t0) as f64 / self.dt as f64;
        if x < 0.0 {
            return None;
        }
        let i = x.floor() as usize;
        let f = x - i as f64;
        let a = (*self.v.get(i)?)?;
        let b = (*self.v.get(i + 1)?)?;
        Some(geom::add(geom::scale(a, 1.0 - f), geom::scale(b, f)))
    }
}

/// Gyro (bias not removed) on a 1 kHz grid (linear interpolation of the samples, by host sample time).
fn gyro_series(imu: &[WinImu], t0: i64, t1: i64) -> Series {
    let dt = 1_000_000;
    let n = ((t1 - t0) / dt).max(0) as usize + 1;
    let mut v = vec![None; n];
    let mut j = 0usize;
    for (k, slot) in v.iter_mut().enumerate() {
        let t = t0 + k as i64 * dt;
        while j + 1 < imu.len() && imu[j + 1].ts_ns <= t {
            j += 1;
        }
        if j + 1 >= imu.len() || imu[j].ts_ns > t {
            continue;
        }
        let (a, b) = (&imu[j], &imu[j + 1]);
        let span = b.ts_ns - a.ts_ns;
        if span <= 0 || span > 5_000_000 {
            continue;
        }
        let f = (t - a.ts_ns) as f64 / span as f64;
        *slot = Some(geom::add(
            geom::scale(a.gyro, 1.0 - f),
            geom::scale(b.gyro, f),
        ));
    }
    Series { t0, dt, v }
}

/// Body angular velocity of the Pimax head from consecutive quaternions (status 3 rows only, gaps < 10 ms),
/// on the same grid.
fn pvr_omega_series(rows: &[PvrRow], t0: i64, t1: i64) -> Series {
    let mut mids: Vec<(i64, Vec3)> = Vec::new();
    for w in rows.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let dt = b.ts_ns - a.ts_ns;
        if a.status != 3 || b.status != 3 || dt <= 200_000 || dt > 10_000_000 {
            continue;
        }
        let om = geom::scale(qlog(qmul(qconj(a.q), b.q)), 1e9 / dt as f64);
        mids.push(((a.ts_ns + b.ts_ns) / 2, om));
    }
    let dt = 1_000_000;
    let n = ((t1 - t0) / dt).max(0) as usize + 1;
    let mut v = vec![None; n];
    let mut j = 0usize;
    for (k, slot) in v.iter_mut().enumerate() {
        let t = t0 + k as i64 * dt;
        while j + 1 < mids.len() && mids[j + 1].0 <= t {
            j += 1;
        }
        if j + 1 >= mids.len() || mids[j].0 > t {
            continue;
        }
        let (a, b) = (mids[j], mids[j + 1]);
        if b.0 - a.0 > 10_000_000 {
            continue;
        }
        let f = (t - a.0) as f64 / (b.0 - a.0) as f64;
        *slot = Some(geom::add(geom::scale(a.1, 1.0 - f), geom::scale(b.1, f)));
    }
    Series { t0, dt, v }
}

/// Mean body angular velocity of the Pimax head over `t ± h_ms` (from the interpolated poses): robust against
/// the ~90 Hz state updates of LibPVR (constant-ω extrapolation between them, small snaps at each update).
fn pvr_omega_avg(rows: &[PvrRow], t0: i64, t1: i64, h_ms: f64) -> Series {
    let dt = 1_000_000;
    let h = (h_ms * 1e6) as i64;
    let n = ((t1 - t0) / dt).max(0) as usize + 1;
    let v = (0..n)
        .map(|k| {
            let t = t0 + k as i64 * dt;
            let (_, a) = pvr_at(rows, t - h)?;
            let (_, b) = pvr_at(rows, t + h)?;
            Some(geom::scale(qlog(qmul(qconj(a), b)), 1e9 / (2 * h) as f64))
        })
        .collect();
    Series { t0, dt, v }
}

/// Box average of a 1 kHz series over ± `h_ms`.
fn box_avg(s: &Series, h_ms: f64) -> Series {
    let h = (h_ms.round() as usize).max(1);
    let n = s.v.len();
    let v = (0..n)
        .map(|k| {
            if k < h || k + h >= n {
                return None;
            }
            let mut m = [0.0; 3];
            for j in k - h..=k + h {
                m = geom::add(m, s.v[j]?);
            }
            Some(geom::scale(m, 1.0 / (2 * h + 1) as f64))
        })
        .collect();
    Series {
        t0: s.t0,
        dt: s.dt,
        v,
    }
}

/// Pimax's tracker vs the IMU: everything fitted from one take.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Fit {
    /// Clock.
    pub clock: ClockFit,
    /// Mean roughness of the gyro with the libpimax sample order / the other order (deg/s, smaller = right).
    pub order_roughness: (f64, f64),
    /// Pimax pose lag behind the IMU (ms, > 0 = late) from |ω| cross-correlation, and its peak correlation.
    pub lag_ms: f64,
    /// See `lag_ms`.
    pub lag_corr: f64,
    /// Lag after the vector fit (ms).
    pub lag_vec_ms: f64,
    /// Head -> IMU rotation (ω_imu = R ω_head) and its angle (deg).
    pub r_head_imu: Mat3,
    /// See `r_head_imu`.
    pub r_angle_deg: f64,
    /// Gyro bias (rad/s, IMU frame).
    pub gyro_bias: Vec3,
    /// Gyro scale per axis (gyro / Pimax, 1 = same) and largest off-diagonal (misalignment).
    pub gyro_scale: Vec3,
    /// See `gyro_scale`.
    pub gyro_offdiag: f64,
    /// RMS of gyro - R ω_pimax after the fit (deg/s), and the RMS gyro signal.
    pub omega_resid_dps: f64,
    /// See `omega_resid_dps`.
    pub omega_rms_dps: f64,
    /// Smoothing: best first-order low-pass time constant (ms) on the gyro to match Pimax, with its lag and
    /// residual; `smooth_table` = (tau ms, lag ms, residual deg/s).
    pub smooth_tau_ms: f64,
    /// See `smooth_tau_ms`.
    pub smooth_table: Vec<(f64, f64, f64)>,
    /// Reported head_w: frame ("world"/"body") and lag vs the quaternion-derived ω (ms).
    pub w_frame: String,
    /// See `w_frame`.
    pub w_lag_ms: f64,
    /// Lever arm: IMU position in the head frame (m, head axes), accel bias (m/s², IMU), residual (m/s²).
    pub imu_in_head: Vec3,
    /// See `imu_in_head`.
    pub accel_bias: Vec3,
    /// See `imu_in_head`.
    pub accel_resid: f64,
    /// Gravity direction in Pimax's world (unit, the "up" the accelerometer sees).
    pub up_world: Vec3,
    /// Accelerometer |f| while still (m/s²) vs 9.80665.
    pub accel_still_norm: f64,
}

fn horn_vectors(pairs: &[(Vec3, Vec3)]) -> (Mat3, Vec3) {
    let pr: Vec<(PoseRow, PoseRow)> = pairs
        .iter()
        .map(|(e, r)| {
            (
                PoseRow {
                    ts: 0,
                    p: *e,
                    q: [0.0, 0.0, 0.0, 1.0],
                },
                PoseRow {
                    ts: 0,
                    p: *r,
                    q: [0.0, 0.0, 0.0, 1.0],
                },
            )
        })
        .collect();
    crate::traj::align(&pr)
}

fn mat_angle_deg(m: &Mat3) -> f64 {
    let tr = m[0][0] + m[1][1] + m[2][2];
    ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
}

/// |ω| cross-correlation lag (ms) between `a` (late) and `b` over the grid, search ±`range_ms`.
fn mag_xcorr(a: &Series, b: &Series, range_ms: f64, step_ms: f64) -> (f64, f64) {
    let mut best = (0.0, -2.0);
    let mut l = -range_ms;
    while l <= range_ms {
        let sh = (l * 1e6) as i64;
        let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0.0);
        let mut k = 0usize;
        while k < a.v.len() {
            let t = a.t0 + k as i64 * a.dt;
            if let (Some(x), Some(y)) = (a.v[k], b.at(t - sh)) {
                let (x, y) = (geom::norm(x), geom::norm(y));
                sa += x;
                sb += y;
                saa += x * x;
                sbb += y * y;
                sab += x * y;
                n += 1.0;
            }
            k += 2;
        }
        if n > 100.0 {
            let c = (sab / n - sa * sb / n / n)
                / ((saa / n - sa * sa / n / n).max(1e-30) * (sbb / n - sb * sb / n / n).max(1e-30))
                    .sqrt();
            if c > best.1 {
                best = (l, c);
            }
        }
        l += step_ms;
    }
    best
}

/// Causal first-order low-pass of a series (tau in ms; 0 = copy).
fn lowpass(s: &Series, tau_ms: f64) -> Series {
    if tau_ms <= 0.0 {
        return s.clone();
    }
    let a = 1.0 - (-(s.dt as f64 / 1e6) / tau_ms).exp();
    let mut y: Option<Vec3> = None;
    let v =
        s.v.iter()
            .map(|x| match x {
                Some(x) => {
                    let n = match y {
                        Some(p) => geom::add(p, geom::scale(geom::sub(*x, p), a)),
                        None => *x,
                    };
                    y = Some(n);
                    y
                }
                None => {
                    y = None;
                    None
                }
            })
            .collect();
    Series {
        t0: s.t0,
        dt: s.dt,
        v,
    }
}

/// Residual RMS (rad/s) of `gyro(t - lag) - bias` vs `R ω_pimax(t)`.
fn vec_resid(om: &Series, gy: &Series, r: &Mat3, bias: Vec3, lag_ms: f64) -> f64 {
    let sh = (lag_ms * 1e6) as i64;
    let (mut s, mut n) = (0.0f64, 0.0f64);
    let mut k = 0usize;
    while k < om.v.len() {
        let t = om.t0 + k as i64 * om.dt;
        if let (Some(x), Some(y)) = (om.v[k], gy.at(t - sh)) {
            let d = geom::sub(geom::sub(y, bias), geom::mat_vec(r, x));
            s += geom::dot(d, d);
            n += 1.0;
        }
        k += 2;
    }
    (s / n.max(1.0)).sqrt()
}

fn best_lag(
    om: &Series,
    gy: &Series,
    r: &Mat3,
    bias: Vec3,
    center: f64,
    half: f64,
    step: f64,
) -> (f64, f64) {
    let mut best = (center, f64::INFINITY);
    let mut l = center - half;
    while l <= center + half + 1e-9 {
        let e = vec_resid(om, gy, r, bias, l);
        if e < best.1 {
            best = (l, e);
        }
        l += step;
    }
    best
}

/// Interpolated Pimax pose (status 3, gap < 20 ms) at host time `t`.
pub fn pvr_at(rows: &[PvrRow], t: i64) -> Option<(Vec3, Quat)> {
    let i = rows.partition_point(|r| r.ts_ns < t);
    if i == 0 || i >= rows.len() {
        return None;
    }
    let (a, b) = (&rows[i - 1], &rows[i]);
    if a.status != 3 || b.status != 3 || b.ts_ns - a.ts_ns > 20_000_000 {
        return None;
    }
    let f = (t - a.ts_ns) as f64 / (b.ts_ns - a.ts_ns).max(1) as f64;
    let p = geom::add(geom::scale(a.p, 1.0 - f), geom::scale(b.p, f));
    let d = qlog(qmul(qconj(a.q), b.q));
    Some((p, qnorm(qmul(a.q, qexp(geom::scale(d, f))))))
}

/// Fit everything (see [`Fit`]). `imu` must have host times (`fit_clock`).
pub fn fit(cap: &WinCapture, rows: &[PvrRow], clock: ClockFit) -> Result<Fit, String> {
    let imu = &cap.imu;
    let mut f = Fit {
        clock,
        ..Default::default()
    };
    // Sample order: roughness of the interleaved gyro (second difference) with both orders.
    {
        let rough = |imu: &[WinImu]| -> f64 {
            let (mut s, mut n) = (0.0f64, 0.0f64);
            for w in imu.windows(3) {
                let d = geom::sub(geom::scale(geom::add(w[0].gyro, w[2].gyro), 0.5), w[1].gyro);
                s += geom::norm(d);
                n += 1.0;
            }
            (s / n.max(1.0)).to_degrees()
        };
        let mut other: Vec<WinImu> = imu
            .iter()
            .map(|s| WinImu {
                dev_ns: s.dev_ns + if s.slot == 1 { -2_000_000 } else { 0 },
                ..*s
            })
            .collect();
        other.sort_by_key(|s| s.dev_ns);
        f.order_roughness = (rough(imu), rough(&other));
    }
    let t0 = rows.first().unwrap().ts_ns.max(imu[0].ts_ns) + 200_000_000;
    let t1 = rows.last().unwrap().ts_ns.min(imu[imu.len() - 1].ts_ns) - 200_000_000;
    if t1 - t0 < 3_000_000_000 {
        return Err("less than 3 s overlap between the pose log and the capture".into());
    }
    // Both sides averaged over the same 24 ms window (LibPVR's 90 Hz state steps average out).
    let gy = box_avg(&gyro_series(imu, t0 - 300_000_000, t1 + 300_000_000), 12.0);
    let om = pvr_omega_avg(rows, t0, t1, 12.0);
    // 1. Lag from |ω| (frame free).
    let (l0, c0) = mag_xcorr(&om, &gy, 150.0, 0.5);
    f.lag_ms = l0;
    f.lag_corr = c0;
    // 2. Rotation head -> IMU and gyro bias (Horn: gyro ≈ R ω + b).
    let sh = (l0 * 1e6) as i64;
    let mut pairs = Vec::new();
    let mut k = 0usize;
    while k < om.v.len() {
        let t = om.t0 + k as i64 * om.dt;
        if let (Some(x), Some(y)) = (om.v[k], gy.at(t - sh)) {
            pairs.push((x, y));
        }
        k += 5;
    }
    if pairs.len() < 200 {
        return Err("too few tracked samples to fit".into());
    }
    let (r, b) = horn_vectors(&pairs);
    f.r_head_imu = r;
    f.r_angle_deg = mat_angle_deg(&r);
    f.gyro_bias = b;
    // 3. Lag refined on the vectors.
    let (lv, res) = best_lag(&om, &gy, &r, b, l0, 8.0, 0.1);
    f.lag_vec_ms = lv;
    f.omega_resid_dps = res.to_degrees();
    let sig: Vec<f64> = pairs.iter().map(|(x, _)| geom::norm(*x)).collect();
    f.omega_rms_dps = rms(&sig).to_degrees();
    // 4. Gyro scale: (gyro - b) = M (R ω): per IMU axis, 3 coefficients.
    {
        let sh = (lv * 1e6) as i64;
        let mut m = [[0.0; 3]; 3];
        for (ax, row) in m.iter_mut().enumerate() {
            let mut rows_ls = Vec::new();
            let mut k = 0usize;
            while k < om.v.len() {
                let t = om.t0 + k as i64 * om.dt;
                if let (Some(x), Some(y)) = (om.v[k], gy.at(t - sh)) {
                    let xi = geom::mat_vec(&r, x);
                    rows_ls.push((vec![xi[0], xi[1], xi[2], 1.0], y[ax]));
                }
                k += 3;
            }
            if let Some(s) = lstsq(&rows_ls) {
                *row = [s[0], s[1], s[2]];
            }
        }
        f.gyro_scale = [m[0][0], m[1][1], m[2][2]];
        f.gyro_offdiag = (0..3)
            .flat_map(|i| (0..3).filter(move |&j| j != i).map(move |j| (i, j)))
            .map(|(i, j)| m[i][j].abs())
            .fold(0.0, f64::max);
    }
    // 5. Smoothing: gyro low-passed with tau, best lag for each.
    for tau in [0.0, 1.0, 2.0, 3.0, 5.0, 8.0, 12.0, 20.0, 30.0] {
        let lp = lowpass(&gy, tau);
        let (l, e) = best_lag(&om, &lp, &r, b, lv - tau, 6.0 + tau, 0.25);
        f.smooth_table.push((tau, l, e.to_degrees()));
    }
    f.smooth_tau_ms = f
        .smooth_table
        .iter()
        .min_by(|a, b| a.2.partial_cmp(&b.2).unwrap())
        .map(|x| x.0)
        .unwrap_or(0.0);
    // 6. Reported head_w: world or body, and its lag vs the quaternion ω.
    {
        let mut sw = (0.0, 0.0, 0.0);
        let mut lags_body = Vec::new();
        for w in rows.windows(2) {
            let (a, bb) = (&w[0], &w[1]);
            let dt = bb.ts_ns - a.ts_ns;
            if a.status != 3 || bb.status != 3 || dt <= 200_000 || dt > 10_000_000 {
                continue;
            }
            let ob = geom::scale(qlog(qmul(qconj(a.q), bb.q)), 1e9 / dt as f64);
            let ow = qrot(a.q, ob);
            sw.0 += geom::norm(geom::sub(a.w, ow));
            sw.1 += geom::norm(geom::sub(a.w, ob));
            sw.2 += 1.0;
            lags_body.push(a.ts_ns);
        }
        f.w_frame = if sw.0 < sw.1 { "world" } else { "body" }.to_string();
        // Lag of the reported ω vs the quaternion ω: magnitude xcorr on the grid.
        let dtg = 1_000_000;
        let n = ((t1 - t0) / dtg) as usize + 1;
        let mut v = vec![None; n];
        let mut j = 0usize;
        for (k, slot) in v.iter_mut().enumerate() {
            let t = t0 + k as i64 * dtg;
            while j + 1 < rows.len() && rows[j + 1].ts_ns <= t {
                j += 1;
            }
            if j + 1 < rows.len()
                && rows[j].status == 3
                && rows[j + 1].ts_ns - rows[j].ts_ns < 10_000_000
            {
                *slot = Some(rows[j].w);
            }
        }
        let ws = Series { t0, dt: dtg, v };
        f.w_lag_ms = mag_xcorr(&ws, &om, 60.0, 0.5).0;
    }
    // 7. Lever arm and gravity from the accelerometer.
    fit_accel(&mut f, imu, rows, t0, t1);
    Ok(f)
}

/// Smoothing of the reference trajectory (ms): removes LibPVR's 90 Hz update snaps; costs at most ~0.1 deg at
/// 100 rad/s² head acceleration.
pub const TRUTH_SIGMA_MS: f64 = 5.0;

/// Lag-free reference: Pimax pose at `t + lag` (head frame), smoothed lightly.
pub struct Truth {
    /// 1 kHz rows (head pose in Pimax's world), may have gaps.
    pub rows: Vec<PoseRow>,
}

impl Truth {
    /// Build from the pose log with the fitted lag, Gaussian-smoothed with `sigma_ms` (0 = raw; rows whose
    /// smoothing window is not complete are dropped).
    pub fn new(rows: &[PvrRow], lag_ms: f64, sigma_ms: f64) -> Truth {
        let sh = (lag_ms * 1e6) as i64;
        let (t0, t1) = (rows[0].ts_ns - sh, rows[rows.len() - 1].ts_ns - sh);
        let mut raw = Vec::new();
        let mut t = t0 - t0.rem_euclid(1_000_000) + 1_000_000;
        while t < t1 {
            if let Some((p, q)) = pvr_at(rows, t + sh) {
                raw.push(PoseRow { ts: t, p, q });
            }
            t += 1_000_000;
        }
        if sigma_ms <= 0.0 {
            return Truth { rows: raw };
        }
        let w = (3.0 * sigma_ms).ceil() as usize;
        let ker: Vec<f64> = (0..=2 * w)
            .map(|k| (-((k as f64 - w as f64) / sigma_ms).powi(2) / 2.0).exp())
            .collect();
        let ks: f64 = ker.iter().sum();
        let mut out = Vec::with_capacity(raw.len());
        for i in w..raw.len().saturating_sub(w) {
            if raw[i + w].ts - raw[i - w].ts != (2 * w) as i64 * 1_000_000 {
                continue;
            }
            let qc = raw[i].q;
            let (mut p, mut rv) = ([0.0; 3], [0.0; 3]);
            for (j, k) in ker.iter().enumerate() {
                let x = &raw[i - w + j];
                p = geom::add(p, geom::scale(x.p, *k / ks));
                rv = geom::add(rv, geom::scale(qlog(qmul(qconj(qc), x.q)), *k / ks));
            }
            out.push(PoseRow {
                ts: raw[i].ts,
                p,
                q: qnorm(qmul(qc, qexp(rv))),
            });
        }
        Truth { rows: out }
    }

    /// Row index at `t` (exact 1 ms grid) if present.
    pub fn idx(&self, t: i64) -> Option<usize> {
        let i = self.rows.partition_point(|r| r.ts < t);
        let r = self.rows.get(i)?;
        (r.ts - t < 1_000_000).then_some(i)
    }

    /// Pose at `t` (linear / slerp between grid rows, no gap > 3 ms).
    pub fn at(&self, t: i64) -> Option<(Vec3, Quat)> {
        let i = self.rows.partition_point(|r| r.ts < t);
        if i == 0 || i >= self.rows.len() {
            return None;
        }
        let (a, b) = (&self.rows[i - 1], &self.rows[i]);
        if b.ts - a.ts > 3_000_000 {
            return None;
        }
        let f = (t - a.ts) as f64 / (b.ts - a.ts).max(1) as f64;
        let p = geom::add(geom::scale(a.p, 1.0 - f), geom::scale(b.p, f));
        let d = qlog(qmul(qconj(a.q), b.q));
        Some((p, qnorm(qmul(a.q, qexp(geom::scale(d, f))))))
    }

    /// Body angular velocity (rad/s, head frame) and world velocity / acceleration at `t` by central differences
    /// over `h` ms.
    pub fn deriv(&self, t: i64, h_ms: f64) -> Option<(Vec3, Vec3, Vec3)> {
        let h = (h_ms * 1e6) as i64;
        let (p0, q0) = self.at(t - h)?;
        let (p1, _q1) = self.at(t)?;
        let (p2, q2) = self.at(t + h)?;
        let hs = h as f64 / 1e9;
        let w = geom::scale(qlog(qmul(qconj(q0), q2)), 0.5 / hs);
        let v = geom::scale(geom::sub(p2, p0), 0.5 / hs);
        let a = geom::scale(
            geom::add(geom::sub(p2, geom::scale(p1, 2.0)), p0),
            1.0 / (hs * hs),
        );
        Some((w, v, a))
    }
}

fn fit_accel(f: &mut Fit, imu: &[WinImu], rows: &[PvrRow], t0: i64, t1: i64) {
    let truth = Truth::new(rows, f.lag_vec_ms, TRUTH_SIGMA_MS);
    let r = f.r_head_imu;
    let rt = geom::transpose(&r);
    // Accel averaged over ±2 ms around t.
    let acc_at = |t: i64| -> Option<Vec3> {
        let i = imu.partition_point(|s| s.ts_ns < t - 2_000_000);
        let v: Vec<Vec3> = imu[i..]
            .iter()
            .take_while(|s| s.ts_ns <= t + 2_000_000)
            .map(|s| s.accel)
            .collect();
        (v.len() >= 3).then(|| mean3(&v))
    };
    // Gravity in Pimax's world: mean of the world-frame specific force (head acceleration averages out).
    let mut up = Vec::new();
    let mut still_norm = Vec::new();
    let mut t = t0;
    while t < t1 {
        if let (Some((_, q)), Some(fm)) = (truth.at(t), acc_at(t)) {
            // world <- imu = world <- head * head <- imu (R^T)
            let fw = qrot(q, geom::mat_vec(&rt, fm));
            up.push(fw);
            if let Some((w, _, _)) = truth.deriv(t, 5.0) {
                if geom::norm(w) < 0.05 {
                    still_norm.push(geom::norm(fm));
                }
            }
        }
        t += 10_000_000;
    }
    let upm = mean3(&up);
    f.up_world = geom::normalize(upm);
    f.accel_still_norm = pct(&mut still_norm, 0.5);
    let gw = geom::scale(f.up_world, -9.80665); // gravity vector
                                                // Lever: f_imu - R q^-1 (a_head - g) = R ([α]x + [ω]x²) r_head + b   (r_head: IMU in head frame)
    let mut ls: Vec<(Vec<f64>, f64)> = Vec::new();
    let mut t = t0;
    while t < t1 {
        if let (Some((_, q)), Some(fm), Some((w, _, a)), Some((w0, _, _)), Some((w1, _, _))) = (
            truth.at(t),
            acc_at(t),
            truth.deriv(t, 8.0),
            truth.deriv(t - 4_000_000, 8.0),
            truth.deriv(t + 4_000_000, 8.0),
        ) {
            let al = geom::scale(geom::sub(w1, w0), 1.0 / 0.008);
            if geom::norm(a) > 40.0 || geom::norm(al) > 400.0 {
                t += 5_000_000;
                continue; // Pimax position jump or loss edge
            }
            let lhs = geom::sub(fm, geom::mat_vec(&r, qrot(qconj(q), geom::sub(a, gw))));
            let sw = skew(w);
            let m = geom::mat_mul(&r, &mat_add(&skew(al), &geom::mat_mul(&sw, &sw)));
            for i in 0..3 {
                let mut row = vec![m[i][0], m[i][1], m[i][2], 0.0, 0.0, 0.0];
                row[3 + i] = 1.0;
                ls.push((row, lhs[i]));
            }
        }
        t += 5_000_000;
    }
    // Two passes: drop the worst 5 % rows (Pimax position glitches) and refit.
    for pass in 0..2 {
        let Some(x) = lstsq(&ls) else {
            break;
        };
        f.imu_in_head = [x[0], x[1], x[2]];
        f.accel_bias = [x[3], x[4], x[5]];
        let res: Vec<f64> = ls
            .iter()
            .map(|(r, y)| r.iter().zip(&x).map(|(a, b)| a * b).sum::<f64>() - y)
            .collect();
        f.accel_resid = rms(&res);
        if pass == 0 {
            let lim = pct(&mut res.iter().map(|x| x.abs()).collect(), 0.95);
            let keep: Vec<bool> = res.iter().map(|x| x.abs() <= lim).collect();
            let mut i = 0;
            ls.retain(|_| {
                i += 1;
                keep[i - 1]
            });
        }
    }
}

fn mat_add(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut m = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            m[i][j] = a[i][j] + b[i][j];
        }
    }
    m
}

/// Pimax's tracker on its own: losses, jumps, jitter at rest, motion statistics.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PvrStats {
    /// Rows, seconds, rows/s.
    pub rows: usize,
    /// See `rows`.
    pub seconds: f64,
    /// Fraction of rows with status 3.
    pub tracked_frac: f64,
    /// Loss segments (status != 3): (start s from the log start, duration s).
    pub losses: Vec<(f64, f64)>,
    /// Position jumps > 5 cm between consecutive tracked rows (count, largest m).
    pub jumps: (usize, f64),
    /// Gyro angular speed (deg/s): p50, p95, max over the take; seconds above 100 / 200 / 300 deg/s.
    pub speed_dps: [f64; 3],
    /// See `speed_dps`.
    pub secs_above: [f64; 3],
    /// Head linear speed p95 / max (m/s) from Pimax.
    pub lin_speed: [f64; 2],
    /// Jitter while still (gyro < 3 deg/s over 0.5 s): position RMS minus 0.25 s average (mm), rotation (deg),
    /// and the still seconds used.
    pub still_jitter_mm: f64,
    /// See `still_jitter_mm`.
    pub still_jitter_deg: f64,
    /// See `still_jitter_mm`.
    pub still_secs: f64,
    /// Lag-induced error of Pimax's "now" pose vs the lag-free trajectory in fast motion (>= 150 deg/s):
    /// rotation p50 / p95 (deg).
    pub fast_lag_err_deg: [f64; 2],
}

/// Pimax-only statistics.
pub fn pvr_stats(cap: &WinCapture, rows: &[PvrRow], fit: &Fit) -> PvrStats {
    let mut s = PvrStats {
        rows: rows.len(),
        ..Default::default()
    };
    let (r0, r1) = (rows[0].ts_ns, rows[rows.len() - 1].ts_ns);
    s.seconds = (r1 - r0) as f64 / 1e9;
    s.tracked_frac = rows.iter().filter(|r| r.status == 3).count() as f64 / rows.len() as f64;
    let mut start: Option<i64> = None;
    for r in rows {
        match (r.status != 3, start) {
            (true, None) => start = Some(r.ts_ns),
            (false, Some(a)) => {
                s.losses
                    .push(((a - r0) as f64 / 1e9, (r.ts_ns - a) as f64 / 1e9));
                start = None;
            }
            _ => {}
        }
    }
    if let Some(a) = start {
        s.losses
            .push(((a - r0) as f64 / 1e9, (r1 - a) as f64 / 1e9));
    }
    for w in rows.windows(2) {
        if w[0].status == 3 && w[1].status == 3 {
            let d = geom::norm(geom::sub(w[1].p, w[0].p));
            if d > 0.05 {
                s.jumps.0 += 1;
                s.jumps.1 = s.jumps.1.max(d);
            }
        }
    }
    // Gyro speed over the pose log span.
    let b = fit.gyro_bias;
    let sp: Vec<f64> = cap
        .imu
        .iter()
        .filter(|x| x.ts_ns >= r0 && x.ts_ns <= r1)
        .map(|x| geom::norm(geom::sub(x.gyro, b)).to_degrees())
        .collect();
    let dt = 0.001;
    s.secs_above = [
        sp.iter().filter(|x| **x > 100.0).count() as f64 * dt,
        sp.iter().filter(|x| **x > 200.0).count() as f64 * dt,
        sp.iter().filter(|x| **x > 300.0).count() as f64 * dt,
    ];
    s.speed_dps = [
        pct(&mut sp.clone(), 0.5),
        pct(&mut sp.clone(), 0.95),
        pct(&mut sp.clone(), 1.0),
    ];
    let mut ls: Vec<f64> = rows
        .iter()
        .filter(|r| r.status == 3)
        .map(|r| geom::norm(r.v))
        .collect();
    s.lin_speed = [pct(&mut ls.clone(), 0.95), pct(&mut ls, 1.0)];
    // Still jitter.
    let truth_raw = Truth::new(rows, 0.0, 0.0);
    let gy = gyro_series(&cap.imu, r0, r1);
    let (mut pj, mut rj) = (Vec::new(), Vec::new());
    let mut still_ms = 0usize;
    let n = truth_raw.rows.len();
    let half = 125usize;
    for i in (half..n.saturating_sub(half)).step_by(2) {
        let t = truth_raw.rows[i].ts;
        // Still: gyro under 3 deg/s for ±250 ms.
        let mut still = true;
        let mut tt = t - 250_000_000;
        while tt <= t + 250_000_000 {
            match gy.at(tt) {
                Some(g) if geom::norm(geom::sub(g, b)).to_degrees() < 3.0 => {}
                _ => {
                    still = false;
                    break;
                }
            }
            tt += 10_000_000;
        }
        if !still {
            continue;
        }
        let seg = &truth_raw.rows[i - half..=i + half];
        if seg[seg.len() - 1].ts - seg[0].ts > 260_000_000 {
            continue;
        }
        still_ms += 2;
        let pm = mean3(&seg.iter().map(|x| x.p).collect::<Vec<_>>());
        pj.push(geom::norm(geom::sub(truth_raw.rows[i].p, pm)) * 1000.0);
        let qc = truth_raw.rows[i].q;
        let rv = mean3(
            &seg.iter()
                .map(|x| qlog(qmul(qconj(qc), x.q)))
                .collect::<Vec<_>>(),
        );
        rj.push(geom::norm(rv).to_degrees());
    }
    s.still_jitter_mm = rms(&pj);
    s.still_jitter_deg = rms(&rj);
    s.still_secs = still_ms as f64 / 1000.0;
    // Pimax's lag error in fast motion.
    let truth = Truth::new(rows, fit.lag_vec_ms, TRUTH_SIGMA_MS);
    let mut e = Vec::new();
    for r in rows.iter().step_by(4) {
        if r.status != 3 {
            continue;
        }
        if let (Some((_, qt)), Some((w, _, _))) = (truth.at(r.ts_ns), truth.deriv(r.ts_ns, 5.0)) {
            if geom::norm(w).to_degrees() >= 150.0 {
                e.push(qangle_deg(qmul(qconj(r.q), qt)));
            }
        }
    }
    s.fast_lag_err_deg = [pct(&mut e.clone(), 0.5), pct(&mut e, 0.95)];
    s
}

/// Our pipeline's replay parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct OursParams {
    /// Output parameters (`HeadParams::from_env()` so every `PIMAX_SLAM_*` switch applies).
    pub head: HeadParams,
    /// Tracker rate (Hz).
    pub vio_hz: f64,
    /// Tracker pose delivery latency after the frame time (ms).
    pub latency_ms: f64,
    /// Tracker noise: position (m), rotation (deg), velocity (m/s), per axis, white per pose.
    pub noise: (f64, f64, f64),
    /// Query rate (Hz) and prediction (query time - now, ms).
    pub query_hz: f64,
    /// See `query_hz`.
    pub predict_ms: f64,
    /// Fast-motion threshold (deg/s) for the "fast" statistics.
    pub fast_dps: f64,
    /// RNG seed.
    pub seed: u64,
    /// Gyro scale applied before the predictor (bench-side A/B; 1 = as decoded).
    pub gyro_scale: Vec3,
}

impl Default for OursParams {
    fn default() -> Self {
        OursParams {
            head: HeadParams::from_env(),
            vio_hz: 25.0,
            latency_ms: 30.0,
            noise: (0.002, 0.05, 0.02),
            query_hz: 90.0,
            predict_ms: 0.0,
            fast_dps: 150.0,
            seed: 7,
            gyro_scale: [1.0; 3],
        }
    }
}

/// Our replay result.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct OursResult {
    /// Queries scored (tracked).
    pub queries: usize,
    /// Rotation error p50 / p95 / max (deg): all, fast.
    pub rot_all: [f64; 3],
    /// See `rot_all`.
    pub rot_fast: [f64; 3],
    /// Position error p50 / p95 / max (mm, after removing the constant offset): all, fast.
    pub pos_all: [f64; 3],
    /// See `pos_all`.
    pub pos_fast: [f64; 3],
    /// Frame-to-frame rotation step error RMS (deg) in fast motion (judder).
    pub rot_step_fast: f64,
    /// Effective rotation lag of the output vs the truth at the query time (ms; < 0 = ahead).
    pub lag_ms: f64,
    /// Fast queries.
    pub fast_n: usize,
    /// Losses of the supervisor and largest correction (mm, deg).
    pub losses: u64,
    /// See `losses`.
    pub max_corr: (f64, f64),
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

/// Run our head pose output on the take: real IMU (available at its USB arrival time), tracker poses from the
/// lag-free Pimax trajectory transformed to the IMU (25 Hz, noise, latency), queries at `query_hz` for
/// `now + predict`. Scored against the same lag-free trajectory (head frame, so the IMU -> head lever is in).
pub fn run_ours(
    cap: &WinCapture,
    rows: &[PvrRow],
    fit: &Fit,
    p: &OursParams,
) -> Result<OursResult, String> {
    let truth = Truth::new(rows, fit.lag_vec_ms, TRUTH_SIGMA_MS);
    if truth.rows.len() < 3000 {
        return Err("too little tracked truth".into());
    }
    // Pimax world -> y-up world (gravity along -y): rotation taking `up_world` to +y.
    let up = fit.up_world;
    let axis = geom::cross(up, [0.0, 1.0, 0.0]);
    let ang = geom::dot(up, [0.0, 1.0, 0.0]).clamp(-1.0, 1.0).acos();
    let q_up = if geom::norm(axis) < 1e-9 {
        [0.0, 0.0, 0.0, 1.0]
    } else {
        qexp(geom::scale(geom::normalize(axis), ang))
    };
    let q_hi: Quat = qnorm(pimax_cam::slam::quat_of(&geom::transpose(&fit.r_head_imu))); // head <- imu
    let lever = fit.imu_in_head; // IMU origin in the head frame
                                 // Head origin in the IMU frame (for the driver's head_in_imu).
    let head_in_imu = geom::scale(geom::mat_vec(&fit.r_head_imu, lever), -1.0);
    // IMU pose (y-up world) at t.
    let imu_pose = |t: i64| -> Option<(Vec3, Quat)> {
        let (ph, qh) = truth.at(t)?;
        let qw = qnorm(qmul(q_up, qh));
        let pw = qrot(q_up, ph);
        let qi = qnorm(qmul(qw, q_hi));
        Some((geom::add(pw, qrot(qw, lever)), qi))
    };
    let head_pose = |t: i64| -> Option<(Vec3, Quat)> {
        let (ph, qh) = truth.at(t)?;
        Some((qrot(q_up, ph), qnorm(qmul(q_up, qh))))
    };
    let mut rng = Rng(p.seed);
    let mut h = HeadTracker::new(SupervisorParams::default(), p.head.clone(), head_in_imu);
    if std::env::var("WINBENCH_DEBUG").is_ok() {
        h.corr_log = Some(Vec::new());
    }
    // Tracker world: yawed and offset from the truth world (Basalt starts its own world).
    let wq = qyaw(0.7);
    let woff = [1.0, 0.0, -2.0];
    let imu_last = cap.imu.last().map(|s| s.ts_ns).unwrap_or(0);
    let (t_start, t_end) = (
        truth.rows[0].ts.max(cap.imu[0].ts_ns + 2_000_000_000) + 500_000_000,
        truth.rows[truth.rows.len() - 1].ts.min(imu_last) - 500_000_000,
    );
    let vio_dt = (1e9 / p.vio_hz) as i64;
    let lat = (p.latency_ms * 1e6) as i64;
    let q_dt = (1e9 / p.query_hz) as i64;
    let pred = (p.predict_ms * 1e6) as i64;
    // Events: IMU sample arrivals, tracker deliveries, queries; processed in host time order.
    let imu: Vec<&WinImu> = cap
        .imu
        .iter()
        .filter(|s| s.ts_ns >= t_start - 2_000_000_000 && s.ts_ns <= t_end)
        .collect();
    let mut ii = 0usize;
    let mut next_frame = t_start;
    let mut pending: std::collections::VecDeque<(i64, VioSample)> = Default::default();
    let mut next_q = t_start;
    let mut outs: Vec<(i64, Rel)> = Vec::new();
    let mut epochs: Vec<u64> = Vec::new();
    let mut now = t_start - 2_000_000_000;
    let step = 250_000i64; // 0.25 ms simulation step
    while now < t_end {
        now += step;
        while ii < imu.len() && imu[ii].arr_ns.max(imu[ii].ts_ns) <= now {
            let s = imu[ii];
            let g = [
                s.gyro[0] * p.gyro_scale[0],
                s.gyro[1] * p.gyro_scale[1],
                s.gyro[2] * p.gyro_scale[2],
            ];
            h.push_imu(ImuSample {
                ts_ns: s.ts_ns,
                accel: s.accel,
                gyro: g,
            });
            ii += 1;
        }
        while next_frame <= now {
            let ft = next_frame;
            next_frame += vio_dt;
            let (Some((pi, qi)), Some((p0, _)), Some((p1, _))) = (
                imu_pose(ft),
                imu_pose(ft - 5_000_000),
                imu_pose(ft + 5_000_000),
            ) else {
                continue; // Pimax lost: our synthetic tracker is lost too
            };
            let v = geom::scale(geom::sub(p1, p0), 100.0);
            let qn = qexp(rng.vec(p.noise.1.to_radians()));
            let s = VioSample {
                ts_ns: ft,
                pos: geom::add(geom::add(qrot(wq, pi), woff), rng.vec(p.noise.0)),
                q: qnorm(qmul(qmul(wq, qi), qn)),
                vel: geom::add(qrot(wq, v), rng.vec(p.noise.2)),
                q3dof: qi,
            };
            pending.push_back((ft + lat, s));
        }
        let mut delivered = false;
        while pending.front().is_some_and(|(due, _)| *due <= now) {
            let (_, s) = pending.pop_front().unwrap();
            h.update(now, Some(s));
            delivered = true;
        }
        if now >= next_q && now >= t_start {
            next_q += q_dt;
            if !delivered {
                h.update(now, None);
            }
            let at = now + pred;
            let Some((_, q3)) = imu_pose(at).or_else(|| imu_pose(now)) else {
                continue;
            };
            let r3 = Rel {
                q: q3,
                ang_vel: [0.0; 3],
                flags: flags::ORIENTATION_VALID | flags::ORIENTATION_TRACKED,
                ..Default::default()
            };
            let o = h.head(now, at, &r3);
            outs.push((at, o));
            epochs.push(h.sup.acquisitions);
        }
    }
    // Score.
    let warm = t_start + 2_000_000_000;
    // Each acquisition sets a new room alignment: one constant offset per acquisition (robust median); the
    // first 0.5 s after an acquisition (corrections blending in) are not scored.
    let mut tracked: Vec<(i64, Rel, u64)> = Vec::new();
    let mut ep_start: std::collections::HashMap<u64, i64> = Default::default();
    for ((t, o), e) in outs.iter().zip(&epochs) {
        if o.flags & flags::POSITION_TRACKED == 0 {
            continue;
        }
        let st = *ep_start.entry(*e).or_insert(*t);
        if *t > warm && *t > st + 500_000_000 && head_pose(*t).is_some() {
            tracked.push((*t, *o, *e));
        }
    }
    if tracked.len() < 100 {
        return Err(format!("only {} tracked queries", tracked.len()));
    }
    let mut offs: std::collections::HashMap<u64, [Vec<f64>; 3]> = Default::default();
    for (t, o, e) in &tracked {
        let d = geom::sub(o.pos, head_pose(*t).unwrap().0);
        let v = offs.entry(*e).or_default();
        for c in 0..3 {
            v[c].push(d[c]);
        }
    }
    let offm: std::collections::HashMap<u64, Vec3> = offs
        .into_iter()
        .map(|(e, mut v)| {
            (
                e,
                [
                    pct(&mut v[0], 0.5),
                    pct(&mut v[1], 0.5),
                    pct(&mut v[2], 0.5),
                ],
            )
        })
        .collect();
    let (mut ra, mut rf, mut pa, mut pf, mut sf) = (vec![], vec![], vec![], vec![], vec![]);
    let (mut lag_num, mut lag_den) = (0.0, 0.0);
    for (i, (t, o, e)) in tracked.iter().enumerate() {
        let (pt, qt) = head_pose(*t).unwrap();
        let re = qangle_deg(qmul(qconj(o.q), qt));
        let pe = geom::norm(geom::sub(o.pos, geom::add(pt, offm[e]))) * 1000.0;
        let Some((wb, _, _)) = truth.deriv(*t, 4.0) else {
            continue;
        };
        let ww = qrot(qt, wb); // world angular velocity
                               // Lag: error rotation projected on the motion direction: q_err ≈ exp(-ω L).
        let ev = qlog(qmul(qt, qconj(o.q)));
        lag_num += geom::dot(ev, ww);
        lag_den += geom::dot(ww, ww);
        ra.push(re);
        pa.push(pe);
        let fast = geom::norm(wb).to_degrees() >= p.fast_dps;
        if fast {
            rf.push(re);
            pf.push(pe);
            if i > 0 {
                let (tp, op, _) = &tracked[i - 1];
                if *t - *tp < 2 * q_dt {
                    let (_, qtp) = head_pose(*tp).unwrap();
                    let d_out = qmul(qconj(op.q), o.q);
                    let d_true = qmul(qconj(qtp), qt);
                    sf.push(qangle_deg(qmul(qconj(d_out), d_true)));
                }
            }
        }
    }
    if std::env::var("WINBENCH_DEBUG").is_ok() {
        let mut worst: Vec<(f64, i64)> = tracked
            .iter()
            .filter_map(|(t, o, _)| {
                head_pose(*t).map(|(_, qt)| (qangle_deg(qmul(qconj(o.q), qt)), *t))
            })
            .collect();
        worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
        for (e, t) in worst.iter().take(8) {
            let w = truth
                .deriv(*t, 4.0)
                .map(|x| geom::norm(x.0).to_degrees())
                .unwrap_or(f64::NAN);
            eprintln!(
                "worst rot {e:.2} deg at {:.3} s (truth {w:.0} deg/s)",
                (*t - t_start) as f64 / 1e9
            );
        }
        if let Some(l) = h.corr_log.as_ref() {
            for c in l.iter().filter(|c| c.rot > 0.05) {
                eprintln!(
                    "corr at {:.3} s rot {:.2} deg pos {:.1} mm pose_only {}",
                    (c.ts_ns - t_start) as f64 / 1e9,
                    c.rot.to_degrees(),
                    geom::norm(c.offset) * 1000.0,
                    c.pose_only
                );
            }
        }
    }
    let p3 = |v: &mut Vec<f64>| {
        [
            pct(&mut v.clone(), 0.5),
            pct(&mut v.clone(), 0.95),
            pct(v, 1.0),
        ]
    };
    Ok(OursResult {
        queries: ra.len(),
        rot_all: p3(&mut ra),
        rot_fast: p3(&mut rf),
        pos_all: p3(&mut pa),
        pos_fast: p3(&mut pf),
        rot_step_fast: rms(&sf),
        lag_ms: lag_num / lag_den.max(1e-12) * 1000.0,
        fast_n: rf.len(),
        losses: h.sup.losses,
        max_corr: (h.corr.max_pos * 1000.0, h.corr.max_rot.to_degrees()),
    })
}

/// Same scoring for Pimax's own "now" pose (what an app gets from LibPVR at that time, no display prediction).
pub fn score_pimax(rows: &[PvrRow], fit: &Fit, fast_dps: f64) -> OursResult {
    let truth = Truth::new(rows, fit.lag_vec_ms, TRUTH_SIGMA_MS);
    let (mut ra, mut rf, mut pa, mut pf) = (vec![], vec![], vec![], vec![]);
    let (mut lag_num, mut lag_den) = (0.0, 0.0);
    for r in rows.iter().step_by(7) {
        if r.status != 3 {
            continue;
        }
        let (Some((pt, qt)), Some((wb, _, _))) = (truth.at(r.ts_ns), truth.deriv(r.ts_ns, 4.0))
        else {
            continue;
        };
        let re = qangle_deg(qmul(qconj(r.q), qt));
        let pe = geom::norm(geom::sub(r.p, pt)) * 1000.0;
        let ww = qrot(qt, wb);
        let ev = qlog(qmul(qt, qconj(r.q)));
        lag_num += geom::dot(ev, ww);
        lag_den += geom::dot(ww, ww);
        ra.push(re);
        pa.push(pe);
        if geom::norm(wb).to_degrees() >= fast_dps {
            rf.push(re);
            pf.push(pe);
        }
    }
    let p3 = |v: &mut Vec<f64>| {
        [
            pct(&mut v.clone(), 0.5),
            pct(&mut v.clone(), 0.95),
            pct(v, 1.0),
        ]
    };
    OursResult {
        queries: ra.len(),
        fast_n: rf.len(),
        rot_all: p3(&mut ra),
        rot_fast: p3(&mut rf),
        pos_all: p3(&mut pa),
        pos_fast: p3(&mut pf),
        lag_ms: lag_num / lag_den.max(1e-12) * 1000.0,
        ..Default::default()
    }
}

/// Exposure statistics: Pimax's AE command values (tracking cameras 0/1) overall and while the head turns fast.
pub fn exposure_stats(cap: &WinCapture, fit: &Fit) -> Option<(f64, f64, f64, f64, usize)> {
    if cap.exp.is_empty() {
        return None;
    }
    let gy = gyro_series(&cap.imu, cap.span_ns.0, cap.span_ns.1);
    let (mut all, mut fast, mut gains) = (vec![], vec![], vec![]);
    for e in &cap.exp {
        let x = e.exp[0] as f64;
        all.push(x);
        gains.push(e.gain[0] as f64);
        if let Some(g) = gy.at(e.ts_ns) {
            if geom::norm(geom::sub(g, fit.gyro_bias)).to_degrees() > 150.0 {
                fast.push(x);
            }
        }
    }
    let n = cap.exp.len();
    Some((
        pct(&mut all.clone(), 0.5),
        pct(&mut all, 1.0),
        pct(&mut fast, 0.5),
        pct(&mut gains, 0.5),
        n,
    ))
}

/// A take: pcap + pose log found from a take name (e.g. `fast-turns`) or a pose directory.
pub fn find_take(root: &Path, name: &str) -> Result<(PathBuf, PathBuf), String> {
    let caps = root.join("usb-captures");
    let mut pcap = None;
    let mut poses = None;
    for e in std::fs::read_dir(&caps)
        .map_err(|e| format!("{}: {e}", caps.display()))?
        .flatten()
    {
        let n = e.file_name().to_string_lossy().to_string();
        if n.ends_with(".pcap") && n.contains(name) {
            pcap = Some(e.path());
        }
    }
    for e in std::fs::read_dir(root)
        .map_err(|e| format!("{}: {e}", root.display()))?
        .flatten()
    {
        let n = e.file_name().to_string_lossy().to_string();
        if n.ends_with("-poses") && n.contains(name) {
            for d in std::fs::read_dir(e.path())
                .map_err(|e| e.to_string())?
                .flatten()
            {
                let f = d.path().join("pimax-poses.csv");
                if f.exists() {
                    poses = Some(f);
                }
            }
        }
    }
    match (pcap, poses) {
        (Some(a), Some(b)) => Ok((a, b)),
        (a, b) => Err(format!(
            "take {name:?} in {}: pcap {}, pose log {}",
            root.display(),
            if a.is_some() { "found" } else { "missing" },
            if b.is_some() { "found" } else { "missing" }
        )),
    }
}

/// Everything for one take.
pub struct Take {
    /// Capture.
    pub cap: WinCapture,
    /// Pose log.
    pub rows: Vec<PvrRow>,
    /// Fit.
    pub fit: Fit,
    /// Pimax statistics.
    pub stats: PvrStats,
}

/// Below this RMS angular speed (deg/s) a take cannot fit the head -> IMU rotation, lag and lever: they are
/// taken from a moving take (`reference`).
pub const MIN_FIT_DPS: f64 = 10.0;

/// Load and fit a take. `reference`: the fit of a moving take used for the device constants (rotation, lag,
/// lever, gravity) when this take hardly moves.
pub fn load_take(pcap: &Path, poses: &Path, reference: Option<&Fit>) -> Result<Take, String> {
    let mut cap = read_pcap(pcap, true)?;
    let clock = fit_clock(&mut cap.imu);
    let text = std::fs::read_to_string(poses).map_err(|e| format!("{}: {e}", poses.display()))?;
    let rows = parse_pvr(&text)?;
    let mut fit = fit(&cap, &rows, clock)?;
    if fit.omega_rms_dps < MIN_FIT_DPS {
        if let Some(r) = reference {
            // Still take: the bias is the mean gyro; everything else from the moving take.
            let still: Vec<Vec3> = cap.imu.iter().map(|s| s.gyro).collect();
            let bias = mean3(&still);
            let up = fit.up_world;
            fit = Fit {
                clock,
                gyro_bias: bias,
                order_roughness: fit.order_roughness,
                accel_still_norm: fit.accel_still_norm,
                omega_rms_dps: fit.omega_rms_dps,
                ..r.clone()
            };
            let _ = up;
            let t0 = rows[0].ts_ns.max(cap.imu[0].ts_ns) + 200_000_000;
            let t1 = rows[rows.len() - 1]
                .ts_ns
                .min(cap.imu[cap.imu.len() - 1].ts_ns)
                - 200_000_000;
            let mut tmp = fit.clone();
            fit_accel(&mut tmp, &cap.imu, &rows, t0, t1);
            fit.up_world = tmp.up_world;
            fit.accel_still_norm = tmp.accel_still_norm;
            fit.w_frame = format!("{} (device constants from the reference take)", r.w_frame);
        }
    }
    let stats = pvr_stats(&cap, &rows, &fit);
    Ok(Take {
        cap,
        rows,
        fit,
        stats,
    })
}

/// One-line summary of a scored run.
pub fn score_line(name: &str, r: &OursResult) -> String {
    format!(
        "{name:<34} rot p50/p95/max {:.2}/{:.2}/{:.2} deg, fast {:.2}/{:.2}/{:.2} deg | pos {:.1}/{:.1}/{:.1} mm, fast {:.1}/{:.1}/{:.1} mm | lag {:+.1} ms | judder {:.3} deg | n {} fast {} | lost {} corr max {:.0} mm {:.1} deg",
        r.rot_all[0], r.rot_all[1], r.rot_all[2], r.rot_fast[0], r.rot_fast[1], r.rot_fast[2],
        r.pos_all[0], r.pos_all[1], r.pos_all[2], r.pos_fast[0], r.pos_fast[1], r.pos_fast[2],
        r.lag_ms, r.rot_step_fast, r.queries, r.fast_n, r.losses, r.max_corr.0, r.max_corr.1
    )
}

/// Variants of our pipeline run on every take (name, params).
pub fn variants(base: &OursParams) -> Vec<(String, OursParams)> {
    let mut v = vec![("ours now (predict 0)".to_string(), base.clone())];
    let mut p20 = base.clone();
    p20.predict_ms = 20.0;
    v.push(("ours predict 20 ms".to_string(), p20.clone()));
    if std::env::var("WINBENCH_EXPLORE").is_ok() {
        let mut a = base.clone();
        a.noise.1 = 0.0;
        v.push(("explore: tracker rot noise 0".into(), a));
        for kw in [0.0, 0.1] {
            let mut a = base.clone();
            a.head.k_w = kw;
            v.push((format!("explore: k_w {kw}"), a.clone()));
        }
        for tr in [0.05, 0.2] {
            let mut a = base.clone();
            a.head.tau_rot_s = tr;
            v.push((format!("explore: tau_rot {tr}"), a.clone()));
        }
        let mut a = base.clone();
        a.latency_ms = 50.0;
        v.push(("explore: latency 50".into(), a));
        let mut a = base.clone();
        a.vio_hz = 50.0;
        v.push(("explore: tracker 50 Hz".into(), a));
    }
    if let Ok(x) = std::env::var("WINBENCH_GYRO_SCALE") {
        if let Ok(k) = x.trim().parse::<f64>() {
            let mut a = base.clone();
            a.gyro_scale = [k; 3];
            v.push((format!("ours now, gyro x{k}"), a.clone()));
            a.predict_ms = 20.0;
            v.push((format!("ours predict 20, gyro x{k}"), a));
        }
    }
    v
}

/// Text report for one take.
pub fn report(name: &str, t: &Take, p: &OursParams) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let f = &t.fit;
    let st = &t.stats;
    let _ = writeln!(
        s,
        "== take {name}: {} IMU samples, {} pose rows over {:.1} s",
        t.cap.imu.len(),
        st.rows,
        st.seconds
    );
    let _ = writeln!(
        s,
        "camera iso bytes captured {:.1} % of {:.0} MB (frames cannot be rebuilt); {} AE commands",
        100.0 * t.cap.cam_bytes.1 as f64 / t.cap.cam_bytes.0.max(1) as f64,
        t.cap.cam_bytes.0 as f64 / 1e6,
        t.cap.exp.len()
    );
    let _ = writeln!(
        s,
        "clock: drift {:+.1} ppm, USB arrival after the fitted clock p50/p95/p99/max {:.2}/{:.2}/{:.2}/{:.2} ms",
        f.clock.ppm, f.clock.delay_ms[0], f.clock.delay_ms[1], f.clock.delay_ms[2], f.clock.delay_ms[3]
    );
    let _ = writeln!(
        s,
        "sample order: roughness libpimax {:.3} vs swapped {:.3} deg/s ({})",
        f.order_roughness.0,
        f.order_roughness.1,
        if f.order_roughness.0 <= f.order_roughness.1 {
            "libpimax order right"
        } else {
            "SWAPPED order fits better"
        }
    );
    let _ = writeln!(
        s,
        "Pimax lag behind the IMU: |w| xcorr {:+.1} ms (r {:.3}), vector fit {:+.2} ms; resid {:.2} deg/s of {:.1} deg/s RMS",
        f.lag_ms, f.lag_corr, f.lag_vec_ms, f.omega_resid_dps, f.omega_rms_dps
    );
    let _ = writeln!(
        s,
        "head->IMU rotation {:.2} deg; gyro bias ({:.3} {:.3} {:.3}) deg/s; gyro scale ({:.4} {:.4} {:.4}) offdiag {:.4}",
        f.r_angle_deg,
        f.gyro_bias[0].to_degrees(), f.gyro_bias[1].to_degrees(), f.gyro_bias[2].to_degrees(),
        f.gyro_scale[0], f.gyro_scale[1], f.gyro_scale[2], f.gyro_offdiag
    );
    let tab: Vec<String> = f
        .smooth_table
        .iter()
        .map(|(a, b, c)| format!("{a:.0}:{b:+.1}/{c:.2}"))
        .collect();
    let _ = writeln!(
        s,
        "smoothing fit tau ms:lag/resid  {} -> best tau {:.0} ms",
        tab.join("  "),
        f.smooth_tau_ms
    );
    let _ = writeln!(
        s,
        "reported head_w frame {}, lag vs quaternion w {:+.1} ms",
        f.w_frame, f.w_lag_ms
    );
    let _ = writeln!(
        s,
        "accel: IMU at ({:.3} {:.3} {:.3}) m in the head frame, bias ({:.3} {:.3} {:.3}), resid {:.3} m/s2; up in Pimax world ({:.3} {:.3} {:.3}); still |f| {:.4}",
        f.imu_in_head[0], f.imu_in_head[1], f.imu_in_head[2], f.accel_bias[0], f.accel_bias[1], f.accel_bias[2],
        f.accel_resid, f.up_world[0], f.up_world[1], f.up_world[2], f.accel_still_norm
    );
    let _ = writeln!(
        s,
        "Pimax: tracked {:.1} %, losses {:?}, jumps > 5 cm {} (max {:.2} m); gyro speed p50/p95/max {:.0}/{:.0}/{:.0} deg/s, s above 100/200/300: {:.1}/{:.1}/{:.1}; head speed p95/max {:.2}/{:.2} m/s",
        st.tracked_frac * 100.0,
        st.losses.iter().map(|(a, b)| format!("{a:.1}+{b:.1}s")).collect::<Vec<_>>(),
        st.jumps.0, st.jumps.1, st.speed_dps[0], st.speed_dps[1], st.speed_dps[2],
        st.secs_above[0], st.secs_above[1], st.secs_above[2], st.lin_speed[0], st.lin_speed[1]
    );
    let _ = writeln!(
        s,
        "Pimax still jitter: {:.3} mm, {:.4} deg RMS over {:.1} s still; lag error in fast turns p50/p95 {:.2}/{:.2} deg",
        st.still_jitter_mm, st.still_jitter_deg, st.still_secs, st.fast_lag_err_deg[0], st.fast_lag_err_deg[1]
    );
    if let Some((m, mx, fm, g, n)) = exposure_stats(&t.cap, f) {
        let _ = writeln!(s, "Pimax AE: exposure median {m:.0} max {mx:.0}, median while > 150 deg/s {fm:.0}, gain median {g:.0} ({n} commands)");
    }
    let _ = writeln!(
        s,
        "{}",
        score_line("Pimax now (LibPVR)", &score_pimax(&t.rows, f, p.fast_dps))
    );
    for (n, q) in variants(p) {
        match run_ours(&t.cap, &t.rows, f, &q) {
            Ok(r) => {
                let _ = writeln!(s, "{}", score_line(&n, &r));
            }
            Err(e) => {
                let _ = writeln!(s, "{n:<34} failed: {e}");
            }
        }
    }
    s
}

/// Tab-separated summary row (for the analysis tables).
pub fn tsv_row(name: &str, t: &Take, p: &OursParams) -> String {
    let pm = score_pimax(&t.rows, &t.fit, p.fast_dps);
    let mut s = format!(
        "take\t{name}\tlag_ms\t{:.2}\tpimax_rot_fast_p95\t{:.3}\tpimax_jitter_mm\t{:.3}\n",
        t.fit.lag_vec_ms, pm.rot_fast[1], t.stats.still_jitter_mm
    );
    for (n, q) in variants(p) {
        if let Ok(r) = run_ours(&t.cap, &t.rows, &t.fit, &q) {
            s += &format!(
                "{n}\t{:.3}\t{:.3}\t{:.3}\t{:.3}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\t{:.2}\n",
                r.rot_all[0],
                r.rot_all[1],
                r.rot_fast[0],
                r.rot_fast[1],
                r.pos_all[1],
                r.pos_fast[0],
                r.pos_fast[1],
                r.lag_ms,
                r.rot_step_fast
            );
        }
    }
    s
}

/// Debug dump: 1 kHz grid of gyro (bias removed, IMU frame) and Pimax ω (head frame, from quaternions), TSV.
pub fn dump(t: &Take, path: &Path) -> std::io::Result<()> {
    let r0 = t.rows[0].ts_ns;
    let r1 = t.rows[t.rows.len() - 1].ts_ns;
    let gy = gyro_series(&t.cap.imu, r0, r1);
    let om = pvr_omega_series(&t.rows, r0, r1);
    let mut s = String::from("t\tgx\tgy\tgz\tpx\tpy\tpz\n");
    for k in 0..om.v.len() {
        let tt = om.t0 + k as i64 * om.dt;
        if let (Some(g), Some(o)) = (gy.at(tt), om.v[k]) {
            let g = geom::sub(g, t.fit.gyro_bias);
            s += &format!(
                "{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\t{:.4}\n",
                (tt - r0) as f64 / 1e9,
                g[0],
                g[1],
                g[2],
                o[0],
                o[1],
                o[2]
            );
        }
    }
    std::fs::write(path, s)
}

/// Basalt in fast head motion on one of our Linux recordings (EuRoC IMU + a `slambatch` `tracking.csv`): per
/// tracker interval, the tracker's relative rotation vs the gyro's (deg), the tracker's speed, and gaps, binned
/// by the gyro's mean angular speed (deg/s). Returns text rows.
pub fn basalt_fast(imu: &[crate::euroc::ImuRow], est: &[PoseRow]) -> String {
    let bins = [0.0, 50.0, 150.0, 300.0, f64::INFINITY];
    let mut rot: Vec<Vec<f64>> = vec![Vec::new(); 4];
    let mut spd: Vec<Vec<f64>> = vec![Vec::new(); 4];
    let mut gaps = [0usize; 4];
    let mut peak = 0.0f64;
    for w in est.windows(2) {
        let (a, b) = (&w[0], &w[1]);
        let i0 = imu.partition_point(|s| s.ts < a.ts);
        let i1 = imu.partition_point(|s| s.ts < b.ts);
        if i1 <= i0 + 2 || i1 >= imu.len() {
            continue;
        }
        let mut q = [0.0, 0.0, 0.0, 1.0];
        let mut wsum = 0.0;
        let mut t = a.ts;
        for s in &imu[i0..=i1] {
            let end = s.ts.min(b.ts);
            if end > t {
                q = qmul(q, qexp(geom::scale(s.gyro, (end - t) as f64 / 1e9)));
                wsum += geom::norm(s.gyro) * (end - t) as f64;
                t = end;
            }
        }
        let dt = (b.ts - a.ts) as f64;
        let wm = (wsum / dt).to_degrees();
        peak = peak.max(wm);
        let k = bins
            .windows(2)
            .position(|x| wm >= x[0] && wm < x[1])
            .unwrap_or(3);
        if dt > 60e6 {
            gaps[k] += 1;
        }
        let d = qmul(qconj(a.q), b.q);
        rot[k].push(qangle_deg(qmul(qconj(d), q)));
        spd[k].push(geom::norm(geom::sub(b.p, a.p)) / (dt / 1e9));
    }
    let mut s = format!("peak mean angular speed per interval {peak:.0} deg/s\n");
    for k in 0..4 {
        let n = rot[k].len();
        s += &format!(
            "  {:>4.0}-{:<4.0} deg/s: {n:>5} intervals, rot vs gyro p50/p95/max {:.3}/{:.3}/{:.3} deg, speed p95/max {:.2}/{:.2} m/s, gaps > 60 ms {}\n",
            bins[k], bins[k + 1].min(9999.0),
            pct(&mut rot[k].clone(), 0.5), pct(&mut rot[k].clone(), 0.95), pct(&mut rot[k], 1.0),
            pct(&mut spd[k].clone(), 0.95), pct(&mut spd[k], 1.0), gaps[k]
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unwrap_handles_u32_wrap() {
        let mut u = Unwrap { last: None, ext: 0 };
        assert_eq!(u.push(u32::MAX - 999), (u32::MAX - 999) as i64 * 1000);
        assert_eq!(u.push(1000), (u32::MAX as i64 + 1001) * 1000);
    }

    #[test]
    fn clock_fit_takes_the_lower_envelope() {
        // Device clock 20 ppm slow, arrivals 1-3 ms late with a 1 ms floor.
        let mut imu: Vec<WinImu> = (0..20_000)
            .map(|i| {
                let dev = i as i64 * 1_000_000;
                let late = 1_000_000 + (i * 7919 % 2000) as i64 * 1000;
                WinImu {
                    dev_ns: dev,
                    arr_ns: 5_000_000_000 + dev + dev / 50_000 + late,
                    ts_ns: 0,
                    accel: [0.0; 3],
                    gyro: [0.0; 3],
                    slot: 0,
                }
            })
            .collect();
        let c = fit_clock(&mut imu);
        assert!((c.ppm - 20.0).abs() < 1.0, "{c:?}");
        assert!(c.delay_ms[0] > 0.5 && c.delay_ms[3] < 3.5, "{c:?}");
    }

    #[test]
    fn pvr_rows_parse_and_map_to_the_host_clock() {
        let mut t = String::from("t_qpc,utc_filetime,pvr_now,track_res,head_time,head_status,head_px,head_py,head_pz,head_qx,head_qy,head_qz,head_qw,head_wx,head_wy,head_wz,head_vx,head_vy,head_vz\n");
        for i in 0..200 {
            let ht = 100.0 + i as f64 * 0.0015;
            let ft = 134_365_129_912_124_688i64 + i * 15_000 + (i % 3) * 2000;
            t += &format!("0,{ft},{ht},0,{ht},3,0,1.6,0,0,0,0,1,0,0,0,0,0,0\n");
        }
        t += "0,1343"; // cut last line
        let r = parse_pvr(&t).unwrap();
        assert_eq!(r.len(), 200);
        let d = (r[199].ts_ns - r[0].ts_ns) as f64 / 1e6;
        assert!((d - 298.5).abs() < 0.01, "{d}");
    }
}
