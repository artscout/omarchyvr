//! Offline re-fit of a saved run (`pimax-eyecal --refit <run.json>`) and the sample-level fit that the live
//! procedure uses ([`fit_eye`]).
//!
//! The old fit used one median point per target and the gaze direction only. The tracker's direction error follows
//! its reported eye-origin error (about 6 degrees per millimetre, see `pimax_eye::calib`), so the fit here works on
//! the fixation samples with the origin as a feature ([`pimax_eye::calfit`]) and the calibration drops or
//! down-weights an eye whose origin is far from where this wearer's eye normally is.
//!
//! A run is `eyecal-<utc>.json` (targets, eye positions, conversion) plus `eyecal-<utc>-raw.csv` (every raw sample
//! with its phase tag). Only the calibration targets are used for the fit; the validation targets only score it.

use crate::collect::{analyze_eye, eye_center, eye_error_deg, evaluate_window, truth_tangent, RawEye, RawSample, WindowEval};
use crate::layout::Target;
use pimax_eye::calfit::{self, SamplePt, SampleSelection, TargetSamples};
use pimax_eye::calib::{
    select_model, stats, Calibration, Combine, EyeCal, EyePoint, OriginTerms, Selection, Stats,
};
use pimax_eye::filter::FilterParams;
use pimax_eye::fixation::{label, FixParams, Label};
use pimax_eye::correction::tangent_of;
use pimax_eye::track::FrameConv;
use std::path::{Path, PathBuf};

type RawPt = ((f64, f64), [f64; 3], f64);

/// How samples are taken out of a target window.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ExtractOpts {
    /// Seconds at the start of the window that are dropped (the gaze settles after the saccade).
    pub skip_s: f64,
    /// Keep only the longest fixation stretch (velocity threshold) instead of every non-blink sample.
    pub ivt: bool,
    /// Samples whose origin is farther than this from the eye's reference origin are dropped (millimetres); also
    /// the gate written into the calibration. 0: none.
    pub gate_mm: f64,
    /// Fewest samples for a target to count.
    pub min_samples: usize,
    /// Offer the pupil-diameter candidates (the pupil centre moves with the pupil size; only taken when the
    /// leave-one-out error is clearly better).
    pub pupil: bool,
}

impl Default for ExtractOpts {
    fn default() -> Self {
        ExtractOpts {
            skip_s: 0.25,
            ivt: false,
            gate_mm: 5.0,
            min_samples: 20,
            pupil: false,
        }
    }
}

