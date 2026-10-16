//! Picking the fixation samples out of a target window: blinks and their edges, saccades, and the steadiest
//! stretch of fixation.
//!
//! The raw 120 Hz gaze of this tracker has about 0.7 degrees of noise per sample and axis, which makes a plain
//! sample-to-sample velocity useless (noise alone gives ~100 deg/s). The velocity is therefore taken on a running
//! median (7 samples, ±25 ms: it keeps the step of a saccade sharp and removes the noise) over a ±2 sample baseline,
//! with a 50 deg/s threshold: target jumps of 5 degrees and more (peak velocity > 150 deg/s) are found, the noise is
//! not. A fixation is a run of non-saccade, non-blink samples whose smoothed track stays within `max_dispersion_deg`.
//!
//! Works offline on a whole window (it may look both ways in time): the samples just before a blink are corrupted by
//! the closing lid as much as those just after it.

/// Settings.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FixParams {
    /// Faster than this (on the smoothed track) is a saccade, degrees per second.
    pub velocity_dps: f64,
    /// Samples this close in time to a blink are dropped (seconds).
    pub blink_edge_s: f64,
    /// A gap of invalid samples this long (from the last valid sample to the next) is a blink, shorter ones are
    /// dropped frames (seconds).
    pub min_blink_s: f64,
    /// Unusable stretches up to this long do not end a fixation (seconds).
    pub max_gap_s: f64,
    /// Samples next to a saccade sample are dropped too (count on each side).
    pub saccade_pad: usize,
    /// Largest spread of the smoothed track during a fixation (degrees, max over u and v of the range).
    pub max_dispersion_deg: f64,
    /// A fixation must last this long (seconds).
    pub min_duration_s: f64,
}

impl Default for FixParams {
    fn default() -> Self {
        FixParams {
            velocity_dps: 50.0,
            blink_edge_s: 0.06,
            min_blink_s: 0.05,
            max_gap_s: 0.3,
            saccade_pad: 2,
            max_dispersion_deg: 2.0,
            min_duration_s: 0.25,
        }
    }
}

/// What a sample is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Label {
    /// No valid direction.
    Invalid,
    /// Valid but next to a blink / lost frame.
    BlinkEdge,
    /// Part of (or next to) a saccade.
    Saccade,
    /// Usable.
    Fixation,
}

fn ang(a: (f64, f64), b: (f64, f64)) -> f64 {
    crate::calib::tangent_angle_deg(a, b)
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

/// Running median (half width `h` samples) over the valid samples; `None` where the sample itself is invalid.
pub fn smooth(tan: &[Option<(f64, f64)>], h: usize) -> Vec<Option<(f64, f64)>> {
    let n = tan.len();
    (0..n)
        .map(|i| {
            tan[i]?;
            let lo = i.saturating_sub(h);
            let hi = (i + h + 1).min(n);
            let w: Vec<(f64, f64)> = tan[lo..hi].iter().flatten().copied().collect();
            Some((median(w.iter().map(|p| p.0).collect()), median(w.iter().map(|p| p.1).collect())))
        })
        .collect()
}

/// Label every sample of one eye: `t` seconds, `tan` the gaze tangent (`None` = invalid).
pub fn label(t: &[f64], tan: &[Option<(f64, f64)>], p: &FixParams) -> Vec<Label> {
    let n = t.len().min(tan.len());
    let tan = &tan[..n];
    let mut out: Vec<Label> = (0..n)
        .map(|i| if tan[i].is_some() { Label::Fixation } else { Label::Invalid })
        .collect();
    // blinks: runs of invalid samples lasting at least `min_blink_s`; their neighbours within `blink_edge_s` are
    // corrupted by the lid. A single dropped frame is just a missing sample.
    let mut blinks: Vec<(f64, f64)> = Vec::new();
    let mut i = 0;
    while i < n {
        if tan[i].is_some() {
            i += 1;
            continue;
        }
        let a = i;
        while i < n && tan[i].is_none() {
            i += 1;
        }
        let (ta, tb) = (t[a], t[i - 1]);
        let before = if a > 0 { t[a - 1] } else { ta };
        let after = if i < n { t[i] } else { tb };
        if after - before >= p.min_blink_s {
            blinks.push((ta, tb));
        }
    }
    for j in 0..n {
        if out[j] == Label::Fixation
            && blinks
                .iter()
                .any(|&(a, b)| t[j] >= a - p.blink_edge_s && t[j] <= b + p.blink_edge_s)
        {
            out[j] = Label::BlinkEdge;
        }
    }
    let sm = smooth(tan, 3);
    let fast: Vec<bool> = (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(2), (i + 2).min(n.saturating_sub(1)));
            match (sm[a], sm[b]) {
                (Some(x), Some(y)) => {
                    let dt = t[b] - t[a];
                    dt > 1e-6 && ang(x, y) / dt > p.velocity_dps
                }
                _ => false,
            }
        })
        .collect();
    for (i, &f) in fast.iter().enumerate() {
        if f {
            let lo = i.saturating_sub(p.saccade_pad);
            let hi = (i + p.saccade_pad).min(n.saturating_sub(1));
            for o in out.iter_mut().take(hi + 1).skip(lo) {
                if *o == Label::Fixation {
                    *o = Label::Saccade;
                }
            }
        }
    }
    out
}

