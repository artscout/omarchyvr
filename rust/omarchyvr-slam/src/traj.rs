//! Trajectories and error metrics: load (EuRoC/Monado CSV `ts,px,py,pz,qw,qx,qy,qz`, or the Windows PVR pose log
//! `pimax-poses.csv`), associate by time, rigid alignment (Horn, rotation + translation, no scale: VIO is
//! metric), ATE / RPE, and loop-closure drift for runs that start and end at the same spot.

use pimax_cam::geom::{self, Mat3, Vec3};
use pimax_cam::slam::{qangle_deg, qconj, qmul, qnorm, Quat};

/// One pose.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PoseRow {
    /// ns.
    pub ts: i64,
    /// Position.
    pub p: Vec3,
    /// Orientation x y z w.
    pub q: Quat,
}

/// Input format.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    /// `ts_ns,px,py,pz,qw,qx,qy,qz` (EuRoC ground truth, Monado tracking.csv / slambatch output).
    Euroc,
    /// Windows PVR log: columns head_time (s), head_status, head_px..head_qw; status 1 rows dropped.
    PimaxWindows,
}

impl Format {
    /// Parse a name.
    pub fn parse(s: &str) -> Result<Format, String> {
        match s {
            "euroc" | "monado" => Ok(Format::Euroc),
            "pimax-windows" | "pvr" => Ok(Format::PimaxWindows),
            _ => Err(format!(
                "unknown trajectory format {s:?} (euroc, pimax-windows)"
            )),
        }
    }
}

/// Parse a trajectory.
pub fn parse(text: &str, fmt: Format) -> Result<Vec<PoseRow>, String> {
    let mut out = Vec::new();
    match fmt {
        Format::Euroc => {
            for (n, l) in text.lines().enumerate() {
                let l = l.trim();
                if l.is_empty() || l.starts_with('#') {
                    continue;
                }
                let f: Result<Vec<f64>, _> =
                    l.split(',').map(|x| x.trim().parse::<f64>()).collect();
                let f = f.map_err(|_| format!("line {}: not numbers", n + 1))?;
                if f.len() < 8 {
                    return Err(format!("line {}: {} fields, want 8", n + 1, f.len()));
                }
                out.push(PoseRow {
                    ts: f[0] as i64,
                    p: [f[1], f[2], f[3]],
                    q: qnorm([f[5], f[6], f[7], f[4]]),
                });
            }
        }
        Format::PimaxWindows => {
            let mut lines = text.lines();
            let head: Vec<&str> = lines.next().ok_or("empty file")?.split(',').collect();
            let col = |name: &str| {
                head.iter()
                    .position(|h| h.trim() == name)
                    .ok_or_else(|| format!("column {name} missing"))
            };
            let (ct, cs) = (col("head_time")?, col("head_status")?);
            let cp = [col("head_px")?, col("head_py")?, col("head_pz")?];
            let cq = [
                col("head_qx")?,
                col("head_qy")?,
                col("head_qz")?,
                col("head_qw")?,
            ];
            let mut last_ts = i64::MIN;
            for l in lines {
                let f: Vec<&str> = l.split(',').collect();
                let get = |i: usize| f.get(i).and_then(|x| x.trim().parse::<f64>().ok());
                let (Some(t), Some(st)) = (get(ct), get(cs)) else {
                    continue;
                };
                if st as i64 == 1 {
                    continue; // sentinel pose (y ~ -48.9)
                }
                let ts = (t * 1e9) as i64;
                if ts <= last_ts {
                    continue; // the log repeats the same PVR sample many times
                }
                let p = [get(cp[0]), get(cp[1]), get(cp[2])];
                let q = [get(cq[0]), get(cq[1]), get(cq[2]), get(cq[3])];
                if let ([Some(px), Some(py), Some(pz)], [Some(qx), Some(qy), Some(qz), Some(qw)]) =
                    (p, q)
                {
                    last_ts = ts;
                    out.push(PoseRow {
                        ts,
                        p: [px, py, pz],
                        q: qnorm([qx, qy, qz, qw]),
                    });
                }
            }
        }
    }
    out.sort_by_key(|r| r.ts);
    Ok(out)
}

