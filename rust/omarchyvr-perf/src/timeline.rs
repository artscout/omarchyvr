//! Frame-health timeline: one plain line every [`TIMELINE_S`] seconds on stderr (the session's `perf.log`), so a
//! session's dropped frames can be lined up afterwards with what else ran (voice commands, AI requests, backdrop
//! loads). Monado's stats file only holds cumulative counters; without this line a session ends with one total.
//!
//! Format (`key=value`, space separated):
//! `perf: unix=1792145400.512 mono_ms=27237605 fps=89.9 frames+450 dropped+3 late+212 app_fps=90.0 app_gpu_ms=1.77`
//! * `unix`: wall clock (s); `mono_ms`: Monado's `t_ms` (CLOCK_MONOTONIC ms, the clock of `head-imu.csv`);
//! * `frames+`, `dropped+` (`comp_skipped`: display periods that showed the previous image again), `late+`
//!   (`comp_late`, see docs/RENDER-QUALITY.md): increments since the previous line.

use crate::stats::Stats;

/// Seconds between two timeline lines.
pub const TIMELINE_S: f64 = 5.0;

/// Remembers the previous snapshot and says when the next line is due.
#[derive(Default)]
pub struct Timeline {
    prev: Option<(f64, Stats)>,
}

impl Timeline {
    pub fn new() -> Timeline {
        Timeline::default()
    }

    /// Feed a snapshot read at `now_s` (monotonic seconds); returns the line when one is due.
    /// `unix_s` is the wall clock for the line. A counter that went backwards (Monado restarted) starts over.
    pub fn push(&mut self, s: &Stats, now_s: f64, unix_s: f64) -> Option<String> {
        let restart = self
            .prev
            .as_ref()
            .is_some_and(|(_, p)| s.frames < p.frames || s.skipped < p.skipped || s.late < p.late);
        let Some((t0, p)) = self.prev.as_ref().filter(|_| !restart) else {
            self.prev = Some((now_s, s.clone()));
            return None;
        };
        if now_s - t0 < TIMELINE_S {
            return None;
        }
        let (fps, gpu) = match s.main_app() {
            Some(a) => (format!("{:.1}", a.fps), format!("{:.2}", a.gpu_ms)),
            None => ("-".into(), "-".into()),
        };
        let line = format!(
            "perf: unix={:.3} mono_ms={} fps={:.1} frames+{} dropped+{} late+{} app_fps={} app_gpu_ms={}",
            unix_s,
            s.t_ms,
            s.comp_fps,
            s.frames - p.frames,
            s.skipped - p.skipped,
            s.late - p.late,
            fps,
            gpu
        );
        self.prev = Some((now_s, s.clone()));
        Some(line)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(t_ms: i64, frames: u64, skipped: u64, late: u64) -> Stats {
        Stats {
            t_ms,
            period_ms: 11.111,
            comp_fps: 89.5,
            frames,
            late,
            skipped,
            apps: vec![],
        }
    }

    #[test]
    fn line_every_five_seconds_with_increments() {
        let mut t = Timeline::new();
        assert_eq!(t.push(&st(1000, 100, 1, 10), 0.0, 100.0), None);
        assert_eq!(t.push(&st(3000, 280, 2, 20), 2.0, 102.0), None);
        let l = t.push(&st(6000, 550, 5, 40), 5.0, 105.0).unwrap();
        assert_eq!(
            l,
            "perf: unix=105.000 mono_ms=6000 fps=89.5 frames+450 dropped+4 late+30 app_fps=- app_gpu_ms=-"
        );
        // next window starts from the last line
        assert_eq!(t.push(&st(9000, 820, 5, 50), 8.0, 108.0), None);
        let l = t.push(&st(11000, 1000, 9, 60), 10.0, 110.0).unwrap();
        assert!(l.contains("frames+450 dropped+4 late+20"), "{l}");
    }

    #[test]
    fn monado_restart_starts_over() {
        let mut t = Timeline::new();
        t.push(&st(1000, 5000, 50, 10), 0.0, 0.0);
        // counters went backwards: no negative increments, no line, new baseline
        assert_eq!(t.push(&st(7000, 20, 0, 0), 6.0, 6.0), None);
        let l = t.push(&st(12000, 470, 2, 1), 11.0, 11.0).unwrap();
        assert!(l.contains("frames+450 dropped+2 late+1"), "{l}");
    }
}
