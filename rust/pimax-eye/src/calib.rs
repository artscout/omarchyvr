//! Per-eye host-side gaze calibration (fitted by `pimax-eyecal`, applied by the live pipeline).
//!
//! The EyeChip's own calibration is never touched (no calibration opcode is sent). Each eye's raw head-frame gaze
//! direction (after the device-to-head conversion of [`crate::track::FrameConv`]) is mapped on its own, then the two
//! corrected rays are combined.
//!
//! # Per-eye map
//!
//! On the tangent `(u, v) = (x / -z, y / -z)` of a head-frame direction (OpenXR axes, -z forward) a polynomial
//! `u' = cu . f(u, v)`, `v' = cv . f(u, v)`, `f = (1, u, v, u^2, u v, v^2)` (the [`Correction`] of the old combined
//! correction). Three model sizes:
//!
//! * `offset`: identity plus a constant (2 free numbers per eye): what a kappa angle / headset slip looks like,
//! * `affine`: plus gain and cross terms (6 per eye): the ~0.6 gain of the uncalibrated chip,
//! * `quadratic`: plus the second-order terms (12 per eye), ridge-regularised towards affine: the compressed upper
//!   field seen in the captures.
//!
//! The fit ([`fit_eye`]) is weighted least squares on per-target central values (median of the fixation samples),
//! with the identity map as the prior (ridge on the departure from it: free offset, small penalty on the linear
//! terms, a selectable one on the quadratic terms) and Huber reweighting (IRLS) so that one bad target cannot drag the
//! map. [`select_model`] picks the model by leave-one-target-out error and prefers the simpler one unless the richer
//! one is clearly better.
//!
//! # Origin terms
//!
//! The chip's direction is "from its estimate of the eye centre to the pupil", so an error of 1 mm in the reported
//! eye-centre origin is a direction error of about 1 mm / 10 mm = 6 degrees (measured on the Dream Air: 0.10 tangent
//! per mm, 14 Oct). A map that sees only the direction cannot remove that; [`OriginTerms`] adds
//! `u' += ou . d, v' += ov . d` with `d` the reported origin minus a reference origin (millimetres), and a gate
//! that drops (hard) or down-weights (soft) an eye whose origin is far from the reference (the chip's bad state:
//! origin jumps of 8 mm and more with a garbage direction). Fitted on the individual fixation samples by
//! [`crate::calfit`].
//!
//! The truth for an eye is the direction from *that eye's rendering position* (the OpenXR view position) to the
//! target: that is the direction in which the eye saw the target's pixels (collimated optics).
//!
//! # Combination
//!
//! Each valid eye's corrected ray is followed to a fixation distance ([`Combine::depth_m`], default 1.5 m) from its
//! eye position; the hit points are averaged with weights (inverse error variance per eye, from the validation) and
//! the combined ray goes from the midpoint of the eyes to that point. With one eye the same construction keeps the
//! direction consistent (no jump of half the vergence angle when the other eye blinks). When both eyes are valid but
//! disagree by more than [`Combine::max_disagree_deg`], the eye with the smaller expected error is used alone. An eye
//! that comes back after being invalid is ignored for [`Combine::holdoff`] samples (eyelid artefacts at blink end).
//!
//! # File (`~/.config/pimax/eye-calibration.toml`)
//!
//! Written by `pimax-eyecal` (an existing file is kept as `.bak-<utc>`), read by the service at every tracker start
//! (`PIMAX_EYE_CALIBRATION=<path>|none`; unset: the default file when it exists). See [`Calibration::to_toml`].

use crate::correction::{dir_of, tangent_of, Correction};
use crate::filter::FilterParams;
use crate::track::{EyeRay, FrameConv, Gaze, Sample};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

/// Model size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelKind {
    /// Identity plus a constant offset.
    Offset,
    /// Affine.
    Affine,
    /// Quadratic (ridge towards affine).
    Quadratic,
}

impl ModelKind {
    /// Number of features used.
    pub fn features(self) -> usize {
        match self {
            ModelKind::Offset => 1,
            ModelKind::Affine => 3,
            ModelKind::Quadratic => 6,
        }
    }

    /// File name of the model.
    pub fn name(self) -> &'static str {
        match self {
            ModelKind::Offset => "offset",
            ModelKind::Affine => "affine",
            ModelKind::Quadratic => "quadratic",
        }
    }

    /// Parse a file name.
    pub fn parse(s: &str) -> Option<ModelKind> {
        match s {
            "offset" => Some(ModelKind::Offset),
            "affine" => Some(ModelKind::Affine),
            "quadratic" => Some(ModelKind::Quadratic),
            _ => None,
        }
    }

    /// Minimum targets for a fit.
    pub fn min_points(self) -> usize {
        match self {
            ModelKind::Offset => 1,
            ModelKind::Affine => 4,
            ModelKind::Quadratic => 7,
        }
    }
}

/// One calibration target seen by one eye: central raw tangent, truth tangent, weight.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EyePoint {
    /// Robust central raw gaze tangent (u, v).
    pub raw: (f64, f64),
    /// Tangent of the direction from the eye's rendering position to the target.
    pub truth: (f64, f64),
    /// Relative weight (number of good samples, dispersion); 1 is normal.
    pub weight: f64,
}

/// Angle in degrees between two tangents (as directions).
pub fn tangent_angle_deg(a: (f64, f64), b: (f64, f64)) -> f64 {
    let (da, db) = (dir_of(a.0, a.1), dir_of(b.0, b.1));
    let c = [
        da[1] * db[2] - da[2] * db[1],
        da[2] * db[0] - da[0] * db[2],
        da[0] * db[1] - da[1] * db[0],
    ];
    let s = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let d = da[0] * db[0] + da[1] * db[1] + da[2] * db[2];
    s.atan2(d).to_degrees()
}

/// Mean, median, 95th percentile and maximum.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Stats {
    /// Count.
    pub n: usize,
    /// Mean.
    pub mean: f64,
    /// Median.
    pub median: f64,
    /// 95th percentile.
    pub p95: f64,
    /// Maximum.
    pub max: f64,
}

