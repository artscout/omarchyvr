//! A simulated wearer and uncalibrated tracker, to run the whole procedure without a headset (`--simulate`, tests).
//!
//! The wearer looks at the first mark of the scene (the target) after a reaction time, with a 40 ms saccade, small
//! fixation drift and blinks. The "tracker" reports each eye with what the uncalibrated EyeChip shows on Linux: a
//! gain of about 0.6, per-eye offsets of several degrees (the eyes seem to diverge by ~16 degrees), a compressed
//! upper field, 0.7 degrees of noise per axis and sample, dropped samples, and the eyelid artefact at blink edges.

use crate::collect::{RawEye, RawSample};
use crate::procedure::{Cmd, Effect, Phase, Procedure};
use pimax_eye::correction::{dir_of, tangent_of};
use pimax_eye::rng::Rng;

/// Simulation settings.
#[derive(Debug, Clone)]
pub struct SimParams {
    /// Noise per axis and sample, degrees.
    pub noise_deg: f64,
    /// Probability that a sample of one eye is dropped.
    pub drop: f64,
    /// Mean time between blinks, seconds.
    pub blink_every_s: f64,
    /// Reaction time to a new target, seconds.
    pub latency_s: f64,
    /// Seed.
    pub seed: u64,
    /// Head turn (degrees) at this time (seconds), once: tests the head-motion check.
    pub head_turn: Option<(f64, f64)>,
    /// The wearer looks this many degrees beside one calibration target (index): tests the outlier handling.
    pub glance: Option<(usize, f64)>,
}

impl Default for SimParams {
    fn default() -> Self {
        SimParams {
            noise_deg: 0.7,
            drop: 0.03,
            blink_every_s: 4.0,
            latency_s: 0.22,
            seed: 1,
            head_turn: None,
            glance: None,
        }
    }
}

/// What the uncalibrated chip makes of a true per-eye tangent.
pub fn distort(eye: usize, t: (f64, f64)) -> (f64, f64) {
    let (u, v) = t;
    let off = if eye == 0 { (0.13, -0.17) } else { (-0.12, -0.18) };
    (
        off.0 + 0.62 * u + 0.04 * v,
        off.1 + 0.03 * u + 0.62 * v - 0.6 * v.max(0.0) * v,
    )
}

/// Result of a simulated run.
#[derive(Debug, Clone)]
pub struct SimOutcome {
    /// Everything the procedure printed.
    pub log: Vec<String>,
    /// The procedure at the end.
    pub procedure: Procedure,
    /// The calibration file text, if it was saved.
    pub saved: Option<String>,
}

