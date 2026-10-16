//! Per-frame timing of the client loop (`--timing` or `PIMAX_LENSTEST_TIMING=1`): where the frame time goes.
//! Every stage is accumulated and a summary line (mean / max in ms) is printed every two seconds.

use std::time::{Duration, Instant};

/// Stages of one frame, in loop order.
pub const STAGES: [&str; 7] = [
    "wait", "begin", "locate", "render", "upload", "end", "frame",
];
pub const WAIT: usize = 0;
pub const BEGIN: usize = 1;
pub const LOCATE: usize = 2;
/// CPU rendering of the images (0 when the GPU draws).
pub const RENDER: usize = 3;
/// Copy of the staging buffer to the swapchain (CPU path) or the GPU draw (swim shader), including the fence wait.
pub const UPLOAD: usize = 4;
pub const END: usize = 5;
/// Everything between the end of one xrWaitFrame and the end of the next xrEndFrame.
pub const FRAME: usize = 6;

pub struct Timing {
    enabled: bool,
    sum: [f64; STAGES.len()],
    max: [f64; STAGES.len()],
    frames: u32,
    since: Instant,
    every: Duration,
}

impl Timing {
    pub fn new(enabled: bool) -> Timing {
        Timing {
            enabled,
            sum: [0.0; STAGES.len()],
            max: [0.0; STAGES.len()],
            frames: 0,
            since: Instant::now(),
            every: Duration::from_secs(2),
        }
    }

    pub fn enabled(&self) -> bool {
        self.enabled
    }

    pub fn add(&mut self, stage: usize, d: Duration) {
        if self.enabled {
            let ms = d.as_secs_f64() * 1e3;
            self.sum[stage] += ms;
            self.max[stage] = self.max[stage].max(ms);
        }
    }

    /// Count a frame; returns the summary text when the interval is over.
    pub fn frame_done(&mut self, now: Instant) -> Option<String> {
        if !self.enabled {
            return None;
        }
        self.frames += 1;
        let el = now.duration_since(self.since);
        if el < self.every {
            return None;
        }
        let s = self.summary(el.as_secs_f64());
        self.sum = [0.0; STAGES.len()];
        self.max = [0.0; STAGES.len()];
        self.frames = 0;
        self.since = now;
        Some(s)
    }

    fn summary(&self, secs: f64) -> String {
        let n = f64::from(self.frames.max(1));
        let parts: Vec<String> = STAGES
            .iter()
            .enumerate()
            .map(|(i, name)| format!("{name} {:.2}/{:.1}", self.sum[i] / n, self.max[i]))
            .collect();
        format!(
            "timing: {:.1} fps over {} frames | ms mean/max: {}",
            n / secs,
            self.frames,
            parts.join(" | ")
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_is_silent() {
        let mut t = Timing::new(false);
        t.add(RENDER, Duration::from_millis(5));
        assert!(t
            .frame_done(Instant::now() + Duration::from_secs(5))
            .is_none());
    }

    #[test]
    fn summary_has_means_and_resets() {
        let mut t = Timing::new(true);
        let t0 = Instant::now();
        for ms in [4, 6] {
            t.add(RENDER, Duration::from_millis(ms));
            assert!(t.frame_done(t0).is_none());
        }
        let s = t.frame_done(t0 + Duration::from_secs(3)).unwrap();
        // 3 frames were counted, 10 ms of render in total
        assert!(
            s.contains("3 frames") && s.contains("render 3.33/6.0"),
            "{s}"
        );
        assert!(t.frame_done(t0 + Duration::from_secs(3)).is_none());
    }
}