/// The longest (in time) run of `usable` samples whose smoothed track stays within `max_dispersion_deg`; `None` when
/// nothing lasts `min_duration_s`. Indices into the inputs.
pub fn longest_fixation(
    t: &[f64],
    tan: &[Option<(f64, f64)>],
    usable: &[bool],
    p: &FixParams,
) -> Option<std::ops::Range<usize>> {
    let n = t.len().min(tan.len()).min(usable.len());
    let sm = smooth(&tan[..n], 3);
    let disp = p.max_dispersion_deg.to_radians().tan();
    let mut best: Option<(std::ops::Range<usize>, f64)> = None;
    let mut start = 0;
    while start < n {
        if !usable[start] || sm[start].is_none() {
            start += 1;
            continue;
        }
        let (mut lo_u, mut hi_u, mut lo_v, mut hi_v) = (f64::MAX, f64::MIN, f64::MAX, f64::MIN);
        let mut end = start;
        // invalid or unusable samples inside are skipped (not counted) as long as the gap is short
        let mut last_good = start;
        while end < n {
            if !usable[end] || sm[end].is_none() {
                if t[end] - t[last_good] > p.max_gap_s {
                    break;
                }
                end += 1;
                continue;
            }
            let (u, v) = sm[end].unwrap_or((0.0, 0.0));
            let (nlu, nhu, nlv, nhv) = (lo_u.min(u), hi_u.max(u), lo_v.min(v), hi_v.max(v));
            if nhu - nlu > disp || nhv - nlv > disp {
                break;
            }
            (lo_u, hi_u, lo_v, hi_v) = (nlu, nhu, nlv, nhv);
            last_good = end;
            end += 1;
        }
        let dur = t[last_good] - t[start];
        if dur >= p.min_duration_s && best.as_ref().is_none_or(|b| dur > b.1) {
            best = Some((start..last_good + 1, dur));
        }
        start = last_good + 1;
    }
    best.map(|b| b.0)
}

/// Component-wise median of tangents and the median absolute deviation (radius, tangent units).
pub fn median_tangent(v: &[(f64, f64)]) -> Option<((f64, f64), f64)> {
    if v.is_empty() {
        return None;
    }
    let m = (
        median(v.iter().map(|p| p.0).collect()),
        median(v.iter().map(|p| p.1).collect()),
    );
    let mad = median(v.iter().map(|p| ((p.0 - m.0).powi(2) + (p.1 - m.1).powi(2)).sqrt()).collect());
    Some((m, mad))
}

/// RMS of the angle between consecutive samples (degrees): the usual precision figure (RMS-S2S).
pub fn rms_s2s_deg(v: &[(f64, f64)]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let ss: f64 = v.windows(2).map(|w| ang(w[0], w[1]).powi(2)).sum();
    (ss / (v.len() - 1) as f64).sqrt()
}

