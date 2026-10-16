//! `pimax-ipd manual`: pick the sharpest IPD by hand. The user types blind with the headset on; the panel shows a
//! sharpness test pattern, the target IPD, the live lens IPD and a status. Keys only change the TARGET; the motor
//! command is sent by the [`Pacer`] after a quiet period, never during a move, never faster than the minimum gap.
//!
//! Everything time-dependent takes the clock as an argument (`now`), so the pacing is tested with a fake clock and a
//! fake [`Motor`]. The only HID write is [`pimax_eye::ipd::send_motor`] (`58 06 5c 05 01 <mm*100 LE>`).

use pimax_eye::ipd;
use pimax_hud::screen::{Line, Screen, CYAN, GREEN, RED, WHITE, YELLOW};
use pimax_hud::Sink;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;

/// Left/Right step and Up/Down step, mm.
pub const STEP_SMALL_MM: f32 = 0.5;
pub const STEP_BIG_MM: f32 = 1.0;
/// A move is finished when the lens reports within this many mm of what was sent.
pub const ARRIVE_TOL_MM: f32 = 0.3;
/// A target this close to the lens reading is "the same value": never sent.
pub const SAME_MM: f32 = 0.05;

/// All the timings of the motor pacing, in one place.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Timing {
    /// Send only after this long without a key press.
    pub quiet: Duration,
    /// At least this long between two sends.
    pub min_gap: Duration,
    /// Give up waiting for the lens to arrive after this long ("motor did not reach X").
    pub move_timeout: Duration,
    /// Main loop period (key wait + lens poll).
    pub poll: Duration,
}

impl Timing {
    pub const REAL: Timing = Timing {
        quiet: Duration::from_millis(700),
        min_gap: Duration::from_millis(1500),
        move_timeout: Duration::from_secs(6),
        poll: Duration::from_millis(50),
    };

    /// Everything divided by `n` (for `--simulate`).
    pub fn scaled(n: u32) -> Timing {
        let t = Timing::REAL;
        Timing { quiet: t.quiet / n, min_gap: t.min_gap / n, move_timeout: t.move_timeout / n, poll: (t.poll / n).max(Duration::from_millis(2)) }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Left,
    Right,
    Up,
    Down,
    Enter,
    Quit,
}

/// Terminal bytes to keys: arrows (`ESC [ A..D`, `ESC O A..D`), Enter, `q`, and a lone Esc (via [`KeyParser::timeout`]).
#[derive(Default)]
pub struct KeyParser {
    state: u8, // 0 idle, 1 after ESC, 2 inside CSI/SS3
}

impl KeyParser {
    pub fn feed(&mut self, b: u8) -> Option<Key> {
        match (self.state, b) {
            (0, 0x1b) => {
                self.state = 1;
                None
            }
            (0, b'\r' | b'\n') => Some(Key::Enter),
            (0, b'q' | b'Q') => Some(Key::Quit),
            (0, b'a' | b'A') => Some(Key::Left),
            (0, b'd' | b'D') => Some(Key::Right),
            (0, b'w' | b'W') => Some(Key::Up),
            (0, b's' | b'S') => Some(Key::Down),
            (0, _) => None,
            (1, b'[' | b'O') => {
                self.state = 2;
                None
            }
            (1, 0x1b) => None, // two Esc: still waiting for what follows
            (1, _) => {
                self.state = 0; // Esc followed by something else: Esc itself does nothing
                None
            }
            (_, 0x40..=0x7e) => {
                self.state = 0;
                match b {
                    b'A' => Some(Key::Up),
                    b'B' => Some(Key::Down),
                    b'C' => Some(Key::Right),
                    b'D' => Some(Key::Left),
                    _ => None,
                }
            }
            _ => None, // parameter bytes
        }
    }

