//! Trajectory statistics that need no reference: rate and gaps, speed, still-segment noise (jitter while the
//! headset rests), high-frequency noise in motion (residual of a local quadratic fit), loop drift per metre.
//!
//! These numbers can be compared between our tracker and the Windows PVR pose log even though the two were never
//! recorded together (no paired frames): similar motion -> compare the distributions (`pimax-slam stats A --vs B`).

use crate::traj::{loop_drift, PoseRow};
use pimax_cam::geom::{self, Vec3};
use pimax_cam::headpose::qlog;
use pimax_cam::slam::{qconj, qmul};

/// Summary of one trajectory.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TrajStats {
    /// Poses.
    pub n: usize,
    /// Duration (s).
    pub duration_s: f64,
    /// Median pose rate (Hz).
    pub rate_hz: f64,
    /// Gaps longer than 3 median intervals.
    pub gaps: usize,
    /// Longest gap (s).
    pub max_gap_s: f64,
    /// Path length (m), from positions resampled every 50 ms (noise does not inflate it).
    pub path_m: f64,
    /// Speed percentiles p50 / p95 / max (m/s), 0.1 s central differences.
    pub speed: [f64; 3],
    /// Angular speed percentiles p50 / p95 / max (deg/s).
    pub ang_speed: [f64; 3],
    /// Still segments found (>= `still_min_s` below the speed thresholds).
    pub still_segments: usize,
    /// Total still time (s).
    pub still_s: f64,
    /// Position noise while still: RMS distance from the segment mean (mm), pooled over segments.
    pub still_pos_rms_mm: f64,
    /// Rotation noise while still: RMS angle from the segment mean (deg).
    pub still_rot_rms_deg: f64,
    /// Drift while still: median |end - start| / duration over segments (mm/s).
    pub still_drift_mm_s: f64,
    /// High-frequency position noise in motion: residual RMS of a local quadratic fit over +-`fit_s`, p50/p95 (mm).
    pub hf_pos_mm: [f64; 2],
    /// High-frequency rotation noise in motion (deg), same fit.
    pub hf_rot_deg: [f64; 2],
    /// Loop: end-start distance (m) and in % of the path (runs that end where they started).
    pub loop_m: f64,
    /// Loop distance in % of the path length (= drift per metre x 100).
    pub loop_pct: f64,
}

/// Parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct StatsParams {
    /// Still: speed below this (m/s) ...
    pub still_speed: f64,
    /// ... and angular speed below this (deg/s) ...
    pub still_ang_deg_s: f64,
    /// ... for at least this long (s).
    pub still_min_s: f64,
    /// Half window of the local fit (s).
    pub fit_s: f64,
}

impl Default for StatsParams {
    fn default() -> Self {
        StatsParams {
            still_speed: 0.02,
            still_ang_deg_s: 2.0,
            still_min_s: 1.0,
            fit_s: 0.1,
        }
    }
}

fn pct(v: &mut [f64], q: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let i = ((v.len() - 1) as f64 * q).round() as usize;
    v[i.min(v.len() - 1)]
}

/// Index of the first pose at or after `ts`.
fn at(r: &[PoseRow], ts: i64) -> usize {
    r.partition_point(|x| x.ts < ts)
}

#[allow(clippy::needless_range_loop)]
/// Residual RMS of a least-squares quadratic fit of `ys` against `xs` (per component, combined).
fn quad_residual_rms(xs: &[f64], ys: &[Vec3]) -> Option<f64> {
    let n = xs.len();
    if n < 5 {
        return None;
    }
    // Normal equations for [1, x, x^2].
    let mut m = [[0.0f64; 3]; 3];
    for &x in xs {
        let b = [1.0, x, x * x];
        for i in 0..3 {
            for j in 0..3 {
                m[i][j] += b[i] * b[j];
            }
        }
    }
    let inv = invert3(&m)?;
    let mut sse = 0.0;
    for c in 0..3 {
        let mut rhs = [0.0; 3];
        for (k, &x) in xs.iter().enumerate() {
            let b = [1.0, x, x * x];
            for i in 0..3 {
                rhs[i] += b[i] * ys[k][c];
            }
        }
        let coef = geom::mat_vec(&inv, rhs);
        for (k, &x) in xs.iter().enumerate() {
            let f = coef[0] + coef[1] * x + coef[2] * x * x;
            sse += (ys[k][c] - f).powi(2);
        }
    }
    Some((sse / n as f64).sqrt())
}