/// Run a full calibration + validation (+ pursuit if `proc.s.pursuit`) with a simulated wearer; presses A at the
/// start and saves at the end.
pub fn run(mut proc: Procedure, p: &SimParams) -> SimOutcome {
    let mut rng = Rng::new(p.seed);
    let mut log = Vec::new();
    let mut saved = None;
    let rate = 120.0;
    let frame_dt = 1.0 / 72.0;
    let mut t = 0.0f64;
    let mut next_sample = 0.0f64;
    let mut gaze_target: Option<[f64; 3]> = None; // where the eyes are now (head-frame point)
    let mut pending: Option<([f64; 3], f64)> = None; // new point, time it was seen
    let mut saccade: Option<([f64; 3], [f64; 3], f64)> = None;
    let mut next_blink = p.blink_every_s * (0.5 + rng.uniform());
    let mut blink_until = -1.0;
    let mut turned = false;
    let mut head_yaw = 0.0f64;
    let mut started = false;
    let eye_pos = [[-0.032, 0.0, 0.0], [0.032, 0.0, 0.0]];
    let handle = |fx: Vec<Effect>, log: &mut Vec<String>, saved: &mut Option<String>| {
        for e in fx {
            match e {
                Effect::Print(s) => log.push(s),
                Effect::SaveCalibration(s) => *saved = Some(s),
                _ => {}
            }
        }
    };
    while t < 400.0 {
        t += frame_dt;
        // the wearer's target: the first mark of the scene, plus the planned glance error
        let scene = proc.scene();
        let mut want = scene.marks.first().map(|m| m.pos);
        if let (Some((k, deg)), Phase::Point { set: crate::procedure::Set::Calibration, i, .. }) = (p.glance, proc.phase) {
            if i == k {
                if let Some(w) = want.as_mut() {
                    let (u, v) = tangent_of(*w).unwrap_or((0.0, 0.0));
                    let d = dir_of(u + deg.to_radians().tan(), v);
                    *w = [d[0] * 2.0, d[1] * 2.0, d[2] * 2.0];
                }
            }
        }
        if let Some(w) = want {
            let same = gaze_target.is_some_and(|g| dist(g, w) < 1e-6);
            let pend = pending.is_some_and(|(g, _)| dist(g, w) < 1e-6);
            if !same && !pend {
                pending = Some((w, t));
            }
        }
        if let Some((w, seen)) = pending {
            if t - seen >= p.latency_s {
                saccade = Some((gaze_target.unwrap_or(w), w, t));
                pending = None;
            }
        }
        // samples since the last frame
        let mut batch = Vec::new();
        while next_sample <= t {
            let ts = next_sample;
            next_sample += 1.0 / rate;
            let point = match saccade {
                Some((a, b, t0)) if ts - t0 < 0.04 => {
                    let f = ((ts - t0) / 0.04).clamp(0.0, 1.0);
                    [a[0] + (b[0] - a[0]) * f, a[1] + (b[1] - a[1]) * f, a[2] + (b[2] - a[2]) * f]
                }
                Some((_, b, _)) => {
                    gaze_target = Some(b);
                    b
                }
                None => gaze_target.unwrap_or([0.0, 0.0, -2.0]),
            };
            if ts >= next_blink {
                blink_until = ts + 0.12;
                next_blink = ts + p.blink_every_s * (0.5 + rng.uniform());
            }
            let blinking = ts < blink_until;
            let edge = !blinking && (ts - blink_until).abs() < 0.03;
            let mut eyes = [None, None];
            for (e, slot) in eyes.iter_mut().enumerate() {
                if blinking || rng.uniform() < p.drop {
                    continue;
                }
                let o = eye_pos[e];
                let Some(tt) = tangent_of([point[0] - o[0], point[1] - o[1], point[2] - o[2]]) else {
                    continue;
                };
                let (mut u, mut v) = distort(e, tt);
                let s = p.noise_deg.to_radians();
                u += s * rng.gauss();
                v += s * rng.gauss();
                if edge {
                    v -= 3f64.to_radians(); // the lid pulls the pupil estimate down
                }
                *slot = Some(RawEye {
                    dir: dir_of(u, v),
                    origin: [o[0], o[1], o[2] + 0.02],
                    pupil_mm: 3.5,
                });
            }
            batch.push(RawSample { t: ts, eyes });
        }
        if let Some((at, deg)) = p.head_turn {
            if !turned && t >= at {
                turned = true;
                head_yaw = deg;
            }
        }
        let half = head_yaw.to_radians() * 0.5;
        let head = [0.0, half.sin(), 0.0, half.cos()];
        let fx = proc.frame(t, Some(head), eye_pos, &batch, None);
        handle(fx, &mut log, &mut saved);
        if !started && t > 0.5 {
            started = true;
            let fx = proc.command(Cmd::Next);
            handle(fx, &mut log, &mut saved);
        }
        if started && matches!(proc.phase, Phase::Results) && t > 2.0 {
            let fx = proc.command(Cmd::Accept);
            handle(fx, &mut log, &mut saved);
            break;
        }
        if matches!(proc.phase, Phase::Intro) && started && proc.calibration.is_none() && t > 1.0 {
            // restarted (axis fix): start again
            let fx = proc.command(Cmd::Next);
            handle(fx, &mut log, &mut saved);
        }
    }
    SimOutcome {
        log,
        procedure: proc,
        saved,
    }
}

