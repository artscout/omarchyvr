//! `pimax-eye watch`: live full-screen terminal view of what each eye camera reports (pupil position in the
//! sensor area, pupil size, validity, origin) with detection of the known "bad" right-eye state, where the
//! reported origin jumps ~14 mm in x and 4-8 mm in z. Pure logic and rendering live here; `cli::cmd_watch` feeds it
//! samples from the same `live::Tracker` that `pimax-eye live` uses.

use crate::track::{EyeRay, Sample};
use std::collections::VecDeque;
use std::fmt::Write as _;

/// An origin more than this far (mm, in x or z) from the running median is BAD.
pub const BAD_MM: f32 = 2.5;
/// Window of the BAD percentage, seconds of device time.
pub const BAD_WINDOW_S: f64 = 5.0;
const MEDIAN_N: usize = 600; // 5 s at 120 Hz
const TRAIL_N: usize = 40;
const BOX_W: usize = 33;
const BOX_H: usize = 13;

/// Per-eye running statistics.
#[derive(Default)]
pub struct EyeStat {
    xs: VecDeque<f32>,
    zs: VecDeque<f32>,
    /// (device time s, bad) over the BAD window.
    bad: VecDeque<(f64, bool)>,
    /// (device time s, valid) over the BAD window.
    valid: VecDeque<(f64, bool)>,
    pub trail: VecDeque<[f32; 2]>,
    pub last: Option<EyeRay>,
    pub last_bad: bool,
    pub last_dev: [f32; 2],
    pub median: [f32; 2],
    pub total: u64,
    pub total_bad: u64,
}

fn median(v: &VecDeque<f32>) -> f32 {
    let mut s: Vec<f32> = v.iter().copied().collect();
    if s.is_empty() {
        return 0.0;
    }
    s.sort_by(|a, b| a.total_cmp(b));
    s[s.len() / 2]
}

impl EyeStat {
    /// Feed one sample of this eye. Returns whether it is BAD (origin far from the running median).
    pub fn push(&mut self, t_s: f64, e: &EyeRay) -> bool {
        let valid = e.origin_valid && e.direction_valid;
        self.valid.push_back((t_s, valid));
        let mut bad = false;
        if e.origin_valid {
            let (x, z) = (e.origin[0] * 1000.0, e.origin[2] * 1000.0);
            // Judge against the median of the PREVIOUS samples so a sudden jump is not absorbed by the median.
            if self.xs.len() >= 30 {
                self.median = [median(&self.xs), median(&self.zs)];
                self.last_dev = [x - self.median[0], z - self.median[1]];
                bad = self.last_dev[0].abs() > BAD_MM || self.last_dev[1].abs() > BAD_MM;
            }
            // A median over everything (bad included) follows the majority state; with ~30% bad it stays good.
            self.xs.push_back(x);
            self.zs.push_back(z);
            if self.xs.len() > MEDIAN_N {
                self.xs.pop_front();
                self.zs.pop_front();
            }
            self.bad.push_back((t_s, bad));
            self.total += 1;
            self.total_bad += u64::from(bad);
        }
        if e.pupil_pos_valid {
            self.trail.push_back(e.pupil_pos);
            if self.trail.len() > TRAIL_N {
                self.trail.pop_front();
            }
        }
        while self.bad.front().is_some_and(|f| t_s - f.0 > BAD_WINDOW_S) {
            self.bad.pop_front();
        }
        while self.valid.front().is_some_and(|f| t_s - f.0 > BAD_WINDOW_S) {
            self.valid.pop_front();
        }
        self.last = Some(*e);
        self.last_bad = bad;
        bad
    }

    pub fn bad_pct(&self) -> f64 {
        pct(self.bad.iter().filter(|b| b.1).count(), self.bad.len())
    }

    pub fn valid_pct(&self) -> f64 {
        pct(self.valid.iter().filter(|b| b.1).count(), self.valid.len())
    }
}

fn pct(n: usize, d: usize) -> f64 {
    if d == 0 {
        0.0
    } else {
        100.0 * n as f64 / d as f64
    }
}

/// CSV header of `--log`.
pub const LOG_HEADER: &str = "t_s,seq,eye,origin_valid,dir_valid,ox_mm,oy_mm,oz_mm,dev_x_mm,dev_z_mm,bad,pupil_mm,openness,pp_valid,pp_x,pp_y";

