//! The eye-tracking accuracy test (`gaze` pattern): the pure part. Target geometry, the test procedure as a
//! time-driven state machine ([`GazeTest`]), per-target statistics, the sign-flip check of the head-frame axes, the
//! host-side correction fit and the result files. The OpenXR plumbing is in [`crate::gazexr`], the drawing in
//! [`crate::gazedraw`].
//!
//! # Procedure
//!
//! Head-locked targets (VIEW space, [`TARGET_DIST`] = 2 m ahead, one disc per eye with the proper stereo projection so
//! that they fuse). First the 5-point axis check (centre, 15 deg right / left / up / down): the direction of the
//! reported gaze must follow the targets, otherwise one of `PIMAX_EYE_FLIP_X/Y/Z` is wrong and the test says which
//! ([`axis_verdict`]). Then the accuracy grid ([`grid_targets`]): per target a shrinking dot for [`SETTLE_S`] seconds,
//! then [`COLLECT_S`] seconds of samples (one per displayed frame), invalid frames and outliers (blinks, saccades)
//! dropped ([`compute_target`]). Last a live cursor, raw and corrected.
//!
//! # Numbers
//!
//! Directions are head-frame unit vectors (x right, y up, -z forward). A target at azimuth `az` and elevation `el`
//! (degrees; azimuth right, elevation up) is the point `2 m * (sin az cos el, sin el, -cos az cos el)`; the *wanted*
//! gaze direction is from the reported gaze origin to that point. Per target:
//! * `err_deg`: mean over the used samples of the angle between sample direction and wanted direction (accuracy,
//!   includes the noise); `bias_deg`: the angle of the mean direction (what a correction can remove),
//! * `precision_deg`: RMS of the angle between consecutive used samples (sample-to-sample),
//! * `valid_ratio`: used samples / frames of the collection window (invalid and rejected frames count against it).

use pimax_eye::correction::{dir_of, features, tangent_of, Correction};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

pub type V3 = [f64; 3];

/// Distance of the head-locked targets, metres.
pub const TARGET_DIST: f64 = 2.0;
/// Seconds the dot is shown (shrinking) before samples are taken.
pub const SETTLE_S: f64 = 0.6;
/// Seconds of samples per target.
pub const COLLECT_S: f64 = 1.0;
/// A sample further than this from the median direction of its target is a blink / saccade artefact.
pub const OUTLIER_DEG: f64 = 5.0;
/// A target needs at least this many used samples ...
pub const MIN_USED: usize = 10;
/// ... and this fraction of the frames to count in the fit.
pub const MIN_RATIO: f64 = 0.3;
/// Axis-check separation (difference of the right/left x, up/down y of the unit direction; 0.52 expected for 15 deg
/// targets) below which the sign is not decided.
pub const AXIS_MIN_SEP: f64 = 0.15;
/// A correction must improve the leave-one-out mean error by at least this much (below it is measurement noise).
pub const MIN_GAIN_DEG: f64 = 0.05;
/// Largest frame time counted (a stall must not eat a whole target).
pub const MAX_DT: f64 = 0.1;

// ---------------------------------------------------------------------------------------------------- vectors

pub fn dot(a: V3, b: V3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

pub fn sub(a: V3, b: V3) -> V3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub fn len(a: V3) -> f64 {
    dot(a, a).sqrt()
}

pub fn normalize(a: V3) -> V3 {
    let l = len(a);
    if l > 1e-12 {
        [a[0] / l, a[1] / l, a[2] / l]
    } else {
        a
    }
}

/// Angle between two directions in degrees (robust for tiny and for large angles).
pub fn angle_deg(a: V3, b: V3) -> f64 {
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    len(c).atan2(dot(a, b)).to_degrees()
}

/// Unit head-frame direction at azimuth `az` (right) and elevation `el` (up), degrees.
pub fn dir_from_az_el(az: f64, el: f64) -> V3 {
    let (a, e) = (az.to_radians(), el.to_radians());
    [a.sin() * e.cos(), e.sin(), -a.cos() * e.cos()]
}

/// The head-locked target point, metres.
pub fn target_point(az: f64, el: f64) -> V3 {
    let d = dir_from_az_el(az, el);
    [d[0] * TARGET_DIST, d[1] * TARGET_DIST, d[2] * TARGET_DIST]
}

/// Where the gaze cursor is drawn: [`TARGET_DIST`] along the gaze ray (the same distance as the targets, so the
/// cursor lies on the targets' sphere); `None` when the ray does not go forward.
pub fn plane_hit(origin: V3, dir: V3) -> Option<V3> {
    if dir[2] > -0.05 {
        return None;
    }
    Some([
        origin[0] + TARGET_DIST * dir[0],
        origin[1] + TARGET_DIST * dir[1],
        origin[2] + TARGET_DIST * dir[2],
    ])
}

/// Mean and percentiles of a list (sorted internally).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Stats {
    pub n: usize,
    pub mean: f64,
    pub median: f64,
    pub p95: f64,
    pub max: f64,
}

pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    match sorted.len() {
        0 => 0.0,
        1 => sorted[0],
        n => {
            let x = p.clamp(0.0, 1.0) * (n - 1) as f64;
            let i = x.floor() as usize;
            let f = x - i as f64;
            sorted[i] + (sorted[(i + 1).min(n - 1)] - sorted[i]) * f
        }
    }
}

pub fn stats(v: &[f64]) -> Stats {
    if v.is_empty() {
        return Stats::default();
    }
    let mut s = v.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    Stats {
        n: s.len(),
        mean: s.iter().sum::<f64>() / s.len() as f64,
        median: percentile(&s, 0.5),
        p95: percentile(&s, 0.95),
        max: s[s.len() - 1],
    }
}

// --------------------------------------------------------------------------------------------------- targets

#[derive(Clone, Debug, PartialEq)]
pub struct Target {
    /// Azimuth (right positive) and elevation (up positive), degrees.
    pub az: f64,
    pub el: f64,
    pub name: String,
}

impl Target {
    fn new(az: f64, el: f64, name: &str) -> Target {
        Target {
            az,
            el,
            name: name.to_string(),
        }
    }

    pub fn point(&self) -> V3 {
        target_point(self.az, self.el)
    }

    pub fn dir(&self) -> V3 {
        dir_from_az_el(self.az, self.el)
    }

    /// Angle from straight ahead.
    pub fn eccentricity(&self) -> f64 {
        angle_deg(self.dir(), [0.0, 0.0, -1.0])
    }
}

/// Angle of the axis-check targets from the centre.
pub const AXIS_DEG: f64 = 15.0;