/// Statistics of a list.
pub fn stats(v: &[f64]) -> Stats {
    if v.is_empty() {
        return Stats::default();
    }
    let mut s: Vec<f64> = v.iter().copied().filter(|x| x.is_finite()).collect();
    if s.is_empty() {
        return Stats::default();
    }
    s.sort_by(|a, b| a.total_cmp(b));
    let pct = |p: f64| {
        let x = p * (s.len() - 1) as f64;
        let i = x.floor() as usize;
        let f = x - i as f64;
        s[i] + (s[(i + 1).min(s.len() - 1)] - s[i]) * f
    };
    Stats {
        n: s.len(),
        mean: s.iter().sum::<f64>() / s.len() as f64,
        median: pct(0.5),
        p95: pct(0.95),
        max: s[s.len() - 1],
    }
}

fn features(u: f64, v: f64) -> [f64; 6] {
    crate::correction::features(u, v)
}

/// Gaussian elimination with partial pivoting; `None` for a singular system.
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| a[i][c].abs().total_cmp(&a[j][c].abs()))?;
        if a[p][c].abs() < 1e-14 {
            return None;
        }
        a.swap(c, p);
        b.swap(c, p);
        for r in c + 1..n {
            let f = a[r][c] / a[c][c];
            if f == 0.0 {
                continue;
            }
            let pivot = a[c].clone();
            for (x, p) in a[r].iter_mut().zip(&pivot).skip(c) {
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

/// Ridge strengths of a fit, relative to each feature's own information (`sum w f^2`): 1 halves a coefficient that
/// the data alone would set, 0.01 barely touches it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Ridge {
    /// On the linear terms' departure from identity.
    pub linear: f64,
    /// On the quadratic terms.
    pub quadratic: f64,
}

impl Default for Ridge {
    fn default() -> Self {
        Ridge {
            linear: 1e-3,
            quadratic: 0.2,
        }
    }
}

/// Weighted ridge least squares of the departure from identity; `w` overrides the point weights.
fn fit_weighted(points: &[EyePoint], w: &[f64], kind: ModelKind, ridge: Ridge) -> Option<Correction> {
    let k = kind.features();
    let total: f64 = w.iter().sum();
    if points.len() < kind.min_points().min(k) || total <= 0.0 {
        return None;
    }
    let mut ata = vec![vec![0.0; k]; k];
    let (mut bu, mut bv) = (vec![0.0; k], vec![0.0; k]);
    for (p, &wi) in points.iter().zip(w) {
        let f = features(p.raw.0, p.raw.1);
        let (du, dv) = (p.truth.0 - p.raw.0, p.truth.1 - p.raw.1);
        for i in 0..k {
            for j in 0..k {
                ata[i][j] += wi * f[i] * f[j];
            }
            bu[i] += wi * f[i] * du;
            bv[i] += wi * f[i] * dv;
        }
    }
    // relative ridge: each penalty is a fraction of that feature's own information, so the strengths do not depend
    // on the size of the tangents
    for (i, row) in ata.iter_mut().enumerate() {
        let pen = match i {
            0 => 1e-12,
            1 | 2 => ridge.linear,
            _ => ridge.quadratic,
        };
        row[i] += pen * row[i].max(1e-9 * total);
    }
    let du = solve(ata.clone(), bu)?;
    let dv = solve(ata, bv)?;
    let mut c = Correction::identity();
    for i in 0..k {
        c.cx[i] += du[i];
        c.cy[i] += dv[i];
    }
    c.model = kind.name().to_string();
    Some(c)
}

/// Huber threshold in tangent units (about 1.1 degrees).
pub const HUBER_K: f64 = 0.02;

/// Robust fit: weighted ridge least squares with Huber reweighting (5 iterations).
pub fn fit_eye(points: &[EyePoint], kind: ModelKind, ridge: Ridge) -> Option<Correction> {
    let base: Vec<f64> = points.iter().map(|p| p.weight.max(0.0)).collect();
    let mut w = base.clone();
    let mut c = fit_weighted(points, &w, kind, ridge)?;
    for _ in 0..5 {
        for (i, p) in points.iter().enumerate() {
            let (u, v) = c.map_tangent(p.raw.0, p.raw.1);
            let r = ((u - p.truth.0).powi(2) + (v - p.truth.1).powi(2)).sqrt();
            w[i] = base[i] * if r <= HUBER_K { 1.0 } else { HUBER_K / r };
        }
        c = fit_weighted(points, &w, kind, ridge)?;
    }
    Some(c)
}

/// Error in degrees of map `c` at point `p`.
pub fn point_error_deg(c: &Correction, p: &EyePoint) -> f64 {
    tangent_angle_deg(c.map_tangent(p.raw.0, p.raw.1), p.truth)
}

/// One evaluated model.
#[derive(Debug, Clone, PartialEq)]
pub struct ModelFit {
    /// Model size.
    pub kind: ModelKind,
    /// Ridge used.
    pub ridge: Ridge,
    /// The map fitted on all points.
    pub map: Correction,
    /// Leave-one-target-out error (degrees).
    pub loo: Stats,
    /// Error at the fit points.
    pub train: Stats,
    /// Origin coefficients `(u, v)` per millimetre when the model has them (sample-level fits, [`crate::calfit`]).
    pub org: Option<([f64; 3], [f64; 3])>,
    /// Leave-one-out error of every target, in the order of the fit points.
    pub loo_each: Vec<f64>,
}

/// Fit `kind` on all points and score it by leave-one-target-out.
pub fn evaluate(points: &[EyePoint], kind: ModelKind, ridge: Ridge) -> Option<ModelFit> {
    if points.len() < kind.min_points() + 1 {
        return None;
    }
    let map = fit_eye(points, kind, ridge)?;
    let train: Vec<f64> = points.iter().map(|p| point_error_deg(&map, p)).collect();
    let mut loo = Vec::new();
    for i in 0..points.len() {
        let rest: Vec<EyePoint> = points
            .iter()
            .enumerate()
            .filter(|(j, _)| *j != i)
            .map(|(_, p)| *p)
            .collect();
        let m = fit_eye(&rest, kind, ridge)?;
        loo.push(point_error_deg(&m, &points[i]));
    }
    Some(ModelFit {
        kind,
        ridge,
        map,
        loo: stats(&loo),
        train: stats(&train),
        org: None,
        loo_each: loo,
    })
}

/// All candidate models and the chosen one.
#[derive(Debug, Clone, PartialEq)]
pub struct Selection {
    /// Error of the raw gaze at the points.
    pub raw: Stats,
    /// Candidates in increasing complexity.
    pub candidates: Vec<ModelFit>,
    /// Index of the chosen candidate.
    pub chosen: Option<usize>,
}

impl Selection {
    /// The chosen fit.
    pub fn best(&self) -> Option<&ModelFit> {
        self.chosen.map(|i| &self.candidates[i])
    }
}

/// Fit offset, affine and quadratic (three ridge strengths) and keep the one with the lowest leave-one-out mean
/// error, unless a simpler one is within 7 % (or 0.05 degrees) of it.
pub fn select_model(points: &[EyePoint]) -> Selection {
    let raw = stats(
        &points
            .iter()
            .map(|p| tangent_angle_deg(p.raw, p.truth))
            .collect::<Vec<_>>(),
    );
    let mut candidates = Vec::new();
    candidates.extend(evaluate(points, ModelKind::Offset, Ridge::default()));
    candidates.extend(evaluate(points, ModelKind::Affine, Ridge::default()));
    for q in [1.0, 0.2, 0.02] {
        candidates.extend(evaluate(
            points,
            ModelKind::Quadratic,
            Ridge {
                quadratic: q,
                ..Ridge::default()
            },
        ));
    }
    let best = candidates
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.loo.mean.total_cmp(&b.1.loo.mean))
        .map(|(i, m)| (i, m.loo.mean));
    let chosen = best.map(|(bi, bm)| {
        // the simplest candidate that is close enough to the best (candidates are in increasing complexity)
        candidates
            .iter()
            .position(|m| m.loo.mean <= bm * 1.07 || m.loo.mean - bm <= 0.05)
            .unwrap_or(bi)
    });
    Selection {
        raw,
        candidates,
        chosen,
    }
}