/// Standard deviation of the angle from the mean direction (degrees): precision as SD.
pub fn sd_deg(v: &[(f64, f64)]) -> f64 {
    if v.len() < 2 {
        return 0.0;
    }
    let m = (
        v.iter().map(|p| p.0).sum::<f64>() / v.len() as f64,
        v.iter().map(|p| p.1).sum::<f64>() / v.len() as f64,
    );
    (v.iter().map(|p| ang(*p, m).powi(2)).sum::<f64>() / (v.len() - 1) as f64).sqrt()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rng::Rng;

    /// 1.2 s at 120 Hz: a 10 degree saccade in the first 40 ms, fixation with 0.7 deg noise per axis (what this
    /// tracker gives), a blink at 0.6..0.7 s.
    fn window(seed: u64) -> (Vec<f64>, Vec<Option<(f64, f64)>>) {
        let mut r = Rng::new(seed);
        let n = 144;
        let t: Vec<f64> = (0..n).map(|i| i as f64 / 120.0).collect();
        let tgt = (10f64.to_radians().tan(), 0.0);
        let tan = t
            .iter()
            .map(|&ti| {
                if (0.6..0.7).contains(&ti) {
                    return None;
                }
                let s = 0.7f64.to_radians();
                let base = if ti < 0.04 { (tgt.0 * ti / 0.04, 0.0) } else { tgt };
                Some((base.0 + s * r.gauss(), base.1 + s * r.gauss()))
            })
            .collect();
        (t, tan)
    }

    #[test]
    fn labels_find_the_saccade_and_the_blink_edges_not_the_noise() {
        for seed in 1..6 {
            let (t, tan) = window(seed);
            let l = label(&t, &tan, &FixParams::default());
            assert!(l[..4].contains(&Label::Saccade), "seed {seed}: {:?}", &l[..10]);
            let i_blink = t.iter().position(|&x| x >= 0.6).unwrap();
            assert_eq!(l[i_blink], Label::Invalid);
            assert_eq!(l[i_blink - 1], Label::BlinkEdge);
            let after = t.iter().position(|&x| x >= 0.7).unwrap();
            assert_eq!(l[after], Label::BlinkEdge);
            // fixation samples outside the saccade and the blink are kept, the noise is not a saccade
            let late: Vec<&Label> = l[20..i_blink - 8].iter().collect();
            let kept = late.iter().filter(|x| ***x == Label::Fixation).count();
            assert!(kept as f64 > 0.9 * late.len() as f64, "seed {seed}: {kept} of {}", late.len());
        }
    }

    #[test]
    fn longest_fixation_is_the_steady_stretch() {
        let (t, tan) = window(3);
        let l = label(&t, &tan, &FixParams::default());
        let usable: Vec<bool> = l.iter().map(|x| *x == Label::Fixation).collect();
        let r = longest_fixation(&t, &tan, &usable, &FixParams::default()).unwrap();
        assert!(t[r.start] > 0.03 && t[r.start] < 0.15, "{}", t[r.start]);
        assert!(t[r.end - 1] - t[r.start] > 0.4, "{:?}", r);
        let vals: Vec<(f64, f64)> = r.clone().filter(|&i| usable[i]).filter_map(|i| tan[i]).collect();
        let ((u, v), mad) = median_tangent(&vals).unwrap();
        assert!((u.atan().to_degrees() - 10.0).abs() < 0.3 && v.atan().to_degrees().abs() < 0.3);
        assert!(mad.atan().to_degrees() < 1.2);
        let p = rms_s2s_deg(&vals);
        assert!(p > 0.6 && p < 1.6, "{p}");
        assert!(sd_deg(&vals) > 0.6);
        let short = FixParams {
            min_duration_s: 5.0,
            ..FixParams::default()
        };
        assert!(longest_fixation(&t, &tan, &usable, &short).is_none());
    }

    #[test]
    fn two_fixations_pick_the_longer_one() {
        // 0.3 s at the old place, saccade, 0.8 s at the new one
        let mut r = Rng::new(11);
        let t: Vec<f64> = (0..132).map(|i| i as f64 / 120.0).collect();
        let tan: Vec<Option<(f64, f64)>> = t
            .iter()
            .map(|&ti| {
                let base = if ti < 0.3 { (0.0, 0.0) } else { (0.0, 0.15) };
                Some((base.0 + 0.005 * r.gauss(), base.1 + 0.005 * r.gauss()))
            })
            .collect();
        let l = label(&t, &tan, &FixParams::default());
        let usable: Vec<bool> = l.iter().map(|x| *x == Label::Fixation).collect();
        let f = longest_fixation(&t, &tan, &usable, &FixParams::default()).unwrap();
        assert!(t[f.start] > 0.3, "{}", t[f.start]);
    }
}