/// Reference position at `ts` by linear interpolation (None outside or across a gap > `max_gap_ns`).
pub fn interp(r: &[PoseRow], ts: i64, max_gap_ns: i64) -> Option<PoseRow> {
    let i = r.partition_point(|x| x.ts < ts);
    if i < r.len() && r[i].ts == ts {
        return Some(r[i]);
    }
    if i == 0 || i >= r.len() {
        return None;
    }
    let (a, b) = (r[i - 1], r[i]);
    if b.ts - a.ts > max_gap_ns {
        return None;
    }
    let u = (ts - a.ts) as f64 / (b.ts - a.ts) as f64;
    let p = geom::add(geom::scale(a.p, 1.0 - u), geom::scale(b.p, u));
    // nlerp is fine at these spacings
    let mut qb = b.q;
    if a.q[0] * qb[0] + a.q[1] * qb[1] + a.q[2] * qb[2] + a.q[3] * qb[3] < 0.0 {
        qb = [-qb[0], -qb[1], -qb[2], -qb[3]];
    }
    let q = qnorm([
        a.q[0] * (1.0 - u) + qb[0] * u,
        a.q[1] * (1.0 - u) + qb[1] * u,
        a.q[2] * (1.0 - u) + qb[2] * u,
        a.q[3] * (1.0 - u) + qb[3] * u,
    ]);
    Some(PoseRow { ts, p, q })
}

/// Pairs (estimate, reference at the same time).
pub fn associate(
    est: &[PoseRow],
    reference: &[PoseRow],
    max_gap_ns: i64,
) -> Vec<(PoseRow, PoseRow)> {
    est.iter()
        .filter_map(|e| interp(reference, e.ts, max_gap_ns).map(|r| (*e, r)))
        .collect()
}

/// Largest-eigenvalue eigenvector of a symmetric 4x4 (cyclic Jacobi).
#[allow(clippy::needless_range_loop)] // index-heavy Jacobi rotations read better with indices
fn top_eigvec4(mut a: [[f64; 4]; 4]) -> [f64; 4] {
    let mut v = [[0.0; 4]; 4];
    for (i, row) in v.iter_mut().enumerate() {
        row[i] = 1.0;
    }
    for _ in 0..60 {
        let mut off = 0.0;
        for p in 0..4 {
            for q in p + 1..4 {
                off += a[p][q] * a[p][q];
            }
        }
        if off < 1e-22 {
            break;
        }
        for p in 0..4 {
            for q in p + 1..4 {
                if a[p][q].abs() < 1e-300 {
                    continue;
                }
                let th = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = th.signum() / (th.abs() + (th * th + 1.0).sqrt());
                let t = if th == 0.0 { 1.0 } else { t };
                let c = 1.0 / (t * t + 1.0).sqrt();
                let s = t * c;
                for k in 0..4 {
                    let (akp, akq) = (a[k][p], a[k][q]);
                    a[k][p] = c * akp - s * akq;
                    a[k][q] = s * akp + c * akq;
                }
                for k in 0..4 {
                    let (apk, aqk) = (a[p][k], a[q][k]);
                    a[p][k] = c * apk - s * aqk;
                    a[q][k] = s * apk + c * aqk;
                }
                for row in v.iter_mut() {
                    let (vkp, vkq) = (row[p], row[q]);
                    row[p] = c * vkp - s * vkq;
                    row[q] = s * vkp + c * vkq;
                }
            }
        }
    }
    let mut best = 0;
    for i in 1..4 {
        if a[i][i] > a[best][best] {
            best = i;
        }
    }
    [v[0][best], v[1][best], v[2][best], v[3][best]]
}

/// Rigid alignment `ref ≈ R est + t` (Horn 1987, unit quaternions). Returns (R, t).
pub fn align(pairs: &[(PoseRow, PoseRow)]) -> (Mat3, Vec3) {
    let n = pairs.len().max(1) as f64;
    let mut ce = [0.0; 3];
    let mut cr = [0.0; 3];
    for (e, r) in pairs {
        ce = geom::add(ce, geom::scale(e.p, 1.0 / n));
        cr = geom::add(cr, geom::scale(r.p, 1.0 / n));
    }
    let mut s = [[0.0; 3]; 3];
    for (e, r) in pairs {
        let a = geom::sub(e.p, ce);
        let b = geom::sub(r.p, cr);
        for i in 0..3 {
            for j in 0..3 {
                s[i][j] += a[i] * b[j];
            }
        }
    }
    let (sxx, sxy, sxz) = (s[0][0], s[0][1], s[0][2]);
    let (syx, syy, syz) = (s[1][0], s[1][1], s[1][2]);
    let (szx, szy, szz) = (s[2][0], s[2][1], s[2][2]);
    let nmat = [
        [sxx + syy + szz, syz - szy, szx - sxz, sxy - syx],
        [syz - szy, sxx - syy - szz, sxy + syx, szx + sxz],
        [szx - sxz, sxy + syx, -sxx + syy - szz, syz + szy],
        [sxy - syx, szx + sxz, syz + szy, -sxx - syy + szz],
    ];
    let e = top_eigvec4(nmat); // (w, x, y, z)
    let q = qnorm([e[1], e[2], e[3], e[0]]);
    let r = pimax_cam::slam::mat_of(q);
    let t = geom::sub(cr, geom::mat_vec(&r, ce));
    (r, t)
}