/// Origin-dependent part of an eye's correction and the reliability gate (all zero: not used).
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct OriginTerms {
    /// Reference origin (the median reported origin of the calibration fixations), metres.
    pub reference_m: [f64; 3],
    /// Tangent added to `u` per millimetre of origin deviation (x, y, z).
    pub u: [f64; 3],
    /// Tangent added to `v` per millimetre of origin deviation (x, y, z).
    pub v: [f64; 3],
    /// Origin farther than this from the reference (millimetres) is the chip's bad state: the eye is dropped.
    /// 0: no gate.
    pub gate_mm: f64,
    /// Reference pupil diameter (the median of the calibration fixations), millimetres.
    pub pupil_ref_mm: f64,
    /// Tangent added to `u` per millimetre of pupil diameter deviation (clamped to +-[`PUPIL_CLAMP_MM`]).
    pub pupil_u: f64,
    /// Same for `v`.
    pub pupil_v: f64,
}

/// Largest pupil deviation the pupil terms are applied for, millimetres (the terms are fitted inside the range the
/// calibration scene produced; content of another brightness must not extrapolate them).
pub const PUPIL_CLAMP_MM: f64 = 1.0;

impl OriginTerms {
    /// Whether anything is set.
    pub fn is_active(&self) -> bool {
        self.gate_mm > 0.0 || self.u.iter().chain(&self.v).any(|c| *c != 0.0) || self.pupil_u != 0.0 || self.pupil_v != 0.0
    }

    /// Tangent shift of the pupil terms for a reported diameter (zero when unknown or the terms are not set).
    pub fn pupil_shift(&self, pupil_mm: Option<f64>) -> (f64, f64) {
        match pupil_mm {
            Some(p) if p.is_finite() && p > 0.0 && (self.pupil_u != 0.0 || self.pupil_v != 0.0) => {
                let d = (p - self.pupil_ref_mm).clamp(-PUPIL_CLAMP_MM, PUPIL_CLAMP_MM);
                (self.pupil_u * d, self.pupil_v * d)
            }
            _ => (0.0, 0.0),
        }
    }

    /// Deviation of `origin` (metres) from the reference, millimetres.
    pub fn deviation_mm(&self, origin: [f64; 3]) -> [f64; 3] {
        [
            (origin[0] - self.reference_m[0]) * 1e3,
            (origin[1] - self.reference_m[1]) * 1e3,
            (origin[2] - self.reference_m[2]) * 1e3,
        ]
    }

    /// Distance of `origin` from the reference, millimetres.
    pub fn distance_mm(&self, origin: [f64; 3]) -> f64 {
        let d = self.deviation_mm(origin);
        (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt()
    }

    /// Tangent shift for `origin` (zero when the terms are not set).
    pub fn shift(&self, origin: [f64; 3]) -> (f64, f64) {
        if !self.is_active() {
            return (0.0, 0.0);
        }
        let d = self.deviation_mm(origin);
        (
            self.u[0] * d[0] + self.u[1] * d[1] + self.u[2] * d[2],
            self.v[0] * d[0] + self.v[1] * d[1] + self.v[2] * d[2],
        )
    }

    /// False for a non-finite origin or one beyond the gate.
    pub fn usable(&self, origin: [f64; 3]) -> bool {
        origin.iter().all(|c| c.is_finite()) && (self.gate_mm <= 0.0 || self.distance_mm(origin) <= self.gate_mm)
    }

    /// Soft reliability in (0, 1]: 1 at the reference, Gaussian in the distance with sigma = gate / 2 (1 without
    /// a gate).
    pub fn reliability(&self, origin: [f64; 3]) -> f64 {
        if self.gate_mm <= 0.0 {
            return 1.0;
        }
        let z = self.distance_mm(origin) / (0.5 * self.gate_mm);
        (-0.5 * z * z).exp()
    }
}

/// One eye of a calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeCal {
    /// The map (its `model` names the size).
    pub map: Correction,
    /// Rendering position of the eye in the head frame, metres (OpenXR VIEW space).
    pub origin_m: [f64; 3],
    /// Expected angular error of this eye after calibration (validation), degrees.
    pub accuracy_deg: f64,
    /// Sample-to-sample noise (RMS), degrees.
    pub precision_deg: f64,
    /// Drift offset added to the corrected tangent (recentre).
    pub drift: (f64, f64),
    /// Origin terms and gate (version 2 files).
    pub org: OriginTerms,
}

impl EyeCal {
    /// Calibrated head-frame direction of a raw head-frame direction.
    pub fn apply(&self, raw: [f64; 3]) -> Option<[f64; 3]> {
        self.apply_with(raw, (0.0, 0.0))
    }

