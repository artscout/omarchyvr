//! Which calibration error explains the vertical disparities of a camera pair?
//!
//! In a rectified pair (x along the baseline, focal length `f`) a small **rotation** `w = (wx, wy, wz)` of
//! camera b (rectified axes) and a small error `(ty, tz)` of the **baseline direction** (true direction
//! `(1, ty, tz)`) give, to first order, the vertical disparity
//!
//! ```text
//! dy = f [ -wx (1 + y^2) + wy x y + wz x ]  +  d [ -ty + tz y ]
//! ```
//!
//! with `(x, y)` the match's normalised position in image b and `d` its disparity in pixels. The rotation part
//! does not depend on depth; the translation part is proportional to the disparity, so it vanishes for far
//! points. That is how the two are told apart, and why a scene with a single depth cannot separate `wx` from
//! `ty`. [`fit`] estimates the parameters of one model robustly (least median of squares, then least squares on
//! the inliers); [`compare`] runs the rotation-only, translation-only and combined models.
//!
//! This is a diagnosis next to the bundle adjustment ([`crate::ba`]), which estimates rotations only: if the
//! translation terms are significant here, a rotation-only overlay is not the whole story for that pair.

use crate::ba::solve_dense;
use crate::matcher::StereoMatch;
use crate::rect::RectGeom;

/// One match as the fit sees it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Sample {
    /// Normalised x in image b (`(u - cx) / f`).
    pub x: f64,
    /// Normalised y (mean of both images).
    pub y: f64,
    /// Disparity (pixels).
    pub disp: f64,
    /// Vertical disparity (pixels).
    pub dy: f64,
}

/// Samples of a set of matches under rectification `g`.
pub fn samples(g: &RectGeom, matches: &[StereoMatch]) -> Vec<Sample> {
    let (cx, cy) = g.centre();
    matches
        .iter()
        .map(|m| Sample {
            x: (m.x - m.disp - cx) / g.focal,
            y: (m.y + 0.5 * m.dy - cy) / g.focal,
            disp: m.disp,
            dy: m.dy,
        })
        .collect()
}

/// Error model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Model {
    /// No correction (the statistics of `dy` itself).
    None,
    /// Rotation of camera b: `wx, wy, wz`.
    Rotation,
    /// Baseline direction: `ty, tz`.
    Translation,
    /// Rotation and baseline direction.
    Both,
}

impl Model {
    /// Indices into `[wx, wy, wz, ty, tz]` that the model estimates.
    pub fn columns(self) -> &'static [usize] {
        match self {
            Model::None => &[],
            Model::Rotation => &[0, 1, 2],
            Model::Translation => &[3, 4],
            Model::Both => &[0, 1, 2, 3, 4],
        }
    }

    /// Short name.
    pub fn name(self) -> &'static str {
        match self {
            Model::None => "none",
            Model::Rotation => "rotation",
            Model::Translation => "translation",
            Model::Both => "rotation+translation",
        }
    }
}

/// Result of [`fit`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fit {
    /// The model.
    pub model: Model,
    /// `[wx, wy, wz, ty, tz]`: radians (rectified axes) and baseline-direction components; zero where the model
    /// does not estimate.
    pub params: [f64; 5],
    /// Standard error of each estimated parameter (`NaN` where not estimated or not determined).
    pub stderr: [f64; 5],
    /// Samples.
    pub n: usize,
    /// Samples within 2.5 robust sigma of the model.
    pub inliers: usize,
    /// Robust residual scale over all samples (1.4826 x median absolute residual), pixels.
    pub sigma: f64,
    /// Root mean square residual of the inliers (pixels).
    pub rms_inliers: f64,
    /// Fraction of all samples with a residual of at most 1 px.
    pub within_1: f64,
}

/// Design row `[d dy / d wx, wy, wz, ty, tz]` of a sample.
pub fn row(s: &Sample, f: f64) -> [f64; 5] {
    [
        -f * (1.0 + s.y * s.y),
        f * s.x * s.y,
        f * s.x,
        -s.disp,
        s.disp * s.y,
    ]
}