fn dist(a: [f64; 3], b: [f64; 3]) -> f64 {
    ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procedure::Settings;
    use crate::report::summary;
    use pimax_eye::calib::Calibration;

    #[test]
    fn full_run_calibrates_the_simulated_chip_to_about_a_degree() {
        let out = run(Procedure::new(Settings::default()), &SimParams::default());
        let p = &out.procedure;
        let s = summary(p);
        assert_eq!(p.val_res.len(), 10, "{:#?}", out.log);
        assert!(s.raw.mean > 6.0, "the raw simulated chip is bad: {:?}", s.raw);
        assert!(s.accuracy.mean < 1.0, "validation {:?}\n{}", s.accuracy, out.log.join("\n"));
        let text = out.saved.expect("saved");
        let cal = Calibration::parse(&text).unwrap();
        assert!(cal.conv.flip_x, "the conversion of the run is stored");
        assert!(text.contains("# validated"));
    }

    #[test]
    fn a_head_turn_restarts_the_target_and_a_glance_is_remeasured() {
        let sp = SimParams {
            head_turn: Some((5.0, 6.0)),
            glance: Some((3, 7.0)),
            seed: 9,
            ..SimParams::default()
        };
        let out = run(Procedure::new(Settings::default()), &sp);
        let p = &out.procedure;
        assert!(p.head_restarts >= 1, "{}", out.log.join("\n"));
        assert!(
            out.log.iter().any(|l| l.contains("measuring it again")),
            "the 7 degree glance must be caught: {}",
            out.log.join("\n")
        );
        assert!(summary(p).accuracy.mean < 1.3, "{:?}", summary(p).accuracy);
    }

    #[test]
    fn a_mirrored_x_axis_is_detected_and_fixed() {
        // the tracker of this run uses the wrong FLIP_X: the simulated chip's x comes out mirrored, which the
        // simulation models by mirroring the true tangent
        let mut s = Settings::default();
        s.conv.flip_x = false;
        let proc = Procedure::new(s);
        let out = run_mirrored(proc);
        assert!(out.log.iter().any(|l| l.contains("restarting the tracker")), "{}", out.log.join("\n"));
    }

    fn run_mirrored(proc: Procedure) -> SimOutcome {
        // drive the calibration part with x-mirrored raw samples and stop at the first restart request
        let mut proc = proc;
        let mut log = Vec::new();
        let mut rng = Rng::new(3);
        let mut t = 0.0;
        let mut started = false;
        let eye_pos = [[-0.032, 0.0, 0.0], [0.032, 0.0, 0.0]];
        let mut last_target = [0.0, 0.0, -2.0];
        let mut since = 0.0;
        while t < 120.0 {
            t += 1.0 / 72.0;
            let want = proc.scene().marks.first().map_or([0.0, 0.0, -2.0], |m| m.pos);
            if dist(want, last_target) > 1e-6 {
                last_target = want;
                since = t;
            }
            let mut batch = Vec::new();
            for k in 0..2 {
                let ts = t - k as f64 / 120.0;
                let mut eyes = [None, None];
                for (e, slot) in eyes.iter_mut().enumerate() {
                    let o = eye_pos[1 - e];
                    let p = if t - since < 0.2 { [0.0, 0.0, -2.0] } else { want };
                    let (u, v) = tangent_of([p[0] - o[0], p[1] - o[1], p[2] - o[2]]).unwrap();
                    let (du, dv) = distort(e, (u, v));
                    let s = 0.3f64.to_radians();
                    *slot = Some(RawEye {
                        dir: dir_of(-du + s * rng.gauss(), dv + s * rng.gauss()),
                        origin: o,
                        pupil_mm: 3.0,
                    });
                }
                batch.push(RawSample { t: ts, eyes });
            }
            batch.sort_by(|a, b| a.t.total_cmp(&b.t));
            for e in proc.frame(t, Some([0.0, 0.0, 0.0, 1.0]), eye_pos, &batch, None) {
                if let Effect::Print(s) = e {
                    log.push(s);
                }
            }
            if !started && t > 0.5 {
                started = true;
                proc.command(Cmd::Next);
            }
            if log.iter().any(|l| l.contains("restarting the tracker")) {
                break;
            }
        }
        SimOutcome {
            log,
            procedure: proc,
            saved: None,
        }
    }
}