    /// [`EyeCal::apply`] with an extra (running) drift offset.
    pub fn apply_with(&self, raw: [f64; 3], extra: (f64, f64)) -> Option<[f64; 3]> {
        self.apply_full(raw, None, None, extra)
    }

    /// [`EyeCal::apply_with`] with the origin the tracker reported for this sample (metres): the origin terms are
    /// added, and `None` is returned for an origin beyond the gate.
    pub fn apply_full(
        &self,
        raw: [f64; 3],
        origin: Option<[f64; 3]>,
        pupil_mm: Option<f64>,
        extra: (f64, f64),
    ) -> Option<[f64; 3]> {
        let (u, v) = tangent_of(raw)?;
        let (u2, v2) = self.map.map_tangent(u, v);
        let (ou, ov) = match origin {
            Some(o) => {
                if !self.org.usable(o) {
                    return None;
                }
                self.org.shift(o)
            }
            None => (0.0, 0.0),
        };
        let (pu, pv) = self.org.pupil_shift(pupil_mm);
        Some(dir_of(
            u2 + self.drift.0 + extra.0 + ou + pu,
            v2 + self.drift.1 + extra.1 + ov + pv,
        ))
    }

    /// Inverse error variance used for the combination.
    pub fn weight(&self) -> f64 {
        let a = self.accuracy_deg.max(0.2);
        let p = self.precision_deg.max(0.1);
        1.0 / (a * a + p * p)
    }
}

/// How the two eyes become one ray.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Combine {
    /// Fixation distance the rays are followed to, metres.
    pub depth_m: f64,
    /// Larger disagreement between the eyes means one of them is wrong (degrees).
    pub max_disagree_deg: f64,
    /// Samples an eye is ignored after it was invalid.
    pub holdoff: u32,
}

impl Default for Combine {
    fn default() -> Self {
        Combine {
            depth_m: 1.5,
            max_disagree_deg: 4.0,
            holdoff: 2,
        }
    }
}

/// A complete calibration.
#[derive(Debug, Clone, PartialEq)]
pub struct Calibration {
    /// Device-to-head conversion the calibration was measured with (the service uses it).
    pub conv: FrameConv,
    /// `[left, right]`.
    pub eyes: [EyeCal; 2],
    /// Combination.
    pub combine: Combine,
    /// Output filter.
    pub filter: FilterParams,
    /// Unix time of the measurement.
    pub created_unix: u64,
    /// Free text lines written as comments (validation summary, ...).
    pub notes: Vec<String>,
}

fn fmt_arr(c: &[f64]) -> String {
    let v: Vec<String> = c.iter().map(|x| format!("{x:.9}")).collect();
    format!("[{}]", v.join(", "))
}

impl Calibration {
    /// Per-eye identity maps (for tests and as a start).
    pub fn identity(conv: FrameConv, ipd_m: f64) -> Calibration {
        let eye = |x: f64| EyeCal {
            map: Correction::identity(),
            origin_m: [x, 0.0, 0.0],
            accuracy_deg: 1.0,
            precision_deg: 0.5,
            drift: (0.0, 0.0),
            org: OriginTerms::default(),
        };
        Calibration {
            conv,
            eyes: [eye(-ipd_m / 2.0), eye(ipd_m / 2.0)],
            combine: Combine::default(),
            filter: FilterParams::default(),
            created_unix: 0,
            notes: Vec::new(),
        }
    }

    /// The file text.
    pub fn to_toml(&self) -> String {
        let mut s = String::from("# pimax eye calibration: per-eye host-side maps (written by pimax-eyecal; the eye chip's own calibration is not touched)\n");
        for n in &self.notes {
            let _ = writeln!(s, "# {n}");
        }
        s.push_str("# per eye: u' = u . (1, u, v, u*u, u*v, v*v), v' = v . (...) on the tangent (x / -z, y / -z) of the raw head-frame direction; then drift and (version 2) origin_u/v . (origin - origin_ref) in mm are added\n");
        let v2 = self.eyes.iter().any(|e| e.org.is_active());
        let _ = writeln!(s, "version = {}", if v2 { 2 } else { 1 });
        let _ = writeln!(s, "created_unix = {}", self.created_unix);
        let c = &self.conv;
        let _ = writeln!(
            s,
            "flip = \"{},{},{}\"",
            u8::from(c.flip_x),
            u8::from(c.flip_y),
            u8::from(c.flip_z)
        );
        let _ = writeln!(s, "swap = {}", u8::from(c.swap_eyes));
        let _ = writeln!(s, "offset_mm = {}", fmt_arr(&c.offset_mm));
        let _ = writeln!(s, "depth_m = {:.4}", self.combine.depth_m);
        let _ = writeln!(s, "max_disagree_deg = {:.3}", self.combine.max_disagree_deg);
        let _ = writeln!(s, "holdoff = {}", self.combine.holdoff);
        for (name, e) in ["left", "right"].iter().zip(&self.eyes) {
            let _ = writeln!(s, "\n[{name}]");
            let _ = writeln!(s, "model = \"{}\"", e.map.model);
            let _ = writeln!(s, "u = {}", fmt_arr(&e.map.cx));
            let _ = writeln!(s, "v = {}", fmt_arr(&e.map.cy));
            let _ = writeln!(s, "origin_m = {}", fmt_arr(&e.origin_m));
            let _ = writeln!(s, "accuracy_deg = {:.4}", e.accuracy_deg);
            let _ = writeln!(s, "precision_deg = {:.4}", e.precision_deg);
            let _ = writeln!(s, "drift = {}", fmt_arr(&[e.drift.0, e.drift.1]));
            if e.org.is_active() {
                let _ = writeln!(s, "origin_ref_m = {}", fmt_arr(&e.org.reference_m));
                let _ = writeln!(s, "origin_u = {}", fmt_arr(&e.org.u));
                let _ = writeln!(s, "origin_v = {}", fmt_arr(&e.org.v));
                let _ = writeln!(s, "origin_gate_mm = {:.3}", e.org.gate_mm);
                if e.org.pupil_u != 0.0 || e.org.pupil_v != 0.0 {
                    let _ = writeln!(s, "pupil_ref_mm = {:.4}", e.org.pupil_ref_mm);
                    let _ = writeln!(s, "pupil_uv = {}", fmt_arr(&[e.org.pupil_u, e.org.pupil_v]));
                }
            }
        }
        let f = &self.filter;
        let _ = writeln!(s, "\n[filter]");
        let _ = writeln!(s, "kind = \"{}\"", if f.enabled { "one-euro" } else { "none" });
        let _ = writeln!(s, "min_cutoff_hz = {:.4}", f.min_cutoff_hz);
        let _ = writeln!(s, "beta = {:.5}", f.beta);
        let _ = writeln!(s, "d_cutoff_hz = {:.4}", f.d_cutoff_hz);
        let _ = writeln!(s, "snap_deg = {:.3}", f.snap_deg);
        s
    }

