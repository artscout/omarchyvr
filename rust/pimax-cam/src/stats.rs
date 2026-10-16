//! Capture statistics for the CLI and live tests: frame intervals, rates per frame kind, transfer latency
//! (host arrival minus device time, relative to its minimum) and the exposure/gain/row-offset sets seen.

use crate::camera::FrameInfo;
use crate::frame::FrameKind;
use std::collections::BTreeMap;

/// Simple running summary of a series.
#[derive(Clone, Debug, Default)]
pub struct Series {
    v: Vec<f64>,
}

impl Series {
    /// Add a sample.
    pub fn push(&mut self, x: f64) {
        self.v.push(x);
    }
    /// Count.
    pub fn len(&self) -> usize {
        self.v.len()
    }
    /// Empty?
    pub fn is_empty(&self) -> bool {
        self.v.is_empty()
    }
    /// Mean.
    pub fn mean(&self) -> f64 {
        self.v.iter().sum::<f64>() / self.v.len().max(1) as f64
    }
    /// Standard deviation.
    pub fn std(&self) -> f64 {
        let m = self.mean();
        (self.v.iter().map(|x| (x - m).powi(2)).sum::<f64>() / self.v.len().max(1) as f64).sqrt()
    }
    /// Minimum.
    pub fn min(&self) -> f64 {
        self.v.iter().copied().fold(f64::INFINITY, f64::min)
    }
    /// Maximum.
    pub fn max(&self) -> f64 {
        self.v.iter().copied().fold(f64::NEG_INFINITY, f64::max)
    }
    /// Quantile 0..1.
    pub fn quantile(&self, q: f64) -> f64 {
        if self.v.is_empty() {
            return f64::NAN;
        }
        let mut s = self.v.clone();
        s.sort_by(|a, b| a.partial_cmp(b).unwrap());
        s[((s.len() - 1) as f64 * q).round() as usize]
    }
    /// "mean ± std [min..max]".
    pub fn fmt(&self, unit: &str) -> String {
        format!(
            "{:.3} ± {:.3} [{:.3} .. {:.3}] {unit} (n={})",
            self.mean(),
            self.std(),
            self.min(),
            self.max(),
            self.len()
        )
    }
}

/// (kind, exposure, gain, tile rows) of a frame.
pub type SettingsKey = (u32, [u16; 4], [u16; 4], [u16; 4]);

/// Accumulates [`FrameInfo`]s.
#[derive(Clone, Debug, Default)]
pub struct CaptureStats {
    /// All frame intervals by host time (ms).
    pub host_dt_ms: Series,
    /// All frame intervals by device time (ms).
    pub dev_dt_ms: Series,
    /// Intervals between tracking frames by device time (ms).
    pub tracking_dt_ms: Series,
    /// Host arrival - device time, minus the minimum seen (ms): transfer latency jitter.
    pub latency_ms: Series,
    /// Mapped time - host time (ms): how much earlier than arrival the frame is placed (fallback clock).
    pub mono_minus_host_ms: Series,
    /// Counts of (kind, exposure, gain, y_offset) combinations.
    pub settings: BTreeMap<SettingsKey, u64>,
    /// Frames.
    pub frames: u64,
    /// Lost frames (counter gaps).
    pub dropped: u64,
    first_host: Option<u64>,
    last_host: u64,
    last: Option<FrameInfo>,
    last_tracking_dev: Option<u64>,
    raw_lat: Vec<f64>,
}

impl CaptureStats {
    /// Add one frame.
    pub fn add(&mut self, i: &FrameInfo) {
        self.frames += 1;
        self.dropped += i.dropped as u64;
        if self.first_host.is_none() {
            self.first_host = Some(i.host_ts_ns);
        }
        self.last_host = i.host_ts_ns;
        if let Some(p) = &self.last {
            self.host_dt_ms
                .push((i.host_ts_ns as f64 - p.host_ts_ns as f64) / 1e6);
            if i.dev_ts_us > 0 && p.dev_ts_us > 0 {
                self.dev_dt_ms
                    .push((i.dev_ts_us as f64 - p.dev_ts_us as f64) / 1e3);
            }
        }
        if i.kind == FrameKind::Tracking as u32 {
            if let Some(d) = self.last_tracking_dev {
                self.tracking_dt_ms
                    .push((i.dev_ts_us as f64 - d as f64) / 1e3);
            }
            self.last_tracking_dev = Some(i.dev_ts_us);
        }
        if i.dev_ts_us > 0 {
            self.raw_lat
                .push((i.host_ts_ns as f64 - i.dev_ts_us as f64 * 1e3) / 1e6);
            self.mono_minus_host_ms
                .push((i.mono_ts_ns as f64 - i.host_ts_ns as f64) / 1e6);
        }
        *self
            .settings
            .entry((i.kind, i.exposure, i.gain, i.y_offset))
            .or_insert(0) += 1;
        self.last = Some(*i);
    }