/// Axis check: centre, right, left, up, down (in this order; [`axis_verdict`] relies on it).
pub fn axis_targets() -> Vec<Target> {
    vec![
        Target::new(0.0, 0.0, "centre"),
        Target::new(AXIS_DEG, 0.0, "right 15"),
        Target::new(-AXIS_DEG, 0.0, "left 15"),
        Target::new(0.0, AXIS_DEG, "up 15"),
        Target::new(0.0, -AXIS_DEG, "down 15"),
    ]
}

/// The accuracy grid, 13 or 17 points within +-25 deg horizontally (plus the +-35 deg points) and +-20 deg
/// vertically, ordered as a short path from the centre outwards.
///
/// 13: centre, (+-25, 0), (+-35, 0), (0, +-10), (0, +-20), (+-25, +-20); 17 adds (+-12.5, +-10).
pub fn grid_targets(points: usize) -> Vec<Target> {
    let mut v: Vec<(f64, f64)> = vec![(0.0, 0.0)];
    for s in [1.0, -1.0] {
        v.push((25.0 * s, 0.0));
        v.push((35.0 * s, 0.0));
        v.push((0.0, 10.0 * s));
        v.push((0.0, 20.0 * s));
        for t in [1.0, -1.0] {
            v.push((25.0 * s, 20.0 * t));
        }
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v.dedup();
    if points >= 17 {
        for s in [1.0, -1.0] {
            for t in [1.0, -1.0] {
                v.push((12.5 * s, 10.0 * t));
            }
        }
    }
    // greedy nearest-neighbour path from the centre
    let mut left = v;
    let mut path = Vec::new();
    let mut cur = (0.0f64, 0.0f64);
    while !left.is_empty() {
        let (i, _) = left
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let d = angle_deg(dir_from_az_el(cur.0, cur.1), dir_from_az_el(p.0, p.1));
                // tie-break on position for a deterministic order
                (
                    i,
                    (d * 1e6).round() / 1e6 + (p.0 + 100.0) * 1e-9 + (p.1 + 100.0) * 1e-12,
                )
            })
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .unwrap();
        let p = left.remove(i);
        cur = p;
        path.push(Target::new(p.0, p.1, &format!("{:+.1},{:+.1}", p.0, p.1)));
    }
    path
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Region {
    Centre,
    Mid,
    Edge,
}

impl Region {
    pub const ALL: [Region; 3] = [Region::Centre, Region::Mid, Region::Edge];

    pub fn name(self) -> &'static str {
        match self {
            Region::Centre => "centre",
            Region::Mid => "mid",
            Region::Edge => "edge",
        }
    }
}

/// Centre: within 10 deg of straight ahead, mid: up to 25 deg, edge: further out.
pub fn region_of(t: &Target) -> Region {
    let e = t.eccentricity();
    if e <= 10.0 + 1e-6 {
        Region::Centre
    } else if e <= 25.0 + 1e-6 {
        Region::Mid
    } else {
        Region::Edge
    }
}

// ------------------------------------------------------------------------------------------ per-target stats

/// One gaze reading in the head frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Reading {
    pub dir: V3,
    pub origin: V3,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TargetResult {
    pub target: Target,
    /// Frames of the collection window, frames with a valid gaze, samples kept after outlier rejection.
    pub frames: usize,
    pub valid: usize,
    pub used: usize,
    pub valid_ratio: f64,
    pub mean_dir: V3,
    pub want_dir: V3,
    pub err_deg: f64,
    pub bias_deg: f64,
    pub precision_deg: f64,
    /// Mean gaze tangent and wanted tangent (the fit data).
    pub gaze_tan: (f64, f64),
    pub want_tan: (f64, f64),
    /// Enough good samples to count in the fit.
    pub ok: bool,
    /// Per used sample: (direction, wanted direction).
    pub pairs: Vec<(V3, V3)>,
}

fn median_dir(v: &[Reading]) -> V3 {
    let med = |k: usize| {
        let mut c: Vec<f64> = v.iter().map(|r| r.dir[k]).collect();
        c.sort_by(|a, b| a.total_cmp(b));
        percentile(&c, 0.5)
    };
    normalize([med(0), med(1), med(2)])
}

/// Statistics of one target from the valid readings of its collection window (`frames` frames, readings of invalid
/// frames are simply absent).
pub fn compute_target(target: &Target, readings: &[Reading], frames: usize) -> TargetResult {
    let valid = readings.len();
    let kept: Vec<Reading> = if valid == 0 {
        Vec::new()
    } else {
        let m = median_dir(readings);
        readings
            .iter()
            .filter(|r| angle_deg(r.dir, m) <= OUTLIER_DEG)
            .copied()
            .collect()
    };
    let used = kept.len();
    let p = target.point();
    let want_of = |o: V3| normalize(sub(p, o));
    let mut res = TargetResult {
        target: target.clone(),
        frames,
        valid,
        used,
        valid_ratio: if frames > 0 {
            used as f64 / frames as f64
        } else {
            0.0
        },
        mean_dir: [0.0, 0.0, -1.0],
        want_dir: target.dir(),
        err_deg: 0.0,
        bias_deg: 0.0,
        precision_deg: 0.0,
        gaze_tan: (0.0, 0.0),
        want_tan: (0.0, 0.0),
        ok: false,
        pairs: Vec::new(),
    };
    if used == 0 {
        return res;
    }
    let n = used as f64;
    let mut sd = [0.0; 3];
    let mut so = [0.0; 3];
    for r in &kept {
        for k in 0..3 {
            sd[k] += r.dir[k];
            so[k] += r.origin[k];
        }
        res.pairs.push((r.dir, want_of(r.origin)));
    }
    res.mean_dir = normalize(sd);
    res.want_dir = want_of([so[0] / n, so[1] / n, so[2] / n]);
    res.err_deg = res
        .pairs
        .iter()
        .map(|(d, w)| angle_deg(*d, *w))
        .sum::<f64>()
        / n;
    res.bias_deg = angle_deg(res.mean_dir, res.want_dir);
    if used > 1 {
        let ss: f64 = kept
            .windows(2)
            .map(|w| angle_deg(w[0].dir, w[1].dir).powi(2))
            .sum();
        res.precision_deg = (ss / (used - 1) as f64).sqrt();
    }
    if let (Some(g), Some(w)) = (tangent_of(res.mean_dir), tangent_of(res.want_dir)) {
        res.gaze_tan = g;
        res.want_tan = w;
        res.ok = used >= MIN_USED && res.valid_ratio >= MIN_RATIO;
    }
    res
}

// ------------------------------------------------------------------------------------------- axis check

