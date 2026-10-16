//! Colour logic and the four lines of the panel.

use crate::stats::Stats;
use std::collections::VecDeque;

/// How many seconds of missed frames the panel counts.
pub const WINDOW_S: f64 = 5.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Green,
    Yellow,
    Red,
    /// No data.
    Grey,
}

impl Level {
    /// `#rrggbb` for `wayvrctl panel-modify ... set-color`.
    pub fn hex(self) -> &'static str {
        match self {
            Level::Green => "#4cdc6c",
            Level::Yellow => "#ffd23c",
            Level::Red => "#ff4d4d",
            Level::Grey => "#9aa0a6",
        }
    }

    fn rank(self) -> u8 {
        match self {
            Level::Grey => 0,
            Level::Green => 1,
            Level::Yellow => 2,
            Level::Red => 3,
        }
    }

    /// The worse of two.
    pub fn worst(self, o: Level) -> Level {
        if o.rank() > self.rank() {
            o
        } else {
            self
        }
    }
}

/// Rate against its target: green at 95 % or more, yellow from 85 %, red below.
pub fn level(value: f64, target: f64) -> Level {
    if target <= 0.0 || !value.is_finite() {
        return Level::Grey;
    }
    let r = value / target;
    if r >= 0.95 {
        Level::Green
    } else if r >= 0.85 {
        Level::Yellow
    } else {
        Level::Red
    }
}

/// A time against the frame period: green under 80 % of it, yellow under 100 %, red beyond.
pub fn time_level(ms: f64, period_ms: f64) -> Level {
    if period_ms <= 0.0 || !ms.is_finite() {
        return Level::Grey;
    }
    if ms < period_ms * 0.8 {
        Level::Green
    } else if ms < period_ms {
        Level::Yellow
    } else {
        Level::Red
    }
}

/// What the panel shows: four lines with their colours.
#[derive(Clone, Debug, PartialEq)]
pub struct View {
    pub lines: [(String, Level); 4],
}

impl View {
    /// No data (Monado not running, stats file missing or stale).
    pub fn none() -> View {
        View {
            lines: [
                ("perf: no data".into(), Level::Grey),
                (String::new(), Level::Grey),
                (String::new(), Level::Grey),
                (String::new(), Level::Grey),
            ],
        }
    }
}

/// Keeps the recent history of the cumulative missed-frame counter.
#[derive(Default)]
pub struct Tracker {
    hist: VecDeque<(f64, u64)>,
}

impl Tracker {
    pub fn new() -> Tracker {
        Tracker::default()
    }

    /// Missed frames in the last [`WINDOW_S`] seconds, and the window length actually covered.
    fn missed_in_window(&mut self, now_s: f64, missed: u64) -> (u64, f64) {
        // a counter that went backwards means Monado restarted
        if self.hist.back().is_some_and(|&(_, m)| missed < m) {
            self.hist.clear();
        }
        self.hist.push_back((now_s, missed));
        while self.hist.len() > 2 && now_s - self.hist[1].0 >= WINDOW_S {
            self.hist.pop_front();
        }
        let (t0, m0) = self.hist[0];
        (missed - m0, now_s - t0)
    }

