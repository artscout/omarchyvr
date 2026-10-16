//! Sample-level fit of the per-eye calibration (with the origin terms of [`crate::calib::OriginTerms`]).
//!
//! [`crate::calib::select_model`] fits one median point per target (13 points per eye). That cannot see what limits
//! the Dream Air tracker: the reported direction moves about 6 degrees per millimetre of error in the reported eye
//! origin (see the module docs of [`crate::calib`]), and the origin wanders by 1-3 mm within one session. Here every
//! cleaned fixation sample is a data point `(raw tangent, origin deviation) -> target tangent`, so the origin
//! coefficients are identified by the sample-to-sample and target-to-target origin variation, and the polynomial
//! stays small.
//!
//! * features: `1, u, v` (+ `u*u, u*v, v*v` for the quadratic model) and, with origin terms, the three origin
//!   deviations in millimetres,
//! * each target counts the same (weights sum to the target's weight, whatever its sample count),
//! * Huber IRLS on the sample residuals ([`HUBER_SAMPLE`]), ridge relative to each feature's own information,
//! * the model is chosen by leave-one-target-out error of the per-target median, simpler candidates preferred
//!   unless a richer one is clearly better (same rule as [`crate::calib::select_model`]).

use crate::calib::{solve, stats, tangent_angle_deg, ModelFit, ModelKind, Ridge, Selection, PUPIL_CLAMP_MM};
use crate::correction::Correction;

/// Huber threshold on a sample residual, tangent units (about 1.7 degrees).
pub const HUBER_SAMPLE: f64 = 0.03;

/// One fixation sample of one eye.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SamplePt {
    /// Raw tangent (u, v).
    pub raw: (f64, f64),
    /// Reported origin minus the reference origin, millimetres.
    pub org_mm: [f64; 3],
    /// Reported pupil diameter minus the reference, millimetres (clamped to +-[`PUPIL_CLAMP_MM`]).
    pub pupil_mm: f64,
}

/// All the cleaned samples of one eye at one target.
#[derive(Debug, Clone, PartialEq)]
pub struct TargetSamples {
    /// Tangent of the direction from the eye's rendering position to the target.
    pub truth: (f64, f64),
    /// The samples.
    pub samples: Vec<SamplePt>,
    /// Relative weight of the target (1 is normal).
    pub weight: f64,
}

/// One candidate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Spec {
    /// Polynomial size.
    pub kind: ModelKind,
    /// Whether the origin deviation is a feature.
    pub org: bool,
    /// Ridge on the polynomial terms.
    pub ridge: Ridge,
    /// Relative ridge on the origin coefficients.
    pub org_ridge: f64,
    /// Whether the pupil diameter deviation is a feature (only with `org`).
    pub pupil: bool,
}

fn poly_len(kind: ModelKind) -> usize {
    kind.features()
}

fn features(p: &SamplePt, spec: &Spec) -> Vec<f64> {
    let (u, v) = p.raw;
    let mut f = vec![1.0, u, v, u * u, u * v, v * v];
    f.truncate(poly_len(spec.kind));
    if spec.org {
        f.extend_from_slice(&p.org_mm);
        if spec.pupil {
            f.push(p.pupil_mm.clamp(-PUPIL_CLAMP_MM, PUPIL_CLAMP_MM));
        }
    }
    f
}

/// A fitted candidate: polynomial map and origin coefficients (per millimetre).
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    /// The polynomial part.
    pub map: Correction,
    /// Tangent per millimetre of origin deviation, `u`.
    pub org_u: [f64; 3],
    /// Tangent per millimetre of origin deviation, `v`.
    pub org_v: [f64; 3],
    /// Tangent per millimetre of pupil deviation, `(u, v)`.
    pub pupil_uv: (f64, f64),
}

impl Fitted {
    /// Corrected tangent of one sample.
    pub fn apply(&self, p: &SamplePt) -> (f64, f64) {
        let (u, v) = self.map.map_tangent(p.raw.0, p.raw.1);
        let d = p.org_mm;
        let q = p.pupil_mm.clamp(-PUPIL_CLAMP_MM, PUPIL_CLAMP_MM);
        (
            u + self.org_u[0] * d[0] + self.org_u[1] * d[1] + self.org_u[2] * d[2] + self.pupil_uv.0 * q,
            v + self.org_v[0] * d[0] + self.org_v[1] * d[1] + self.org_v[2] * d[2] + self.pupil_uv.1 * q,
        )
    }
}