/// Result of the 5-point axis check. `flip_*`: `Some(true)` = this axis comes out mirrored (toggle the matching
/// `PIMAX_EYE_FLIP_*`), `Some(false)` = fine, `None` = not decided.
#[derive(Clone, Debug, PartialEq)]
pub struct AxisVerdict {
    pub flip_x: Option<bool>,
    pub flip_y: Option<bool>,
    pub flip_z: Option<bool>,
    /// Horizontal targets moved the gaze mostly vertically and vice versa.
    pub swapped: bool,
    /// Angle between the gaze for the right/left and the up/down targets (30 deg expected).
    pub span_x_deg: f64,
    pub span_y_deg: f64,
}

impl AxisVerdict {
    pub fn conclusive(&self) -> bool {
        self.flip_x.is_some() && self.flip_y.is_some() && self.flip_z.is_some()
    }

    pub fn any_flip(&self) -> bool {
        [self.flip_x, self.flip_y, self.flip_z].contains(&Some(true))
    }

    pub fn all_ok(&self) -> bool {
        self.conclusive() && !self.any_flip() && !self.swapped
    }

    /// Console text: what was found and which `PIMAX_EYE_FLIP_*` to set. `current` = the settings in force
    /// (x, y, z).
    pub fn lines(&self, current: [bool; 3]) -> Vec<String> {
        let mut l = Vec::new();
        let names = ["X", "Y", "Z"];
        let what = [
            "looking right must give +x",
            "looking up must give +y",
            "looking ahead must give -z",
        ];
        for (i, f) in [self.flip_x, self.flip_y, self.flip_z]
            .into_iter()
            .enumerate()
        {
            match f {
                Some(true) => l.push(format!(
                    "axis {}: MIRRORED ({}): set PIMAX_EYE_FLIP_{}={} in the service environment (it is {} now){}",
                    names[i].to_lowercase(),
                    what[i],
                    names[i],
                    u8::from(!current[i]),
                    u8::from(current[i]),
                    if i == 0 { "; this also swaps which eye is left" } else { "" }
                )),
                Some(false) => l.push(format!("axis {}: ok ({})", names[i].to_lowercase(), what[i])),
                None => l.push(format!(
                    "axis {}: not decided (the gaze barely moved: did you follow the dots?)",
                    names[i].to_lowercase()
                )),
            }
        }
        if self.swapped {
            l.push("horizontal and vertical look swapped (right/left moved y, up/down moved x): there is no setting for that, report it".to_string());
        }
        l.push(format!(
            "gaze span for 30 deg of target: right-left {:.1} deg, up-down {:.1} deg",
            self.span_x_deg, self.span_y_deg
        ));
        l
    }

    /// Short on-screen lines.
    pub fn hud(&self, current: [bool; 3]) -> Vec<String> {
        if self.all_ok() {
            return vec!["AXES OK: RIGHT +X, UP +Y, FORWARD -Z".to_string()];
        }
        let mut l = Vec::new();
        for (i, f) in [self.flip_x, self.flip_y, self.flip_z]
            .into_iter()
            .enumerate()
        {
            let n = ["X", "Y", "Z"][i];
            match f {
                Some(true) => l.push(format!(
                    "AXIS {n} MIRRORED: SET PIMAX_EYE_FLIP_{n}={}",
                    u8::from(!current[i])
                )),
                None => l.push(format!("AXIS {n} NOT DECIDED (NO MOVEMENT?)")),
                Some(false) => {}
            }
        }
        if self.swapped {
            l.push("AXES SWAPPED: NO SETTING FOR THAT".to_string());
        }
        l
    }
}

/// Decide the signs from the mean gaze directions at the five axis targets, in the order of [`axis_targets`]:
/// centre, right, left, up, down.
pub fn axis_verdict(m: &[V3; 5]) -> AxisVerdict {
    let [_c, r, l, u, d] = *m;
    let z_mean = m.iter().map(|v| v[2]).sum::<f64>() / 5.0;
    let flip_z = if z_mean.abs() < 0.3 {
        None
    } else {
        Some(z_mean > 0.0)
    };
    let dx = r[0] - l[0];
    let dy = u[1] - d[1];
    let flip_x = (dx.abs() >= AXIS_MIN_SEP).then_some(dx < 0.0);
    let flip_y = (dy.abs() >= AXIS_MIN_SEP).then_some(dy < 0.0);
    let cross_h = (r[1] - l[1]).abs();
    let cross_v = (u[0] - d[0]).abs();
    AxisVerdict {
        flip_x,
        flip_y,
        flip_z,
        swapped: cross_h > dx.abs() && cross_v > dy.abs(),
        span_x_deg: angle_deg(r, l),
        span_y_deg: angle_deg(u, d),
    }
}

// ------------------------------------------------------------------------------------------------------ fit

/// One fit point: mean gaze tangent -> wanted tangent.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FitPoint {
    pub gaze: (f64, f64),
    pub want: (f64, f64),
}

fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-12 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            let pivot_row = a[c].clone();
            for (x, p) in a[r].iter_mut().zip(pivot_row.iter()).skip(c) {
                *x -= f * p;
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = vec![0.0; n];
    for r in (0..n).rev() {
        let s: f64 = (r + 1..n).map(|k| a[r][k] * x[k]).sum();
        x[r] = (b[r] - s) / a[r][r];
    }
    Some(x)
}

/// Least squares fit of the first `k` features (3 = affine, 6 = quadratic), one output at a time.
fn fit_coeffs(points: &[FitPoint], k: usize) -> Option<([f64; 6], [f64; 6])> {
    if points.len() < k {
        return None;
    }
    let mut ata = vec![vec![0.0; k]; k];
    let (mut bu, mut bv) = (vec![0.0; k], vec![0.0; k]);
    for p in points {
        let f = features(p.gaze.0, p.gaze.1);
        for i in 0..k {
            for j in 0..k {
                ata[i][j] += f[i] * f[j];
            }
            bu[i] += f[i] * p.want.0;
            bv[i] += f[i] * p.want.1;
        }
    }
    for (i, row) in ata.iter_mut().enumerate() {
        row[i] += 1e-10;
    }
    let (xu, xv) = (solve(ata.clone(), bu)?, solve(ata, bv)?);
    let (mut cx, mut cy) = ([0.0; 6], [0.0; 6]);
    cx[..k].copy_from_slice(&xu);
    cy[..k].copy_from_slice(&xv);
    Some((cx, cy))
}

fn correction_of(model: &str, c: ([f64; 6], [f64; 6])) -> Correction {
    Correction {
        model: model.to_string(),
        cx: c.0,
        cy: c.1,
        flips: None,
    }
}

