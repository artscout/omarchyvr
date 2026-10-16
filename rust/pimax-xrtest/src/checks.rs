//! Pass/fail rules of `xr-handtest`.

use crate::joints::{expected_flexion, Finger, INDEX_FLEXION_PER_CURL, OTHER_FLEXION_PER_CURL};
use pimax_hands::demo::demo_input;
use pimax_hands::{HandState, Params};

/// Limits of the checks (defaults are what the Monado driver is expected to meet).
#[derive(Clone, Copy, Debug)]
pub struct Tolerances {
    /// Wrist joint vs grip pose * grip->wrist offset, metres.
    pub grip_pos_m: f64,
    /// Same, radians.
    pub grip_ang_rad: f64,
    /// Demo pattern: allowed flexion error of a finger, in curl units (x the flexion per curl).
    pub demo_curl: f64,
    /// Demo pattern: allowed flexion error of the thumb, radians (its flexion is not linear in curl).
    pub demo_thumb_rad: f64,
}

impl Default for Tolerances {
    fn default() -> Self {
        Self {
            grip_pos_m: 0.003,
            grip_ang_rad: 2.0_f64.to_radians(),
            demo_curl: 0.12,
            demo_thumb_rad: 0.5,
        }
    }
}

/// Running maximum of a non-negative error.
#[derive(Clone, Copy, Debug, Default)]
pub struct MaxErr {
    pub max: f64,
    pub count: usize,
}

impl MaxErr {
    pub fn add(&mut self, e: f64) {
        self.max = self.max.max(e);
        self.count += 1;
    }
}

/// One flexion measurement of a hand at monotonic time `t_ns` (`Finger::ALL` order).
#[derive(Clone, Copy, Debug)]
pub struct DemoSample {
    pub t_ns: u64,
    pub flexion: [f64; 5],
}

#[derive(Clone, Debug, Default)]
pub struct DemoReport {
    pub samples: usize,
    /// Largest |measured - expected| flexion per finger, radians.
    pub max_err: [f64; 5],
    /// Measured and expected flexion range (max - min) per finger.
    pub measured_range: [f64; 5],
    pub expected_range: [f64; 5],
    pub failures: Vec<String>,
}

impl DemoReport {
    pub fn ok(&self) -> bool {
        self.failures.is_empty()
    }
}

fn tolerance_rad(f: Finger, t: &Tolerances) -> f64 {
    match f {
        Finger::Thumb => t.demo_thumb_rad,
        Finger::Index => t.demo_curl * INDEX_FLEXION_PER_CURL,
        _ => t.demo_curl * OTHER_FLEXION_PER_CURL,
    }
}

/// Compare what the hand did with what the scripted controller (`PIMAX_CTRL_HANDS_DEMO=1`) told it to do: the
/// expected curls come from running the same Rust state machine over the same inputs at the sample times.
///
/// The driver and this client stamp their samples with clocks a fraction of a millisecond apart, so a scripted step
/// (a touch starting, a press) can land one sample earlier on one side. A sample therefore passes if it matches the
/// expectation of the sample before, at or after it.
pub fn check_demo(samples: &[DemoSample], right: bool, tol: &Tolerances) -> DemoReport {
    let mut rep = DemoReport {
        samples: samples.len(),
        ..DemoReport::default()
    };
    if samples.len() < 8 {
        rep.failures.push(format!("only {} samples", samples.len()));
        return rep;
    }
    let mut state = HandState::new(Params::default());
    let expected: Vec<[f64; 5]> = samples
        .iter()
        .map(|s| {
            let curls = state.update(s.t_ns, &demo_input(s.t_ns, right)).to_array();
            let mut e = [0.0; 5];
            for (k, f) in Finger::ALL.iter().enumerate() {
                e[k] = expected_flexion(*f, f64::from(curls[k]));
            }
            e
        })
        .collect();
    let mut lo_m = [f64::MAX; 5];
    let mut hi_m = [f64::MIN; 5];
    let mut lo_e = [f64::MAX; 5];
    let mut hi_e = [f64::MIN; 5];
    for (i, s) in samples.iter().enumerate() {
        for k in 0..5 {
            let from = i.saturating_sub(1);
            let to = (i + 1).min(samples.len() - 1);
            let err = (from..=to)
                .map(|j| (s.flexion[k] - expected[j][k]).abs())
                .fold(f64::MAX, f64::min);
            rep.max_err[k] = rep.max_err[k].max(err);
            lo_m[k] = lo_m[k].min(s.flexion[k]);
            hi_m[k] = hi_m[k].max(s.flexion[k]);
            lo_e[k] = lo_e[k].min(expected[i][k]);
            hi_e[k] = hi_e[k].max(expected[i][k]);
        }
    }
    for (k, f) in Finger::ALL.iter().enumerate() {
        rep.measured_range[k] = hi_m[k] - lo_m[k];
        rep.expected_range[k] = hi_e[k] - lo_e[k];
        if rep.max_err[k] > tolerance_rad(*f, tol) {
            rep.failures.push(format!(
                "{}: flexion error {:.2} rad > {:.2} rad",
                f.name(),
                rep.max_err[k],
                tolerance_rad(*f, tol)
            ));
        }
        // A frozen hand would pass the error check while the script is idle; demand the motion happened.
        if rep.expected_range[k] > 0.5 && rep.measured_range[k] < 0.5 * rep.expected_range[k] {
            rep.failures.push(format!(
                "{}: moved {:.2} rad, the script moves it {:.2} rad",
                f.name(),
                rep.measured_range[k],
                rep.expected_range[k]
            ));
        }
    }
    rep
}

