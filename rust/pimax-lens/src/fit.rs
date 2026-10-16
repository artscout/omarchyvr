//! Least-squares fit of the radial polynomial to an angle -> panel radius table, with an optional continuation
//! out to the panel corners.
//!
//! Model: `tan(angle) = base * r * (1 + k1 r^2 + k2 r^4 + ...)`, with `r` the panel radius in half-display-heights.
//! It is linear in `a0 = base`, `a_i = base * k_i`, so the fit is a weighted linear least squares problem. The
//! weights `cos^4(angle)` turn the error of `tan` into an error in angle (`d angle = cos^2(angle) d tan`).

use crate::config::K_TERMS;
use crate::model::radial_tangent;

/// "Dream Air like Pimax" table, green channel: (angle in degrees, radius in half-display-heights; the source table
/// is in percent of half the smaller panel dimension, 1776 px here). Source: `analysis/customheadsetopenvr.md`
/// section 2a (sboys3/CustomHeadsetOpenVR, fitted to Pimax's own mesh within 0.1 deg). Used as a fit reference and
/// in tests only.
pub const DREAM_AIR_LIKE_PIMAX_GREEN: [(f64, f64); 15] = [
    (0.0, 0.0),
    (2.5, 0.06420),
    (5.0, 0.12869),
    (10.0, 0.25641),
    (15.0, 0.38185),
    (20.0, 0.50377),
    (25.0, 0.62062),
    (30.0, 0.73145),
    (35.0, 0.83496),
    (40.0, 0.93232),
    (45.0, 1.02044),
    (47.5, 1.05735),
    (50.0, 1.08985),
    (52.5, 1.11722),
    (55.0, 1.14836),
];

/// Result of a fit.
#[derive(Clone, Debug, PartialEq)]
pub struct Fit {
    pub base: f64,
    pub k: [f64; K_TERMS],
    /// Largest and rms angle error in degrees over the fitted samples (including continuation samples).
    pub max_err_deg: f64,
    pub rms_err_deg: f64,
}

impl Fit {
    pub fn k_f32(&self) -> [f32; K_TERMS] {
        let mut k = [0.0f32; K_TERMS];
        for (d, s) in k.iter_mut().zip(self.k.iter()) {
            *d = *s as f32;
        }
        k
    }

    /// Angle in degrees the fitted model shows at panel radius `r`.
    pub fn angle_deg(&self, r: f64) -> f64 {
        radial_tangent(self.base, 1.0, &self.k_f32(), r)
            .0
            .atan()
            .to_degrees()
    }
}