fn invert3(m: &[[f64; 3]; 3]) -> Option<[[f64; 3]; 3]> {
    let d = geom::det(m);
    if d.abs() < 1e-300 {
        return None;
    }
    let c =
        |r0: usize, c0: usize, r1: usize, c1: usize| m[r0][c0] * m[r1][c1] - m[r0][c1] * m[r1][c0];
    let adj = [
        [c(1, 1, 2, 2), -c(0, 1, 2, 2), c(0, 1, 1, 2)],
        [-c(1, 0, 2, 2), c(0, 0, 2, 2), -c(0, 0, 1, 2)],
        [c(1, 0, 2, 1), -c(0, 0, 2, 1), c(0, 0, 1, 1)],
    ];
    let mut o = [[0.0; 3]; 3];
    for i in 0..3 {
        for j in 0..3 {
            o[i][j] = adj[i][j] / d;
        }
    }
    Some(o)
}

/// Compute the statistics.
pub fn stats(r: &[PoseRow], p: &StatsParams) -> TrajStats {
    let mut s = TrajStats {
        n: r.len(),
        ..Default::default()
    };
    if r.len() < 10 {
        return s;
    }
    s.duration_s = (r[r.len() - 1].ts - r[0].ts) as f64 / 1e9;
    let mut dts: Vec<f64> = r
        .windows(2)
        .map(|w| (w[1].ts - w[0].ts) as f64 / 1e9)
        .collect();
    let med = pct(&mut dts.clone(), 0.5).max(1e-6);
    s.rate_hz = 1.0 / med;
    for d in &dts {
        if *d > 3.0 * med {
            s.gaps += 1;
        }
        s.max_gap_s = s.max_gap_s.max(*d);
    }
    dts.clear();
    // Path from 50 ms resampling.
    let mut last: Option<Vec3> = None;
    let mut t = r[0].ts;
    while t <= r[r.len() - 1].ts {
        let p0 = r[at(r, t).min(r.len() - 1)].p;
        if let Some(l) = last {
            s.path_m += geom::norm(geom::sub(p0, l));
        }
        last = Some(p0);
        t += 50_000_000;
    }
    // Speeds with 0.1 s central differences.
    let h = 50_000_000i64;
    let mut sp = Vec::new();
    let mut asp = Vec::new();
    let mut still = vec![false; r.len()];
    for (i, row) in r.iter().enumerate() {
        let (a, b) = (at(r, row.ts - h), at(r, row.ts + h).min(r.len() - 1));
        if b <= a {
            continue;
        }
        let dt = (r[b].ts - r[a].ts) as f64 / 1e9;
        if dt < 0.05 {
            continue;
        }
        let v = geom::norm(geom::sub(r[b].p, r[a].p)) / dt;
        let w = geom::norm(qlog(qmul(qconj(r[a].q), r[b].q))).to_degrees() / dt;
        sp.push(v);
        asp.push(w);
        // Still test on a 0.5 s baseline: 1 mm of noise reads as 0.5 cm/s there (2.4 cm/s over 0.1 s).
        let (a2, b2) = (
            at(r, row.ts - 250_000_000),
            at(r, row.ts + 250_000_000).min(r.len() - 1),
        );
        let dt2 = (r[b2].ts - r[a2].ts) as f64 / 1e9;
        if dt2 >= 0.4 {
            let v2 = geom::norm(geom::sub(r[b2].p, r[a2].p)) / dt2;
            let w2 = geom::norm(qlog(qmul(qconj(r[a2].q), r[b2].q))).to_degrees() / dt2;
            still[i] = v2 < p.still_speed && w2 < p.still_ang_deg_s;
        }
    }
    s.speed = [pct(&mut sp, 0.5), pct(&mut sp, 0.95), pct(&mut sp, 1.0)];
    s.ang_speed = [pct(&mut asp, 0.5), pct(&mut asp, 0.95), pct(&mut asp, 1.0)];
    // Still segments.
    let (mut sum_p2, mut sum_r2, mut cnt) = (0.0, 0.0, 0usize);
    let mut drifts = Vec::new();
    let mut i = 0;
    while i < r.len() {
        if !still[i] {
            i += 1;
            continue;
        }
        let j0 = i;
        while i < r.len() && still[i] {
            i += 1;
        }
        let seg = &r[j0..i];
        let dur = (seg[seg.len() - 1].ts - seg[0].ts) as f64 / 1e9;
        if dur < p.still_min_s || seg.len() < 5 {
            continue;
        }
        s.still_segments += 1;
        s.still_s += dur;
        let n = seg.len() as f64;
        let mean = seg
            .iter()
            .fold([0.0; 3], |a, x| geom::add(a, geom::scale(x.p, 1.0 / n)));
        let q0 = seg[seg.len() / 2].q;
        let wm = seg.iter().fold([0.0; 3], |a, x| {
            geom::add(a, geom::scale(qlog(qmul(qconj(q0), x.q)), 1.0 / n))
        });
        for x in seg {
            sum_p2 += geom::norm(geom::sub(x.p, mean)).powi(2);
            sum_r2 += geom::norm(geom::sub(qlog(qmul(qconj(q0), x.q)), wm)).powi(2);
            cnt += 1;
        }
        drifts.push(geom::norm(geom::sub(seg[seg.len() - 1].p, seg[0].p)) / dur * 1000.0);
    }
    if cnt > 0 {
        s.still_pos_rms_mm = (sum_p2 / cnt as f64).sqrt() * 1000.0;
        s.still_rot_rms_deg = (sum_r2 / cnt as f64).sqrt().to_degrees();
        s.still_drift_mm_s = pct(&mut drifts, 0.5);
    }
    // High-frequency residuals in motion (not still), on a subsample of centres to bound the cost.
    let fit = (p.fit_s * 1e9) as i64;
    let step = (r.len() / 4000).max(1);
    let (mut hp, mut hr) = (Vec::new(), Vec::new());
    for c in (0..r.len()).step_by(step) {
        if still[c] {
            continue;
        }
        let (a, b) = (at(r, r[c].ts - fit), at(r, r[c].ts + fit).min(r.len() - 1));
        if b <= a + 5 {
            continue;
        }
        let xs: Vec<f64> = r[a..=b]
            .iter()
            .map(|x| (x.ts - r[c].ts) as f64 / 1e9)
            .collect();
        let ps: Vec<Vec3> = r[a..=b].iter().map(|x| x.p).collect();
        let rs: Vec<Vec3> = r[a..=b]
            .iter()
            .map(|x| qlog(qmul(qconj(r[c].q), x.q)))
            .collect();
        if let (Some(e), Some(f)) = (quad_residual_rms(&xs, &ps), quad_residual_rms(&xs, &rs)) {
            hp.push(e * 1000.0);
            hr.push(f.to_degrees());
        }
    }
    s.hf_pos_mm = [pct(&mut hp, 0.5), pct(&mut hp, 0.95)];
    s.hf_rot_deg = [pct(&mut hr, 0.5), pct(&mut hr, 0.95)];
    if let Some((d, _, _)) = loop_drift(r, 1.0) {
        s.loop_m = d;
        s.loop_pct = 100.0 * d / s.path_m.max(1e-9);
    }
    s
}