    /// Parse the file text.
    pub fn parse(text: &str) -> Result<Calibration, String> {
        let mut cal = Calibration::identity(FrameConv::default(), 0.064);
        let mut section = String::new();
        let mut seen = [false; 2];
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let err = |m: &str| format!("line {}: {m}", n + 1);
            if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = sec.trim().to_string();
                continue;
            }
            let (k, v) = line.split_once('=').ok_or_else(|| err("expected key = value"))?;
            let (k, v) = (k.trim(), v.trim());
            let unq = || v.trim_matches('"').trim().to_string();
            let num = || v.parse::<f64>().map_err(|e| err(&format!("{k}: {e}")));
            let arr = || -> Result<Vec<f64>, String> {
                let inner = v
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .ok_or_else(|| err(&format!("{k} must be [a, b, ...]")))?;
                inner
                    .split(',')
                    .filter(|p| !p.trim().is_empty())
                    .map(|p| p.trim().parse::<f64>().map_err(|e| err(&format!("{k}: {e}"))))
                    .collect()
            };
            match (section.as_str(), k) {
                ("", "version") => {
                    if !matches!(num()? as i64, 1 | 2) {
                        return Err(err("unsupported version"));
                    }
                }
                ("", "created_unix") => cal.created_unix = num()?.max(0.0) as u64,
                ("", "flip") => {
                    let p: Vec<bool> = unq().split(',').map(|t| t.trim() == "1").collect();
                    if p.len() != 3 {
                        return Err(err("flip needs 3 values"));
                    }
                    cal.conv.flip_x = p[0];
                    cal.conv.flip_y = p[1];
                    cal.conv.flip_z = p[2];
                }
                ("", "swap") => cal.conv.swap_eyes = num()? != 0.0,
                ("", "offset_mm") => {
                    let a = arr()?;
                    if a.len() != 3 {
                        return Err(err("offset_mm needs 3 numbers"));
                    }
                    cal.conv.offset_mm = [a[0], a[1], a[2]];
                }
                ("", "depth_m") => cal.combine.depth_m = num()?.clamp(0.2, 100.0),
                ("", "max_disagree_deg") => cal.combine.max_disagree_deg = num()?.max(0.5),
                ("", "holdoff") => cal.combine.holdoff = num()?.clamp(0.0, 30.0) as u32,
                ("left" | "right", key) => {
                    let i = usize::from(section == "right");
                    seen[i] = true;
                    let e = &mut cal.eyes[i];
                    match key {
                        "model" => {
                            let m = unq();
                            if ModelKind::parse(&m).is_none() {
                                return Err(err(&format!("unknown model '{m}'")));
                            }
                            e.map.model = m;
                        }
                        "u" | "v" => {
                            let a = arr()?;
                            if !(a.len() == 3 || a.len() == 6) || a.iter().any(|x| !x.is_finite()) {
                                return Err(err(&format!("{key} needs 3 or 6 finite numbers")));
                            }
                            let mut c = [0.0; 6];
                            c[..a.len()].copy_from_slice(&a);
                            if key == "u" {
                                e.map.cx = c;
                            } else {
                                e.map.cy = c;
                            }
                        }
                        "origin_m" => {
                            let a = arr()?;
                            if a.len() != 3 {
                                return Err(err("origin_m needs 3 numbers"));
                            }
                            e.origin_m = [a[0], a[1], a[2]];
                        }
                        "accuracy_deg" => e.accuracy_deg = num()?.max(0.0),
                        "precision_deg" => e.precision_deg = num()?.max(0.0),
                        "drift" => {
                            let a = arr()?;
                            if a.len() != 2 {
                                return Err(err("drift needs 2 numbers"));
                            }
                            e.drift = (a[0], a[1]);
                        }
                        "origin_ref_m" | "origin_u" | "origin_v" => {
                            let a = arr()?;
                            if a.len() != 3 || a.iter().any(|x| !x.is_finite()) {
                                return Err(err(&format!("{key} needs 3 finite numbers")));
                            }
                            let t = [a[0], a[1], a[2]];
                            match key {
                                "origin_ref_m" => e.org.reference_m = t,
                                "origin_u" => e.org.u = t,
                                _ => e.org.v = t,
                            }
                        }
                        "origin_gate_mm" => e.org.gate_mm = num()?.clamp(0.0, 100.0),
                        "pupil_ref_mm" => e.org.pupil_ref_mm = num()?.clamp(0.0, 20.0),
                        "pupil_uv" => {
                            let a = arr()?;
                            if a.len() != 2 || a.iter().any(|x| !x.is_finite()) {
                                return Err(err("pupil_uv needs 2 finite numbers"));
                            }
                            e.org.pupil_u = a[0];
                            e.org.pupil_v = a[1];
                        }
                        _ => {}
                    }
                }
                ("filter", key) => {
                    let f = &mut cal.filter;
                    match key {
                        "kind" => f.enabled = unq() == "one-euro",
                        "min_cutoff_hz" => f.min_cutoff_hz = num()?.clamp(0.01, 100.0),
                        "beta" => f.beta = num()?.clamp(0.0, 10.0),
                        "d_cutoff_hz" => f.d_cutoff_hz = num()?.clamp(0.01, 100.0),
                        "snap_deg" => f.snap_deg = num()?.clamp(0.0, 90.0),
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        if !(seen[0] && seen[1]) {
            return Err("both [left] and [right] sections are required".into());
        }
        Ok(cal)
    }

    /// Load a file.
    pub fn load(path: &Path) -> Result<Calibration, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Calibration::parse(&t).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `~/.config/pimax/eye-calibration.toml`.
    pub fn default_path() -> Option<PathBuf> {
        std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config/pimax/eye-calibration.toml"))
    }

    /// Selection by `PIMAX_EYE_CALIBRATION` (`none`/`off`/`0`: no calibration; a path: that file, which must load;
    /// unset or empty: the default file when it exists).
    pub fn select(var: Option<&str>, home: Option<PathBuf>) -> Result<Option<(Calibration, PathBuf)>, String> {
        match var.map(str::trim) {
            Some("none" | "off" | "0") => Ok(None),
            Some(p) if !p.is_empty() => {
                let p = PathBuf::from(p);
                Calibration::load(&p).map(|c| Some((c, p)))
            }
            _ => match home.map(|h| h.join(".config/pimax/eye-calibration.toml")) {
                Some(p) if p.exists() => Calibration::load(&p).map(|c| Some((c, p))),
                _ => Ok(None),
            },
        }
    }

    /// [`Calibration::select`] with the real environment.
    pub fn from_env() -> Result<Option<(Calibration, PathBuf)>, String> {
        Calibration::select(
            std::env::var("PIMAX_EYE_CALIBRATION").ok().as_deref(),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }
}

/// Running state of the combination (blink hold-off).
#[derive(Debug, Clone, Default)]
pub struct CombineState {
    invalid_run_end: [u32; 2],
    since_valid: [u32; 2],
}

fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

fn scale(a: [f64; 3], s: f64) -> [f64; 3] {
    [a[0] * s, a[1] * s, a[2] * s]
}

fn unit(a: [f64; 3]) -> Option<[f64; 3]> {
    let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
    (l > 1e-12 && l.is_finite()).then(|| scale(a, 1.0 / l))
}

fn angle_deg(a: [f64; 3], b: [f64; 3]) -> f64 {
    let c = [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ];
    let s = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    s.atan2(a[0] * b[0] + a[1] * b[1] + a[2] * b[2]).to_degrees()
}

fn f64v(v: [f32; 3]) -> [f64; 3] {
    [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])]
}

fn f32v(v: [f64; 3]) -> [f32; 3] {
    [v[0] as f32, v[1] as f32, v[2] as f32]
}

impl Calibration {
    /// Calibrate a raw sample in place: per-eye directions are replaced by the calibrated ones (origins by the
    /// calibration's eye positions), and the combined ray is rebuilt as described in the module docs.
    pub fn apply_sample(&self, s: &mut Sample, st: &mut CombineState) {
        self.apply_sample_with(s, st, [(0.0, 0.0); 2]);
    }