/// Metrics of one comparison.
#[derive(Clone, Debug, PartialEq)]
pub struct Metrics {
    /// Associated pairs.
    pub pairs: usize,
    /// Absolute trajectory error after alignment: RMSE, median, max (m).
    pub ate_rmse: f64,
    /// Median ATE.
    pub ate_median: f64,
    /// Max ATE.
    pub ate_max: f64,
    /// Relative pose error over `rpe_delta_s`: translation RMSE (m) and rotation RMSE (deg).
    pub rpe_trans_rmse: f64,
    /// Rotation RPE.
    pub rpe_rot_rmse_deg: f64,
    /// Interval used.
    pub rpe_delta_s: f64,
    /// Reference path length (m).
    pub path_len: f64,
}

/// ATE + RPE between an estimate and a reference.
pub fn compare(
    est: &[PoseRow],
    reference: &[PoseRow],
    max_gap_ns: i64,
    rpe_delta_s: f64,
) -> Result<Metrics, String> {
    let pairs = associate(est, reference, max_gap_ns);
    if pairs.len() < 10 {
        return Err(format!(
            "only {} time-matched poses (clocks or time ranges differ?)",
            pairs.len()
        ));
    }
    let (r, t) = align(&pairs);
    let mut errs: Vec<f64> = pairs
        .iter()
        .map(|(e, rf)| geom::norm(geom::sub(geom::add(geom::mat_vec(&r, e.p), t), rf.p)))
        .collect();
    let ate_rmse = (errs.iter().map(|x| x * x).sum::<f64>() / errs.len() as f64).sqrt();
    let ate_max = errs.iter().cloned().fold(0.0, f64::max);
    errs.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let ate_median = errs[errs.len() / 2];
    // RPE: for each pair i, the pair j about delta later.
    let d = (rpe_delta_s * 1e9) as i64;
    let (mut st, mut sr, mut nr) = (0.0, 0.0, 0usize);
    let mut j = 0;
    for i in 0..pairs.len() {
        while j < pairs.len() && pairs[j].0.ts < pairs[i].0.ts + d {
            j += 1;
        }
        if j >= pairs.len() {
            break;
        }
        let ((e0, r0), (e1, r1)) = (pairs[i], pairs[j]);
        // relative motion in the first pose's frame
        let de = pimax_cam::slam::qrot(qconj(e0.q), geom::sub(e1.p, e0.p));
        let dr = pimax_cam::slam::qrot(qconj(r0.q), geom::sub(r1.p, r0.p));
        st += geom::norm(geom::sub(de, dr)).powi(2);
        let qe = qmul(qconj(e0.q), e1.q);
        let qr = qmul(qconj(r0.q), r1.q);
        sr += qangle_deg(qmul(qconj(qe), qr)).powi(2);
        nr += 1;
    }
    let path_len: f64 = reference
        .windows(2)
        .map(|w| geom::norm(geom::sub(w[1].p, w[0].p)))
        .sum();
    Ok(Metrics {
        pairs: pairs.len(),
        ate_rmse,
        ate_median,
        ate_max,
        rpe_trans_rmse: if nr > 0 {
            (st / nr as f64).sqrt()
        } else {
            f64::NAN
        },
        rpe_rot_rmse_deg: if nr > 0 {
            (sr / nr as f64).sqrt()
        } else {
            f64::NAN
        },
        rpe_delta_s,
        path_len,
    })
}

