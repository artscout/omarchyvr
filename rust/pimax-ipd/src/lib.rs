//! The guided IPD procedure, independent of the device and of the screen: it drives a [`Devices`] and shows
//! each step on a [`Sink`] (the headset panel, or PNG files in `--simulate`), printing the same to the terminal.

use pimax_eye::ipd::{self, Measurement};
use pimax_hud::screen::{Line, Screen, CYAN, GREEN, RED, WHITE, YELLOW};
use pimax_hud::Sink;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

pub mod devices;
pub mod manual;
pub mod png_sink;

/// What the procedure needs from the hardware.
pub trait Devices {
    /// Check the eye chip is free, read the current lens IPD, start the eye tracker and wait for the stream.
    /// Returns the lens IPD (None when unreadable).
    fn prepare(&mut self) -> Result<Option<f32>, String>;
    /// Collect samples for `seconds`, calling `tick(0..1)`; stops early when `stop()` is true.
    fn collect(&mut self, seconds: f64, tick: &mut dyn FnMut(f32), stop: &dyn Fn() -> bool) -> Measurement;
    /// The only HID write: the IPD motor command. Returns a description for the log.
    fn send(&mut self, mm: f32) -> Result<String, String>;
    /// Poll the live lens IPD until it is within tolerance of `target` or `timeout`; returns the last reading.
    fn verify(&mut self, target: f32, timeout: Duration, tick: &mut dyn FnMut(f32)) -> Option<f32>;
}

#[derive(Clone, Debug)]
pub struct Plan {
    pub seconds: f64,
    pub apply: bool,
    pub mm: Option<f32>,
    /// Length of each countdown step (1 s in real use).
    pub step: Duration,
    /// How long the result screen stays.
    pub hold: Duration,
    /// Write ~/.config/pimax/ipd.env after a verified apply.
    pub write_env: bool,
}

impl Default for Plan {
    fn default() -> Plan {
        Plan {
            seconds: 5.0,
            apply: false,
            mm: None,
            step: Duration::from_secs(1),
            hold: Duration::from_secs(5),
            write_env: true,
        }
    }
}

/// Word-wrap `text` to at most `width` characters per line.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for w in text.split_whitespace() {
        if !cur.is_empty() && cur.len() + 1 + w.len() > width {
            out.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

struct Ui<'a> {
    sink: &'a dyn Sink,
    say: &'a mut dyn FnMut(&str),
}

impl Ui<'_> {
    fn show(&self, lines: Vec<Line>, progress: Option<f32>) {
        self.sink.show(Screen { lines, progress, pattern: false });
    }

    fn err(&mut self, title: &str, detail: &str) {
        (self.say)(&format!("{title}: {detail}"));
        let mut l = vec![Line::new(title).color(RED).scale(7), Line::new("")];
        l.extend(wrap(detail, 44).into_iter().map(|t| Line::new(t).scale(4)));
        self.show(l, None);
    }
}

fn sleep_checked(d: Duration, stop: &dyn Fn() -> bool) {
    let t0 = Instant::now();
    while t0.elapsed() < d && !stop() {
        std::thread::sleep(Duration::from_millis(20).min(d));
    }
}

