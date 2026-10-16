//! One target window: raw per-eye samples in, per-eye fixation statistics out, and the evaluation of a fitted
//! calibration on a window (per eye and combined, the honest numbers of the validation).

use crate::layout::Target;
use pimax_eye::calib::{tangent_angle_deg, Calibration, CombineState};
use pimax_eye::correction::{dir_of, tangent_of};
use pimax_eye::fixation::{label, longest_fixation, median_tangent, rms_s2s_deg, sd_deg, FixParams, Label};
use pimax_eye::track::{EyeRay, Sample};

/// One eye of a raw sample (head frame after the device conversion, uncalibrated).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawEye {
    /// Unit direction.
    pub dir: [f64; 3],
    /// Origin reported by the tracker, metres.
    pub origin: [f64; 3],
    /// Pupil diameter, mm (negative: unknown).
    pub pupil_mm: f64,
}

/// One raw sample.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RawSample {
    /// Host time, seconds (`CLOCK_MONOTONIC`).
    pub t: f64,
    /// `[left, right]`, `None` = not tracked.
    pub eyes: [Option<RawEye>; 2],
}

impl RawSample {
    /// From a tracker sample (raw tracker: no calibration).
    pub fn from_sample(s: &Sample) -> RawSample {
        let eye = |e: &EyeRay| {
            (e.origin_valid && e.direction_valid).then(|| RawEye {
                dir: e.direction.map(f64::from),
                origin: e.origin.map(f64::from),
                pupil_mm: f64::from(e.pupil_mm),
            })
        };
        RawSample {
            t: s.host_ts_ns as f64 * 1e-9,
            eyes: [eye(&s.eyes[0]), eye(&s.eyes[1])],
        }
    }

    /// Back to a tracker sample (for the calibration code); `t` becomes `host_ts_ns`.
    pub fn to_sample(&self, seq: u64) -> Sample {
        let mut s = Sample {
            seq,
            host_ts_ns: (self.t * 1e9) as i64,
            ..Sample::default()
        };
        for (i, e) in self.eyes.iter().enumerate() {
            if let Some(e) = e {
                s.eyes[i] = EyeRay {
                    origin: e.origin.map(|x| x as f32),
                    direction: e.dir.map(|x| x as f32),
                    origin_valid: true,
                    direction_valid: true,
                    pupil_mm: e.pupil_mm as f32,
                    ..EyeRay::default()
                };
            }
        }
        s
    }
}

/// Fixation statistics of one eye in one window.
#[derive(Debug, Clone, PartialEq)]
pub struct EyeWindow {
    /// Samples in the window.
    pub total: usize,
    /// Samples with this eye tracked.
    pub valid: usize,
    /// Samples used (inside the fixation, not blink edge / saccade).
    pub used: usize,
    /// Duration of the fixation stretch, seconds.
    pub fix_s: f64,
    /// Median raw tangent of the used samples.
    pub center: (f64, f64),
    /// Median distance from the centre, degrees.
    pub mad_deg: f64,
    /// RMS sample-to-sample, degrees.
    pub rms_s2s_deg: f64,
    /// Standard deviation, degrees.
    pub sd_deg: f64,
    /// The used raw tangents.
    pub used_tan: Vec<(f64, f64)>,
    /// The reported origin of each used sample (metres), aligned with `used_tan`.
    pub used_origin: Vec<[f64; 3]>,
    /// The time of each used sample (seconds), aligned with `used_tan`.
    pub used_t: Vec<f64>,
    /// The reported pupil diameter of each used sample (mm), aligned with `used_tan`.
    pub used_pupil: Vec<f64>,
}