/// Model prediction of `dy` for a sample.
pub fn predict(params: &[f64; 5], s: &Sample, f: f64) -> f64 {
    row(s, f).iter().zip(params).map(|(a, b)| a * b).sum()
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

/// Least squares over the samples in `idx` for the model's columns. `None` if singular.
fn least_squares(
    s: &[Sample],
    idx: &[usize],
    cols: &[usize],
    f: f64,
) -> Option<([f64; 5], Vec<Vec<f64>>)> {
    let p = cols.len();
    let mut a = vec![vec![0.0; p]; p];
    let mut b = vec![0.0; p];
    for &i in idx {
        let r = row(&s[i], f);
        for (j, &cj) in cols.iter().enumerate() {
            b[j] += r[cj] * s[i].dy;
            for (k, &ck) in cols.iter().enumerate() {
                a[j][k] += r[cj] * r[ck];
            }
        }
    }
    // scale-aware singularity test: a column that is (numerically) a combination of the others
    for j in 0..p {
        if a[j][j] <= 0.0 {
            return None;
        }
    }
    let x = solve_dense(&a, &b)?;
    if x.iter().any(|v| !v.is_finite()) {
        return None;
    }
    let mut out = [0.0; 5];
    for (j, &cj) in cols.iter().enumerate() {
        out[cj] = x[j];
    }
    Some((out, a))
}

fn residuals(s: &[Sample], params: &[f64; 5], f: f64) -> Vec<f64> {
    s.iter().map(|x| x.dy - predict(params, x, f)).collect()
}

/// Robust scale of residuals: 1.4826 x the median absolute value.
fn robust_sigma(r: &[f64]) -> f64 {
    let mut a: Vec<f64> = r.iter().map(|x| x.abs()).collect();
    1.4826 * median(&mut a)
}

/// Fit `model` to the samples (`f` = rectified focal length in pixels). `None` with fewer than `parameters + 3`
/// samples or a degenerate configuration.
pub fn fit(s: &[Sample], f: f64, model: Model) -> Option<Fit> {
    let cols = model.columns();
    let p = cols.len();
    let n = s.len();
    if n < p + 3 {
        return None;
    }
    let all: Vec<usize> = (0..n).collect();
    // least median of squares over random minimal subsets (deterministic generator)
    let mut best: Option<([f64; 5], f64)> = None;
    if p == 0 {
        best = Some(([0.0; 5], robust_sigma(&residuals(s, &[0.0; 5], f))));
    } else {
        let mut seed = 0x9e37_79b9_7f4a_7c15u64 ^ (n as u64).wrapping_mul(0x2545_f491_4f6c_dd1d);
        let mut rnd = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let consider = |params: [f64; 5], best: &mut Option<([f64; 5], f64)>| {
            let sg = robust_sigma(&residuals(s, &params, f));
            if sg.is_finite() && best.is_none_or(|b| sg < b.1) {
                *best = Some((params, sg));
            }
        };
        if let Some((params, _)) = least_squares(s, &all, cols, f) {
            consider(params, &mut best);
        }
        let tries = if n <= p + 3 { 50 } else { 1500 };
        let mut sub = vec![0usize; p];
        for _ in 0..tries {
            for k in 0..p {
                loop {
                    let c = (rnd() % n as u64) as usize;
                    if !sub[..k].contains(&c) {
                        sub[k] = c;
                        break;
                    }
                }
            }
            if let Some((params, _)) = least_squares(s, &sub, cols, f) {
                // reject wild minimal solutions (more than ~17 deg / a baseline tilted by 45 deg)
                if params[..3].iter().all(|w| w.abs() < 0.3)
                    && params[3..].iter().all(|t| t.abs() < 1.0)
                {
                    consider(params, &mut best);
                }
            }
        }
    }
    let (mut params, mut sigma) = best?;
    // refit on the inliers (two passes), with a floor on the scale so that a perfect subset keeps its neighbours
    let inliers_of = |params: &[f64; 5], sigma: f64| -> Vec<usize> {
        let r = residuals(s, params, f);
        let thr = 2.5 * sigma.max(0.3);
        (0..n).filter(|&i| r[i].abs() <= thr).collect()
    };
    let mut normal = None;
    for _ in 0..2 {
        let inl = inliers_of(&params, sigma);
        if p == 0 || inl.len() < p + 1 {
            break;
        }
        match least_squares(s, &inl, cols, f) {
            Some((pp, a)) => {
                params = pp;
                normal = Some(a);
                sigma = robust_sigma(&residuals(s, &params, f));
            }
            None => break,
        }
    }
    let r = residuals(s, &params, f);
    let inl_final = inliers_of(&params, sigma);
    let ss: f64 = inl_final.iter().map(|&i| r[i] * r[i]).sum();
    let rms_inliers = if inl_final.is_empty() {
        f64::NAN
    } else {
        (ss / inl_final.len() as f64).sqrt()
    };
    let mut stderr = [f64::NAN; 5];
    if let Some(a) = normal {
        let dof = inl_final.len().saturating_sub(p).max(1) as f64;
        let s2 = ss / dof;
        for (j, &cj) in cols.iter().enumerate() {
            let mut e = vec![0.0; p];
            e[j] = 1.0;
            if let Some(x) = solve_dense(&a, &e) {
                if x[j] > 0.0 {
                    stderr[cj] = (x[j] * s2).sqrt();
                }
            }
        }
    }
    Some(Fit {
        model,
        params,
        stderr,
        n,
        inliers: inl_final.len(),
        sigma,
        rms_inliers,
        within_1: r.iter().filter(|x| x.abs() <= 1.0).count() as f64 / n as f64,
    })
}

/// The four models on one sample set (those that could be fitted).
pub fn compare(s: &[Sample], f: f64) -> Vec<Fit> {
    [
        Model::None,
        Model::Rotation,
        Model::Translation,
        Model::Both,
    ]
    .into_iter()
    .filter_map(|m| fit(s, f, m))
    .collect()
}

/// What [`compare`] says about a pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Too few matches to say anything.
    TooFew,
    /// The vertical disparities are already at the noise level.
    Fine,
    /// A rotation explains them (no credible baseline-direction signal).
    Rotation,
    /// A baseline-direction error of believable size is significant on top of the rotation.
    RotationAndTranslation,
    /// The combined model fits clearly better than the rotation, but only with a baseline error too large to be
    /// real: the matches do not cover enough depths and image positions to tell the two apart.
    Ambiguous,
    /// No model brings the residuals down: mismatches, or an error of another kind (intrinsics, lens model).
    Unexplained,
}