/// Angular error of the correction at one fit point (degrees).
pub fn point_error(c: &Correction, p: &FitPoint) -> f64 {
    let (u, v) = c.map_tangent(p.gaze.0, p.gaze.1);
    angle_deg(dir_of(u, v), dir_of(p.want.0, p.want.1))
}

#[derive(Clone, Debug, PartialEq)]
pub struct ModelScore {
    pub name: String,
    pub correction: Correction,
    /// Leave-one-out error over the targets: fit on all others, evaluate on the left-out one (degrees).
    pub loo: Stats,
    /// Error at the fit points themselves.
    pub train: Stats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FitReport {
    pub n: usize,
    /// Error of the raw gaze at the fit points.
    pub before: Stats,
    /// Index into `models` of the chosen model; `None`: no correction helps.
    pub chosen: Option<usize>,
    pub models: Vec<ModelScore>,
}

impl FitReport {
    pub fn chosen_model(&self) -> Option<&ModelScore> {
        self.chosen.map(|i| &self.models[i])
    }
}

fn score(name: &str, points: &[FitPoint], k: usize) -> Option<ModelScore> {
    let c = correction_of(name, fit_coeffs(points, k)?);
    let train: Vec<f64> = points.iter().map(|p| point_error(&c, p)).collect();
    let mut loo = Vec::new();
    for i in 0..points.len() {
        let rest: Vec<FitPoint> = points
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, p)| *p)
            .collect();
        let ci = correction_of(name, fit_coeffs(&rest, k)?);
        loo.push(point_error(&ci, &points[i]));
    }
    Some(ModelScore {
        name: name.to_string(),
        correction: c,
        loo: stats(&loo),
        train: stats(&train),
    })
}

/// Fit the affine and (with enough points) the quadratic map and choose by leave-one-out error: the simpler model
/// unless the more complex one is clearly better (5 % for affine over none, 10 % for quadratic over affine, and at
/// least [`MIN_GAIN_DEG`] each).
pub fn fit(points: &[FitPoint]) -> FitReport {
    let before = stats(
        &points
            .iter()
            .map(|p| point_error(&Correction::identity(), p))
            .collect::<Vec<_>>(),
    );
    let mut models = Vec::new();
    if points.len() >= 6 {
        models.extend(score("affine", points, 3));
    }
    if points.len() >= 12 {
        models.extend(score("quadratic", points, 6));
    }
    let mut chosen: Option<usize> = None;
    let mut best = before.mean;
    for (i, m) in models.iter().enumerate() {
        let margin = if m.name == "affine" { 0.95 } else { 0.90 };
        if m.loo.mean < best * margin && best - m.loo.mean > MIN_GAIN_DEG {
            best = m.loo.mean;
            chosen = Some(i);
        }
    }
    FitReport {
        n: points.len(),
        before,
        chosen,
        models,
    }
}

// ----------------------------------------------------------------------------------------------- the report

#[derive(Clone, Debug, PartialEq)]
pub struct RegionStats {
    pub region: Region,
    pub targets: usize,
    pub err: Stats,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Report {
    pub targets: usize,
    pub targets_ok: usize,
    /// Pooled over all used samples of the targets that count.
    pub overall: Stats,
    /// The correction applied to every sample (None when there is no correction).
    pub after: Option<Stats>,
    pub worst_target_deg: f64,
    pub precision_deg: f64,
    pub valid_ratio: f64,
    pub regions: Vec<RegionStats>,
    pub fit: Option<FitReport>,
    pub summary: String,
}

fn pooled(results: &[&TargetResult], c: Option<&Correction>) -> Vec<f64> {
    results
        .iter()
        .flat_map(|r| r.pairs.iter())
        .map(|(d, w)| match c {
            Some(c) => angle_deg(c.apply(*d), *w),
            None => angle_deg(*d, *w),
        })
        .collect()
}

pub fn build_report(results: &[TargetResult]) -> Report {
    let ok: Vec<&TargetResult> = results.iter().filter(|r| r.ok).collect();
    let overall = stats(&pooled(&ok, None));
    let worst = ok.iter().map(|r| r.err_deg).fold(0.0, f64::max);
    let precision = if ok.is_empty() {
        0.0
    } else {
        ok.iter().map(|r| r.precision_deg).sum::<f64>() / ok.len() as f64
    };
    let frames: usize = results.iter().map(|r| r.frames).sum();
    let used: usize = results.iter().map(|r| r.used).sum();
    let valid_ratio = if frames > 0 {
        used as f64 / frames as f64
    } else {
        0.0
    };
    let regions: Vec<RegionStats> = Region::ALL
        .into_iter()
        .map(|g| {
            let sel: Vec<&TargetResult> = ok
                .iter()
                .filter(|r| region_of(&r.target) == g)
                .copied()
                .collect();
            RegionStats {
                region: g,
                targets: sel.len(),
                err: stats(&pooled(&sel, None)),
            }
        })
        .collect();
    let points: Vec<FitPoint> = ok
        .iter()
        .map(|r| FitPoint {
            gaze: r.gaze_tan,
            want: r.want_tan,
        })
        .collect();
    let fit_report = (points.len() >= 6).then(|| fit(&points));
    let after = fit_report
        .as_ref()
        .and_then(|f| f.chosen_model())
        .map(|m| stats(&pooled(&ok, Some(&m.correction))));
    let reg = |g: Region| {
        regions
            .iter()
            .find(|r| r.region == g)
            .filter(|r| r.targets > 0)
            .map_or("-".to_string(), |r| format!("{:.2}", r.err.mean))
    };
    let summary = format!(
        "eye-accuracy: {}/{} targets | error mean {:.2} median {:.2} p95 {:.2} max {:.2} deg | precision {:.2} deg | valid {:.0}% | centre {} mid {} edge {} deg",
        ok.len(),
        results.len(),
        overall.mean,
        overall.median,
        overall.p95,
        overall.max,
        precision,
        100.0 * valid_ratio,
        reg(Region::Centre),
        reg(Region::Mid),
        reg(Region::Edge),
    );
    Report {
        targets: results.len(),
        targets_ok: ok.len(),
        overall,
        after,
        worst_target_deg: worst,
        precision_deg: precision,
        valid_ratio,
        regions,
        fit: fit_report,
        summary,
    }
}

impl Report {
    /// The candidate correction, if the fit found one worth having.
    pub fn correction(&self, flips: [bool; 3]) -> Option<Correction> {
        let mut c = self.fit.as_ref()?.chosen_model()?.correction.clone();
        c.flips = Some(flips);
        Some(c)
    }

