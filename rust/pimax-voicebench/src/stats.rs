//! Small statistics helpers for latency tables.

/// Summary of a sample set (any unit).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Summary {
    pub n: usize,
    pub min: f64,
    pub mean: f64,
    pub p50: f64,
    pub p95: f64,
    pub max: f64,
}

/// Linear-interpolated percentile of an ascending-sorted slice; `q` in 0..=1. NaN for an empty slice.
pub fn percentile(sorted: &[f64], q: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let pos = q.clamp(0.0, 1.0) * (sorted.len() - 1) as f64;
    let (lo, hi) = (pos.floor() as usize, pos.ceil() as usize);
    sorted[lo] + (sorted[hi] - sorted[lo]) * (pos - lo as f64)
}

/// Summarises `values`; `None` when empty.
pub fn summarize(values: &[f64]) -> Option<Summary> {
    if values.is_empty() {
        return None;
    }
    let mut v = values.to_vec();
    v.sort_by(|a, b| a.total_cmp(b));
    Some(Summary {
        n: v.len(),
        min: v[0],
        mean: v.iter().sum::<f64>() / v.len() as f64,
        p50: percentile(&v, 0.5),
        p95: percentile(&v, 0.95),
        max: *v.last().unwrap(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_of_known_values() {
        let s = summarize(&[5.0, 1.0, 3.0, 2.0, 4.0]).unwrap();
        assert_eq!((s.n, s.min, s.max), (5, 1.0, 5.0));
        assert!((s.mean - 3.0).abs() < 1e-12);
        assert!((s.p50 - 3.0).abs() < 1e-12);
        assert!((s.p95 - 4.8).abs() < 1e-12);
    }

    #[test]
    fn empty_and_single() {
        assert!(summarize(&[]).is_none());
        let s = summarize(&[2.5]).unwrap();
        assert_eq!((s.min, s.p50, s.p95, s.max), (2.5, 2.5, 2.5, 2.5));
        assert!(percentile(&[], 0.5).is_nan());
        assert_eq!(percentile(&[1.0, 2.0], 5.0), 2.0);
    }
}
