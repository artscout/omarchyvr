//! The stats file written by Monado's multi compositor (`comp_multi_system.c`, `perf_report`).
//!
//! ```text
//! v=1
//! t_ms=123456
//! period_ms=13.889
//! comp_fps=72.0
//! comp_frames=1000
//! comp_late=0
//! comp_skipped=0
//! app overlay=0 active=1 fps=71.9 frames=900 cpu_ms=3.10 gpu_ms=5.20
//! ```

/// One client session.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct App {
    pub overlay: bool,
    pub active: bool,
    pub fps: f64,
    pub frames: u64,
    pub cpu_ms: f64,
    pub gpu_ms: f64,
}

/// One snapshot. `late` and `skipped` are cumulative since Monado started the compositor.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Stats {
    pub t_ms: i64,
    pub period_ms: f64,
    pub comp_fps: f64,
    pub frames: u64,
    pub late: u64,
    pub skipped: u64,
    pub apps: Vec<App>,
}

impl Stats {
    /// The refresh rate the compositor aims for (72 / 90), rounded to the nearest integer.
    pub fn target_fps(&self) -> f64 {
        if self.period_ms > 0.0 {
            (1000.0 / self.period_ms).round()
        } else {
            0.0
        }
    }

    /// Frames the headset missed so far: the skipped display periods. `late` (the compositor's commit returned after
    /// the predicted display time) is not counted: it was 92 % of all frames in a smooth 72 fps session (2026-10-15),
    /// so counting it turned the wrist readout red for frames that were shown on time.
    pub fn missed(&self) -> u64 {
        self.skipped
    }

    /// The main app: the active non-overlay session that delivered the most frames (the game, or passthrough).
    pub fn main_app(&self) -> Option<&App> {
        self.apps
            .iter()
            .filter(|a| !a.overlay && a.active)
            .max_by_key(|a| a.frames)
    }
}

fn kv(tok: &str) -> Option<(&str, &str)> {
    tok.split_once('=')
}

/// Parse the file; `None` for an unknown version or when the compositor lines are missing.
pub fn parse(text: &str) -> Option<Stats> {
    let mut s = Stats::default();
    let mut version = 0;
    let mut have_period = false;
    for line in text.lines() {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("app ") {
            let mut a = App::default();
            for tok in rest.split_whitespace() {
                let Some((k, v)) = kv(tok) else { continue };
                match k {
                    "overlay" => a.overlay = v == "1",
                    "active" => a.active = v == "1",
                    "fps" => a.fps = v.parse().ok()?,
                    "frames" => a.frames = v.parse().ok()?,
                    "cpu_ms" => a.cpu_ms = v.parse().ok()?,
                    "gpu_ms" => a.gpu_ms = v.parse().ok()?,
                    _ => {}
                }
            }
            s.apps.push(a);
            continue;
        }
        let Some((k, v)) = kv(line) else { continue };
        match k {
            "v" => version = v.parse().ok()?,
            "t_ms" => s.t_ms = v.parse().ok()?,
            "period_ms" => {
                s.period_ms = v.parse().ok()?;
                have_period = true;
            }
            "comp_fps" => s.comp_fps = v.parse().ok()?,
            "comp_frames" => s.frames = v.parse().ok()?,
            "comp_late" => s.late = v.parse().ok()?,
            "comp_skipped" => s.skipped = v.parse().ok()?,
            _ => {}
        }
    }
    (version == 1 && have_period && s.period_ms > 0.0).then_some(s)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "v=1\nt_ms=5000\nperiod_ms=13.889\ncomp_fps=71.8\ncomp_frames=1000\ncomp_late=3\ncomp_skipped=2\n\
app overlay=1 active=1 fps=60.0 frames=500 cpu_ms=1.00 gpu_ms=0.50\n\
app overlay=0 active=1 fps=70.5 frames=900 cpu_ms=3.10 gpu_ms=5.20\n";

    #[test]
    fn parses_the_file() {
        let s = parse(SAMPLE).unwrap();
        assert_eq!(s.target_fps(), 72.0);
        assert_eq!((s.frames, s.late, s.skipped, s.missed()), (1000, 3, 2, 2));
        assert_eq!(s.apps.len(), 2);
        let m = s.main_app().unwrap();
        assert_eq!((m.fps, m.cpu_ms, m.gpu_ms), (70.5, 3.1, 5.2));
    }

    #[test]
    fn target_is_90_for_a_11_1_ms_period() {
        let s = parse("v=1\nperiod_ms=11.111\n").unwrap();
        assert_eq!(s.target_fps(), 90.0);
        assert!(s.main_app().is_none());
    }

    #[test]
    fn rejects_garbage_wrong_version_and_missing_period() {
        assert!(parse("").is_none());
        assert!(parse("v=2\nperiod_ms=13.9\n").is_none());
        assert!(parse("v=1\ncomp_fps=72\n").is_none());
        assert!(parse("v=1\nperiod_ms=abc\n").is_none());
    }

    #[test]
    fn inactive_and_overlay_sessions_are_not_the_main_app() {
        let s = parse("v=1\nperiod_ms=13.9\napp overlay=0 active=0 fps=0 frames=10 cpu_ms=0 gpu_ms=0\napp overlay=1 active=1 fps=1 frames=99 cpu_ms=0 gpu_ms=0\n").unwrap();
        assert!(s.main_app().is_none());
    }
}