    /// Multi-line console text.
    pub fn text(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "targets with data: {}/{}   worst target mean error {:.2} deg",
            self.targets_ok, self.targets, self.worst_target_deg
        );
        let o = &self.overall;
        let _ = writeln!(
            s,
            "error (all samples): mean {:.2}  median {:.2}  95% {:.2}  max {:.2} deg   precision {:.2} deg   valid {:.0}%",
            o.mean, o.median, o.p95, o.max, self.precision_deg, 100.0 * self.valid_ratio
        );
        for r in &self.regions {
            if r.targets > 0 {
                let _ = writeln!(
                    s,
                    "  {:<6} ({} targets): mean {:.2}  median {:.2}  95% {:.2}  max {:.2} deg",
                    r.region.name(),
                    r.targets,
                    r.err.mean,
                    r.err.median,
                    r.err.p95,
                    r.err.max
                );
            }
        }
        match &self.fit {
            None => {
                let _ = writeln!(s, "correction: too few good targets to fit");
            }
            Some(f) => {
                let _ = write!(
                    s,
                    "correction fit on {} targets, leave-one-out mean error: none {:.2}",
                    f.n, f.before.mean
                );
                for m in &f.models {
                    let _ = write!(s, ", {} {:.2}", m.name, m.loo.mean);
                }
                let _ = writeln!(s, " deg");
                match (f.chosen_model(), &self.after) {
                    (Some(m), Some(a)) => {
                        let _ = writeln!(
                            s,
                            "chosen: {}   error before: mean {:.2} 95% {:.2} deg   after: mean {:.2} 95% {:.2} deg (fit points, per-target leave-one-out mean {:.2})",
                            m.name, o.mean, o.p95, a.mean, a.p95, m.loo.mean
                        );
                    }
                    _ => {
                        let _ = writeln!(
                            s,
                            "no correction offered: it would not improve the leave-one-out error"
                        );
                    }
                }
            }
        }
        s.push_str(&self.summary);
        s.push('\n');
        s
    }
}

// ----------------------------------------------------------------------------------------------------- JSON

fn jnum(x: f64) -> String {
    if x.is_finite() {
        format!("{x:.4}")
    } else {
        "null".to_string()
    }
}

fn jstr(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            c if (c as u32) < 0x20 => {
                let _ = write!(o, "\\u{:04x}", c as u32);
            }
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

fn jstats(s: &Stats) -> String {
    format!(
        "{{\"n\": {}, \"mean\": {}, \"median\": {}, \"p95\": {}, \"max\": {}}}",
        s.n,
        jnum(s.mean),
        jnum(s.median),
        jnum(s.p95),
        jnum(s.max)
    )
}

fn jv3(v: V3) -> String {
    format!("[{}, {}, {}]", jnum(v[0]), jnum(v[1]), jnum(v[2]))
}

fn jtarget(r: &TargetResult) -> String {
    format!(
        "{{\"name\": {}, \"az_deg\": {}, \"el_deg\": {}, \"region\": {}, \"frames\": {}, \"valid\": {}, \"used\": {}, \"valid_ratio\": {}, \"error_deg\": {}, \"bias_deg\": {}, \"precision_deg\": {}, \"mean_dir\": {}, \"want_dir\": {}, \"counts_in_fit\": {}}}",
        jstr(&r.target.name),
        jnum(r.target.az),
        jnum(r.target.el),
        jstr(region_of(&r.target).name()),
        r.frames,
        r.valid,
        r.used,
        jnum(r.valid_ratio),
        jnum(r.err_deg),
        jnum(r.bias_deg),
        jnum(r.precision_deg),
        jv3(r.mean_dir),
        jv3(r.want_dir),
        r.ok
    )
}

/// Everything the JSON file holds.
pub struct JsonInput<'a> {
    pub time_unix: u64,
    pub flips: [bool; 3],
    pub service_correction: Option<&'a str>,
    pub axis: &'a [TargetResult],
    pub verdict: Option<&'a AxisVerdict>,
    pub grid: &'a [TargetResult],
    pub report: &'a Report,
    pub correction: Option<&'a Correction>,
}

pub fn results_json(i: &JsonInput) -> String {
    let mut s = String::from("{\n");
    let _ = writeln!(s, "  \"tool\": \"pimax-lenstest gaze\",");
    let _ = writeln!(s, "  \"time_unix\": {},", i.time_unix);
    let _ = writeln!(s, "  \"target_distance_m\": {},", jnum(TARGET_DIST));
    let _ = writeln!(
        s,
        "  \"env_flip_xyz\": [{}, {}, {}],",
        i.flips[0], i.flips[1], i.flips[2]
    );
    let _ = writeln!(
        s,
        "  \"service_correction\": {},",
        i.service_correction.map_or("null".to_string(), jstr)
    );
    let _ = writeln!(s, "  \"summary\": {},", jstr(&i.report.summary));
    let r = i.report;
    let _ = writeln!(s, "  \"overall_error_deg\": {},", jstats(&r.overall));
    let _ = writeln!(
        s,
        "  \"after_correction_error_deg\": {},",
        r.after.as_ref().map_or("null".to_string(), jstats)
    );
    let _ = writeln!(s, "  \"worst_target_deg\": {},", jnum(r.worst_target_deg));
    let _ = writeln!(s, "  \"precision_deg\": {},", jnum(r.precision_deg));
    let _ = writeln!(s, "  \"valid_ratio\": {},", jnum(r.valid_ratio));
    let regs: Vec<String> = r
        .regions
        .iter()
        .map(|g| {
            format!(
                "{}: {{\"targets\": {}, \"error_deg\": {}}}",
                jstr(g.region.name()),
                g.targets,
                jstats(&g.err)
            )
        })
        .collect();
    let _ = writeln!(s, "  \"regions\": {{{}}},", regs.join(", "));
    let v = i.verdict.map_or("null".to_string(), |v| {
        let f = |o: Option<bool>| o.map_or("null".to_string(), |b| b.to_string());
        format!(
            "{{\"flip_x\": {}, \"flip_y\": {}, \"flip_z\": {}, \"swapped\": {}, \"span_x_deg\": {}, \"span_y_deg\": {}}}",
            f(v.flip_x), f(v.flip_y), f(v.flip_z), v.swapped, jnum(v.span_x_deg), jnum(v.span_y_deg)
        )
    });
    let _ = writeln!(s, "  \"axis_check\": {v},");
    let list = |rs: &[TargetResult]| {
        rs.iter()
            .map(|t| format!("    {}", jtarget(t)))
            .collect::<Vec<_>>()
            .join(",\n")
    };
    let _ = writeln!(s, "  \"axis_targets\": [\n{}\n  ],", list(i.axis));
    let _ = writeln!(s, "  \"grid_targets\": [\n{}\n  ],", list(i.grid));
    let fit_json = match &r.fit {
        None => "null".to_string(),
        Some(f) => {
            let ms: Vec<String> = f
                .models
                .iter()
                .map(|m| {
                    format!(
                        "{{\"model\": {}, \"loo_error_deg\": {}, \"train_error_deg\": {}, \"x\": [{}], \"y\": [{}]}}",
                        jstr(&m.name),
                        jstats(&m.loo),
                        jstats(&m.train),
                        m.correction.cx.map(|c| format!("{c:.9}")).join(", "),
                        m.correction.cy.map(|c| format!("{c:.9}")).join(", ")
                    )
                })
                .collect();
            format!(
                "{{\"points\": {}, \"before_error_deg\": {}, \"chosen\": {}, \"models\": [{}]}}",
                f.n,
                jstats(&f.before),
                f.chosen_model()
                    .map_or("null".to_string(), |m| jstr(&m.name)),
                ms.join(", ")
            )
        }
    };
    let _ = writeln!(s, "  \"correction_fit\": {fit_json},");
    let _ = writeln!(
        s,
        "  \"correction_candidate\": {}",
        i.correction
            .map_or("null".to_string(), |c| jstr(&c.to_toml(&[])))
    );
    s.push_str("}\n");
    s
}