/// One eye's cleaned samples.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeData {
    /// Targets with enough samples.
    pub targets: Vec<TargetSamples>,
    /// Window index of each entry of `targets`.
    pub window_index: Vec<usize>,
    /// Reference origin (median of the kept samples), metres.
    pub reference_m: [f64; 3],
    /// Reference pupil diameter (median of the kept samples), millimetres.
    pub pupil_ref_mm: f64,
    /// Sample-to-sample noise (RMS), degrees, median over the windows with a fixation.
    pub precision_deg: f64,
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    let n = v.len();
    if n == 0 {
        f64::NAN
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

fn median3(o: &[[f64; 3]]) -> [f64; 3] {
    [0, 1, 2].map(|a| median(o.iter().map(|p| p[a]).collect()))
}

fn dist_mm(a: [f64; 3], b: [f64; 3]) -> f64 {
    (0..3).map(|i| ((a[i] - b[i]) * 1e3).powi(2)).sum::<f64>().sqrt()
}

/// Cleaned samples of `eye` in the windows of the calibration targets.
pub fn eye_data(wins: &[(Target, &[RawSample])], eye: usize, eye_pos: [f64; 3], opts: &ExtractOpts, fix: &FixParams) -> EyeData {
    // pass 1: samples that are tracked, not at a blink edge, after the settling time
    let mut per: Vec<(usize, Vec<RawPt>)> = Vec::new();
    for (wi, (_, w)) in wins.iter().enumerate() {
        if w.is_empty() {
            continue;
        }
        let t: Vec<f64> = w.iter().map(|s| s.t).collect();
        let tan: Vec<Option<(f64, f64)>> = w.iter().map(|s| s.eyes[eye].and_then(|e| tangent_of(e.dir))).collect();
        let labels = label(&t, &tan, fix);
        let keep: Vec<bool> = if opts.ivt {
            let usable: Vec<bool> = labels.iter().map(|l| *l == Label::Fixation).collect();
            match pimax_eye::fixation::longest_fixation(&t, &tan, &usable, fix) {
                Some(r) => (0..w.len()).map(|i| r.contains(&i) && usable[i]).collect(),
                None => vec![false; w.len()],
            }
        } else {
            labels.iter().map(|l| matches!(l, Label::Fixation | Label::Saccade)).collect()
        };
        let t0 = t[0];
        let v: Vec<RawPt> = (0..w.len())
            .filter(|&i| keep[i] && t[i] - t0 >= opts.skip_s)
            .filter_map(|i| {
                let e = w[i].eyes[eye]?;
                Some((tan[i]?, e.origin, e.pupil_mm))
            })
            .collect();
        per.push((wi, v));
    }
    let all: Vec<[f64; 3]> = per.iter().flat_map(|p| p.1.iter().map(|s| s.1)).collect();
    let mut reference = if all.is_empty() { eye_pos } else { median3(&all) };
    if opts.gate_mm > 0.0 && !all.is_empty() {
        // second pass: the reference of the samples that pass the first gate
        let inside: Vec<[f64; 3]> = all.iter().copied().filter(|o| dist_mm(*o, reference) <= opts.gate_mm).collect();
        if inside.len() >= 10 {
            reference = median3(&inside);
        }
    }
    let pupils: Vec<f64> = per
        .iter()
        .flat_map(|p| p.1.iter())
        .filter(|s| opts.gate_mm <= 0.0 || dist_mm(s.1, reference) <= opts.gate_mm)
        .map(|s| s.2)
        .filter(|p| p.is_finite() && *p > 0.0)
        .collect();
    let pupil_ref_mm = if pupils.is_empty() { 0.0 } else { median(pupils) };
    let mut targets = Vec::new();
    let mut window_index = Vec::new();
    for (wi, v) in per {
        let kept: Vec<&RawPt> = v
            .iter()
            .filter(|s| opts.gate_mm <= 0.0 || dist_mm(s.1, reference) <= opts.gate_mm)
            .collect();
        if kept.len() < opts.min_samples {
            continue;
        }
        let Some(truth) = truth_tangent(&wins[wi].0, eye_pos) else { continue };
        let samples: Vec<SamplePt> = kept
            .iter()
            .map(|s| SamplePt {
                raw: s.0,
                org_mm: [0, 1, 2].map(|a| (s.1[a] - reference[a]) * 1e3),
                pupil_mm: if s.2.is_finite() && s.2 > 0.0 { s.2 - pupil_ref_mm } else { 0.0 },
            })
            .collect();
        targets.push(TargetSamples {
            truth,
            weight: (samples.len() as f64 / 60.0).clamp(0.3, 1.0),
            samples,
        });
        window_index.push(wi);
    }
    let prec: Vec<f64> = wins
        .iter()
        .filter_map(|(_, w)| analyze_eye(w, eye, fix).map(|e| e.rms_s2s_deg))
        .collect();
    EyeData {
        targets,
        window_index,
        reference_m: reference,
        pupil_ref_mm,
        precision_deg: if prec.is_empty() { 1.0 } else { median(prec) / std::f64::consts::SQRT_2 },
    }
}

/// The result of fitting one eye.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeFitResult {
    /// The calibration of the eye.
    pub cal: EyeCal,
    /// Candidates and the chosen one (same record as the point fit).
    pub selection: Selection,
    /// Targets used.
    pub points: usize,
    /// Window index of every target of `selection`'s fits, with the leave-one-out error of the chosen model.
    pub loo_by_window: Vec<(usize, f64)>,
    /// Whether the chosen model has origin terms.
    pub uses_origin: bool,
}