fn median(mut x: Vec<f64>) -> f64 {
    x.sort_by(|a, b| a.total_cmp(b));
    let n = x.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        x[n / 2]
    } else {
        0.5 * (x[n / 2 - 1] + x[n / 2])
    }
}

/// Median corrected tangent of a target's samples.
pub fn target_center(f: &Fitted, t: &TargetSamples) -> Option<(f64, f64)> {
    if t.samples.is_empty() {
        return None;
    }
    let c: Vec<(f64, f64)> = t.samples.iter().map(|s| f.apply(s)).collect();
    Some((median(c.iter().map(|x| x.0).collect()), median(c.iter().map(|x| x.1).collect())))
}

/// Error in degrees of the corrected median of a target.
pub fn target_error_deg(f: &Fitted, t: &TargetSamples) -> Option<f64> {
    target_center(f, t).map(|c| tangent_angle_deg(c, t.truth))
}

/// Weighted ridge least squares plus Huber IRLS on all samples of `targets`.
pub fn fit(targets: &[&TargetSamples], spec: &Spec) -> Option<Fitted> {
    let first = targets.iter().find_map(|t| t.samples.first())?;
    let k = features(first, spec).len();
    let np = poly_len(spec.kind);
    let rows: Vec<(Vec<f64>, (f64, f64), f64)> = targets
        .iter()
        .filter(|t| !t.samples.is_empty() && t.weight > 0.0)
        .flat_map(|t| {
            let w = t.weight / t.samples.len() as f64;
            t.samples.iter().map(move |s| (features(s, spec), (t.truth.0 - s.raw.0, t.truth.1 - s.raw.1), w))
        })
        .collect();
    let ntargets = targets.iter().filter(|t| !t.samples.is_empty() && t.weight > 0.0).count();
    if rows.is_empty() || ntargets < spec.kind.min_points().min(poly_len(spec.kind)) {
        return None;
    }
    let base: Vec<f64> = rows.iter().map(|r| r.2).collect();
    let mut w = base.clone();
    let mut sol: Option<(Vec<f64>, Vec<f64>)> = None;
    for _ in 0..5 {
        let total: f64 = w.iter().sum();
        if total <= 0.0 {
            return None;
        }
        let mut ata = vec![vec![0.0; k]; k];
        let (mut bu, mut bv) = (vec![0.0; k], vec![0.0; k]);
        for (r, &wi) in rows.iter().zip(&w) {
            for i in 0..k {
                for (j, a) in ata[i].iter_mut().enumerate() {
                    *a += wi * r.0[i] * r.0[j];
                }
                bu[i] += wi * r.0[i] * r.1 .0;
                bv[i] += wi * r.0[i] * r.1 .1;
            }
        }
        for (i, row) in ata.iter_mut().enumerate() {
            let pen = match i {
                0 => 1e-12,
                1 | 2 => spec.ridge.linear,
                i if i < np => spec.ridge.quadratic,
                _ => spec.org_ridge,
            };
            row[i] += pen * row[i].max(1e-9 * total);
        }
        let su = solve(ata.clone(), bu)?;
        let sv = solve(ata, bv)?;
        for (n, r) in rows.iter().enumerate() {
            let pu: f64 = su.iter().zip(&r.0).map(|(a, b)| a * b).sum();
            let pv: f64 = sv.iter().zip(&r.0).map(|(a, b)| a * b).sum();
            let res = ((pu - r.1 .0).powi(2) + (pv - r.1 .1).powi(2)).sqrt();
            w[n] = base[n] * if res <= HUBER_SAMPLE { 1.0 } else { HUBER_SAMPLE / res };
        }
        sol = Some((su, sv));
    }
    let (su, sv) = sol?;
    let mut map = Correction::identity();
    for i in 0..np {
        map.cx[i] += su[i];
        map.cy[i] += sv[i];
    }
    map.model = spec.kind.name().to_string();
    let mut org_u = [0.0; 3];
    let mut org_v = [0.0; 3];
    let mut pupil_uv = (0.0, 0.0);
    if spec.org {
        org_u.copy_from_slice(&su[np..np + 3]);
        org_v.copy_from_slice(&sv[np..np + 3]);
        if spec.pupil {
            pupil_uv = (su[np + 3], sv[np + 3]);
        }
    }
    Some(Fitted {
        map,
        org_u,
        org_v,
        pupil_uv,
    })
}