// ------------------------------------------------------------------------------------------- state machine

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Phase {
    Intro,
    Axis(usize),
    AxisDone,
    Grid(usize),
    /// Grid finished: results on screen.
    Done,
    /// Live cursor.
    Cursor,
}

/// What the wearer asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Cmd {
    /// Space / A: start, next phase.
    Next,
    /// R: start over.
    Restart,
    /// Y: accept (write) the correction.
    Accept,
}

/// Things for the caller to do.
#[derive(Clone, Debug, PartialEq)]
pub enum Effect {
    Print(String),
    /// Write the results file.
    SaveResults(String),
    /// Write the correction file (text of the TOML).
    WriteCorrection(String),
}

/// A dot or ring of the scene, head frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mark {
    pub pos: V3,
    pub kind: MarkKind,
    pub radius_deg: f64,
    pub color: [f32; 3],
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MarkKind {
    Disc,
    /// Ring of this line width (degrees).
    Ring(f64),
}

/// Shrinking dot radius: from 1.2 deg at the start of the settle time to 0.3 deg when the samples begin.
pub fn dot_radius_deg(t: f64) -> f64 {
    let f = (t / SETTLE_S).clamp(0.0, 1.0);
    1.2 + (0.3 - 1.2) * f
}

pub const CURSOR_RADIUS_DEG: f64 = 0.8;
pub const CURSOR_RAW: [f32; 3] = [0.85, 0.85, 0.85];
pub const CURSOR_FIXED: [f32; 3] = [0.1, 1.0, 0.2];

#[derive(Clone, Debug)]
pub struct GazeTest {
    /// Grid size (13 or 17).
    pub points: usize,
    /// `PIMAX_EYE_FLIP_X/Y/Z` as this process sees them (the service has the same environment when started by
    /// `eye_accuracy.sh`).
    pub flips: [bool; 3],
    /// `PIMAX_EYE_CORRECTION` as set, when the service applies a correction itself (the samples are then corrected).
    pub service_correction: Option<String>,
    /// Why the test cannot run (extension missing, system without eye gaze).
    pub unavailable: Option<String>,
    pub phase: Phase,
    axis: Vec<Target>,
    grid: Vec<Target>,
    t: f64,
    frames: usize,
    readings: Vec<Reading>,
    axis_res: Vec<TargetResult>,
    grid_res: Vec<TargetResult>,
    pub verdict: Option<AxisVerdict>,
    pub report: Option<Report>,
    pub candidate: Option<Correction>,
    status: String,
}

impl GazeTest {
    pub fn new(points: usize, flips: [bool; 3]) -> GazeTest {
        GazeTest {
            points,
            flips,
            service_correction: None,
            unavailable: None,
            phase: Phase::Intro,
            axis: axis_targets(),
            grid: grid_targets(points),
            t: 0.0,
            frames: 0,
            readings: Vec::new(),
            axis_res: Vec::new(),
            grid_res: Vec::new(),
            verdict: None,
            report: None,
            candidate: None,
            status: String::new(),
        }
    }

    pub fn axis_len(&self) -> usize {
        self.axis.len()
    }

    pub fn grid_len(&self) -> usize {
        self.grid.len()
    }

    pub fn axis_results(&self) -> &[TargetResult] {
        &self.axis_res
    }

    pub fn grid_results(&self) -> &[TargetResult] {
        &self.grid_res
    }

    /// True while a target is being measured (no cursor then, no feedback).
    pub fn measuring(&self) -> bool {
        matches!(self.phase, Phase::Axis(_) | Phase::Grid(_))
    }

    fn reset_target(&mut self) {
        self.t = 0.0;
        self.frames = 0;
        self.readings.clear();
    }

    fn restart(&mut self) {
        self.phase = Phase::Intro;
        self.reset_target();
        self.axis_res.clear();
        self.grid_res.clear();
        self.verdict = None;
        self.report = None;
        self.candidate = None;
        self.status.clear();
    }

    pub fn command(&mut self, c: Cmd) -> Vec<Effect> {
        let mut fx = Vec::new();
        if self.unavailable.is_some() {
            return fx;
        }
        match (c, self.phase) {
            (Cmd::Restart, _) => {
                self.restart();
                fx.push(Effect::Print("gaze test restarted".to_string()));
            }
            (Cmd::Next, Phase::Intro) => {
                self.reset_target();
                self.phase = Phase::Axis(0);
                fx.push(Effect::Print(format!(
                    "axis check: {} dots, look at each one",
                    self.axis.len()
                )));
            }
            (Cmd::Next, Phase::AxisDone) => {
                self.reset_target();
                self.phase = Phase::Grid(0);
                fx.push(Effect::Print(format!(
                    "accuracy grid: {} dots, keep your head still and look at each one",
                    self.grid.len()
                )));
            }
            (Cmd::Next, Phase::Done) => self.phase = Phase::Cursor,
            (Cmd::Next, Phase::Cursor) => self.phase = Phase::Done,
            (Cmd::Next, _) => {}
            (Cmd::Accept, Phase::Done | Phase::Cursor) => fx.extend(self.accept()),
            (Cmd::Accept, _) => fx.push(Effect::Print(
                "nothing to accept yet: finish the accuracy grid first".to_string(),
            )),
        }
        fx
    }