/// Select and fit one eye from its cleaned samples; `None` with fewer than 4 targets.
pub fn fit_eye(data: &EyeData, eye_pos: [f64; 3], opts: &ExtractOpts) -> Option<EyeFitResult> {
    if data.targets.len() < 4 {
        return None;
    }
    let SampleSelection { selection, fitted } = calfit::select(&data.targets, 0.3, opts.pupil);
    let i = selection.chosen?;
    let best = &selection.candidates[i];
    let f = &fitted[i];
    let mut org = OriginTerms {
        reference_m: data.reference_m,
        gate_mm: opts.gate_mm,
        ..OriginTerms::default()
    };
    if best.org.is_some() {
        org.u = f.org_u;
        org.v = f.org_v;
        if f.pupil_uv != (0.0, 0.0) {
            org.pupil_ref_mm = data.pupil_ref_mm;
            org.pupil_u = f.pupil_uv.0;
            org.pupil_v = f.pupil_uv.1;
        }
    }
    let cal = EyeCal {
        map: f.map.clone(),
        origin_m: eye_pos,
        accuracy_deg: best.loo.mean,
        precision_deg: data.precision_deg,
        drift: (0.0, 0.0),
        org,
    };
    let loo_by_window = data.window_index.iter().copied().zip(best.loo_each.iter().copied()).collect();
    Some(EyeFitResult {
        cal,
        points: data.targets.len(),
        loo_by_window,
        uses_origin: best.org.is_some(),
        selection,
    })
}

/// A one-line description of the chosen model, e.g. `affine + origin`.
pub fn model_name(r: &EyeFitResult) -> String {
    let n = r.cal.map.model.clone();
    if r.cal.org.pupil_u != 0.0 || r.cal.org.pupil_v != 0.0 {
        format!("{n} + origin + pupil")
    } else if r.uses_origin {
        format!("{n} + origin")
    } else {
        n
    }
}

/// A saved run.
#[derive(Debug, Clone, PartialEq)]
pub struct Run {
    /// Conversion the run was measured with.
    pub conv: FrameConv,
    /// Rendering positions of the eyes (head frame, metres).
    pub eye_pos: [[f64; 3]; 2],
    /// Calibration targets and their recorded windows (last attempt).
    pub cal: Vec<(Target, Vec<RawSample>)>,
    /// Validation targets and windows.
    pub val: Vec<(Target, Vec<RawSample>)>,
    /// The summary stored in the run.
    pub summary: String,
    /// The calibration targets of the run that had no window in the log (should be none).
    pub missing: usize,
}

/// The numbers following the first `"key":` in `text` (flattened, stops at the matching bracket or at the end of
/// the scalar).
fn numbers_after(text: &str, key: &str) -> Vec<f64> {
    let Some(p) = text.find(&format!("\"{key}\"")) else { return Vec::new() };
    let rest = &text[p + key.len() + 2..];
    let rest = rest.trim_start().trim_start_matches(':').trim_start();
    let mut depth = 0i32;
    let mut end = rest.len();
    for (i, c) in rest.char_indices() {
        match c {
            '[' => depth += 1,
            ']' => {
                depth -= 1;
                if depth <= 0 {
                    end = i + 1;
                    break;
                }
            }
            ',' | '\n' | '}' if depth == 0 => {
                end = i;
                break;
            }
            _ => {}
        }
    }
    rest[..end]
        .split(|c: char| !(c.is_ascii_digit() || c == '.' || c == '-' || c == '+' || c == 'e' || c == 'E'))
        .filter(|s| !s.is_empty())
        .filter_map(|s| s.parse().ok())
        .collect()
}

/// Targets of one JSON array (`"az": a, "el": e` pairs between the key and the next of `stops`).
fn targets_in(text: &str, key: &str, stops: &[&str]) -> Vec<Target> {
    let Some(p) = text.find(&format!("\"{key}\"")) else { return Vec::new() };
    let mut end = text.len();
    for s in stops {
        if let Some(q) = text[p..].find(&format!("\"{s}\"")) {
            end = end.min(p + q);
        }
    }
    let sec = &text[p..end];
    let mut out = Vec::new();
    let mut rest = sec;
    while let Some(i) = rest.find("\"az\"") {
        rest = &rest[i..];
        let az = numbers_after(rest, "az");
        let el = numbers_after(&rest[4..], "el");
        if let (Some(a), Some(e)) = (az.first(), el.first()) {
            out.push(Target::new(*a, *e));
        }
        rest = &rest[4..];
    }
    out
}