    /// No byte followed the ESC for a while: it was the Esc key, which does nothing now (only `q` quits).
    pub fn timeout(&mut self) -> Option<Key> {
        if self.state == 1 {
            self.state = 0;
        }
        None
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Status {
    Ready,
    /// The target changed; waiting for the keys to settle / the gap / the end of a move.
    Pending,
    Moving(f32),
    /// The lens did not reach this value in time.
    NotReached(f32),
}

/// The pacing state machine (no I/O, no clock of its own).
pub struct Pacer {
    timing: Timing,
    target: f32,
    lens: Option<f32>,
    dirty: bool,
    last_key: Duration,
    last_send: Option<Duration>,
    moving: Option<(f32, Duration)>,
    failed: Option<f32>,
}

impl Pacer {
    /// Target = the current lens IPD; nothing is sent until a key changes it.
    pub fn new(lens: f32, timing: Timing) -> Pacer {
        Pacer {
            timing,
            target: round2(ipd::clamp_mm(lens)),
            lens: Some(lens),
            dirty: false,
            last_key: Duration::ZERO,
            last_send: None,
            moving: None,
            failed: None,
        }
    }

    pub fn target(&self) -> f32 {
        self.target
    }

    pub fn lens(&self) -> Option<f32> {
        self.lens
    }

    /// A key press: only the target changes (clamped 58-72 mm). Enter / Quit are not handled here.
    pub fn key(&mut self, now: Duration, k: Key) {
        let d = match k {
            Key::Left => -STEP_SMALL_MM,
            Key::Right => STEP_SMALL_MM,
            Key::Up => STEP_BIG_MM,
            Key::Down => -STEP_BIG_MM,
            Key::Enter | Key::Quit => return,
        };
        self.target = round2(ipd::clamp_mm(self.target + d));
        self.dirty = true;
        self.last_key = now;
    }

    pub fn status(&self) -> Status {
        if let Some((to, _)) = self.moving {
            Status::Moving(to)
        } else if self.dirty {
            Status::Pending
        } else if let Some(x) = self.failed {
            Status::NotReached(x)
        } else {
            Status::Ready
        }
    }

    /// Nothing waiting and no move in progress.
    pub fn settled(&self) -> bool {
        self.moving.is_none() && !self.dirty
    }

    /// The send itself failed: no move is in progress, the value was not reached.
    pub fn send_failed(&mut self, to: f32) {
        self.moving = None;
        self.failed = Some(to);
    }

    /// Feed the newest lens reading (None = no new reading) and ask whether to send now. `Some(mm)` is returned at
    /// most once per decision; the caller must send exactly that value.
    pub fn tick(&mut self, now: Duration, lens: Option<f32>) -> Option<f32> {
        if lens.is_some() {
            self.lens = lens;
        }
        if let Some((to, since)) = self.moving {
            if self.lens.is_some_and(|l| (l - to).abs() <= ARRIVE_TOL_MM) {
                self.moving = None;
                self.failed = None;
            } else if now.saturating_sub(since) >= self.timing.move_timeout {
                self.moving = None;
                self.failed = Some(to);
            }
        }
        if self.moving.is_some() || !self.dirty {
            return None;
        }
        if now.saturating_sub(self.last_key) < self.timing.quiet {
            return None;
        }
        if self.last_send.is_some_and(|t| now.saturating_sub(t) < self.timing.min_gap) {
            return None;
        }
        if self.lens.is_some_and(|l| (l - self.target).abs() < SAME_MM) {
            self.dirty = false; // already there
            return None;
        }
        self.dirty = false;
        self.moving = Some((self.target, now));
        self.last_send = Some(now);
        self.failed = None;
        Some(self.target)
    }
}

fn round2(x: f32) -> f32 {
    (x * 100.0).round() / 100.0
}

/// What the loop needs from the headset.
pub trait Motor {
    /// The newest live lens IPD (may block briefly), `None` when no report arrived.
    fn lens(&mut self) -> Option<f32>;
    /// The one HID write. Returns a description for the log.
    fn send(&mut self, mm: f32) -> Result<String, String>;
}

/// One loop iteration of reading the lens, pacing and (maybe) sending. Logs sends and move results.
pub fn step(p: &mut Pacer, m: &mut dyn Motor, now: Duration, say: &mut dyn FnMut(&str)) {
    let lens = m.lens();
    let before = p.status();
    let send = p.tick(now, lens);
    let stamp = format!("[{:7.3}s]", now.as_secs_f64());
    let lens_s = lens.or(p.lens()).map_or("?".into(), |l| format!("{l:.2}"));
    if let Status::Moving(to) = before {
        match p.status() {
            Status::NotReached(_) => say(&format!("{stamp} motor did not reach {to:.2} mm (lens {lens_s} mm)")),
            Status::Moving(_) => {}
            _ if send.is_none() || send != Some(to) => say(&format!("{stamp} arrived: lens {lens_s} mm (sent {to:.2})")),
            _ => {}
        }
    }
    if let Some(mm) = send {
        let before_s = lens_s;
        match m.send(mm) {
            Ok(d) => say(&format!("{stamp} SEND {mm:.2} mm (target {:.2}, lens before {before_s} mm): {d}", p.target())),
            Err(e) => {
                say(&format!("{stamp} send {mm:.2} mm FAILED: {e}"));
                p.send_failed(mm);
            }
        }
    }
}

pub fn screen(p: &Pacer, finishing: bool) -> Screen {
    let lens = p.lens().map_or("unknown".to_string(), |l| format!("{l:.2} mm"));
    let (status, color) = match p.status() {
        Status::Moving(_) => ("moving...".to_string(), YELLOW),
        Status::Pending => ("target changed, sending soon".to_string(), CYAN),
        Status::NotReached(x) => (format!("motor did not reach {x:.1} mm"), RED),
        Status::Ready if finishing => ("saving".to_string(), GREEN),
        Status::Ready => ("ready".to_string(), GREEN),
    };
    Screen {
        lines: vec![
            Line::new(format!("Target {:.1} mm", p.target())).scale(5).color(WHITE),
            Line::new(format!("Lens {lens}")).scale(5).color(CYAN),
            Line::new(status).scale(5).color(color),
            Line::new("A/D 0.5 mm   W/S 1 mm   Enter save   Q quit").scale(2),
        ],
        progress: None,
        pattern: true,
    }
}

/// Result of [`run`].
#[derive(Debug, PartialEq)]
pub enum Outcome {
    Saved(f32),
    Quit,
    /// Enter, but no lens reading to save.
    NoLens,
}

/// The interactive loop. `clock` is the time since start. Enter waits until the motor is settled, then calls `save`
/// with the lens IPD; q (or `stop`) exits at once without sending anything more.
#[allow(clippy::too_many_arguments)]
pub fn run(
    p: &mut Pacer,
    m: &mut dyn Motor,
    sink: &dyn Sink,
    keys: &Receiver<Key>,
    clock: &dyn Fn() -> Duration,
    say: &mut dyn FnMut(&str),
    save: &mut dyn FnMut(f32) -> Result<String, String>,
    stop: &dyn Fn() -> bool,
) -> Outcome {
    let mut finishing = false;
    let poll = p.timing.poll;
    sink.show(screen(p, false));
    loop {
        if stop() {
            say("quit: interrupted (Ctrl-C or the headset screen ended), no further moves");
            return Outcome::Quit;
        }
        match keys.recv_timeout(poll) {
            Ok(Key::Quit) => {
                say("quit: q pressed, no further moves");
                return Outcome::Quit;
            }
            Ok(Key::Enter) => {
                finishing = true;
                say("Enter: waiting for the motor to settle, then saving");
            }
            Ok(k) => {
                if !finishing {
                    p.key(clock(), k);
                    say(&format!("[{:7.3}s] target {:.2} mm", clock().as_secs_f64(), p.target()));
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) if finishing => std::thread::sleep(poll),
            Err(RecvTimeoutError::Disconnected) => {
                say("quit: keyboard input ended (stdin closed or not a terminal), no further moves");
                return Outcome::Quit;
            }
        }
        step(p, m, clock(), say);
        sink.show(screen(p, finishing));
        if finishing && p.settled() {
            return match p.lens() {
                Some(l) => {
                    match save(l) {
                        Ok(d) => say(&d),
                        Err(e) => say(&format!("could not save: {e}")),
                    }
                    Outcome::Saved(l)
                }
                None => {
                    say("no lens reading: nothing saved");
                    Outcome::NoLens
                }
            };
        }
    }
}

/// Write `OXR_DEBUG_IPD_MM=<mm>` to ~/.config/pimax/ipd.env (same as `pimax-eye ipd --apply`).
pub fn save_env(mm: f32) -> Result<String, String> {
    let home = std::env::var_os("HOME").ok_or("HOME is not set")?;
    let p = PathBuf::from(home).join(".config/pimax/ipd.env");
    let line = ipd::monado_line(mm);
    std::fs::create_dir_all(p.parent().unwrap_or(std::path::Path::new("."))).map_err(|e| e.to_string())?;
    std::fs::write(&p, format!("{line}\n")).map_err(|e| format!("{}: {e}", p.display()))?;
    Ok(format!("wrote {}: {line}", p.display()))
}

/// The real headset: info reports on interface 2, the one motor write on interface 0.
pub struct RealMotor {
    info: PathBuf,
    cmd: PathBuf,
}

impl RealMotor {
    pub fn new() -> Result<RealMotor, String> {
        let (hc, dev) = (std::path::Path::new("/sys/class/hidraw"), std::path::Path::new("/dev"));
        let info = ipd::headset_node(hc, dev, "02").ok_or("no hidraw node for interface 2 of 34a4:0044 (headset not connected?)")?;
        let cmd = ipd::headset_node(hc, dev, "00").ok_or("no hidraw node for interface 0 of 34a4:0044")?;
        Ok(RealMotor { info, cmd })
    }
}

impl Motor for RealMotor {
    fn lens(&mut self) -> Option<f32> {
        ipd::read_lens_ipd(&self.info, Duration::from_millis(100))
    }

    fn send(&mut self, mm: f32) -> Result<String, String> {
        let rep = ipd::motor_report(mm);
        ipd::send_motor(&self.cmd, mm).map_err(|e| format!("{}: {e}", self.cmd.display()))?;
        Ok(format!(
            "sent to {}: {}",
            self.cmd.display(),
            rep[..7].iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(" ")
        ))
    }
}

/// A fake lens for `--simulate`: arrives `delay` after a send.
pub struct SimMotor {
    pub lens: f32,
    pub delay: Duration,
    pub sent: Vec<f32>,
    moving: Option<(f32, std::time::Instant)>,
}

impl SimMotor {
    pub fn new(lens: f32, delay: Duration) -> SimMotor {
        SimMotor { lens, delay, sent: vec![], moving: None }
    }
}

impl Motor for SimMotor {
    fn lens(&mut self) -> Option<f32> {
        if let Some((to, t)) = self.moving {
            if t.elapsed() >= self.delay {
                self.lens = to;
                self.moving = None;
            }
        }
        Some(self.lens)
    }

    fn send(&mut self, mm: f32) -> Result<String, String> {
        self.sent.push(mm);
        self.moving = Some((mm, std::time::Instant::now()));
        Ok(format!("simulated: would send lens IPD {mm:.2} mm"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Timing = Timing::REAL;
    fn ms(n: u64) -> Duration {
        Duration::from_millis(n)
    }

    /// Fake motor on a fake clock: the lens reaches a sent value `delay` later (never when `stuck`).
    struct Fake {
        now: Duration,
        lens: f32,
        delay: Duration,
        stuck: bool,
        moving: Option<(f32, Duration)>,
        sends: Vec<(Duration, f32)>,
    }
    impl Motor for Fake {
        fn lens(&mut self) -> Option<f32> {
            if let Some((to, t)) = self.moving {
                if self.now >= t + self.delay && !self.stuck {
                    self.lens = to;
                    self.moving = None;
                }
            }
            Some(self.lens)
        }
        fn send(&mut self, mm: f32) -> Result<String, String> {
            self.sends.push((self.now, mm));
            self.moving = Some((mm, self.now));
            Ok("fake".into())
        }
    }

    fn fake(lens: f32, delay_ms: u64) -> Fake {
        Fake { now: Duration::ZERO, lens, delay: ms(delay_ms), stuck: false, moving: None, sends: vec![] }
    }

    /// Run the pacer on a 10 ms fake clock until `until_ms`, pressing `keys` at their times.
    fn drive(p: &mut Pacer, f: &mut Fake, keys: &[(u64, Key)], until_ms: u64) -> Vec<String> {
        let mut log = Vec::new();
        let mut t = 0;
        while t <= until_ms {
            f.now = ms(t);
            for (_, k) in keys.iter().filter(|(kt, _)| *kt == t) {
                p.key(f.now, *k);
            }
            let n = f.now;
            step(p, f, n, &mut |s| log.push(s.to_string()));
            t += 10;
        }
        log
    }

    #[test]
    fn starts_at_the_lens_value_and_sends_nothing() {
        let mut p = Pacer::new(63.27, T);
        let mut f = fake(63.27, 500);
        drive(&mut p, &mut f, &[], 5000);
        assert_eq!(p.target(), 63.27);
        assert!(f.sends.is_empty());
        assert_eq!(p.status(), Status::Ready);
    }

    #[test]
    fn a_burst_of_keys_sends_once_after_the_quiet_period() {
        let mut p = Pacer::new(63.0, T);
        let mut f = fake(63.0, 500);
        let keys: Vec<_> = (0..6).map(|i| (100 + i * 120, Key::Right)).collect(); // last key at 700 ms
        let log = drive(&mut p, &mut f, &keys, 4000);
        assert_eq!(f.sends.len(), 1, "{log:?}");
        assert_eq!(f.sends[0].1, 66.0);
        assert_eq!(f.sends[0].0, ms(700 + 700)); // last key + 700 ms
        assert!(log.iter().any(|l| l.contains("SEND 66.00")));
        assert_eq!(p.status(), Status::Ready);
    }

    #[test]
    fn nothing_is_sent_during_a_move_and_the_newest_target_wins() {
        let mut p = Pacer::new(63.0, T);
        let mut f = fake(63.0, 2000); // slow motor
        // first move to 63.5; while it moves (until 2700 ms): 64.0, 64.5, 65.5; the quiet period is over mid-move
        let keys = [(0, Key::Right), (1000, Key::Right), (1200, Key::Right), (1400, Key::Up)];
        drive(&mut p, &mut f, &keys, 12_000);
        let sent: Vec<f32> = f.sends.iter().map(|s| s.1).collect();
        assert_eq!(sent, vec![63.5, 65.5], "{:?}", f.sends);
        // second send only after the first move finished (700 + 2000 ms)
        assert!(f.sends[1].0 >= f.sends[0].0 + ms(2000));
        assert_eq!(p.status(), Status::Ready);
    }

    #[test]
    fn minimum_gap_between_sends_holds() {
        let mut p = Pacer::new(63.0, T);
        let mut f = fake(63.0, 100); // arrives quickly
        let keys = [(0, Key::Right), (1000, Key::Right)]; // 2nd quiet period ends at 1700 < 700 + 1500
        drive(&mut p, &mut f, &keys, 6000);
        assert_eq!(f.sends.len(), 2);
        assert_eq!(f.sends[0].0, ms(700));
        assert_eq!(f.sends[1].0, ms(700 + 1500));
        for w in f.sends.windows(2) {
            assert!(w[1].0 - w[0].0 >= T.min_gap);
        }
    }

    #[test]
    fn never_resends_the_value_the_lens_already_reports() {
        let mut p = Pacer::new(63.0, T);
        let mut f = fake(63.0, 100);
        // right then left: target is back at 63.0 = lens
        drive(&mut p, &mut f, &[(0, Key::Right), (100, Key::Left)], 4000);
        assert!(f.sends.is_empty());
        assert_eq!(p.status(), Status::Ready);
    }

    #[test]
    fn clamps_to_58_72() {
        let mut p = Pacer::new(71.2, T);
        for _ in 0..5 {
            p.key(Duration::ZERO, Key::Up);
        }
        assert_eq!(p.target(), 72.0);
        for _ in 0..40 {
            p.key(Duration::ZERO, Key::Down);
        }
        assert_eq!(p.target(), 58.0);
        p.key(Duration::ZERO, Key::Left);
        assert_eq!(p.target(), 58.0);
        // a lens reading outside the range still starts inside it
        assert_eq!(Pacer::new(75.0, T).target(), 72.0);
    }

    #[test]
    fn timeout_is_reported_and_not_resent_until_a_new_key() {
        let mut p = Pacer::new(63.0, T);
        let mut f = fake(63.0, 100);
        f.stuck = true;
        let log = drive(&mut p, &mut f, &[(0, Key::Up)], 20_000);
        assert_eq!(f.sends.len(), 1, "no resend of an unreached value");
        assert_eq!(p.status(), Status::NotReached(64.0));
        assert!(log.iter().any(|l| l.contains("motor did not reach 64.00")));
        assert!(screen(&p, false).lines.iter().any(|l| l.text.contains("motor did not reach 64.0")));
        assert!(f.sends[0].0 == ms(700));
        // the user presses again (target 65): sent once the old move timed out
        p.key(ms(20_000), Key::Up);
        f.now = ms(21_000);
        let n = f.now;
        step(&mut p, &mut f, n, &mut |_| {});
        assert_eq!(f.sends.len(), 2);
    }

    #[test]
    fn failed_send_does_not_leave_the_pacer_moving() {
        struct Bad;
        impl Motor for Bad {
            fn lens(&mut self) -> Option<f32> {
                Some(63.0)
            }
            fn send(&mut self, _: f32) -> Result<String, String> {
                Err("nope".into())
            }
        }
        let mut p = Pacer::new(63.0, T);
        p.key(Duration::ZERO, Key::Up);
        let mut log = vec![];
        step(&mut p, &mut Bad, ms(800), &mut |s| log.push(s.to_string()));
        assert_eq!(p.status(), Status::NotReached(64.0));
        assert!(log[0].contains("FAILED"));
    }

    #[test]
    fn key_parser() {
        let mut k = KeyParser::default();
        let mut feed = |bytes: &[u8]| bytes.iter().filter_map(|&b| k.feed(b)).collect::<Vec<_>>();
        assert_eq!(feed(b"\x1b[D\x1b[C\x1b[A\x1b[B"), vec![Key::Left, Key::Right, Key::Up, Key::Down]);
        assert_eq!(feed(b"\x1bOA"), vec![Key::Up]);
        assert_eq!(feed(b"\x1b[1;5C"), vec![Key::Right]);
        assert_eq!(feed(b"\r"), vec![Key::Enter]);
        assert_eq!(feed(b"q"), vec![Key::Quit]);
        assert_eq!(feed(b"x\x1b[H"), vec![]);
        // a sequence split across reads (bytes arrive one at a time, no timeout in between)
        let mut k = KeyParser::default();
        assert_eq!(k.feed(0x1b), None);
        assert_eq!(k.feed(b'['), None);
        assert_eq!(k.feed(b'C'), Some(Key::Right));
        assert_eq!(k.timeout(), None);
        assert_eq!(feed_all(b"wasd"), vec![Key::Up, Key::Left, Key::Down, Key::Right]);
        // lone Esc
        let mut k = KeyParser::default();
        assert_eq!(k.feed(0x1b), None);
        assert_eq!(k.timeout(), None); // a lone Esc is ignored, and does not break the next sequence
        assert_eq!(k.feed(b'['), None);
        assert_eq!(feed_all(b"\x1b\x1b[A"), vec![Key::Up]);
        assert_eq!(feed_all(b"\x1bx"), vec![]);
    }

    fn feed_all(b: &[u8]) -> Vec<Key> {
        let mut k = KeyParser::default();
        b.iter().filter_map(|&x| k.feed(x)).collect()
    }

    #[test]
    fn closed_keyboard_quits_with_a_reason_but_not_while_saving() {
        let mut p = Pacer::new(63.0, Timing::scaled(50));
        let mut m = SimMotor::new(63.0, ms(20));
        let (tx, rx) = std::sync::mpsc::channel::<Key>();
        drop(tx);
        let t0 = std::time::Instant::now();
        let mut log = vec![];
        let out = run(&mut p, &mut m, &Rec::default(), &rx, &|| t0.elapsed(), &mut |s| log.push(s.to_string()), &mut |_| Ok("".into()), &|| false);
        assert_eq!(out, Outcome::Quit);
        assert!(log.iter().any(|l| l.contains("keyboard input ended")), "{log:?}");
    }

    #[derive(Default)]
    struct Rec(std::sync::Mutex<Vec<Screen>>);
    impl Sink for Rec {
        fn show(&self, s: Screen) {
            self.0.lock().unwrap().push(s);
        }
    }

    #[test]
    fn run_loop_coalesces_keys_and_enter_saves_the_settled_lens() {
        let timing = Timing::scaled(50); // quiet 14 ms, gap 30 ms, move timeout 120 ms
        let mut p = Pacer::new(63.0, timing);
        let mut m = SimMotor::new(63.0, ms(20));
        let (tx, rx) = std::sync::mpsc::channel();
        for k in [Key::Right, Key::Right, Key::Up, Key::Enter] {
            tx.send(k).unwrap();
        }
        let t0 = std::time::Instant::now();
        let rec = Rec::default();
        let mut saved = vec![];
        let mut log = vec![];
        let out = run(
            &mut p,
            &mut m,
            &rec,
            &rx,
            &|| t0.elapsed(),
            &mut |s| log.push(s.to_string()),
            &mut |mm| {
                saved.push(mm);
                Ok("saved".into())
            },
            &|| false,
        );
        assert_eq!(m.sent, vec![65.0], "{log:?}");
        assert_eq!(out, Outcome::Saved(65.0));
        assert_eq!(saved, vec![65.0]);
        let shown = rec.0.lock().unwrap();
        assert!(shown.iter().all(|s| s.pattern));
        assert!(shown.iter().any(|s| s.lines.iter().any(|l| l.text == "moving...")));
        assert!(shown.last().unwrap().lines.iter().any(|l| l.text.contains("Lens 65.00")));
    }

    #[test]
    fn quit_sends_nothing_more() {
        let mut p = Pacer::new(63.0, Timing::scaled(50));
        let mut m = SimMotor::new(63.0, ms(20));
        let (tx, rx) = std::sync::mpsc::channel();
        tx.send(Key::Right).unwrap();
        tx.send(Key::Quit).unwrap();
        let t0 = std::time::Instant::now();
        let out = run(&mut p, &mut m, &Rec::default(), &rx, &|| t0.elapsed(), &mut |_| {}, &mut |_| Err("no".into()), &|| false);
        assert_eq!(out, Outcome::Quit);
        assert!(m.sent.is_empty());
    }
}