impl Verdict {
    /// One line for reports.
    pub fn text(self) -> &'static str {
        match self {
            Verdict::TooFew => "too few matches",
            Verdict::Fine => "already at the noise level",
            Verdict::Rotation => "a rotation explains it",
            Verdict::RotationAndTranslation => "a rotation plus a baseline-direction error",
            Verdict::Ambiguous => {
                "rotation and baseline direction cannot be told apart (matches at too few depths / positions)"
            }
            Verdict::Unexplained => "not explained by a rotation or a baseline error (mismatches, or intrinsics)",
        }
    }

    /// Whether the rotation-only bundle adjustment may use this pair.
    pub fn usable(self) -> bool {
        !matches!(self, Verdict::TooFew | Verdict::Unexplained)
    }
}

/// Smallest number of matches for a verdict.
pub const MIN_SAMPLES: usize = 25;
/// Largest believable baseline-direction error (the headset is rigid: 0.06 is 2.4 mm on the 40 mm pairs and
/// 6.7 mm on the 112 mm pair).
pub const MAX_BASELINE_DIR: f64 = 0.06;

/// Classify a pair from its model fits (`noise_px`: the matcher's noise level, about 0.5 px on real frames).
pub fn verdict(fits: &[Fit], noise_px: f64) -> Verdict {
    let get = |m: Model| fits.iter().find(|f| f.model == m);
    let (Some(none), Some(rot)) = (get(Model::None), get(Model::Rotation)) else {
        return Verdict::TooFew;
    };
    if none.n < MIN_SAMPLES {
        return Verdict::TooFew;
    }
    if none.sigma <= 1.2 * noise_px && rot.sigma > 0.85 * none.sigma {
        return Verdict::Fine;
    }
    let both = get(Model::Both);
    let best = both.map_or(rot.sigma, |b| b.sigma.min(rot.sigma));
    // nothing helps: the best model still leaves most of the scatter, well above the noise
    if best > 2.4 * noise_px.max(0.25) && best > 0.6 * none.sigma {
        return Verdict::Unexplained;
    }
    let Some(b) = both.filter(|b| b.sigma < 0.8 * rot.sigma) else {
        return Verdict::Rotation;
    };
    let z = |k: usize| (b.params[k] / b.stderr[k]).abs();
    if !(z(3) > 4.0 || z(4) > 4.0) {
        return Verdict::Rotation;
    }
    if b.params[3].abs() > MAX_BASELINE_DIR || b.params[4].abs() > MAX_BASELINE_DIR {
        Verdict::Ambiguous
    } else {
        Verdict::RotationAndTranslation
    }
}

/// Fraction of samples whose `|dy|` is within 20 % of the search limit `band` (pixels): when it is large, the
/// matcher's vertical search range cut the distribution and the true error is bigger than it looks.
pub fn band_saturation(s: &[Sample], band: f64) -> f64 {
    if s.is_empty() {
        return 0.0;
    }
    s.iter().filter(|x| x.dy.abs() > 0.8 * band).count() as f64 / s.len() as f64
}