/// Parse the raw CSV lines of a run into per-(set, index) Collect windows (the last attempt of each).
fn csv_windows(text: &str) -> std::collections::BTreeMap<(String, usize), Vec<RawSample>> {
    let mut blocks: Vec<((String, usize, String), Vec<RawSample>)> = Vec::new();
    for line in text.lines().skip(1) {
        let c: Vec<&str> = line.split(',').collect();
        // tag "Set:idx:az,el:Phase" contains a comma, so the fields after it are shifted by one
        if c.len() < 4 || !(c[1].starts_with("Calibration:") || c[1].starts_with("Validation:")) {
            continue;
        }
        let mut tag = c[1].split(':');
        let (Some(set), Some(idx)) = (tag.next(), tag.next().and_then(|i| i.parse::<usize>().ok())) else { continue };
        let Some(phase) = c[2].split(':').nth(1) else { continue };
        let f = &c[3..];
        if f.len() < 16 {
            continue;
        }
        let Ok(t) = c[0].parse::<f64>() else { continue };
        let eye = |o: usize| -> Option<RawEye> {
            if f[o] != "1" {
                return None;
            }
            let n: Vec<f64> = f[o + 1..o + 8].iter().map(|x| x.parse().unwrap_or(f64::NAN)).collect();
            n.iter().all(|x| x.is_finite()).then(|| RawEye {
                dir: [n[0], n[1], n[2]],
                origin: [n[3], n[4], n[5]],
                pupil_mm: n[6],
            })
        };
        let s = RawSample {
            t,
            eyes: [eye(0), eye(8)],
        };
        let key = (set.to_string(), idx, phase.to_string());
        match blocks.last_mut() {
            Some((k, v)) if *k == key => v.push(s),
            _ => blocks.push((key, vec![s])),
        }
    }
    let mut out = std::collections::BTreeMap::new();
    for ((set, idx, phase), v) in blocks {
        if phase == "Collect" {
            out.insert((set, idx), v);
        }
    }
    out
}

/// Load `<run>.json` and `<run>-raw.csv`.
pub fn load_run(json_path: &Path) -> Result<Run, String> {
    let text = std::fs::read_to_string(json_path).map_err(|e| format!("{}: {e}", json_path.display()))?;
    let stem = json_path
        .file_stem()
        .and_then(|s| s.to_str())
        .ok_or("bad file name")?
        .trim_end_matches("-raw")
        .to_string();
    let csv_path = json_path.with_file_name(format!("{stem}-raw.csv"));
    let csv = std::fs::read_to_string(&csv_path).map_err(|e| format!("{}: {e} (the raw log is needed)", csv_path.display()))?;
    let pos = numbers_after(&text, "eye_pos_m");
    if pos.len() != 6 {
        return Err("eye_pos_m missing in the run".into());
    }
    let eye_pos = [[pos[0], pos[1], pos[2]], [pos[3], pos[4], pos[5]]];
    let flips: Vec<bool> = text
        .find("\"flip_xyz\"")
        .map(|p| {
            let r = &text[p..];
            let e = r.find(']').unwrap_or(r.len());
            r[..e].split(',').map(|s| s.contains("true")).collect()
        })
        .unwrap_or_default();
    let conv = FrameConv {
        flip_x: flips.first().copied().unwrap_or(true),
        flip_y: flips.get(1).copied().unwrap_or(false),
        flip_z: flips.get(2).copied().unwrap_or(false),
        ..FrameConv::default()
    };
    let cal_t = targets_in(&text, "calibration_targets", &["validation_targets"]);
    let val_t = targets_in(&text, "validation_targets", &["pursuit", "service_err_deg", "calibration"]);
    let wins = csv_windows(&csv);
    let mut missing = 0;
    let mut take = |set: &str, ts: Vec<Target>| -> Vec<(Target, Vec<RawSample>)> {
        ts.into_iter()
            .enumerate()
            .filter_map(|(i, t)| match wins.get(&(set.to_string(), i)) {
                Some(w) => Some((t, w.clone())),
                None => {
                    missing += 1;
                    None
                }
            })
            .collect()
    };
    let cal = take("Calibration", cal_t);
    let val = take("Validation", val_t);
    let summary = text
        .find("\"summary\"")
        .map(|p| {
            let r = &text[p + 9..];
            let s = r.find('"').map_or(0, |i| i + 1);
            let e = r[s..].find("\",\n").map_or(r.len(), |i| s + i);
            r[s..e].replace("\\n", "\n")
        })
        .unwrap_or_default();
    Ok(Run {
        conv,
        eye_pos,
        cal,
        val,
        summary,
        missing,
    })
}