    fn accept(&mut self) -> Vec<Effect> {
        if let Some(v) = &self.service_correction {
            self.status = "NOT SAVED: THE SERVICE ALREADY CORRECTS".to_string();
            return vec![Effect::Print(format!(
                "not saved: PIMAX_EYE_CORRECTION={v} means the service already corrects these samples, so a fit on them would be a correction of a correction. Run with PIMAX_EYE_CORRECTION=none (eye_accuracy.sh does)."
            ))];
        }
        let Some(c) = self.candidate.clone() else {
            self.status = "NO CORRECTION TO SAVE".to_string();
            return vec![Effect::Print(
                "no correction to save: the fit found none that beats the raw gaze".to_string(),
            )];
        };
        let notes = self
            .report
            .as_ref()
            .map(|r| {
                let f = r.fit.as_ref();
                vec![
                    r.summary.clone(),
                    format!(
                        "model {}: leave-one-out mean error {:.2} -> {:.2} deg",
                        c.model,
                        f.map_or(0.0, |f| f.before.mean),
                        f.and_then(|f| f.chosen_model()).map_or(0.0, |m| m.loo.mean)
                    ),
                ]
            })
            .unwrap_or_default();
        self.status = "CORRECTION SAVED, RESTART MONADO TO USE IT".to_string();
        vec![Effect::WriteCorrection(c.to_toml(&notes))]
    }

    /// Advance by `dt` seconds with this frame's reading (`None`: no valid gaze).
    pub fn frame(&mut self, dt: f64, reading: Option<Reading>) -> Vec<Effect> {
        let (list_len, idx) = match self.phase {
            Phase::Axis(i) => (self.axis.len(), i),
            Phase::Grid(i) => (self.grid.len(), i),
            _ => return Vec::new(),
        };
        self.t += dt.clamp(0.0, MAX_DT);
        if self.t >= SETTLE_S {
            self.frames += 1;
            if let Some(r) = reading {
                self.readings.push(r);
            }
        }
        if self.t < SETTLE_S + COLLECT_S {
            return Vec::new();
        }
        let is_axis = matches!(self.phase, Phase::Axis(_));
        let target = if is_axis {
            self.axis[idx].clone()
        } else {
            self.grid[idx].clone()
        };
        let res = compute_target(&target, &self.readings, self.frames);
        let mut fx =
            vec![Effect::Print(format!(
            "{} {}/{} ({}): error {:.2} deg, precision {:.2} deg, valid {:.0}% ({} of {} frames){}",
            if is_axis { "axis" } else { "grid" },
            idx + 1,
            list_len,
            target.name,
            res.err_deg,
            res.precision_deg,
            100.0 * res.valid_ratio,
            res.used,
            res.frames,
            if res.ok { "" } else { "  [too few samples, not used]" }
        ))];
        self.reset_target();
        if is_axis {
            self.axis_res.push(res);
        } else {
            self.grid_res.push(res);
        }
        if idx + 1 < list_len {
            self.phase = if is_axis {
                Phase::Axis(idx + 1)
            } else {
                Phase::Grid(idx + 1)
            };
        } else if is_axis {
            fx.extend(self.finish_axis());
        } else {
            fx.extend(self.finish_grid());
        }
        fx
    }

    fn finish_axis(&mut self) -> Vec<Effect> {
        self.phase = Phase::AxisDone;
        let mut fx = Vec::new();
        if self.axis_res.iter().all(|r| r.ok) {
            let m: [V3; 5] = std::array::from_fn(|i| self.axis_res[i].mean_dir);
            let v = axis_verdict(&m);
            let mut text = String::from("axis check result:");
            for l in v.lines(self.flips) {
                text.push_str("\n  ");
                text.push_str(&l);
            }
            if v.all_ok() {
                text.push_str("\n  signs are right: continue with the accuracy grid");
            } else if v.any_flip() {
                text.push_str("\n  fix the flips, restart the service and run again (the grid below would be measured in the wrong frame)");
            }
            fx.push(Effect::Print(text));
            self.verdict = Some(v);
        } else {
            fx.push(Effect::Print(
                "axis check: too few valid samples at some dots (blinks, eyes not tracked?); redo with R".to_string(),
            ));
            self.verdict = None;
        }
        fx
    }

    fn finish_grid(&mut self) -> Vec<Effect> {
        self.phase = Phase::Done;
        let r = build_report(&self.grid_res);
        self.candidate = r.correction(self.flips);
        let mut text = String::from("accuracy grid result:\n");
        text.push_str(&r.text());
        let json = results_json(&JsonInput {
            time_unix: 0,
            flips: self.flips,
            service_correction: self.service_correction.as_deref(),
            axis: &self.axis_res,
            verdict: self.verdict.as_ref(),
            grid: &self.grid_res,
            report: &r,
            correction: self.candidate.as_ref(),
        });
        if self.candidate.is_some() {
            text.push_str("press Y to accept the correction and write ~/.config/omarchyvr/eye-correction.toml (the service reads it at start: relink/restart Monado, see analysis/eye-monado.md)\n");
        }
        self.report = Some(r);
        vec![Effect::Print(text), Effect::SaveResults(json)]
    }

    /// The marks to draw for this moment; `reading` = the current gaze for the cursor.
    pub fn marks(&self, reading: Option<&Reading>) -> Vec<Mark> {
        let mut m = Vec::new();
        if let Phase::Axis(i) | Phase::Grid(i) = self.phase {
            let t = if matches!(self.phase, Phase::Axis(_)) {
                &self.axis[i]
            } else {
                &self.grid[i]
            };
            m.push(Mark {
                pos: t.point(),
                kind: MarkKind::Disc,
                radius_deg: dot_radius_deg(self.t),
                color: [1.0, 1.0, 1.0],
            });
            return m;
        }
        if let Some(r) = reading {
            if let Some(p) = plane_hit(r.origin, r.dir) {
                m.push(Mark {
                    pos: p,
                    kind: MarkKind::Ring(0.15),
                    radius_deg: CURSOR_RADIUS_DEG,
                    color: CURSOR_RAW,
                });
            }
            if matches!(self.phase, Phase::Done | Phase::Cursor) {
                if let Some(c) = &self.candidate {
                    if let Some(p) = plane_hit(r.origin, c.apply(r.dir)) {
                        m.push(Mark {
                            pos: p,
                            kind: MarkKind::Ring(0.15),
                            radius_deg: CURSOR_RADIUS_DEG * 0.7,
                            color: CURSOR_FIXED,
                        });
                    }
                }
            }
        }
        m
    }