#[cfg(test)]
mod tests {
    use super::*;

    const MS: u64 = 1_000_000;

    /// What a perfect driver returns: run the same state machine and bend the fingers by `expected_flexion`.
    fn perfect(
        right: bool,
        secs: u64,
        step_ms: u64,
        start_ms: u64,
        wrong: Option<usize>,
    ) -> Vec<DemoSample> {
        let mut st = HandState::new(Params::default());
        let mut out = vec![];
        let mut t = start_ms * MS;
        let end = t + secs * 1000 * MS;
        while t < end {
            let c = st.update(t, &demo_input(t, right)).to_array();
            let mut fl = [0.0; 5];
            for (k, f) in Finger::ALL.iter().enumerate() {
                fl[k] = expected_flexion(*f, f64::from(c[k]));
            }
            if let Some(w) = wrong {
                fl[w] += 1.0; // a finger bent by the wrong amount
            }
            out.push(DemoSample {
                t_ns: t,
                flexion: fl,
            });
            t += step_ms * MS;
        }
        out
    }

    #[test]
    fn perfect_driver_passes() {
        for right in [false, true] {
            let r = check_demo(
                &perfect(right, 24, 50, 123_456, None),
                right,
                &Tolerances::default(),
            );
            assert!(r.ok(), "{:?}", r.failures);
            assert!(r.max_err.iter().all(|e| *e < 1e-6));
            assert!(
                r.expected_range[3] > 2.0,
                "index moves in the script: {:?}",
                r.expected_range
            );
        }
    }

    #[test]
    fn a_one_sample_timing_skew_is_tolerated() {
        // the measured series lags or leads the expected one by one sample (clock offset between driver and client)
        let perfect = perfect(false, 24, 50, 77_000, None);
        for shift in [-1isize, 1] {
            let mut s = perfect.clone();
            for i in 0..s.len() {
                let j = (i as isize + shift).clamp(0, s.len() as isize - 1) as usize;
                s[i].flexion = perfect[j].flexion;
            }
            let r = check_demo(&s, false, &Tolerances::default());
            assert!(r.ok(), "shift {shift}: {:?}", r.failures);
        }
    }

    #[test]
    fn swapped_hands_are_detected() {
        let left_run = perfect(false, 24, 50, 77_000, None);
        let r = check_demo(&left_run, true, &Tolerances::default());
        assert!(!r.ok(), "left data accepted as right");
    }

    #[test]
    fn one_wrong_finger_is_detected() {
        for w in 0..5 {
            let r = check_demo(
                &perfect(false, 24, 50, 5_000, Some(w)),
                false,
                &Tolerances::default(),
            );
            assert!(!r.ok(), "finger {w} off by 1 rad accepted");
        }
    }

    #[test]
    fn frozen_hand_is_detected() {
        let mut s = perfect(false, 24, 50, 5_000, None);
        let first = s[0].flexion;
        for x in &mut s {
            x.flexion = first;
        }
        let r = check_demo(&s, false, &Tolerances::default());
        assert!(!r.ok());
        assert!(
            r.failures.iter().any(|f| f.contains("moved")),
            "{:?}",
            r.failures
        );
    }

    #[test]
    fn too_few_samples_fail() {
        let r = check_demo(
            &perfect(false, 1, 400, 0, None),
            false,
            &Tolerances::default(),
        );
        assert!(!r.ok());
    }

    #[test]
    fn small_measurement_noise_is_tolerated() {
        let mut s = perfect(true, 24, 50, 9_000, None);
        let mut x = 12345u32;
        for smp in &mut s {
            for v in &mut smp.flexion {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                *v += (f64::from(x) / f64::from(u32::MAX) - 0.5) * 0.1;
            }
        }
        assert!(check_demo(&s, true, &Tolerances::default()).ok());
    }

    #[test]
    fn max_err_tracks_the_maximum() {
        let mut m = MaxErr::default();
        for e in [0.1, 0.5, 0.2] {
            m.add(e);
        }
        assert_eq!((m.max, m.count), (0.5, 3));
    }
}