/// Analyse one eye of a window. `None` when no fixation of `min_duration_s` was found.
pub fn analyze_eye(w: &[RawSample], eye: usize, p: &FixParams) -> Option<EyeWindow> {
    let t: Vec<f64> = w.iter().map(|s| s.t).collect();
    let tan: Vec<Option<(f64, f64)>> = w
        .iter()
        .map(|s| s.eyes[eye].and_then(|e| tangent_of(e.dir)))
        .collect();
    let labels = label(&t, &tan, p);
    let usable: Vec<bool> = labels.iter().map(|l| *l == Label::Fixation).collect();
    let r = longest_fixation(&t, &tan, &usable, p)?;
    let idx: Vec<usize> = r.clone().filter(|&i| usable[i] && tan[i].is_some()).collect();
    let used_tan: Vec<(f64, f64)> = idx.iter().filter_map(|&i| tan[i]).collect();
    let used_origin: Vec<[f64; 3]> = idx.iter().filter_map(|&i| w[i].eyes[eye].map(|e| e.origin)).collect();
    let used_t: Vec<f64> = idx.iter().map(|&i| t[i]).collect();
    let used_pupil: Vec<f64> = idx.iter().filter_map(|&i| w[i].eyes[eye].map(|e| e.pupil_mm)).collect();
    let (center, mad) = median_tangent(&used_tan)?;
    Some(EyeWindow {
        total: w.len(),
        valid: tan.iter().filter(|x| x.is_some()).count(),
        used: used_tan.len(),
        fix_s: t[r.end - 1] - t[r.start],
        center,
        mad_deg: mad.atan().to_degrees(),
        rms_s2s_deg: rms_s2s_deg(&used_tan),
        sd_deg: sd_deg(&used_tan),
        used_tan,
        used_origin,
        used_t,
        used_pupil,
    })
}

/// Tangent of the direction from `eye_pos` to the target point.
pub fn truth_tangent(target: &Target, eye_pos: [f64; 3]) -> Option<(f64, f64)> {
    let p = target.point();
    tangent_of([p[0] - eye_pos[0], p[1] - eye_pos[1], p[2] - eye_pos[2]])
}

/// Errors of a calibration on one validation window.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowEval {
    /// Per eye: error of the calibrated fixation centre (degrees), `None` when the eye had no fixation.
    pub eye_err_deg: [Option<f64>; 2],
    /// Combined: error of the median calibrated combined gaze over the fixation (degrees).
    pub comb_err_deg: Option<f64>,
    /// Combined: mean over the fixation samples of the per-sample error (degrees; includes the noise).
    pub comb_sample_err_deg: Option<f64>,
    /// Combined precision, RMS sample-to-sample (degrees).
    pub comb_rms_s2s_deg: f64,
    /// Combined precision, SD (degrees).
    pub comb_sd_deg: f64,
    /// Fraction of samples without any combined gaze.
    pub loss: f64,
    /// Per-sample combined errors (for pooled statistics).
    pub sample_errs: Vec<f64>,
}

/// Calibrated centre of one eye on its fixation: the median of the per-sample calibrated tangents (each with its own
/// reported origin and pupil, samples beyond the origin gate dropped); `None` with fewer than 5 samples left.
pub fn eye_center(cal: &pimax_eye::calib::EyeCal, ew: &EyeWindow) -> Option<(f64, f64)> {
    let mut us = Vec::new();
    let mut vs = Vec::new();
    for (i, (tan, org)) in ew.used_tan.iter().zip(&ew.used_origin).enumerate() {
        let pupil = ew.used_pupil.get(i).copied();
        if let Some(t) = cal
            .apply_full(dir_of(tan.0, tan.1), Some(*org), pupil, (0.0, 0.0))
            .and_then(tangent_of)
        {
            us.push(t.0);
            vs.push(t.1);
        }
    }
    if us.len() < 5 {
        return None;
    }
    let med = |mut x: Vec<f64>| {
        x.sort_by(|a, b| a.total_cmp(b));
        let n = x.len();
        if n % 2 == 1 {
            x[n / 2]
        } else {
            0.5 * (x[n / 2 - 1] + x[n / 2])
        }
    };
    Some((med(us), med(vs)))
}

/// Error of one calibrated eye on its fixation against `truth`, degrees (see [`eye_center`]).
pub fn eye_error_deg(cal: &pimax_eye::calib::EyeCal, ew: &EyeWindow, truth: (f64, f64)) -> Option<f64> {
    eye_center(cal, ew).map(|c| tangent_angle_deg(c, truth))
}