    /// [`Calibration::apply_sample`] with running drift offsets per eye (see [`crate::drift`]).
    pub fn apply_sample_with(&self, s: &mut Sample, st: &mut CombineState, extra: [(f64, f64); 2]) {
        let mut hits: Vec<([f64; 3], f64, usize)> = Vec::with_capacity(2);
        #[allow(clippy::needless_range_loop)] // four parallel per-eye arrays
        for i in 0..2 {
            let e = &mut s.eyes[i];
            let ok = e.origin_valid && e.direction_valid;
            if !ok {
                st.since_valid[i] = 0;
                st.invalid_run_end[i] = self.combine.holdoff;
                continue;
            }
            let raw_origin = f64v(e.origin);
            if !self.eyes[i].org.usable(raw_origin) {
                // the chip's bad state (origin far from where this wearer's eye is): drop the eye like a blink
                e.direction_valid = false;
                st.since_valid[i] = 0;
                st.invalid_run_end[i] = self.combine.holdoff;
                continue;
            }
            st.since_valid[i] = st.since_valid[i].saturating_add(1);
            let Some(d) = self.eyes[i].apply_full(f64v(e.direction), Some(raw_origin), Some(f64::from(e.pupil_mm)), extra[i]) else {
                e.direction_valid = false;
                continue;
            };
            e.direction = f32v(d);
            e.origin = f32v(self.eyes[i].origin_m);
            if st.invalid_run_end[i] > 0 {
                // just back from a blink: corrected, published, but not used for the combined ray yet
                st.invalid_run_end[i] -= 1;
                continue;
            }
            let o = self.eyes[i].origin_m;
            // soft reliability: the farther the reported origin from this wearer's usual one, the less the eye counts
            let w = self.eyes[i].weight() * self.eyes[i].org.reliability(raw_origin);
            hits.push((add(o, scale(d, self.combine.depth_m)), w, i));
        }
        let mid = scale(add(self.eyes[0].origin_m, self.eyes[1].origin_m), 0.5);
        if hits.len() == 2 {
            let da = unit(add(hits[0].0, scale(mid, -1.0)));
            let db = unit(add(hits[1].0, scale(mid, -1.0)));
            if let (Some(a), Some(b)) = (da, db) {
                if angle_deg(a, b) > self.combine.max_disagree_deg {
                    // keep the more trustworthy eye
                    let keep = if hits[0].1 >= hits[1].1 { 0 } else { 1 };
                    hits = vec![hits[keep]];
                }
            }
        }
        let wsum: f64 = hits.iter().map(|h| h.1).sum();
        s.gaze = if hits.is_empty() || wsum <= 0.0 {
            None
        } else {
            let mut p = [0.0; 3];
            for h in &hits {
                p = add(p, scale(h.0, h.1 / wsum));
            }
            unit(add(p, scale(mid, -1.0))).map(|d| Gaze {
                origin: f32v(mid),
                direction: f32v(d),
                eyes: hits.iter().fold(0u8, |m, h| m | (1 << h.2)),
            })
        };
    }
}

/// A raw eye ray of a sample as (origin m, direction) in f64 when valid.
pub fn raw_ray(e: &EyeRay) -> Option<([f64; 3], [f64; 3])> {
    (e.origin_valid && e.direction_valid).then(|| (f64v(e.origin), f64v(e.direction)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    /// A fake eye: true tangent -> raw tangent (what the uncalibrated chip reports).
    fn distort(t: (f64, f64), eye: usize) -> (f64, f64) {
        // gain 0.62, offsets per eye (kappa-like), upper field compressed (quadratic in v), slight cross talk
        let (u, v) = t;
        let off = if eye == 0 { (0.05, -0.17) } else { (-0.07, -0.16) };
        (
            off.0 + 0.62 * u + 0.03 * v,
            off.1 + 0.02 * u + 0.6 * v - 0.5 * v.max(0.0) * v,
        )
    }

    fn grid(az: &[f64], el: &[f64]) -> Vec<(f64, f64)> {
        let mut v = Vec::new();
        for &a in az {
            for &e in el {
                v.push((a.to_radians().tan(), e.to_radians().tan()));
            }
        }
        v
    }

    fn points(eye: usize, truths: &[(f64, f64)], noise: f64, rng: &mut Rng) -> Vec<EyePoint> {
        truths
            .iter()
            .map(|&t| {
                let r = distort(t, eye);
                EyePoint {
                    raw: (r.0 + noise * rng.gauss(), r.1 + noise * rng.gauss()),
                    truth: t,
                    weight: 1.0,
                }
            })
            .collect()
    }

    #[test]
    fn affine_recovers_gain_and_offset_exactly_without_noise() {
        let truths = grid(&[-15.0, 0.0, 15.0], &[-12.0, 0.0, 12.0]);
        // affine-only distortion
        let pts: Vec<EyePoint> = truths
            .iter()
            .map(|&t| EyePoint {
                raw: (0.05 + 0.6 * t.0, -0.17 + 0.62 * t.1 + 0.02 * t.0),
                truth: t,
                weight: 1.0,
            })
            .collect();
        let c = fit_eye(&pts, ModelKind::Affine, Ridge { linear: 0.0, quadratic: 0.0 }).unwrap();
        for p in &pts {
            assert!(point_error_deg(&c, p) < 1e-6, "{}", point_error_deg(&c, p));
        }
    }

    #[test]
    fn selection_beats_raw_and_validates_on_new_points() {
        let mut seed = Rng::new(0x1234_5678_9abc_def0);
        let cal_t = grid(&[-15.0, 0.0, 15.0], &[-12.0, 0.0, 12.0]);
        let val_t = grid(&[-10.0, -4.0, 4.0, 10.0], &[-8.0, 3.0, 9.0]);
        for eye in 0..2 {
            // 0.15 deg of noise on each target centre (the median of ~100 samples)
            let pts = points(eye, &cal_t, 0.15f64.to_radians(), &mut seed);
            let sel = select_model(&pts);
            let best = sel.best().expect("a model");
            assert!(sel.raw.mean > 8.0, "raw error {:?}", sel.raw);
            let val: Vec<f64> = val_t
                .iter()
                .map(|&t| {
                    let r = distort(t, eye);
                    tangent_angle_deg(best.map.map_tangent(r.0, r.1), t)
                })
                .collect();
            let v = stats(&val);
            assert!(v.mean < 0.8, "eye {eye}: {} validation {v:?} loo {:?}", best.kind.name(), best.loo);
            assert!(best.kind != ModelKind::Offset, "gain 0.6 needs more than an offset");
        }
    }

    #[test]
    fn one_bad_target_does_not_drag_the_fit() {
        let mut seed = Rng::new(42);
        let cal_t = grid(&[-15.0, 0.0, 15.0], &[-12.0, 0.0, 12.0]);
        let mut pts = points(0, &cal_t, 0.1f64.to_radians(), &mut seed);
        let clean = fit_eye(&pts, ModelKind::Affine, Ridge::default()).unwrap();
        // the wearer looked 6 degrees away at one target
        pts[4].raw.0 += 6f64.to_radians().tan() * 0.62;
        let robust = fit_eye(&pts, ModelKind::Affine, Ridge::default()).unwrap();
        let mut w = vec![1.0; pts.len()];
        w[4] = 1.0;
        let plain = fit_weighted(&pts, &w, ModelKind::Affine, Ridge::default()).unwrap();
        let err = |c: &Correction| {
            pts.iter()
                .enumerate()
                .filter(|(i, _)| *i != 4)
                .map(|(_, p)| point_error_deg(c, p))
                .sum::<f64>()
                / 8.0
        };
        assert!(err(&robust) < err(&plain) * 0.7, "robust {} plain {}", err(&robust), err(&plain));
        assert!(err(&robust) < err(&clean) + 0.6);
    }

    #[test]
    fn quadratic_wins_when_the_upper_field_is_compressed_and_points_allow_it() {
        let mut seed = Rng::new(7);
        let cal_t = grid(&[-15.0, -7.5, 0.0, 7.5, 15.0], &[-12.0, 0.0, 12.0]);
        let pts = points(1, &cal_t, 0.05f64.to_radians(), &mut seed);
        let sel = select_model(&pts);
        assert_eq!(sel.best().unwrap().kind, ModelKind::Quadratic, "{sel:?}");
        let aff = sel.candidates.iter().find(|m| m.kind == ModelKind::Affine).unwrap();
        assert!(sel.best().unwrap().loo.mean < aff.loo.mean);
    }

    #[test]
    fn toml_round_trip() {
        let mut c = Calibration::identity(
            FrameConv {
                flip_x: true,
                ..FrameConv::default()
            },
            0.0655,
        );
        c.eyes[0].map.cx = [0.01, 1.2, 0.03, 0.0, 0.1, -0.2];
        c.eyes[0].map.model = "quadratic".into();
        c.eyes[1].map.cy = [-0.2, 0.01, 1.6, 0.0, 0.0, 0.0];
        c.eyes[1].drift = (0.001, -0.002);
        c.eyes[1].accuracy_deg = 0.8;
        c.filter.enabled = false;
        c.combine.depth_m = 2.0;
        c.created_unix = 1_791_956_330;
        c.notes = vec!["validation mean 1.0 deg".into()];
        let t = c.to_toml();
        assert!(t.contains("# validation mean 1.0 deg"));
        let back = Calibration::parse(&t).unwrap();
        assert_eq!(back.conv, c.conv);
        assert_eq!(back.created_unix, c.created_unix);
        assert!(!back.filter.enabled);
        for i in 0..2 {
            for k in 0..6 {
                assert!((back.eyes[i].map.cx[k] - c.eyes[i].map.cx[k]).abs() < 1e-9);
                assert!((back.eyes[i].map.cy[k] - c.eyes[i].map.cy[k]).abs() < 1e-9);
            }
            assert_eq!(back.eyes[i].map.model, c.eyes[i].map.model);
        }
        assert!((back.eyes[1].drift.1 + 0.002).abs() < 1e-12);
        assert!((back.combine.depth_m - 2.0).abs() < 1e-12);
        assert!(Calibration::parse("version = 1\n[left]\nmodel = \"affine\"\n").is_err());
        assert!(Calibration::parse("version = 2\n[left]\n[right]\n").is_err());
        assert!(Calibration::parse("[left]\nmodel = \"cubic\"\n[right]\n").is_err());
    }

    fn sample(l: Option<[f64; 3]>, r: Option<[f64; 3]>) -> Sample {
        let mut s = Sample::default();
        for (i, d) in [l, r].into_iter().enumerate() {
            if let Some(d) = d {
                s.eyes[i] = EyeRay {
                    origin: [if i == 0 { -0.03 } else { 0.03 }, 0.0, 0.02],
                    direction: f32v(d),
                    origin_valid: true,
                    direction_valid: true,
                    pupil_mm: 3.0,
                    ..EyeRay::default()
                };
            }
        }
        s
    }

    #[test]
    fn combination_is_vergence_consistent_and_survives_one_eye() {
        let cal = Calibration::identity(FrameConv::default(), 0.064);
        let target = [0.3, 0.1, -1.5];
        let dir_from = |o: [f64; 3]| unit(add(target, scale(o, -1.0))).unwrap();
        let (l, r) = (dir_from(cal.eyes[0].origin_m), dir_from(cal.eyes[1].origin_m));
        let want = unit(target).unwrap();
        let mut st = CombineState::default();
        let mut s = sample(Some(l), Some(r));
        cal.apply_sample(&mut s, &mut st);
        let g = s.gaze.unwrap();
        assert_eq!(g.eyes, 3);
        // the 1.5 m fixation depth vs the 1.53 m target: a few hundredths of a degree
        assert!(angle_deg(f64v(g.direction), want) < 0.05);
        // the right eye blinks: the left eye alone gives the same direction (no half-vergence jump)
        let mut s = sample(Some(l), None);
        cal.apply_sample(&mut s, &mut st);
        let g = s.gaze.unwrap();
        assert_eq!(g.eyes, 1);
        assert!(angle_deg(f64v(g.direction), want) < 0.05);
        // back from the blink: the right eye is held off for 2 samples, then used again
        for k in 0..3 {
            let mut s = sample(Some(l), Some(r));
            cal.apply_sample(&mut s, &mut st);
            let g = s.gaze.unwrap();
            assert_eq!(g.eyes, if k < 2 { 1 } else { 3 }, "sample {k}");
            assert!(s.eyes[1].direction_valid, "the eye itself is still published");
        }
    }

    #[test]
    fn disagreeing_eyes_fall_back_to_the_better_one() {
        let mut cal = Calibration::identity(FrameConv::default(), 0.064);
        cal.eyes[0].accuracy_deg = 0.5;
        cal.eyes[1].accuracy_deg = 2.0;
        let mut st = CombineState::default();
        let good = [0.0, 0.0, -1.0];
        let bad = dir_of(0.2, 0.0); // 11 degrees off
        let mut s = sample(Some(good), Some(bad));
        cal.apply_sample(&mut s, &mut st);
        let g = s.gaze.unwrap();
        assert_eq!(g.eyes, 1, "left eye only");
        assert!(angle_deg(f64v(g.direction), good) < 1.5);
        let mut s = sample(None, None);
        cal.apply_sample(&mut s, &mut st);
        assert!(s.gaze.is_none());
    }

    #[test]
    fn select_follows_the_variable() {
        let dir = std::env::temp_dir().join(format!("pimax-cal-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join(".config/pimax")).unwrap();
        let home = Some(dir.clone());
        assert!(Calibration::select(None, home.clone()).unwrap().is_none());
        let c = Calibration::identity(FrameConv::default(), 0.064);
        std::fs::write(dir.join(".config/pimax/eye-calibration.toml"), c.to_toml()).unwrap();
        assert!(Calibration::select(None, home.clone()).unwrap().is_some());
        assert!(Calibration::select(Some("none"), home.clone()).unwrap().is_none());
        assert!(Calibration::select(Some("/nonexistent.toml"), home).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn origin_terms_round_trip_shift_and_gate() {
        let mut c = Calibration::identity(FrameConv::default(), 0.064);
        c.eyes[1].org = OriginTerms {
            reference_m: [0.026, 0.003, 0.02],
            u: [0.1, 0.0, -0.05],
            v: [0.0, 0.12, 0.0],
            gate_mm: 5.0,
            pupil_ref_mm: 3.3,
            pupil_u: -0.02,
            pupil_v: 0.01,
        };
        let t = c.to_toml();
        assert!(t.contains("version = 2") && t.contains("origin_gate_mm = 5.000"));
        let back = Calibration::parse(&t).unwrap();
        assert_eq!(back.eyes[1].org.gate_mm, 5.0);
        assert!((back.eyes[1].org.u[2] + 0.05).abs() < 1e-9 && (back.eyes[1].org.pupil_u + 0.02).abs() < 1e-9);
        assert!(!back.eyes[0].org.is_active());
        // a file without origin terms stays version 1
        assert!(Calibration::identity(FrameConv::default(), 0.064).to_toml().contains("version = 1"));
        // 1 mm of origin error in x moves u by 0.1
        let e = &back.eyes[1];
        let d = dir_of(0.0, 0.0);
        let o = [0.027, 0.003, 0.02];
        let (u, v) = tangent_of(e.apply_full(d, Some(o), Some(3.3), (0.0, 0.0)).unwrap()).unwrap();
        assert!((u - 0.1).abs() < 1e-6 && v.abs() < 1e-6, "{u} {v}");
        // the pupil term is clamped
        let (u2, _) = tangent_of(e.apply_full(d, Some(o), Some(9.3), (0.0, 0.0)).unwrap()).unwrap();
        assert!((u2 - (0.1 - 0.02 * PUPIL_CLAMP_MM)).abs() < 1e-6, "{u2}");
        // far from the reference: the eye is dropped, the other one carries the gaze alone
        let mut s = sample(Some([0.0, 0.0, -1.0]), Some([0.0, 0.0, -1.0]));
        s.eyes[1].origin = [0.04, 0.0, 0.02];
        let mut st = CombineState::default();
        back.apply_sample(&mut s, &mut st);
        assert!(!s.eyes[1].direction_valid);
        assert_eq!(s.gaze.map(|g| g.eyes), Some(1));
    }
}