/// Which fit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    /// The previous one: one median point per target, direction only.
    Points,
    /// The sample-level fit with origin terms.
    Samples,
}

/// A fitted calibration of a run.
#[derive(Debug, Clone, PartialEq)]
pub struct Fitted {
    /// The calibration.
    pub cal: Calibration,
    /// Per eye: model name, targets used, leave-one-out mean.
    pub eyes: [Option<(String, usize, f64)>; 2],
}

fn assemble(conv: FrameConv, eyes: [EyeCal; 2]) -> Calibration {
    Calibration {
        conv,
        eyes,
        combine: Combine::default(),
        filter: FilterParams::default(),
        created_unix: crate::report::unix_now(),
        notes: Vec::new(),
    }
}

/// Fit a run's calibration targets (nothing from the validation targets is used).
pub fn fit_run(run: &Run, method: Method, opts: &ExtractOpts, fix: &FixParams) -> Result<Fitted, String> {
    let wins: Vec<(Target, &[RawSample])> = run.cal.iter().map(|(t, w)| (t.clone(), w.as_slice())).collect();
    let mut eyes: [Option<EyeCal>; 2] = [None, None];
    let mut info: [Option<(String, usize, f64)>; 2] = [None, None];
    for e in 0..2 {
        match method {
            Method::Samples => {
                let data = eye_data(&wins, e, run.eye_pos[e], opts, fix);
                if let Some(r) = fit_eye(&data, run.eye_pos[e], opts) {
                    info[e] = Some((model_name(&r), r.points, r.cal.accuracy_deg));
                    eyes[e] = Some(r.cal);
                }
            }
            Method::Points => {
                let pts: Vec<EyePoint> = wins
                    .iter()
                    .filter_map(|(t, w)| {
                        let ew = analyze_eye(w, e, fix)?;
                        Some(EyePoint {
                            raw: ew.center,
                            truth: truth_tangent(t, run.eye_pos[e])?,
                            weight: (ew.used as f64 / 60.0).clamp(0.3, 1.0),
                        })
                    })
                    .collect();
                if pts.len() < 4 {
                    continue;
                }
                let sel = select_model(&pts);
                let Some(b) = sel.best() else { continue };
                let prec: Vec<f64> = wins.iter().filter_map(|(_, w)| analyze_eye(w, e, fix).map(|x| x.rms_s2s_deg)).collect();
                info[e] = Some((b.kind.name().to_string(), pts.len(), b.loo.mean));
                eyes[e] = Some(EyeCal {
                    map: b.map.clone(),
                    origin_m: run.eye_pos[e],
                    accuracy_deg: b.loo.mean,
                    precision_deg: if prec.is_empty() { 1.0 } else { median(prec) / std::f64::consts::SQRT_2 },
                    drift: (0.0, 0.0),
                    org: OriginTerms::default(),
                });
            }
        }
    }
    let pair = match (eyes[0].clone(), eyes[1].clone()) {
        (Some(l), Some(r)) => [l, r],
        (Some(l), None) => {
            let mut r = l.clone();
            r.origin_m = run.eye_pos[1];
            [l, r]
        }
        (None, Some(r)) => {
            let mut l = r.clone();
            l.origin_m = run.eye_pos[0];
            [l, r]
        }
        _ => return Err("no eye had 4 usable calibration targets".into()),
    };
    Ok(Fitted {
        cal: assemble(run.conv, pair),
        eyes: info,
    })
}