/// Run the whole procedure. Returns the process exit code (0 ok, 2 refused/failed, 3 lens did not arrive).
/// `skip` ends the result hold early (a key press); `stop` aborts everything (Ctrl-C).
pub fn run(
    dev: &mut dyn Devices,
    sink: &dyn Sink,
    plan: &Plan,
    say: &mut dyn FnMut(&str),
    skip: &AtomicBool,
    stop: &dyn Fn() -> bool,
) -> i32 {
    let mut ui = Ui { sink, say };
    let hold = |ui: &Ui, code: i32| {
        let _ = ui;
        let t0 = Instant::now();
        while t0.elapsed() < plan.hold && !skip.load(Ordering::SeqCst) && !stop() {
            std::thread::sleep(Duration::from_millis(50).min(plan.hold));
        }
        code
    };
    ui.show(vec![Line::new("Starting eye tracker...").color(CYAN)], None);
    (ui.say)("starting the eye tracker");
    let lens0 = match dev.prepare() {
        Ok(l) => l,
        Err(e) => {
            ui.err("Eye tracker error", &e);
            return hold(&ui, 2);
        }
    };
    for n in (1..=3).rev() {
        ui.show(
            vec![
                Line::new("Look straight ahead").color(YELLOW).scale(8),
                Line::new(""),
                Line::new(n.to_string()).color(CYAN).scale(16),
            ],
            None,
        );
        (ui.say)(&format!("look straight ahead: {n}"));
        sleep_checked(plan.step, stop);
    }
    if stop() {
        return 2;
    }
    let head = |s: String| vec![Line::new(s).color(YELLOW).scale(7), Line::new("Keep looking straight ahead").scale(5)];
    let sec = plan.seconds;
    ui.show(head(format!("Measuring {sec:.0} s")), Some(0.0));
    let m = {
        let sink = ui.sink;
        let mut tick = |p: f32| sink.show(Screen { lines: head(format!("Measuring {sec:.0} s")), progress: Some(p), pattern: false });
        dev.collect(sec, &mut tick, stop)
    };
    if stop() {
        return 2;
    }
    (ui.say)(&format!(
        "samples: {} seen, {} valid, {} good; IPD field 34 {:.2} mm, origin distance {:.2} mm",
        m.seen, m.valid, m.good, m.field34_mm, m.origin_mm
    ));
    let measured = m.field34_mm;
    let have = m.good > 0 && measured.is_finite() && measured > 0.0;
    let mut lines = vec![Line::new("IPD result").color(CYAN).scale(7)];
    if have {
        lines.push(Line::new(format!("Measured: {measured:.1} mm")).scale(8).color(GREEN));
    } else {
        lines.push(Line::new("No valid eye samples").color(RED).scale(6));
        lines.push(Line::new("Is the headset on? Eyes open?").scale(4));
    }
    match lens0 {
        Some(l) => lines.push(Line::new(format!("Lenses: {l:.1} mm")).scale(8)),
        None => lines.push(Line::new("Lenses: unknown").scale(6)),
    }
    if let (true, Some(l)) = (have, lens0) {
        lines.push(Line::new(format!("Difference: {:+.1} mm", measured - l)).scale(8).color(WHITE));
    }
    if !plan.apply {
        lines.push(Line::new(""));
        lines.push(Line::new("Nothing sent (no --apply)").scale(4));
        (ui.say)(&format!(
            "measured {measured:.2} mm, lens {}; nothing sent (no --apply)",
            lens0.map_or("unknown".into(), |l| format!("{l:.2} mm"))
        ));
        ui.show(lines, None);
        return hold(&ui, if have { 0 } else { 2 });
    }
    ui.show(lines.clone(), None);
    let (target, note) = match ipd::decide(&m, plan.mm, lens0) {
        Ok(x) => x,
        Err(e) => {
            let mut l = lines;
            l.push(Line::new(""));
            l.push(Line::new("Measurement looks wrong").color(RED).scale(6));
            l.push(Line::new("Lenses NOT moved").color(RED).scale(5));
            ui.show(l, None);
            (ui.say)(&format!("{e}; nothing sent"));
            return hold(&ui, 2);
        }
    };
    if let Some(n) = note {
        (ui.say)(&format!("note: {n}"));
    }
    sleep_checked(Duration::from_millis(1500).min(plan.hold), stop);
    let moving = |p| vec![Line::new("Moving lenses...").color(YELLOW).scale(8), Line::new(format!("to {target:.1} mm")).scale(6)].into_iter().map(|l| (l, p)).map(|x| x.0).collect::<Vec<_>>();
    ui.show(moving(0.0), Some(0.0));
    match dev.send(target) {
        Ok(d) => (ui.say)(&d),
        Err(e) => {
            ui.err("Could not send", &format!("{e}; nothing moved"));
            return hold(&ui, 2);
        }
    }
    let fin = {
        let sink = ui.sink;
        let mut tick = |p: f32| sink.show(Screen { lines: moving(p), progress: Some(p), pattern: false });
        dev.verify(target, Duration::from_secs(5), &mut tick)
    };
    let ok = fin.is_some_and(|v| ipd::arrived(v, target));
    match fin {
        Some(v) if ok => {
            (ui.say)(&format!("final lens IPD: {v:.2} mm (OK)"));
            ui.show(vec![Line::new("Done").color(GREEN).scale(8), Line::new(format!("Lenses at {v:.1} mm")).color(GREEN).scale(8)], None);
        }
        Some(v) => {
            (ui.say)(&format!("final lens IPD: {v:.2} mm (NOT within 0.5 mm of target {target:.2})"));
            ui.show(
                vec![Line::new("Lenses did not arrive").color(RED).scale(7), Line::new(format!("At {v:.1} mm, wanted {target:.1} mm")).scale(6)],
                None,
            );
        }
        None => {
            (ui.say)("final lens IPD: no info report received");
            ui.show(vec![Line::new("Cannot verify lenses").color(RED).scale(7), Line::new("No info report received").scale(5)], None);
        }
    }
    let line = ipd::monado_line(fin.filter(|_| ok).unwrap_or(target));
    (ui.say)(&format!("Monado render IPD (read once at session start): {line}"));
    if ok && plan.write_env {
        if let Some(home) = std::env::var_os("HOME") {
            let p = PathBuf::from(home).join(".config/pimax/ipd.env");
            if std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new("."))).is_ok()
                && std::fs::write(&p, format!("{line}\n")).is_ok()
            {
                (ui.say)(&format!("wrote {} (source it before monado-service)", p.display()));
            }
        }
    }
    hold(&ui, if ok { 0 } else { 3 })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::devices::Sim;
    use std::sync::Mutex;

    #[derive(Default)]
    struct Rec(Mutex<Vec<Screen>>);
    impl Sink for Rec {
        fn show(&self, s: Screen) {
            let mut g = self.0.lock().unwrap();
            if g.last() != Some(&s) {
                g.push(s);
            }
        }
    }

    fn plan(apply: bool, mm: Option<f32>) -> Plan {
        Plan { seconds: 0.1, apply, mm, step: Duration::from_millis(1), hold: Duration::from_millis(1), write_env: false }
    }

    fn text(r: &Rec) -> String {
        r.0.lock().unwrap().iter().flat_map(|s| s.lines.iter().map(|l| l.text.clone())).collect::<Vec<_>>().join("|")
    }

    fn go(sim: &mut Sim, p: &Plan) -> (i32, Rec, Vec<String>) {
        let rec = Rec::default();
        let mut log = Vec::new();
        let code = run(sim, &rec, p, &mut |s| log.push(s.to_string()), &AtomicBool::new(false), &|| false);
        (code, rec, log)
    }

    #[test]
    fn measure_only_shows_countdown_and_result() {
        let mut sim = Sim::new(Some(63.0), 64.2, 64.0);
        let (code, rec, _) = go(&mut sim, &plan(false, None));
        assert_eq!(code, 0);
        let t = text(&rec);
        for want in ["Look straight ahead", "3", "2", "1", "Measuring 0 s", "Measured: 64.2 mm", "Lenses: 63.0 mm", "Difference: +1.2 mm"] {
            assert!(t.contains(want), "{want} missing in {t}");
        }
        assert!(sim.sent.is_empty());
    }

    #[test]
    fn apply_moves_lenses_and_reports_done() {
        let mut sim = Sim::new(Some(63.0), 64.2, 64.0);
        let (code, rec, _) = go(&mut sim, &plan(true, None));
        assert_eq!(code, 0);
        assert_eq!(sim.sent, vec![64.2]);
        let t = text(&rec);
        assert!(t.contains("Moving lenses...") && t.contains("Lenses at 64.2 mm"), "{t}");
    }

    #[test]
    fn wrong_measurement_is_refused_in_the_headset() {
        let mut sim = Sim::new(Some(66.01), 52.95, 52.04);
        let (code, rec, log) = go(&mut sim, &plan(true, None));
        assert_eq!(code, 2);
        assert!(sim.sent.is_empty(), "nothing may be sent");
        assert!(text(&rec).contains("Measurement looks wrong"));
        assert!(log.iter().any(|l| l.contains("looks wrong") && l.contains("nothing sent")));
        // explicit --mm overrides
        let mut sim = Sim::new(Some(66.01), 52.95, 52.04);
        let (code, _, _) = go(&mut sim, &plan(true, Some(65.0)));
        assert_eq!((code, sim.sent), (0, vec![65.0]));
    }

    #[test]
    fn errors_are_shown() {
        let mut sim = Sim::new(None, 64.0, 64.0);
        sim.fail = Some("EyeChip busy".into());
        let (code, rec, _) = go(&mut sim, &plan(false, None));
        assert_eq!(code, 2);
        assert!(text(&rec).contains("Eye tracker error") && text(&rec).contains("EyeChip busy"));
    }

    #[test]
    fn wraps_words() {
        assert_eq!(wrap("aaa bbb ccc", 7), vec!["aaa bbb", "ccc"]);
        assert!(wrap("", 5).is_empty());
    }
}