/// Fit on all targets and score by leave-one-target-out.
pub fn evaluate(targets: &[TargetSamples], spec: &Spec) -> Option<(ModelFit, Fitted)> {
    if targets.len() < spec.kind.min_points() + 1 {
        return None;
    }
    let all: Vec<&TargetSamples> = targets.iter().collect();
    let f = fit(&all, spec)?;
    let train: Vec<f64> = targets.iter().filter_map(|t| target_error_deg(&f, t)).collect();
    let mut loo = Vec::new();
    for i in 0..targets.len() {
        let rest: Vec<&TargetSamples> = targets.iter().enumerate().filter(|(j, _)| *j != i).map(|(_, t)| t).collect();
        let m = fit(&rest, spec)?;
        loo.push(target_error_deg(&m, &targets[i])?);
    }
    let org = spec.org.then_some((f.org_u, f.org_v));
    Some((
        ModelFit {
            kind: spec.kind,
            ridge: spec.ridge,
            map: f.map.clone(),
            loo: stats(&loo),
            train: stats(&train),
            org,
            loo_each: loo,
        },
        f,
    ))
}

/// The candidate list, simplest first.
pub fn candidates(with_pupil: bool) -> Vec<Spec> {
    let r = |q: f64| Ridge {
        quadratic: q,
        ..Ridge::default()
    };
    let org_ridge = 0.005;
    let mut v = Vec::new();
    for org in [false, true] {
        v.push(Spec {
            kind: ModelKind::Offset,
            org,
            ridge: Ridge::default(),
            org_ridge,
            pupil: false,
        });
        v.push(Spec {
            kind: ModelKind::Affine,
            org,
            ridge: Ridge::default(),
            org_ridge,
            pupil: false,
        });
        for q in [1.0, 0.2, 0.02] {
            v.push(Spec {
                kind: ModelKind::Quadratic,
                org,
                ridge: r(q),
                org_ridge,
                pupil: false,
            });
        }
    }
    if with_pupil {
        // after the others: only chosen when clearly better
        v.push(Spec {
            kind: ModelKind::Affine,
            org: true,
            ridge: Ridge::default(),
            org_ridge,
            pupil: true,
        });
        v.push(Spec {
            kind: ModelKind::Quadratic,
            org: true,
            ridge: r(0.2),
            org_ridge,
            pupil: true,
        });
    }
    v
}

/// Result of [`select`].
#[derive(Debug, Clone, PartialEq)]
pub struct SampleSelection {
    /// The usual selection record (raw error, candidates, chosen index).
    pub selection: Selection,
    /// The fitted maps, aligned with `selection.candidates`.
    pub fitted: Vec<Fitted>,
}

impl SampleSelection {
    /// The chosen map.
    pub fn best(&self) -> Option<(&ModelFit, &Fitted)> {
        self.selection.chosen.map(|i| (&self.selection.candidates[i], &self.fitted[i]))
    }
}

/// Fit all candidates and keep the one with the lowest leave-one-out mean error, unless a simpler one is within 7 %
/// (or 0.05 degrees) of it. Candidates with origin terms are only allowed when the origin actually varies
/// (`min_org_sd_mm`, over all samples).
pub fn select(targets: &[TargetSamples], min_org_sd_mm: f64, with_pupil: bool) -> SampleSelection {
    let raw = stats(
        &targets
            .iter()
            .filter(|t| !t.samples.is_empty())
            .map(|t| {
                let c = (
                    median(t.samples.iter().map(|s| s.raw.0).collect()),
                    median(t.samples.iter().map(|s| s.raw.1).collect()),
                );
                tangent_angle_deg(c, t.truth)
            })
            .collect::<Vec<_>>(),
    );
    let sd = org_sd_mm(targets);
    let mut cands = Vec::new();
    let mut fitted = Vec::new();
    for spec in candidates(with_pupil) {
        if spec.org && sd < min_org_sd_mm {
            continue;
        }
        if let Some((m, f)) = evaluate(targets, &spec) {
            cands.push(m);
            fitted.push(f);
        }
    }
    let best = cands
        .iter()
        .enumerate()
        .min_by(|a, b| a.1.loo.mean.total_cmp(&b.1.loo.mean))
        .map(|(i, m)| (i, m.loo.mean));
    let chosen = best.map(|(bi, bm)| {
        cands
            .iter()
            .position(|m| m.loo.mean <= bm * 1.07 || m.loo.mean - bm <= 0.05)
            .unwrap_or(bi)
    });
    SampleSelection {
        selection: Selection {
            raw,
            candidates: cands,
            chosen,
        },
        fitted,
    }
}