/// Validation of a calibration on a run's validation windows.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// Combined error per target (fixation median), degrees.
    pub combined: Stats,
    /// Same targets, raw gaze.
    pub raw: Stats,
    /// Per eye, mean error over the targets where the eye had a fixation, and the count.
    pub per_eye: [(f64, usize); 2],
    /// Mean fraction of samples without combined gaze.
    pub loss: f64,
    /// Targets with a combined error.
    pub n: usize,
    /// Of [`Scored::combined`], per target in window order (`None` without gaze).
    pub each: Vec<Option<f64>>,
}

/// Score `cal` on the validation windows. The per-eye weights of the combination are `cal`'s `accuracy_deg`;
/// `val_weights` replaces them by the mean validation error of the eye, which is what the live procedure did
/// (not used for the new fit: it would let the validation targets decide).
pub fn score(run: &Run, cal: &Calibration, val_weights: bool, fix: &FixParams) -> Scored {
    let mut cal = cal.clone();
    let mut raw_cal = Calibration::identity(run.conv, run.eye_pos[1][0] - run.eye_pos[0][0]);
    for e in 0..2 {
        raw_cal.eyes[e].origin_m = run.eye_pos[e];
    }
    let evs = |c: &Calibration| -> Vec<WindowEval> {
        run.val.iter().map(|(t, w)| evaluate_window(c, w, t, fix)).collect()
    };
    if val_weights {
        let ev = evs(&cal);
        for e in 0..2 {
            let errs: Vec<f64> = ev.iter().filter_map(|v| v.eye_err_deg[e]).collect();
            if errs.len() >= 3 {
                cal.eyes[e].accuracy_deg = stats(&errs).mean;
            }
        }
    }
    let ev = evs(&cal);
    let rev = evs(&raw_cal);
    let each: Vec<Option<f64>> = ev.iter().map(|v| v.comb_err_deg).collect();
    let per_eye = [0, 1].map(|e| {
        let v: Vec<f64> = ev.iter().filter_map(|x| x.eye_err_deg[e]).collect();
        (if v.is_empty() { f64::NAN } else { v.iter().sum::<f64>() / v.len() as f64 }, v.len())
    });
    let combined: Vec<f64> = each.iter().flatten().copied().collect();
    Scored {
        n: combined.len(),
        combined: stats(&combined),
        raw: stats(&rev.iter().filter_map(|v| v.comb_err_deg).collect::<Vec<_>>()),
        per_eye,
        loss: if ev.is_empty() { 1.0 } else { ev.iter().map(|v| v.loss).sum::<f64>() / ev.len() as f64 },
        each,
    }
}

/// Per-sample error of one eye on a window with the given calibration (for diagnostics): median calibrated tangent
/// against the truth, `None` without a fixation.
pub fn eye_error_on(cal: &EyeCal, w: &[RawSample], t: &Target, eye: usize, fix: &FixParams) -> Option<f64> {
    let ew = analyze_eye(w, eye, fix)?;
    eye_error_deg(cal, &ew, truth_tangent(t, cal.origin_m)?)
}

/// What is left of the validation error once a constant offset per eye is removed (a session-level shift between
/// the calibration and the validation phase, not a model error). One line per eye.
pub fn offset_diagnosis(run: &Run, cal: &Calibration, fix: &FixParams) -> Vec<String> {
    let mut out = Vec::new();
    for e in 0..2 {
        let mut d: Vec<(usize, f64, f64)> = Vec::new();
        for (i, (t, w)) in run.val.iter().enumerate() {
            let (Some(ew), Some(truth)) = (analyze_eye(w, e, fix), truth_tangent(t, cal.eyes[e].origin_m)) else { continue };
            let Some(c) = eye_center(&cal.eyes[e], &ew) else { continue };
            d.push((i, (c.0.atan() - truth.0.atan()).to_degrees(), (c.1.atan() - truth.1.atan()).to_degrees()));
        }
        if d.len() < 4 {
            continue;
        }
        let n = d.len() as f64;
        let (mu, mv) = (d.iter().map(|x| x.1).sum::<f64>() / n, d.iter().map(|x| x.2).sum::<f64>() / n);
        let left = d.iter().map(|x| ((x.1 - mu).powi(2) + (x.2 - mv).powi(2)).sqrt()).sum::<f64>() / n;
        let mi = d.iter().map(|x| x.0 as f64).sum::<f64>() / n;
        let den: f64 = d.iter().map(|x| (x.0 as f64 - mi).powi(2)).sum();
        let slope = if den > 0.0 { d.iter().map(|x| (x.0 as f64 - mi) * (x.1 - mu)).sum::<f64>() / den } else { 0.0 };
        out.push(format!(
            "{} eye validation: constant offset ({mu:+.2}, {mv:+.2}) deg (u right, v up), mean error {left:.2} deg without it; horizontal drift {slope:+.2} deg per target in the order measured",
            if e == 0 { "left" } else { "right" }
        ));
    }
    out
}