/// Loop closure for a run that starts and ends at the same spot: (end-start distance m, yaw/total rotation
/// difference deg, path length m).
pub fn loop_drift(est: &[PoseRow], avg_s: f64) -> Option<(f64, f64, f64)> {
    let (first, last) = (est.first()?, est.last()?);
    let w = (avg_s * 1e9) as i64;
    let mean = |rows: &mut dyn Iterator<Item = &PoseRow>| {
        let v: Vec<Vec3> = rows.map(|r| r.p).collect();
        let n = v.len().max(1) as f64;
        v.iter()
            .fold([0.0; 3], |a, p| geom::add(a, geom::scale(*p, 1.0 / n)))
    };
    let a = mean(&mut est.iter().take_while(|r| r.ts < first.ts + w));
    let b = mean(&mut est.iter().rev().take_while(|r| r.ts > last.ts - w));
    let path: f64 = est
        .windows(2)
        .map(|w| geom::norm(geom::sub(w[1].p, w[0].p)))
        .sum();
    Some((
        geom::norm(geom::sub(b, a)),
        qangle_deg(qmul(qconj(first.q), last.q)),
        path,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::slam::{qrot, qyaw};

    fn circle(n: usize, noise: f64) -> Vec<PoseRow> {
        (0..n)
            .map(|i| {
                let t = i as f64 * 0.01;
                let k = (i as f64 * 12.9898).sin() * 43758.5453;
                let jit = (k - k.floor() - 0.5) * noise;
                PoseRow {
                    ts: (t * 1e9) as i64,
                    p: [t.cos() + jit, 0.1 * (3.0 * t).sin(), t.sin()],
                    q: qyaw(t),
                }
            })
            .collect()
    }

    #[test]
    fn alignment_recovers_rigid_transform() {
        let reference = circle(1000, 0.0);
        let w = qnorm([0.3, -0.2, 0.5, 0.8]);
        let off = [1.0, -2.0, 0.5];
        // estimate lives in another world: est = R^-1 (ref - off)
        let est: Vec<PoseRow> = reference
            .iter()
            .map(|r| PoseRow {
                ts: r.ts,
                p: qrot(qconj(w), geom::sub(r.p, off)),
                q: qmul(qconj(w), r.q),
            })
            .collect();
        let m = compare(&est, &reference, 20_000_000, 1.0).unwrap();
        assert!(m.ate_rmse < 1e-9, "{m:?}");
        assert!(
            m.rpe_trans_rmse < 1e-9 && m.rpe_rot_rmse_deg < 1e-6,
            "{m:?}"
        );
        assert!((m.path_len - 10.0).abs() < 0.5, "{m:?}");
    }

    #[test]
    fn noisy_estimate_has_error() {
        let reference = circle(1000, 0.0);
        let est = circle(1000, 0.02);
        let m = compare(&est, &reference, 20_000_000, 1.0).unwrap();
        assert!(m.ate_rmse > 0.003 && m.ate_rmse < 0.02, "{m:?}");
    }

    #[test]
    fn formats() {
        let e = "#timestamp [ns],p_RS_R_x [m],p_RS_R_y [m],p_RS_R_z [m],q_RS_w [],q_RS_x [],q_RS_y [],q_RS_z []\n100,1,2,3,1,0,0,0\n200,1,2,4,1,0,0,0\n";
        let v = parse(e, Format::Euroc).unwrap();
        assert_eq!(v.len(), 2);
        assert_eq!(v[1].p, [1.0, 2.0, 4.0]);
        assert_eq!(v[0].q, [0.0, 0.0, 0.0, 1.0]);
        let w = "t_qpc,head_time,head_status,head_px,head_py,head_pz,head_qx,head_qy,head_qz,head_qw\n1,416.9,3,-0.95,0.0,1.1,0,0,0,1\n2,416.9,3,-0.95,0.0,1.1,0,0,0,1\n3,416.95,1,0,-48.9,0,0,0,0,1\n4,417.0,3,-0.9,0.0,1.1,0,0,0,1\n";
        let v = parse(w, Format::PimaxWindows).unwrap();
        assert_eq!(v.len(), 2); // duplicate and sentinel dropped
        assert_eq!(v[1].ts, 417_000_000_000);
        let i = interp(&v, 416_950_000_000, 1_000_000_000).unwrap();
        assert!((i.p[0] + 0.925).abs() < 1e-9);
    }

    #[test]
    fn loop_closure() {
        let reference = circle(629, 0.0); // ~2 pi
        let (d, _, path) = loop_drift(&reference, 0.05).unwrap();
        assert!(d < 0.05, "{d}");
        assert!(path > 6.0);
    }
}