/// Largest standard deviation over the three origin axes of all samples, millimetres.
pub fn org_sd_mm(targets: &[TargetSamples]) -> f64 {
    let all: Vec<&SamplePt> = targets.iter().flat_map(|t| t.samples.iter()).collect();
    if all.len() < 5 {
        return 0.0;
    }
    (0..3)
        .map(|a| {
            let m = all.iter().map(|s| s.org_mm[a]).sum::<f64>() / all.len() as f64;
            (all.iter().map(|s| (s.org_mm[a] - m).powi(2)).sum::<f64>() / all.len() as f64).sqrt()
        })
        .fold(0.0, f64::max)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    /// Chip model: raw = gain * truth + offset, plus 0.1 tangent per mm of origin error on u and 0.06 on v.
    fn world(seed: u64, nt: usize) -> Vec<TargetSamples> {
        let mut r = Rng::new(seed);
        let mut out = Vec::new();
        for i in 0..nt {
            let az = -15.0 + 30.0 * (i % 5) as f64 / 4.0;
            let el = -12.0 + 24.0 * (i / 5) as f64 / ((nt / 5).max(2) - 1) as f64;
            let truth = (az.to_radians().tan(), el.to_radians().tan());
            let samples = (0..60)
                .map(|_| {
                    let o = [1.2 * r.gauss(), 0.6 * r.gauss(), 0.8 * r.gauss()];
                    let n = 0.004;
                    let raw = (
                        0.62 * truth.0 - 0.05 - 0.10 * o[0] + n * r.gauss(),
                        0.62 * truth.1 - 0.2 - 0.06 * o[1] + n * r.gauss(),
                    );
                    SamplePt { raw, org_mm: o, pupil_mm: 0.0 }
                })
                .collect();
            out.push(TargetSamples {
                truth,
                samples,
                weight: 1.0,
            });
        }
        out
    }

    #[test]
    fn origin_terms_are_found_and_win_the_selection() {
        let t = world(5, 15);
        let sel = select(&t, 0.3, false);
        let (m, f) = sel.best().expect("a model");
        assert!(m.org.is_some(), "{:?}", m.kind);
        assert!(m.loo.mean < 0.6, "{:?}", m.loo);
        assert!((f.org_u[0] - 0.10 / 0.62).abs() < 0.03, "{:?}", f.org_u);
        assert!((f.org_v[1] - 0.06 / 0.62).abs() < 0.03, "{:?}", f.org_v);
        let plain = sel.selection.candidates.iter().find(|c| c.org.is_none() && c.kind == ModelKind::Affine).unwrap();
        assert!(plain.loo.mean > 2.0 * m.loo.mean, "{} vs {}", plain.loo.mean, m.loo.mean);
        // new targets
        let v = world(6, 10);
        let errs: Vec<f64> = v.iter().filter_map(|t| target_error_deg(f, t)).collect();
        assert!(stats(&errs).mean < 0.7, "{errs:?}");
    }

    #[test]
    fn no_origin_variation_means_no_origin_candidates() {
        let mut t = world(7, 15);
        for x in t.iter_mut().flat_map(|t| t.samples.iter_mut()) {
            x.org_mm = [0.0; 3];
            x.raw.0 += 0.0;
        }
        let sel = select(&t, 0.3, false);
        assert!(sel.selection.candidates.iter().all(|c| c.org.is_none()));
        assert!(sel.best().is_some());
    }

    #[test]
    fn a_wild_sample_cluster_does_not_drag_the_fit() {
        let mut t = world(8, 15);
        for s in t[3].samples.iter_mut().take(15) {
            s.raw.0 += 0.3;
        }
        let spec = Spec {
            kind: ModelKind::Affine,
            org: true,
            ridge: Ridge::default(),
            org_ridge: 0.005,
            pupil: false,
        };
        let all: Vec<&TargetSamples> = t.iter().collect();
        let f = fit(&all, &spec).unwrap();
        let clean = world(9, 10);
        let errs: Vec<f64> = clean.iter().filter_map(|t| target_error_deg(&f, t)).collect();
        assert!(stats(&errs).mean < 1.0, "{errs:?}");
    }
}