    /// Finalise derived series (call once before printing).
    pub fn finish(&mut self) {
        let min = self.raw_lat.iter().copied().fold(f64::INFINITY, f64::min);
        self.latency_ms = Series::default();
        for &l in &self.raw_lat {
            self.latency_ms.push(l - min);
        }
    }

    /// Wall duration covered (s).
    pub fn duration_s(&self) -> f64 {
        self.first_host
            .map(|f| (self.last_host - f) as f64 / 1e9)
            .unwrap_or(0.0)
    }

    /// Average frame rate.
    pub fn fps(&self) -> f64 {
        if self.frames < 2 {
            return 0.0;
        }
        (self.frames - 1) as f64 / self.duration_s().max(1e-9)
    }

    /// Multi-line report.
    pub fn report(&self) -> String {
        let mut s = format!(
            "frames {} in {:.2} s = {:.3} fps, lost {} ({:.3}%)\n",
            self.frames,
            self.duration_s(),
            self.fps(),
            self.dropped,
            100.0 * self.dropped as f64 / (self.frames + self.dropped).max(1) as f64
        );
        s += &format!("  host interval   {}\n", self.host_dt_ms.fmt("ms"));
        s += &format!("  device interval {}\n", self.dev_dt_ms.fmt("ms"));
        s += &format!(
            "  tracking-frame interval (device) {}\n",
            self.tracking_dt_ms.fmt("ms")
        );
        s += &format!(
            "  arrival latency above minimum: p50 {:.2} p90 {:.2} p99 {:.2} max {:.2} ms\n",
            self.latency_ms.quantile(0.5),
            self.latency_ms.quantile(0.9),
            self.latency_ms.quantile(0.99),
            self.latency_ms.max()
        );
        s += &format!(
            "  fallback clock: mapped time - arrival {}\n",
            self.mono_minus_host_ms.fmt("ms")
        );
        for ((k, e, g, y), n) in &self.settings {
            let kind = match *k {
                1 => "tracking",
                2 => "led",
                _ => "unknown",
            };
            s += &format!("  {n:6} x {kind:8} exposure {e:?} gain {g:?} rows {y:?}\n");
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn series_and_capture_stats() {
        let mut s = Series::default();
        for x in [1.0, 2.0, 3.0, 4.0] {
            s.push(x);
        }
        assert_eq!(s.mean(), 2.5);
        assert!((s.std() - 1.118).abs() < 1e-3);
        assert_eq!(s.quantile(0.0), 1.0);
        assert_eq!(s.quantile(1.0), 4.0);
        let mut c = CaptureStats::default();
        for n in 0..51u64 {
            let i = FrameInfo {
                host_ts_ns: 1_000_000_000 + n * 20_000_000 + (n % 3) * 1_000_000,
                dev_ts_us: 10_000 + n * 20_000,
                mono_ts_ns: 1,
                kind: if n % 2 == 0 { 1 } else { 2 },
                ..Default::default()
            };
            c.add(&i);
        }
        c.finish();
        assert!((c.fps() - 50.0).abs() < 0.2, "{}", c.fps());
        assert!((c.dev_dt_ms.mean() - 20.0).abs() < 1e-9);
        assert!((c.tracking_dt_ms.mean() - 40.0).abs() < 1e-9);
        assert!((c.latency_ms.max() - 2.0).abs() < 1e-6);
        assert_eq!(c.settings.len(), 2);
        assert!(c.report().contains("fps"));
    }
}