/// Table of the fits, one line per model.
pub fn table(fits: &[Fit]) -> String {
    use std::fmt::Write;
    let mut o = String::new();
    for f in fits {
        let d = |k: usize| f.params[k].to_degrees();
        let _ = write!(
            o,
            "    {:21} sigma {:5.2} px, {:3}/{:3} inliers (rms {:.2}), <=1 px {:3.0}%",
            f.model.name(),
            f.sigma,
            f.inliers,
            f.n,
            f.rms_inliers,
            f.within_1 * 100.0
        );
        if f.model == Model::Rotation || f.model == Model::Both {
            let _ = write!(
                o,
                "  w = ({:+.3}, {:+.3}, {:+.3}) deg +- ({:.3}, {:.3}, {:.3})",
                d(0),
                d(1),
                d(2),
                f.stderr[0].to_degrees(),
                f.stderr[1].to_degrees(),
                f.stderr[2].to_degrees()
            );
        }
        if f.model == Model::Translation || f.model == Model::Both {
            let _ = write!(
                o,
                "  t = ({:+.4}, {:+.4}) +- ({:.4}, {:.4})",
                f.params[3], f.params[4], f.stderr[3], f.stderr[4]
            );
        }
        let _ = writeln!(o);
    }
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rnd(seed: &mut u64) -> f64 {
        *seed = seed
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((*seed >> 11) as f64) / ((1u64 << 53) as f64) - 0.5
    }

    /// Samples spread over the image with depths between `near` and `far` (metres, 0.1 m baseline, f = 220).
    fn synth(
        truth: &[f64; 5],
        n: usize,
        near: f64,
        far: f64,
        noise: f64,
        outliers: usize,
    ) -> Vec<Sample> {
        let f = 220.0;
        let mut seed = 77u64;
        (0..n)
            .map(|i| {
                let x = 2.2 * rnd(&mut seed);
                let y = 1.8 * rnd(&mut seed);
                let z = near + (far - near) * (rnd(&mut seed) + 0.5);
                let mut s = Sample {
                    x,
                    y,
                    disp: f * 0.1 / z,
                    dy: 0.0,
                };
                s.dy = predict(truth, &s, f) + noise * 2.0 * rnd(&mut seed);
                if i < outliers {
                    s.dy += 6.0 + 10.0 * rnd(&mut seed).abs();
                }
                s
            })
            .collect()
    }

    #[test]
    fn rotation_is_recovered_and_recognised() {
        let truth = [0.004, -0.006, 0.003, 0.0, 0.0];
        let s = synth(&truth, 200, 0.4, 4.0, 0.3, 30);
        let fits = compare(&s, 220.0);
        let rot = fits.iter().find(|f| f.model == Model::Rotation).unwrap();
        for k in 0..3 {
            assert!(
                (rot.params[k] - truth[k]).abs() < 4e-4,
                "w[{k}] {} vs {}",
                rot.params[k],
                truth[k]
            );
            assert!(rot.stderr[k] > 0.0 && rot.stderr[k] < 4e-4);
        }
        assert!(rot.sigma < 0.4, "{rot:?}");
        assert!(rot.inliers >= 160 && rot.inliers <= 175, "{rot:?}");
        assert_eq!(verdict(&fits, 0.3), Verdict::Rotation);
        assert!(table(&fits).contains("rotation+translation"));
    }

    #[test]
    fn baseline_direction_needs_depth_variety() {
        // near and far points: ty is told apart from wx
        let truth = [0.002, 0.0, -0.001, 0.02, -0.03];
        let s = synth(&truth, 300, 0.25, 5.0, 0.2, 0);
        let fits = compare(&s, 220.0);
        let both = fits.iter().find(|f| f.model == Model::Both).unwrap();
        for k in 0..5 {
            let tol = if k < 3 { 3e-4 } else { 3e-3 };
            assert!(
                (both.params[k] - truth[k]).abs() < tol,
                "p[{k}] {}",
                both.params[k]
            );
        }
        assert_eq!(verdict(&fits, 0.2), Verdict::RotationAndTranslation);
        // one depth only: wx and ty are the same column up to the y^2 term, so their errors grow several times
        let flat = synth(&truth, 300, 2.0, 2.0, 0.2, 0);
        let b2 = fit(&flat, 220.0, Model::Both).unwrap();
        assert!(
            b2.stderr[3] > 4.0 * both.stderr[3],
            "{} vs {}",
            b2.stderr[3],
            both.stderr[3]
        );
        assert!(
            b2.stderr[0] > 4.0 * both.stderr[0],
            "{} vs {}",
            b2.stderr[0],
            both.stderr[0]
        );
    }

    /// The model against real geometry: a rig whose camera b is rotated and displaced relative to the
    /// calibration used to rectify. Checks the signs and frames of the formula in the module docs.
    #[test]
    fn model_matches_the_rectified_geometry() {
        use pimax_cam::geom::{self, Pose};
        let assumed = Pose {
            r: geom::rodrigues([0.05, -0.7, 0.1]),
            t: [-0.1, 0.006, -0.04],
        };
        let g = RectGeom::new(&assumed, 640, 480, 220.0);
        let w_b = [0.004, -0.003, 0.005]; // camera-b axes
        let shift = [0.0015, -0.002, 0.001]; // metres, camera-a frame
        let c_b = geom::add(assumed.inverse().t, shift);
        let e = geom::rodrigues(w_b);
        let mut seed = 3u64;
        let mut ms = Vec::new();
        while ms.len() < 400 {
            // points in the rectified frame, 0.3 .. 4 m
            let z = 0.3 + 3.7 * (rnd(&mut seed) + 0.5);
            let pr = [2.0 * rnd(&mut seed) * z, 1.6 * rnd(&mut seed) * z, z];
            let pa = geom::mat_vec(&g.a_from_rect, pr);
            let xb = geom::mat_vec(&e, geom::mat_vec(&assumed.r, geom::sub(pa, c_b)));
            let (Some((xa, ya)), Some((xbp, ybp))) = (g.pixel(0, pa), g.pixel(1, xb)) else {
                continue;
            };
            ms.push(StereoMatch {
                x: xa,
                y: ya,
                disp: xa - xbp,
                dy: ybp - ya,
                zncc: 1.0,
                ray_a: geom::normalize(pa),
                ray_b: geom::normalize(xb),
            });
        }
        let s = samples(&g, &ms);
        let both = fit(&s, g.focal, Model::Both).unwrap();
        let w_r = geom::mat_vec(&geom::transpose(&g.b_from_rect), w_b);
        let base = geom::mat_vec(&geom::transpose(&g.a_from_rect), c_b);
        let want = [w_r[0], w_r[1], w_r[2], base[1] / base[0], base[2] / base[0]];
        for k in 0..5 {
            // first-order model: second-order terms of a 0.4 deg rotation are ~1e-5 rad
            let tol = if k < 3 { 1.5e-4 } else { 2e-3 };
            assert!(
                (both.params[k] - want[k]).abs() < tol,
                "p[{k}] {} vs {} ({both:?})",
                both.params[k],
                want[k]
            );
        }
        assert!(both.sigma < 0.05, "{both:?}");
    }

    #[test]
    fn good_and_hopeless_pairs() {
        let s = synth(&[0.0; 5], 150, 0.5, 4.0, 0.3, 0);
        assert_eq!(verdict(&compare(&s, 220.0), 0.3), Verdict::Fine);
        // dy unrelated to any model
        let mut seed = 5u64;
        let junk: Vec<Sample> = (0..80)
            .map(|_| Sample {
                x: rnd(&mut seed),
                y: rnd(&mut seed),
                disp: 20.0 + 30.0 * rnd(&mut seed),
                dy: 20.0 * rnd(&mut seed),
            })
            .collect();
        assert_eq!(verdict(&compare(&junk, 220.0), 0.3), Verdict::Unexplained);
        assert_eq!(verdict(&compare(&s[..10], 220.0), 0.3), Verdict::TooFew);
        assert!(fit(&s[..4], 220.0, Model::Both).is_none());
        assert!(
            Verdict::Rotation.usable()
                && Verdict::Ambiguous.usable()
                && !Verdict::Unexplained.usable()
        );
        assert_eq!(
            band_saturation(&junk, 8.0),
            junk.iter().filter(|x| x.dy.abs() > 6.4).count() as f64 / 80.0
        );
        assert_eq!(band_saturation(&[], 8.0), 0.0);
        // a baseline error far too large to be real is reported as "cannot tell", not as a finding
        let big = synth(&[0.001, 0.0, 0.0, 0.15, 0.2], 300, 0.25, 5.0, 0.2, 0);
        assert_eq!(verdict(&compare(&big, 220.0), 0.2), Verdict::Ambiguous);
    }
}