/// Evaluate `cal` on a window for `target`.
pub fn evaluate_window(cal: &Calibration, w: &[RawSample], target: &Target, p: &FixParams) -> WindowEval {
    let mut eye_err = [None, None];
    for (e, slot) in eye_err.iter_mut().enumerate() {
        if let (Some(ew), Some(truth)) = (analyze_eye(w, e, p), truth_tangent(target, cal.eyes[e].origin_m)) {
            *slot = eye_error_deg(&cal.eyes[e], &ew, truth);
        }
    }
    // combined, sample by sample, through the same code the service runs
    let mut st = CombineState::default();
    let mut t = Vec::with_capacity(w.len());
    let mut tan: Vec<Option<(f64, f64)>> = Vec::with_capacity(w.len());
    for (i, rs) in w.iter().enumerate() {
        let mut s = rs.to_sample(i as u64);
        cal.apply_sample(&mut s, &mut st);
        t.push(rs.t);
        tan.push(s.gaze.and_then(|g| tangent_of(g.direction.map(f64::from))));
    }
    let loss = if w.is_empty() {
        1.0
    } else {
        tan.iter().filter(|x| x.is_none()).count() as f64 / w.len() as f64
    };
    let mid = [
        0.5 * (cal.eyes[0].origin_m[0] + cal.eyes[1].origin_m[0]),
        0.5 * (cal.eyes[0].origin_m[1] + cal.eyes[1].origin_m[1]),
        0.5 * (cal.eyes[0].origin_m[2] + cal.eyes[1].origin_m[2]),
    ];
    let truth = truth_tangent(target, mid);
    let labels = label(&t, &tan, p);
    let usable: Vec<bool> = labels.iter().map(|l| *l == Label::Fixation).collect();
    let mut ev = WindowEval {
        eye_err_deg: eye_err,
        comb_err_deg: None,
        comb_sample_err_deg: None,
        comb_rms_s2s_deg: 0.0,
        comb_sd_deg: 0.0,
        loss,
        sample_errs: Vec::new(),
    };
    if let (Some(r), Some(truth)) = (longest_fixation(&t, &tan, &usable, p), truth) {
        let used: Vec<(f64, f64)> = r.filter(|&i| usable[i]).filter_map(|i| tan[i]).collect();
        if let Some((c, _)) = median_tangent(&used) {
            ev.comb_err_deg = Some(tangent_angle_deg(c, truth));
            ev.sample_errs = used.iter().map(|&x| tangent_angle_deg(x, truth)).collect();
            ev.comb_sample_err_deg = Some(ev.sample_errs.iter().sum::<f64>() / ev.sample_errs.len() as f64);
            ev.comb_rms_s2s_deg = rms_s2s_deg(&used);
            ev.comb_sd_deg = sd_deg(&used);
        }
    }
    ev
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_eye::rng::Rng;
    use pimax_eye::track::FrameConv;

    fn window(target: &Target, noise_deg: f64, seed: u64, blink: bool) -> Vec<RawSample> {
        let mut r = Rng::new(seed);
        let pos = [[-0.032, 0.0, 0.0], [0.032, 0.0, 0.0]];
        (0..120)
            .map(|i| {
                let t = i as f64 / 120.0;
                let mut eyes = [None, None];
                for e in 0..2 {
                    if blink && (0.5..0.6).contains(&t) {
                        continue;
                    }
                    let (u, v) = truth_tangent(target, pos[e]).unwrap();
                    let s = noise_deg.to_radians();
                    eyes[e] = Some(RawEye {
                        dir: dir_of(u + s * r.gauss(), v + s * r.gauss()),
                        origin: pos[e],
                        pupil_mm: 3.5,
                    });
                }
                RawSample { t, eyes }
            })
            .collect()
    }

    #[test]
    fn perfect_tracker_validates_near_zero() {
        let cal = Calibration::identity(FrameConv::default(), 0.064);
        let t = Target::new(10.0, -5.0);
        let w = window(&t, 0.5, 4, true);
        let ev = evaluate_window(&cal, &w, &t, &FixParams::default());
        for e in ev.eye_err_deg {
            assert!(e.unwrap() < 0.25, "{ev:?}");
        }
        assert!(ev.comb_err_deg.unwrap() < 0.2, "{ev:?}");
        assert!(ev.loss > 0.05 && ev.loss < 0.15, "blink counted as loss: {}", ev.loss);
        assert!(ev.comb_rms_s2s_deg > 0.2 && ev.comb_rms_s2s_deg < 0.9, "{}", ev.comb_rms_s2s_deg);
        let ew = analyze_eye(&w, 0, &FixParams::default()).unwrap();
        assert!(ew.used > 70 && ew.valid < ew.total, "{ew:?}");
        // round trip of the sample conversion
        let s = w[0].to_sample(7);
        assert_eq!(RawSample::from_sample(&s).eyes[0].map(|e| e.pupil_mm), Some(3.5));
    }
}
