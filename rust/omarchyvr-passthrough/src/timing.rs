//! Camera time -> the time the head pose is looked up for, and latency bookkeeping.
//!
//! Without the HMD driver's IMU clock reference (an OpenXR client does not have it) `pimax_cam` maps the frame's
//! MCU timestamp to `CLOCK_MONOTONIC` with a minimum-delay filter on the V4L2 buffer timestamps. That puts the
//! frame at "MCU time + the smallest observed delay to the host timestamp", i.e. somewhat later than the MCU time.
//! [`CamTiming::delay_ns`] subtracts that (default [`DEFAULT_CAM_DELAY_MS`]); tune it live: if the passthrough
//! world swims *with* a head turn the delay is too small, *against* it too large (`[` / `]` keys).

/// Default correction of the minimum-delay camera clock (ms). Bounds: uvcvideo stamps a buffer when its first
/// packet arrives (start of frame), so the minimum delay is mostly the time from the MCU stamp to the start of
/// the USB transfer; the end of the transfer comes ~8 ms later (2.13 MB at 32 KiB per 125 µs) and Windows captures
/// put the URB completion 11.8 ms after the MCU stamp (`analysis/camera-frames.md`). 4 ms is the middle guess.
pub const DEFAULT_CAM_DELAY_MS: f64 = 4.0;

/// Pose-time policy.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CamTiming {
    /// Subtracted from the mapped camera time.
    pub delay_ns: i64,
    /// Oldest pose time accepted relative to now (older images are shown with this age instead; Monado's pose
    /// history and timewarp are not meant for huge corrections).
    pub max_age_ns: i64,
}

impl Default for CamTiming {
    fn default() -> CamTiming {
        CamTiming {
            delay_ns: (DEFAULT_CAM_DELAY_MS * 1e6) as i64,
            max_age_ns: 250_000_000,
        }
    }
}

impl CamTiming {
    /// Monotonic time (ns) of the head pose that matches an image captured at `cam_mono_ns`, clamped to
    /// `[now - max_age, now]`. A zero camera time (stills) gives `now`.
    pub fn pose_time(&self, cam_mono_ns: u64, now_mono_ns: u64) -> u64 {
        if cam_mono_ns == 0 {
            return now_mono_ns;
        }
        let t = cam_mono_ns as i128 - self.delay_ns as i128;
        let lo = now_mono_ns as i128 - self.max_age_ns as i128;
        t.clamp(lo.max(0), now_mono_ns as i128) as u64
    }
}

/// `CLOCK_MONOTONIC` ns -> XrTime, given a pair of readings of both clocks taken back to back.
pub fn mono_to_xr(mono_ns: u64, xr_now: i64, mono_now_ns: u64) -> i64 {
    xr_now + (mono_ns as i64 - mono_now_ns as i64)
}

/// Running min / mean / max of a latency in ms.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LatencyStats {
    n: u64,
    sum: f64,
    min: f64,
    max: f64,
}

impl LatencyStats {
    /// Add one sample (ms).
    pub fn add(&mut self, ms: f64) {
        if self.n == 0 {
            self.min = ms;
            self.max = ms;
        } else {
            self.min = self.min.min(ms);
            self.max = self.max.max(ms);
        }
        self.n += 1;
        self.sum += ms;
    }

    /// Number of samples.
    pub fn count(&self) -> u64 {
        self.n
    }

    /// `min/mean/max ms (n)` or `-`.
    pub fn summary(&self) -> String {
        if self.n == 0 {
            return "-".into();
        }
        format!(
            "{:.1}/{:.1}/{:.1} ms (n {})",
            self.min,
            self.sum / self.n as f64,
            self.max,
            self.n
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pose_time_applies_the_delay_and_clamps() {
        let t = CamTiming::default();
        let now = 10_000_000_000u64;
        assert_eq!(
            t.pose_time(now - 30_000_000, now),
            now - 30_000_000 - t.delay_ns as u64
        );
        // future camera stamps (clock glitch) -> now
        assert_eq!(t.pose_time(now + 50_000_000, now), now);
        // very old -> max age
        assert_eq!(t.pose_time(1_000, now), now - 250_000_000);
        assert_eq!(t.pose_time(0, now), now);
        // early boot: no underflow
        assert_eq!(t.pose_time(5, 100), 0);
    }

    #[test]
    fn clock_conversion_and_stats() {
        assert_eq!(mono_to_xr(1_000, 50_000, 3_000), 48_000);
        let mut s = LatencyStats::default();
        assert_eq!(s.summary(), "-");
        for v in [30.0, 40.0, 50.0] {
            s.add(v);
        }
        assert_eq!(s.count(), 3);
        assert_eq!(s.summary(), "30.0/40.0/50.0 ms (n 3)");
    }
}