    /// On-screen text for this moment; `have_gaze` = a valid gaze arrived this frame.
    pub fn hud(&self, have_gaze: bool) -> Vec<String> {
        if let Some(u) = &self.unavailable {
            let mut l = vec!["EYE GAZE NOT AVAILABLE".to_string()];
            l.extend(crate::patterns::wrap(&u.to_ascii_uppercase(), 60));
            l.push("RUN ANALYSIS/TOOLS/EYE_ACCURACY.SH (PIMAX_EYE=1)".to_string());
            l.push("Q QUIT".to_string());
            return l;
        }
        let mut l = Vec::new();
        match self.phase {
            Phase::Intro => {
                l.push("EYE TRACKING ACCURACY TEST".to_string());
                l.push("HEAD STILL, LOOK AT EACH DOT UNTIL IT MOVES".to_string());
                l.push(if have_gaze {
                    "GAZE OK (RING = WHERE THE TRACKER THINKS YOU LOOK)".to_string()
                } else {
                    "WAITING FOR GAZE (THE EYE CHIP TAKES ABOUT 2 S TO START)".to_string()
                });
                l.push("SPACE/A START   Q QUIT".to_string());
            }
            Phase::Axis(i) => l.push(format!("AXIS CHECK {}/{}   Q QUIT", i + 1, self.axis.len())),
            Phase::Grid(i) => l.push(format!("GRID {}/{}   Q QUIT", i + 1, self.grid.len())),
            Phase::AxisDone => {
                match &self.verdict {
                    Some(v) => l.extend(v.hud(self.flips)),
                    None => l.push("AXIS CHECK: TOO FEW SAMPLES, PRESS R".to_string()),
                }
                l.push("SPACE/A START ACCURACY GRID   R REDO   Q QUIT".to_string());
            }
            Phase::Done | Phase::Cursor => {
                if let Some(r) = &self.report {
                    if self.phase == Phase::Done {
                        l.push(format!(
                            "ERROR MEAN {:.2} P95 {:.2} MAX {:.2} DEG   PRECISION {:.2}   VALID {:.0}%",
                            r.overall.mean, r.overall.p95, r.overall.max, r.precision_deg, 100.0 * r.valid_ratio
                        ));
                        match (&r.after, r.fit.as_ref().and_then(|f| f.chosen_model())) {
                            (Some(a), Some(m)) => l.push(format!(
                                "CORRECTION {}: MEAN {:.2} -> {:.2}  P95 {:.2} -> {:.2} DEG",
                                m.name.to_ascii_uppercase(),
                                r.overall.mean,
                                a.mean,
                                r.overall.p95,
                                a.p95
                            )),
                            _ => l.push("NO CORRECTION OFFERED".to_string()),
                        }
                    } else {
                        l.push("WHITE RING: RAW GAZE   GREEN RING: CORRECTED".to_string());
                    }
                }
                if !self.status.is_empty() {
                    l.push(self.status.clone());
                }
                l.push(if self.phase == Phase::Done {
                    "Y ACCEPT   SPACE LIVE CURSOR   R REDO   Q QUIT".to_string()
                } else {
                    "Y ACCEPT   SPACE SUMMARY   R REDO   Q QUIT".to_string()
                });
            }
        }
        l
    }
}

// ---------------------------------------------------------------------------------------------------- files

/// Seconds since the Unix epoch to `YYYYMMDD-HHMMSS` (UTC).
pub fn stamp(unix: u64) -> String {
    let days = (unix / 86400) as i64;
    let rem = unix % 86400;
    // civil from days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!(
        "{y:04}{m:02}{d:02}-{:02}{:02}{:02}",
        rem / 3600,
        rem % 3600 / 60,
        rem % 60
    )
}

pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Write `text` to `path`, first copying an existing file to `path.bak-<stamp>` (numbered when that exists too).
/// Returns the backup path, if one was made.
pub fn write_with_backup(path: &Path, text: &str, stamp: &str) -> Result<Option<PathBuf>, String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    let mut backup = None;
    if path.exists() {
        let name = path
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        let mut b = path.with_file_name(format!("{name}.bak-{stamp}"));
        let mut n = 1;
        while b.exists() {
            b = path.with_file_name(format!("{name}.bak-{stamp}-{n}"));
            n += 1;
        }
        std::fs::copy(path, &b).map_err(|e| format!("backup {}: {e}", b.display()))?;
        backup = Some(b);
    }
    let tmp = path.with_extension("tmp-write");
    std::fs::write(&tmp, text).map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(backup)
}

/// Where the results JSON goes: `--gaze-out` (a directory, or a file name ending in .json), else
/// `~/.local/share/omarchyvr/eye-accuracy/eye-accuracy-<stamp>.json`.
pub fn results_path(out: Option<&Path>, stamp: &str) -> PathBuf {
    let file = format!("eye-accuracy-{stamp}.json");
    match out {
        Some(p) if p.extension().is_some_and(|e| e == "json") => p.to_path_buf(),
        Some(p) => p.join(file),
        None => std::env::var_os("HOME")
            .map_or_else(|| PathBuf::from("."), PathBuf::from)
            .join(".local/share/omarchyvr/eye-accuracy")
            .join(file),
    }
}

/// Carry out an [`Effect`]; returns the line to print. `out` is `--gaze-out`.
pub fn execute(fx: &Effect, out: Option<&Path>, correction_path: Option<&Path>) -> String {
    let now = unix_now();
    let st = stamp(now);
    match fx {
        Effect::Print(s) => s.clone(),
        Effect::SaveResults(json) => {
            let json = json.replacen("\"time_unix\": 0,", &format!("\"time_unix\": {now},"), 1);
            let path = results_path(out, &st);
            match write_with_backup(&path, &json, &st) {
                Ok(_) => format!("results saved: {}", path.display()),
                Err(e) => format!("could not save the results: {e}"),
            }
        }
        Effect::WriteCorrection(text) => {
            let Some(path) = correction_path
                .map(Path::to_path_buf)
                .or_else(Correction::default_path)
            else {
                return "cannot write the correction: no $HOME".to_string();
            };
            match write_with_backup(&path, text, &st) {
                Ok(b) => format!(
                    "correction written: {}{}\nthe service reads it when it starts: rebuild/relink the eye Monado (cargo build --release -p pimax-capi, then ninja -C ~/.local/lib/omarchyvr/monado; relinking drops setcap cap_sys_nice, see analysis/eye-monado.md) and restart monado-service",
                    path.display(),
                    b.map_or(String::new(), |b| format!(" (previous file kept as {})", b.display()))
                ),
                Err(e) => format!("could not write the correction: {e}"),
            }
        }
    }
}

#[cfg(test)]
mod tests;