    /// Build the view from a snapshot taken at `now_s` (any monotonic clock, seconds).
    pub fn view(&mut self, s: &Stats, now_s: f64) -> View {
        let target = s.target_fps();
        let comp = (
            format!("HMD {:.0}/{:.0}", s.comp_fps, target),
            level(s.comp_fps, target),
        );
        let (miss, span) = self.missed_in_window(now_s, s.missed());
        // expected frames over the covered span; with no history yet nothing counts as missed
        let expected = (target * span).max(1.0);
        let miss_level = if span < 0.4 {
            Level::Green
        } else {
            level(1.0 - miss as f64 / expected, 1.0)
        };
        let missl = (
            format!("miss {} /{:.0}s", miss, WINDOW_S),
            miss_level,
        );
        let (app, times) = match s.main_app() {
            Some(a) => {
                let cpu = time_level(a.cpu_ms, s.period_ms);
                let gpu = time_level(a.gpu_ms, s.period_ms);
                (
                    (format!("APP {:.0}/{:.0}", a.fps, target), level(a.fps, target)),
                    (
                        format!("cpu {:.1} gpu {:.1}", a.cpu_ms, a.gpu_ms),
                        cpu.worst(gpu),
                    ),
                )
            }
            None => (
                ("APP -".to_string(), Level::Grey),
                ("cpu - gpu -".to_string(), Level::Grey),
            ),
        };
        View {
            lines: [comp, app, missl, times],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::stats::{App, Stats};

    fn stats(comp: f64, skipped: u64, app_fps: f64) -> Stats {
        Stats {
            t_ms: 0,
            period_ms: 13.889,
            comp_fps: comp,
            frames: 0,
            late: 0,
            skipped,
            apps: vec![App {
                overlay: false,
                active: true,
                fps: app_fps,
                frames: 10,
                cpu_ms: 4.0,
                gpu_ms: 12.5,
            }],
        }
    }

    #[test]
    fn thresholds_are_95_and_85_percent() {
        assert_eq!(level(72.0, 72.0), Level::Green);
        assert_eq!(level(68.4, 72.0), Level::Green); // exactly 95 %
        assert_eq!(level(68.3, 72.0), Level::Yellow);
        assert_eq!(level(61.2, 72.0), Level::Yellow); // exactly 85 %
        assert_eq!(level(61.1, 72.0), Level::Red);
        assert_eq!(level(80.0, 72.0), Level::Green);
        assert_eq!(level(1.0, 0.0), Level::Grey);
        assert_eq!(level(f64::NAN, 72.0), Level::Grey);
    }

    #[test]
    fn times_use_the_frame_budget() {
        assert_eq!(time_level(5.0, 13.9), Level::Green);
        assert_eq!(time_level(12.0, 13.9), Level::Yellow);
        assert_eq!(time_level(14.0, 13.9), Level::Red);
        assert_eq!(Level::Green.worst(Level::Red), Level::Red);
        assert_eq!(Level::Yellow.worst(Level::Green), Level::Yellow);
    }

    #[test]
    fn view_texts_and_colours() {
        let mut t = Tracker::new();
        let v = t.view(&stats(72.0, 0, 60.0), 0.0);
        assert_eq!(v.lines[0], ("HMD 72/72".to_string(), Level::Green));
        assert_eq!(v.lines[1], ("APP 60/72".to_string(), Level::Red));
        assert_eq!(v.lines[2].1, Level::Green);
        assert_eq!(v.lines[3], ("cpu 4.0 gpu 12.5".to_string(), Level::Yellow));
    }

    #[test]
    fn missed_frames_are_counted_over_five_seconds_only() {
        let mut t = Tracker::new();
        t.view(&stats(72.0, 100, 72.0), 0.0); // history starts at 100 (earlier misses do not count)
        let v = t.view(&stats(72.0, 100, 72.0), 1.0);
        assert_eq!(v.lines[2].0, "miss 0 /5s");
        let v = t.view(&stats(72.0, 110, 72.0), 2.0);
        assert_eq!(v.lines[2].0, "miss 10 /5s");
        assert_eq!(v.lines[2].1, Level::Yellow); // 10 of 144 expected frames: 93 %
        let v = t.view(&stats(72.0, 140, 72.0), 3.0);
        assert_eq!(v.lines[2].1, Level::Red); // 40 of 216: 81 %
        // the burst leaves the window after five seconds
        let v = t.view(&stats(72.0, 140, 72.0), 9.5);
        assert_eq!(v.lines[2].0, "miss 0 /5s");
        assert_eq!(v.lines[2].1, Level::Green);
    }

    #[test]
    fn a_restarted_monado_resets_the_history() {
        let mut t = Tracker::new();
        t.view(&stats(72.0, 500, 72.0), 0.0);
        let v = t.view(&stats(72.0, 2, 72.0), 1.0);
        assert_eq!(v.lines[2].0, "miss 0 /5s");
    }

    #[test]
    fn no_main_app_is_grey() {
        let mut s = stats(72.0, 0, 72.0);
        s.apps.clear();
        let v = Tracker::new().view(&s, 0.0);
        assert_eq!(v.lines[1], ("APP -".to_string(), Level::Grey));
    }
}