/// Both eyes plus rendering.
#[derive(Default)]
pub struct Watch {
    pub eyes: [EyeStat; 2],
    pub t_s: f64,
    pub samples: u64,
}

impl Watch {
    /// Feed a sample; appends one CSV row per eye to `log` when given.
    pub fn push(&mut self, s: &Sample, t0_us: i64, log: Option<&mut String>) {
        self.t_s = (s.device_ts_us - t0_us) as f64 / 1e6;
        self.samples += 1;
        let mut log = log;
        for (i, e) in s.eyes.iter().enumerate() {
            let bad = self.eyes[i].push(self.t_s, e);
            if let Some(l) = log.as_deref_mut() {
                let st = &self.eyes[i];
                let _ = writeln!(
                    l,
                    "{:.4},{},{},{},{},{:.3},{:.3},{:.3},{:.3},{:.3},{},{:.3},{:.3},{},{:.4},{:.4}",
                    self.t_s, s.seq, if i == 0 { "L" } else { "R" }, u8::from(e.origin_valid), u8::from(e.direction_valid),
                    e.origin[0] * 1000.0, e.origin[1] * 1000.0, e.origin[2] * 1000.0,
                    st.last_dev[0], st.last_dev[1], u8::from(bad), e.pupil_mm, e.openness,
                    u8::from(e.pupil_pos_valid), e.pupil_pos[0], e.pupil_pos[1]
                );
            }
        }
    }

    /// One full screen of text (ANSI colours, no cursor movement; the caller homes the cursor).
    pub fn render(&self, state: &str) -> String {
        let mut o = String::new();
        let _ = writeln!(o, "\x1b[1mpimax-eye watch\x1b[0m  t={:6.1}s  samples={}  link={}\x1b[K", self.t_s, self.samples, state);
        let boxes: Vec<Vec<String>> = self.eyes.iter().map(draw_box).collect();
        let _ = writeln!(o, "  {:^w$}   {:^w$}\x1b[K", "LEFT eye camera", "RIGHT eye camera", w = BOX_W + 2);
        for (l, r) in boxes[0].iter().zip(&boxes[1]) {
            let _ = writeln!(o, "  {}   {}\x1b[K", l, r);
        }
        for (name, e) in ["L", "R"].iter().zip(&self.eyes) {
            let _ = writeln!(o, "{}\x1b[K", info_line(name, e));
        }
        let _ = writeln!(o, "\x1b[K");
        let _ = writeln!(o, "pupil near the box edge = headset/IPD misaligned or lens view blocked;\x1b[K");
        let _ = writeln!(o, "BAD with pupil centered = reflection/lashes/glasses/dirty IR ring.   Ctrl-C quits.\x1b[K");
        o
    }
}

fn info_line(name: &str, e: &EyeStat) -> String {
    let Some(r) = e.last else { return format!("{name}: no data") };
    let flag = if e.last_bad { "\x1b[1;31mBAD\x1b[0m" } else { "\x1b[32mok \x1b[0m" };
    let open = if r.openness >= 0.0 { format!("{:.2}", r.openness) } else { "n/a".into() };
    let pupil = if r.pupil_mm >= 0.0 { format!("{:.2} mm", r.pupil_mm) } else { "n/a".into() };
    let tot = if e.total > 0 { 100.0 * e.total_bad as f64 / e.total as f64 } else { 0.0 };
    format!(
        "{name}: pupil {pupil}  open {open}  valid {:5.1}%  origin x={:+6.1} z={:+6.1} mm (med {:+5.1}/{:+5.1}) {flag}  BAD 5s {:5.1}%  all {:4.1}%",
        e.valid_pct(), r.origin[0] * 1000.0, r.origin[2] * 1000.0, e.median[0], e.median[1], e.bad_pct(), tot
    )
}

fn cell(p: [f32; 2]) -> Option<(usize, usize)> {
    if !(0.0..=1.0).contains(&p[0]) || !(0.0..=1.0).contains(&p[1]) {
        return None;
    }
    let cx = ((p[0] * (BOX_W - 1) as f32).round() as usize).min(BOX_W - 1);
    let cy = ((p[1] * (BOX_H - 1) as f32).round() as usize).min(BOX_H - 1);
    Some((cx, cy))
}