/// Solve `A x = b` (n x n, row major) by Gaussian elimination with partial pivoting.
pub(crate) fn solve(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = b.len();
    for col in 0..n {
        let piv = (col..n).max_by(|&i, &j| a[i][col].abs().total_cmp(&a[j][col].abs()))?;
        if a[piv][col].abs() < 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        let pivot_row = a[col].clone();
        for row in col + 1..n {
            let f = a[row][col] / pivot_row[col];
            for (x, p) in a[row][col..].iter_mut().zip(&pivot_row[col..]) {
                *x -= f * p;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = vec![0.0; n];
    for row in (0..n).rev() {
        let mut s = b[row];
        for c in row + 1..n {
            s -= a[row][c] * x[c];
        }
        x[row] = s / a[row][row];
    }
    Some(x)
}

/// Fit `base` and `k_terms` polynomial coefficients (1..=5) to `(angle_deg, radius)` samples.
pub fn fit(samples: &[(f64, f64)], k_terms: usize) -> Option<Fit> {
    let w: Vec<(f64, f64, f64)> = samples.iter().map(|&(a, r)| (a, r, 1.0)).collect();
    fit_weighted(&w, k_terms)
}

/// Like [`fit`] with an extra weight per sample `(angle_deg, radius, weight)`.
pub fn fit_weighted(samples: &[(f64, f64, f64)], k_terms: usize) -> Option<Fit> {
    if k_terms == 0 || k_terms > K_TERMS || samples.len() < k_terms + 1 {
        return None;
    }
    let n = k_terms + 1;
    let mut ata = vec![vec![0.0; n]; n];
    let mut atb = vec![0.0; n];
    for &(deg, r, extra) in samples {
        let th = deg.to_radians();
        let y = th.tan();
        let w = th.cos().powi(4) * extra;
        let phi: Vec<f64> = (0..n).map(|i| r.powi(2 * i as i32 + 1)).collect();
        for i in 0..n {
            for j in 0..n {
                ata[i][j] += w * phi[i] * phi[j];
            }
            atb[i] += w * phi[i] * y;
        }
    }
    let a = solve(ata, atb)?;
    let base = a[0];
    let mut k = [0.0; K_TERMS];
    for i in 1..n {
        k[i - 1] = a[i] / base;
    }
    let mut res = Fit {
        base,
        k,
        max_err_deg: 0.0,
        rms_err_deg: 0.0,
    };
    let (mut mx, mut sq) = (0.0f64, 0.0f64);
    for &(deg, r, _) in samples {
        let e = (res.angle_deg(r) - deg).abs();
        mx = mx.max(e);
        sq += e * e;
    }
    res.max_err_deg = mx;
    res.rms_err_deg = (sq / samples.len() as f64).sqrt();
    Some(res)
}

/// Continue a table beyond its last radius up to `r_end` (spacing `step`) with `tan(angle)` linear in `r`, using the
/// mean slope of the last `window` segments. This is an assumption, not a measurement: it only keeps the fitted
/// polynomial smooth and monotonic out to the panel corners. Returns the extra samples (angle, radius).
pub fn continuation(
    samples: &[(f64, f64)],
    r_end: f64,
    step: f64,
    window: usize,
) -> Vec<(f64, f64)> {
    let n = samples.len();
    if n < 2 || window == 0 {
        return Vec::new();
    }
    let w = window.min(n - 1);
    let (a0, r0) = samples[n - 1 - w];
    let (a1, r1) = samples[n - 1];
    let slope = (a1.to_radians().tan() - a0.to_radians().tan()) / (r1 - r0);
    let t1 = a1.to_radians().tan();
    let mut out = Vec::new();
    let mut r = r1 + step;
    while r <= r_end + 1e-9 {
        out.push(((t1 + slope * (r - r1)).atan().to_degrees(), r));
        r += step;
    }
    out
}

/// Format a fit as a `lens.conf` fragment.
pub fn conf_fragment(f: &Fit, fit_range: f64) -> String {
    let mut s = format!("base = {:.5}\nk = ", f.base);
    let used =
        f.k.iter()
            .rposition(|&k| k != 0.0)
            .map_or(3, |p| p + 1)
            .max(3);
    let parts: Vec<String> = f.k[..used].iter().map(|k| format!("{k:.5}")).collect();
    s.push_str(&parts.join(" "));
    s.push('\n');
    if fit_range > 0.0 {
        s.push_str(&format!("fit_range = {fit_range}\n"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{LensConfig, K_TERMS};
    use crate::model::Lens;

    const T: &[(f64, f64)] = &DREAM_AIR_LIKE_PIMAX_GREEN;

    #[test]
    fn three_term_fit_reproduces_the_documented_defaults() {
        let f = fit(T, 3).unwrap();
        // analysis/lens-distortion.md: base 0.65307, k1 0.60779, k2 -0.72489, k3 0.58858, max error 0.28 deg
        assert!((f.base - 0.65307).abs() < 0.01, "{f:?}");
        assert!((f.k[0] - 0.60779).abs() < 0.1, "{f:?}");
        assert!((f.k[1] - -0.72489).abs() < 0.15, "{f:?}");
        assert!((f.k[2] - 0.58858).abs() < 0.15, "{f:?}");
        assert!(f.max_err_deg < 0.35, "{f:?}");
    }

    #[test]
    fn the_shipped_defaults_stay_within_the_documented_error() {
        // the defaults are the documented fit: max 0.28 deg against the table over 2.5..55 deg
        let cfg = LensConfig::defaults(3840.0 / 3552.0);
        let e = &cfg.eye[0];
        let mut worst = 0.0f64;
        for &(deg, r) in T {
            let t = radial_tangent(f64::from(e.base[1]), 1.0, &e.k[1], r).0;
            worst = worst.max((t.atan().to_degrees() - deg).abs());
        }
        assert!(worst < 0.32, "{worst}");
        assert!(
            worst > 0.2,
            "the table error should be about 0.28 deg, got {worst}"
        );
    }

    #[test]
    fn more_terms_fit_better() {
        // least squares minimises the (weighted) rms, not the maximum, so compare rms
        let r: Vec<f64> = (3..=5).map(|n| fit(T, n).unwrap().rms_err_deg).collect();
        assert!(r[1] < r[0] && r[2] <= r[1] + 1e-9, "{r:?}");
        assert!(fit(T, 5).unwrap().max_err_deg < 0.25);
    }

    #[test]
    fn rejects_bad_requests() {
        assert!(fit(T, 0).is_none());
        assert!(fit(T, 6).is_none());
        assert!(fit(&T[..3], 3).is_none());
    }

    #[test]
    fn exact_recovery_of_a_synthetic_polynomial() {
        let k = [0.3, -0.2, 0.1, 0.0, 0.0];
        let data: Vec<(f64, f64)> = (0..40)
            .map(|i| {
                let r = 0.04 * f64::from(i);
                (
                    radial_tangent(0.7, 1.0, &[0.3, -0.2, 0.1, 0.0, 0.0], r)
                        .0
                        .atan()
                        .to_degrees(),
                    r,
                )
            })
            .collect();
        let f = fit(&data, 3).unwrap();
        assert!(
            (f.base - 0.7).abs() < 1e-6
                && (f.k[0] - k[0]).abs() < 1e-5
                && (f.k[1] - k[1]).abs() < 1e-5
        );
        assert!(f.max_err_deg < 1e-6);
    }

    #[test]
    fn continuation_is_linear_in_tan_and_reaches_the_corners() {
        let extra = continuation(T, 1.5, 0.04, 3);
        assert!(!extra.is_empty());
        let (a_last, r_last) = *extra.last().unwrap();
        assert!(r_last > 1.46 && r_last <= 1.5 + 1e-9);
        // increasing angle, below 90 deg
        let mut prev = 55.0;
        for &(a, _) in &extra {
            assert!(a > prev && a < 90.0);
            prev = a;
        }
        assert!(a_last > 60.0 && a_last < 85.0, "{a_last}");
        // a table of one sample or window 0 has nothing to continue
        assert!(continuation(&T[..1], 1.5, 0.04, 3).is_empty());
        assert!(continuation(T, 1.5, 0.04, 0).is_empty());
    }

    fn corner_lens(base: f64, k: [f32; K_TERMS], fit_range: f32) -> Lens {
        let mut cfg = LensConfig::defaults(3840.0 / 3552.0);
        for e in cfg.eye.iter_mut() {
            e.base = [base as f32; 3];
            e.k = [k; 3];
            e.fit_range = fit_range;
        }
        Lens::new(cfg)
    }

    #[test]
    fn default_fit_with_fit_range_is_a_bounded_monotonic_model_out_to_the_corners() {
        // The data ends at r = 1.148 (55 deg). Beyond it the tangent line of the fitted curve takes over: slope about
        // 4.3 tan/unit, close to the 4.0..4.1 the table itself has at its end, instead of the polynomial's 22.
        let l = corner_lens(0.65307, [0.60779, -0.72489, 0.58858, 0.0, 0.0], 1.15);
        for c in 0..3 {
            assert_eq!(l.first_fold(0, c, 1.6), None);
        }
        let e = l.eye(0);
        let t_corner = f64::from(l.tangent(0, 1, 1.0, 0.0)[0]); // panel right edge, y = centre line
        assert!(t_corner.is_finite() && t_corner > 1.0);
        let at = |r: f32| f64::from(l.tangent_at(e, 1, r, 0.0)[0]);
        let slope_end = (at(1.5) - at(1.2)) / 0.3;
        assert!((3.8..4.8).contains(&slope_end), "{slope_end}");
        // inside the data range nothing changed w.r.t. the unbounded model
        let plain = corner_lens(0.65307, [0.60779, -0.72489, 0.58858, 0.0, 0.0], 0.0);
        for r in [0.2f32, 0.6, 1.0, 1.14] {
            assert_eq!(
                l.tangent_at(e, 1, r, 0.0),
                plain.tangent_at(plain.eye(0), 1, r, 0.0)
            );
        }
        // the unbounded polynomial is far steeper at the corner
        let steep = (f64::from(plain.tangent_at(plain.eye(0), 1, 1.5, 0.0)[0])
            - f64::from(plain.tangent_at(plain.eye(0), 1, 1.2, 0.0)[0]))
            / 0.3;
        assert!(steep > 2.5 * slope_end, "{steep} vs {slope_end}");
    }

    #[test]
    fn refit_with_continuation_gives_a_monotonic_polynomial_to_r_1_5() {
        let mut samples: Vec<(f64, f64, f64)> = T.iter().map(|&(a, r)| (a, r, 1.0)).collect();
        samples.extend(
            continuation(T, 1.5, 0.04, 3)
                .into_iter()
                .map(|(a, r)| (a, r, 0.1)),
        );
        let f = fit_weighted(&samples, 4).unwrap();
        let table_err = T
            .iter()
            .map(|&(a, r)| (f.angle_deg(r) - a).abs())
            .fold(0.0, f64::max);
        assert!(table_err < 0.8, "{table_err} {f:?}");
        // the refit polynomial alone is only monotonic to about r = 1.47: more reason to cap it with fit_range
        let l = corner_lens(f.base, f.k_f32(), 1.4);
        for c in 0..3 {
            assert_eq!(l.first_fold(0, c, 1.6), None, "{f:?}");
        }
        assert!(corner_lens(f.base, f.k_f32(), 0.0)
            .first_fold(0, 1, 1.6)
            .is_some());
    }

    #[test]
    fn conf_fragment_roundtrips_through_the_parser() {
        let f = fit(T, 5).unwrap();
        let text = conf_fragment(&f, 1.5);
        let (cfg, w) = crate::config::parse(&text, 1.0);
        assert!(w.is_empty(), "{w:?}\n{text}");
        assert!((f64::from(cfg.eye[0].base[0]) - f.base).abs() < 1e-5);
        assert!((f64::from(cfg.eye[0].k[1][4]) - f.k[4]).abs() < 1e-4);
        assert_eq!(cfg.eye[0].fit_range, 1.5);
        let three = conf_fragment(&fit(T, 3).unwrap(), 0.0);
        assert!(!three.contains("fit_range"));
        assert_eq!(three.lines().nth(1).unwrap().split_whitespace().count(), 5);
        // "k = a b c"
    }
}