/// Two-column (or one-column) text table.
pub fn table(a: &TrajStats, b: Option<&TrajStats>, names: (&str, &str)) -> String {
    let mut o = String::new();
    let row = |o: &mut String, k: &str, f: &dyn Fn(&TrajStats) -> String| {
        o.push_str(&format!("{k:<34} {:>22}", f(a)));
        if let Some(b) = b {
            o.push_str(&format!(" {:>22}", f(b)));
        }
        o.push('\n');
    };
    o.push_str(&format!("{:<34} {:>22}", "", names.0));
    if b.is_some() {
        o.push_str(&format!(" {:>22}", names.1));
    }
    o.push('\n');
    row(&mut o, "poses / duration", &|s| {
        format!("{} / {:.1} s", s.n, s.duration_s)
    });
    row(&mut o, "rate (median)", &|s| format!("{:.1} Hz", s.rate_hz));
    row(&mut o, "gaps > 3x median / longest", &|s| {
        format!("{} / {:.0} ms", s.gaps, s.max_gap_s * 1e3)
    });
    row(&mut o, "path (50 ms resampled)", &|s| {
        format!("{:.2} m", s.path_m)
    });
    row(&mut o, "speed p50/p95/max", &|s| {
        format!("{:.2}/{:.2}/{:.2} m/s", s.speed[0], s.speed[1], s.speed[2])
    });
    row(&mut o, "angular speed p50/p95/max", &|s| {
        format!(
            "{:.0}/{:.0}/{:.0} deg/s",
            s.ang_speed[0], s.ang_speed[1], s.ang_speed[2]
        )
    });
    row(&mut o, "still segments / time", &|s| {
        format!("{} / {:.1} s", s.still_segments, s.still_s)
    });
    row(&mut o, "still: position noise RMS", &|s| {
        format!("{:.2} mm", s.still_pos_rms_mm)
    });
    row(&mut o, "still: rotation noise RMS", &|s| {
        format!("{:.3} deg", s.still_rot_rms_deg)
    });
    row(&mut o, "still: drift (median)", &|s| {
        format!("{:.2} mm/s", s.still_drift_mm_s)
    });
    row(&mut o, "motion: HF position p50/p95", &|s| {
        format!("{:.2}/{:.2} mm", s.hf_pos_mm[0], s.hf_pos_mm[1])
    });
    row(&mut o, "motion: HF rotation p50/p95", &|s| {
        format!("{:.3}/{:.3} deg", s.hf_rot_deg[0], s.hf_rot_deg[1])
    });
    row(&mut o, "loop end-start (% of path)", &|s| {
        format!("{:.3} m ({:.2} %)", s.loop_m, s.loop_pct)
    });
    o
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::slam::qyaw;

    fn traj(noise: f64, rate: f64) -> Vec<PoseRow> {
        // 3 s still, 6 s walking a 1 m radius circle, 3 s still at the start point.
        let mut v = Vec::new();
        let n = (12.0 * rate) as usize;
        let mut seed = 7u64;
        let mut rnd = || {
            seed = seed
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((seed >> 11) as f64 / (1u64 << 53) as f64 - 0.5) * 2.0 * noise * 3f64.sqrt()
        };
        for i in 0..n {
            let t = i as f64 / rate;
            let u = ((t - 3.0) / 6.0).clamp(0.0, 1.0);
            let ang = 2.0 * std::f64::consts::PI * (3.0 * u * u - 2.0 * u * u * u);
            let p = [ang.sin() + rnd(), rnd(), 1.0 - ang.cos() + rnd()];
            v.push(PoseRow {
                ts: (t * 1e9) as i64,
                p,
                q: qyaw(ang + rnd() * 0.01),
            });
        }
        v
    }

    #[test]
    fn still_noise_and_loop() {
        let r = traj(0.001, 200.0);
        let s = stats(&r, &StatsParams::default());
        assert!((s.rate_hz - 200.0).abs() < 1.0);
        assert_eq!(s.still_segments, 2, "{s:?}");
        // Uniform noise with std 1 mm per axis: 3D RMS ~ 1.73 mm.
        assert!((s.still_pos_rms_mm - 1.73).abs() < 0.3, "{s:?}");
        assert!(
            s.loop_m < 0.01 && (s.path_m - std::f64::consts::TAU).abs() < 0.3,
            "{s:?}"
        );
        assert!(s.hf_pos_mm[0] > 1.0 && s.hf_pos_mm[0] < 2.2, "{s:?}");
        let clean = stats(&traj(0.0, 200.0), &StatsParams::default());
        assert!(
            clean.hf_pos_mm[1] < 0.2 && clean.still_pos_rms_mm < 1e-6,
            "{clean:?}"
        );
        let t = table(&s, Some(&clean), ("noisy", "clean"));
        assert!(t.contains("still: position noise RMS"));
    }
}