/// Sensor-area box, `BOX_H + 2` rows of visible width `BOX_W + 2`. Trail `.`, current pupil `O` (red when BAD).
pub fn draw_box(e: &EyeStat) -> Vec<String> {
    let mut g = vec![vec![(' ', 0u8); BOX_W]; BOX_H];
    for p in &e.trail {
        if let Some((x, y)) = cell(*p) {
            g[y][x] = ('.', 1);
        }
    }
    if let Some(r) = e.last.filter(|r| r.pupil_pos_valid) {
        if let Some((x, y)) = cell(r.pupil_pos) {
            g[y][x] = ('O', if e.last_bad { 3 } else { 2 });
        }
    }
    let mut rows = vec![format!("+{}+", "-".repeat(BOX_W))];
    for row in g {
        let mut s = String::from("|");
        for (c, k) in row {
            match k {
                2 => { let _ = write!(s, "\x1b[1;32m{c}\x1b[0m"); }
                3 => { let _ = write!(s, "\x1b[1;31m{c}\x1b[0m"); }
                _ => s.push(c),
            }
        }
        s.push('|');
        rows.push(s);
    }
    rows.push(format!("+{}+", "-".repeat(BOX_W)));
    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn eye(x_mm: f32, z_mm: f32, pp: [f32; 2]) -> EyeRay {
        EyeRay {
            origin: [x_mm / 1000.0, 0.0, z_mm / 1000.0],
            origin_valid: true,
            direction_valid: true,
            pupil_mm: 3.0,
            pupil_pos: pp,
            pupil_pos_valid: true,
            openness: -1.0,
            ..EyeRay::default()
        }
    }

    #[test]
    fn flags_jump_from_median() {
        let mut s = EyeStat::default();
        for i in 0..100 {
            assert!(!s.push(i as f64 / 120.0, &eye(26.5, 19.5, [0.5, 0.5])));
        }
        assert!(s.push(1.0, &eye(41.0, 25.0, [0.5, 0.5])));
        assert!(s.push(1.01, &eye(26.5, 24.0, [0.5, 0.5])), "z alone also counts");
        assert!(!s.push(1.02, &eye(27.5, 20.5, [0.5, 0.5])));
        assert!(s.bad_pct() > 0.0 && s.bad_pct() < 5.0);
    }

    #[test]
    fn bad_window_expires_and_median_survives_30pct_bad() {
        let mut s = EyeStat::default();
        let mut bad_seen = 0;
        for i in 0..1200 {
            let bad = i % 10 < 3;
            let e = if bad { eye(41.0, 25.0, [0.5, 0.5]) } else { eye(26.5, 19.5, [0.5, 0.5]) };
            if s.push(i as f64 / 120.0, &e) && i >= 100 {
                bad_seen += 1;
            }
        }
        assert!(bad_seen > 250, "{bad_seen}");
        assert!((s.bad_pct() - 30.0).abs() < 3.0, "{}", s.bad_pct());
        for i in 0..700 {
            s.push(10.0 + i as f64 / 120.0, &eye(26.5, 19.5, [0.5, 0.5]));
        }
        assert_eq!(s.bad_pct(), 0.0);
    }

    #[test]
    fn box_places_pupil_and_clips() {
        let mut s = EyeStat::default();
        s.push(0.0, &eye(26.5, 19.5, [0.0, 0.0]));
        let b = draw_box(&s);
        assert_eq!(b.len(), BOX_H + 2);
        assert!(b[1].contains('O'));
        s.push(0.1, &eye(26.5, 19.5, [-1.0, -1.0]));
        assert!(!draw_box(&s)[BOX_H / 2].contains('O'));
    }

    #[test]
    fn render_and_log() {
        let mut w = Watch::default();
        let mut log = String::new();
        let mut smp = Sample { eyes: [eye(-26.5, 19.5, [0.4, 0.5]), eye(26.5, 19.5, [0.6, 0.5])], ..Sample::default() };
        for i in 0..50 {
            smp.seq = i;
            smp.device_ts_us = 1000 + i as i64 * 8333;
            w.push(&smp, 1000, Some(&mut log));
        }
        assert_eq!(log.lines().count(), 100);
        let r = w.render("streaming");
        assert!(r.contains("RIGHT eye camera") && r.contains("BAD with pupil centered"));
    }
}