fn row(name: &str, fit: &Fitted, s: &Scored) -> String {
    let eye = |e: usize| {
        let (m, n) = s.per_eye[e];
        if n == 0 {
            "   --".to_string()
        } else {
            format!("{m:5.2}")
        }
    };
    let loo = |e: usize| fit.eyes[e].as_ref().map_or("--".to_string(), |x| format!("{:.2}", x.2));
    let model = |e: usize| fit.eyes[e].as_ref().map_or("--".to_string(), |x| x.0.clone());
    format!(
        "{name:<22} {:>6.2} {:>7.2} {:>6.2} {:>6.2}   {} {}   {:>5}/{:<5}  {} | {}",
        s.combined.mean,
        s.combined.median,
        s.combined.p95,
        s.combined.max,
        eye(0),
        eye(1),
        loo(0),
        loo(1),
        model(0),
        model(1),
    )
}

/// Options of `--refit`.
#[derive(Debug, Clone, PartialEq)]
pub struct RefitOpts {
    /// Write the new calibration here (with a backup of an existing file) instead of only printing the command.
    pub write: Option<PathBuf>,
    /// Extraction settings.
    pub extract: ExtractOpts,
}

/// Re-fit a saved run and print the old and new validation accuracy; returns the text printed.
pub fn refit(json_path: &Path, opts: &RefitOpts) -> Result<String, String> {
    use std::fmt::Write as _;
    let run = load_run(json_path)?;
    let fix = FixParams::default();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "{}: {} calibration windows, {} validation windows{}",
        json_path.display(),
        run.cal.len(),
        run.val.len(),
        if run.missing > 0 { format!(" ({} targets have no raw window)", run.missing) } else { String::new() }
    );
    let old = fit_run(&run, Method::Points, &opts.extract, &fix)?;
    let new = fit_run(&run, Method::Samples, &opts.extract, &fix)?;
    let s_old_live = score(&run, &old.cal, true, &fix);
    let s_old = score(&run, &old.cal, false, &fix);
    let s_new = score(&run, &new.cal, false, &fix);
    let _ = writeln!(
        out,
        "validation on {} targets that were not used for the fit (mean/median/p95/max combined, per-eye mean, leave-one-out of the calibration targets):",
        run.val.len()
    );
    let _ = writeln!(
        out,
        "{:<22} {:>6} {:>7} {:>6} {:>6}   {:>5} {:>5}   {:>11}  models (left | right)",
        "", "mean", "median", "p95", "max", "left", "right", "loo L/R"
    );
    let _ = writeln!(
        out,
        "{}",
        row("raw gaze", &old, &Scored { combined: s_old.raw, per_eye: [(f64::NAN, 0); 2], ..s_old.clone() })
    );
    let _ = writeln!(out, "{}", row("old fit (live weights)", &old, &s_old_live));
    let _ = writeln!(out, "{}", row("old fit", &old, &s_old));
    let _ = writeln!(out, "{}", row("new fit", &new, &s_new));
    let _ = writeln!(
        out,
        "data loss {:.0} % -> {:.0} %; targets with a combined error {} -> {}",
        100.0 * s_old.loss,
        100.0 * s_new.loss,
        s_old.n,
        s_new.n
    );
    for l in offset_diagnosis(&run, &new.cal, &fix) {
        let _ = writeln!(out, "{l}");
    }
    for e in 0..2 {
        let o = &new.cal.eyes[e].org;
        if o.is_active() {
            let _ = writeln!(
                out,
                "{} eye origin terms: du/dmm {:+.3} {:+.3} {:+.3}, dv/dmm {:+.3} {:+.3} {:+.3}, reference {:.1} {:.1} {:.1} mm, gate {:.1} mm",
                if e == 0 { "left" } else { "right" },
                o.u[0], o.u[1], o.u[2], o.v[0], o.v[1], o.v[2],
                o.reference_m[0] * 1e3, o.reference_m[1] * 1e3, o.reference_m[2] * 1e3, o.gate_mm
            );
        }
    }
    let mut cal = new.cal.clone();
    cal.notes = vec![
        format!(
            "refit of {} (sample-level fit with origin terms): validation mean {:.2} median {:.2} p95 {:.2} deg on {} targets (old fit {:.2}), raw {:.2}",
            json_path.file_name().and_then(|s| s.to_str()).unwrap_or("run"),
            s_new.combined.mean, s_new.combined.median, s_new.combined.p95, s_new.n, s_old.combined.mean, s_new.raw.mean
        ),
    ];
    let stem = json_path.file_stem().and_then(|s| s.to_str()).unwrap_or("run").to_string();
    let candidate = json_path.with_file_name(format!("{stem}-refit.toml"));
    std::fs::write(&candidate, cal.to_toml()).map_err(|e| format!("{}: {e}", candidate.display()))?;
    let _ = writeln!(out, "new calibration written to {}", candidate.display());
    let target = opts.write.clone().or_else(Calibration::default_path);
    match (&opts.write, target) {
        (Some(p), _) => {
            let bak = crate::report::write_with_backup(p, &cal.to_toml())?;
            let _ = writeln!(
                out,
                "saved to {}{}",
                p.display(),
                bak.map_or(String::new(), |b| format!(" (old file kept as {})", b.display()))
            );
        }
        (None, Some(p)) => {
            let _ = writeln!(out, "to use it (not done): pimax-eyecal --refit {} --write {}", json_path.display(), p.display());
        }
        _ => {}
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_scanners() {
        let t = "{\n  \"flip_xyz\": [true, false, true],\n  \"eye_pos_m\": [[-0.0315, 0.0000, 0.0000], [0.0315, -0.0000, 0.0]],\n  \"calibration_targets\": [\n    {\"az\": 0.0, \"el\": 0.0, \"ok\": true},\n    {\"az\": -15.0, \"el\": 12.0, \"ok\": true}\n  ],\n  \"validation_targets\": [\n    {\"az\": 4.0, \"el\": 3.0, \"region\": \"centre\"}\n  ],\n  \"pursuit\": null\n}";
        assert_eq!(numbers_after(t, "eye_pos_m").len(), 6);
        let c = targets_in(t, "calibration_targets", &["validation_targets"]);
        assert_eq!(c, vec![Target::new(0.0, 0.0), Target::new(-15.0, 12.0)]);
        let v = targets_in(t, "validation_targets", &["pursuit"]);
        assert_eq!(v, vec![Target::new(4.0, 3.0)]);
    }

    #[test]
    fn csv_with_a_comma_in_the_tag() {
        let csv = "t_s,tag,...\n1.0,Intro,0,,,,,,,,0,,,,,,,,0,0,0,1\n\
            2.0,Calibration:0:+0,+0:Collect,1,-0.1,-0.2,-0.97,-0.0287,0.004,0.022,3.2,1,0.1,-0.2,-0.97,0.0251,0.003,0.02,3.3,0,0,0,1\n\
            2.1,Calibration:0:+0,+0:Collect,0,,,,,,,,1,0.1,-0.2,-0.97,0.0251,0.003,0.02,3.3,0,0,0,1\n";
        let w = csv_windows(csv);
        let v = &w[&("Calibration".to_string(), 0)];
        assert_eq!(v.len(), 2);
        assert!(v[0].eyes[0].is_some() && v[0].eyes[1].is_some());
        assert!(v[1].eyes[0].is_none() && v[1].eyes[1].is_some());
        assert!((v[0].eyes[0].unwrap().origin[0] + 0.0287).abs() < 1e-9);
    }
}
