//! `pimax-eye ipd --watch`: full-screen terminal view of the IPD measured from the eyes (chip field 34 and the gated
//! distance between the two gaze origins, last 3 s) against the live lens IPD from the headset info report, with
//! keys to drive the lens motor (`a` apply measured, `+`/`-` target +-0.5 mm after a 1 s pause, `q` quit). Every
//! motor command is the one [`crate::ipd::send_motor`] frame, at most one per second. Pure logic and rendering live
//! here; [`run`] is the terminal loop.

use crate::ipd::{self, Measure, Measurement};
use crate::track::Sample;
use std::collections::VecDeque;
use std::fmt::Write as _;
use std::path::Path;
use std::time::{Duration, Instant};

/// Window of the eye measurement, seconds of device time.
pub const WINDOW_S: f64 = 3.0;
/// Minimum spacing between two motor commands.
pub const MIN_GAP: Duration = Duration::from_secs(1);
/// Pause after a `+`/`-` press before the command goes out (further presses extend it).
pub const KEY_PAUSE: Duration = Duration::from_secs(1);
/// `+`/`-` step, mm.
pub const STEP_MM: f32 = 0.5;
const SCALE_COLS: usize = 57; // 58..72 mm at 4 columns per mm

/// Eye-side view of the last [`WINDOW_S`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct EyeView {
    pub m: Option<Measurement>,
    pub total: usize,
    /// Share of window samples per eye with a valid origin within the gate of that eye's median, percent.
    pub good_pct: [f64; 2],
}

/// Rolling window of samples.
#[derive(Default)]
pub struct Window {
    buf: VecDeque<(f64, Sample)>,
    pub samples: u64,
}

fn med(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else {
        v[v.len() / 2]
    }
}

impl Window {
    pub fn push(&mut self, s: &Sample) {
        let t = s.device_ts_us as f64 / 1e6;
        self.samples += 1;
        self.buf.push_back((t, *s));
        while self.buf.front().is_some_and(|f| t - f.0 > WINDOW_S) {
            self.buf.pop_front();
        }
    }

    pub fn view(&self) -> EyeView {
        let mut me = Measure::default();
        for (_, s) in &self.buf {
            me.push(s);
        }
        let r = me.result();
        let mut good_pct = [0.0; 2];
        for (k, g) in good_pct.iter_mut().enumerate() {
            let mut c = [[0.0f32; 3]; 1];
            let pts: Vec<[f32; 3]> = self.buf.iter().filter(|b| b.1.eyes[k].origin_valid).map(|b| b.1.eyes[k].origin.map(|v| v * 1000.0)).collect();
            for (i, ci) in c[0].iter_mut().enumerate() {
                *ci = med(&mut pts.iter().map(|p| p[i]).collect::<Vec<_>>());
            }
            let good = pts
                .iter()
                .filter(|p| ((p[0] - c[0][0]).powi(2) + (p[1] - c[0][1]).powi(2) + (p[2] - c[0][2]).powi(2)).sqrt() <= ipd::REJECT_MM)
                .count();
            if !self.buf.is_empty() {
                *g = 100.0 * good as f64 / self.buf.len() as f64;
            }
        }
        EyeView { m: (r.good > 0).then_some(r), total: self.buf.len(), good_pct }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    Apply,
    Plus,
    Minus,
    Quit,
}

pub fn key_from_byte(b: u8) -> Option<Key> {
    match b {
        b'a' | b'A' => Some(Key::Apply),
        b'+' | b'=' => Some(Key::Plus),
        b'-' | b'_' => Some(Key::Minus),
        b'q' | b'Q' | 3 => Some(Key::Quit),
        _ => None,
    }
}

/// Key -> motor command logic with the 1 s throttle. Time is passed in so tests need no sleeping.
#[derive(Default)]
pub struct Controller {
    /// Requested lens IPD (what the user asked for), mm.
    pub target: Option<f32>,
    pending: Option<(f32, Instant)>,
    last_sent: Option<Instant>,
    /// Last value actually written to the motor.
    pub applied: Option<f32>,
    pub quit: bool,
    pub msg: String,
}

impl Controller {
    /// Handle a key. `m` is the current eye measurement, `lens` the live lens IPD.
    pub fn key(&mut self, k: Key, now: Instant, m: Option<&Measurement>, lens: Option<f32>) {
        match k {
            Key::Quit => self.quit = true,
            Key::Apply => match m {
                None => self.msg = "apply refused: no good eye samples in the last 3 s".into(),
                Some(m) => match ipd::decide(m, None, lens) {
                    Ok((t, note)) => {
                        self.target = Some(t);
                        self.pending = Some((t, now));
                        self.msg = note.unwrap_or_else(|| format!("applying measured {t:.2} mm"));
                    }
                    Err(e) => self.msg = format!("apply refused: {e}"),
                },
            },
            Key::Plus | Key::Minus => {
                let base = self.target.or(lens).or_else(|| m.map(|m| m.field34_mm));
                let Some(base) = base else {
                    self.msg = "no lens reading and no measurement: nothing to step from".into();
                    return;
                };
                let d = if k == Key::Plus { STEP_MM } else { -STEP_MM };
                let t = ipd::clamp_mm(((base + d) / STEP_MM).round() * STEP_MM);
                self.target = Some(t);
                self.pending = Some((t, now + KEY_PAUSE));
                self.msg = format!("target {t:.1} mm, sending after a 1 s pause");
            }
        }
    }

    /// The value to send now, if a request is due and the 1 s gap since the last command has passed.
    pub fn poll(&mut self, now: Instant) -> Option<f32> {
        let (t, due) = self.pending?;
        if now < due || self.last_sent.is_some_and(|l| now.duration_since(l) < MIN_GAP) {
            return None;
        }
        self.pending = None;
        self.last_sent = Some(now);
        Some(t)
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }
}

const FONT: [[&str; 5]; 11] = [
    ["███", "█ █", "█ █", "█ █", "███"],
    ["  █", "  █", "  █", "  █", "  █"],
    ["███", "  █", "███", "█  ", "███"],
    ["███", "  █", "███", "  █", "███"],
    ["█ █", "█ █", "███", "  █", "  █"],
    ["███", "█  ", "███", "  █", "███"],
    ["███", "█  ", "███", "█ █", "███"],
    ["███", "  █", "  █", "  █", "  █"],
    ["███", "█ █", "███", "█ █", "███"],
    ["███", "█ █", "███", "  █", "███"],
    [" ", " ", " ", " ", "."],
];

/// Five text rows of big digits for `v` ("--.--" when `None`).
pub fn big(v: Option<f32>) -> [String; 5] {
    let text = v.map_or("--.--".to_string(), |v| format!("{v:.2}"));
    let mut rows: [String; 5] = Default::default();
    for ch in text.chars() {
        for (r, row) in rows.iter_mut().enumerate() {
            match ch {
                '0'..='9' => row.push_str(FONT[ch as usize - '0' as usize][r]),
                '.' => row.push_str(FONT[10][r]),
                _ => row.push_str(["   ", "   ", "███", "   ", "   "][r]),
            }
            row.push(' ');
        }
    }
    rows
}

fn col(mm: f32) -> usize {
    (((mm.clamp(ipd::MIN_MM, ipd::MAX_MM) - ipd::MIN_MM) * 4.0).round() as usize).min(SCALE_COLS - 1)
}

/// Difference colour: green < 1 mm, yellow 1-3 mm, red above.
pub fn diff_color(d: f32) -> &'static str {
    if d < 1.0 {
        "\x1b[1;32m"
    } else if d <= 3.0 {
        "\x1b[1;33m"
    } else {
        "\x1b[1;31m"
    }
}

/// What the screen shows.
pub struct View<'a> {
    pub eyes: &'a EyeView,
    pub lens: Option<f32>,
    pub target: Option<f32>,
    pub pending: bool,
    pub msg: &'a str,
    pub link: &'a str,
    pub samples: u64,
}

/// The eye IPD shown and compared: field 34, or the origin distance when field 34 is missing.
pub fn eyes_mm(v: &EyeView) -> Option<f32> {
    v.m.as_ref().map(|m| if m.field34_mm > 0.0 { m.field34_mm } else { m.origin_mm })
}

/// One full screen (ANSI colours, no cursor movement; the caller homes the cursor).
pub fn render(v: &View) -> String {
    let mut o = String::new();
    let e = eyes_mm(v.eyes);
    let _ = writeln!(o, "\x1b[1mpimax-eye ipd --watch\x1b[0m  link={}  samples={}\x1b[K", v.link, v.samples);
    let _ = writeln!(o, "\x1b[K");
    let _ = writeln!(o, "\x1b[1;36mYOUR EYES\x1b[0m (E)  mm\x1b[K");
    for r in big(e) {
        let _ = writeln!(o, "  \x1b[1;36m{r}\x1b[0m\x1b[K");
    }
    match &v.eyes.m {
        Some(m) => {
            let _ = writeln!(o, "  field 34 {:.2} mm   origin distance {:.2} mm   ({} good of {} in 3 s)\x1b[K", m.field34_mm, m.origin_mm, m.good, v.eyes.total);
        }
        None => {
            let _ = writeln!(o, "  no good eye samples (put the headset on)\x1b[K");
        }
    }
    let _ = writeln!(o, "\x1b[K");
    let _ = writeln!(o, "\x1b[1;35mLENSES\x1b[0m (L)  mm{}\x1b[K", v.target.map_or(String::new(), |t| format!("   target {t:.1}{}", if v.pending { " (pending)" } else { "" })));
    for r in big(v.lens) {
        let _ = writeln!(o, "  \x1b[1;35m{r}\x1b[0m\x1b[K");
    }
    let _ = writeln!(o, "\x1b[K");
    // scale
    let (ce, cl) = (e.map(col), v.lens.map(col));
    let mut letters = vec![b' '; SCALE_COLS];
    if let Some(c) = ce {
        letters[c] = b'E';
    }
    if let Some(c) = cl {
        letters[c] = if ce == Some(c) { b'*' } else { b'L' };
    }
    let _ = writeln!(o, "  {}\x1b[K", String::from_utf8_lossy(&letters));
    let mut bar: Vec<char> = (0..SCALE_COLS).map(|c| if c % 8 == 0 { '+' } else { '-' }).collect();
    if let Some(c) = ce {
        bar[c] = 'v';
    }
    if let Some(c) = cl {
        bar[c] = if ce == Some(c) { '*' } else { '^' };
    }
    let _ = writeln!(o, "  {}\x1b[K", bar.iter().collect::<String>());
    let mut labels = String::new();
    for mm in (58..=72).step_by(2) {
        let _ = write!(labels, "{mm:<8}");
    }
    let _ = writeln!(o, "  {}mm\x1b[K", labels.trim_end());
    match (e, v.lens) {
        (Some(e), Some(l)) => {
            let d = (e - l).abs();
            let _ = writeln!(o, "\n  \x1b[K\x1b[1mdifference\x1b[0m {}{d:.2} mm\x1b[0m  (eyes {e:.2}, lenses {l:.2})\x1b[K", diff_color(d));
        }
        _ => {
            let _ = writeln!(o, "\n  \x1b[Kdifference n/a\x1b[K");
        }
    }
    let _ = writeln!(o, "\x1b[K");
    let _ = writeln!(o, "  good samples (3 s)   L {:5.1}%   R {:5.1}%\x1b[K", v.eyes.good_pct[0], v.eyes.good_pct[1]);
    let _ = writeln!(o, "\x1b[K");
    let _ = writeln!(o, "  {}\x1b[K", v.msg);
    let _ = writeln!(o, "  keys: a apply measured   + / - target +-0.5 mm   q quit\x1b[K");
    o
}

struct RawTerm(Option<libc::termios>);

#[allow(unsafe_code)]
impl RawTerm {
    fn enter() -> RawTerm {
        // SAFETY: plain termios calls on fd 0 with a zeroed struct filled by tcgetattr.
        unsafe {
            let mut t: libc::termios = std::mem::zeroed();
            if libc::isatty(0) == 0 || libc::tcgetattr(0, &mut t) != 0 {
                return RawTerm(None);
            }
            let old = t;
            t.c_lflag &= !(libc::ICANON | libc::ECHO);
            t.c_cc[libc::VMIN] = 0;
            t.c_cc[libc::VTIME] = 0;
            libc::tcsetattr(0, libc::TCSANOW, &t);
            RawTerm(Some(old))
        }
    }
}

#[allow(unsafe_code)]
impl Drop for RawTerm {
    fn drop(&mut self) {
        if let Some(t) = &self.0 {
            // SAFETY: restores the attributes saved in `enter`.
            unsafe { libc::tcsetattr(0, libc::TCSANOW, t) };
        }
    }
}

/// Wait up to `ms` for stdin and return the bytes read.
#[allow(unsafe_code)]
fn read_keys(ms: i32) -> Vec<u8> {
    let mut p = libc::pollfd { fd: 0, events: libc::POLLIN, revents: 0 };
    let mut buf = [0u8; 16];
    // SAFETY: one valid pollfd and a valid buffer of the stated length.
    unsafe {
        if libc::poll(&mut p, 1, ms) > 0 && p.revents & libc::POLLIN != 0 {
            let n = libc::read(0, buf.as_mut_ptr().cast(), buf.len());
            if n > 0 {
                return buf[..n as usize].to_vec();
            }
        }
    }
    Vec::new()
}

/// The terminal loop. `info`/`cmd` are the hidraw nodes of IF2 and IF0. Returns (exit code, message for errors).
pub fn run(cfg: crate::live::Config, info: Option<std::path::PathBuf>, cmd: std::path::PathBuf, out: &mut dyn std::io::Write) -> Result<i32, String> {
    use crate::live::{State, Tracker};
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
    use std::sync::Arc;
    let lens_bits = Arc::new(AtomicU32::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let reader = info.map(|n| {
        let (lb, st) = (lens_bits.clone(), stop.clone());
        std::thread::spawn(move || {
            while !st.load(Ordering::Relaxed) {
                match ipd::read_lens_ipd(&n, Duration::from_millis(200)) {
                    Some(v) => lb.store(v.to_bits(), Ordering::Relaxed),
                    None => std::thread::sleep(Duration::from_millis(50)),
                }
            }
        })
    });
    crate::sigint::install();
    let tr = Tracker::start(cfg);
    let raw = RawTerm::enter();
    let _ = write!(out, "\x1b[?1049h\x1b[?25l\x1b[2J");
    let (mut win, mut ctl, mut last_seq) = (Window::default(), Controller::default(), None);
    while !ctl.quit && !crate::sigint::stopped() {
        for b in read_keys(100) {
            if let Some(k) = key_from_byte(b) {
                let view = win.view();
                let lens = lens_opt(&lens_bits);
                ctl.key(k, Instant::now(), view.m.as_ref(), lens);
            }
        }
        for s in tr.samples_since(last_seq) {
            last_seq = Some(s.seq);
            win.push(&s);
        }
        if let Some(t) = ctl.poll(Instant::now()) {
            match ipd::send_motor(&cmd, t) {
                Ok(()) => {
                    ctl.applied = Some(t);
                    ctl.msg = format!("sent {t:.2} mm to the lens motor");
                }
                Err(e) => ctl.msg = format!("motor write failed: {}: {e}", cmd.display()),
            }
        }
        let st = tr.state();
        let link = if st == State::Streaming { "streaming".to_string() } else { format!("{st:?}") };
        let view = win.view();
        let text = render(&View {
            eyes: &view,
            lens: lens_opt(&lens_bits),
            target: ctl.target,
            pending: ctl.has_pending(),
            msg: &ctl.msg,
            link: &link,
            samples: win.samples,
        });
        let _ = write!(out, "\x1b[H{text}\x1b[J");
        let _ = out.flush();
    }
    let _ = write!(out, "\x1b[?25h\x1b[?1049l");
    drop(raw);
    tr.stop();
    stop.store(true, Ordering::Relaxed);
    if let Some(h) = reader {
        let _ = h.join();
    }
    if let Some(t) = ctl.applied {
        let line = ipd::monado_line(t);
        let _ = writeln!(out, "last applied lens IPD {t:.2} mm; Monado render IPD (read once at session start): {line}");
        if let Some(home) = std::env::var_os("HOME") {
            let p = std::path::PathBuf::from(home).join(".config/omarchyvr/ipd.env");
            write_env(&p, &line, out);
        }
    }
    Ok(0)
}

fn write_env(p: &Path, line: &str, out: &mut dyn std::io::Write) {
    if std::fs::create_dir_all(p.parent().unwrap_or(Path::new("."))).is_ok() && std::fs::write(p, format!("{line}\n")).is_ok() {
        let _ = writeln!(out, "wrote {} (source it before monado-service)", p.display());
    }
}

fn lens_opt(a: &std::sync::atomic::AtomicU32) -> Option<f32> {
    let v = f32::from_bits(a.load(std::sync::atomic::Ordering::Relaxed));
    (v > 0.0).then_some(v)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::EyeRay;

    fn sample(i: u64, lx: f32, rx: f32, f34: f32) -> Sample {
        let e = |x: f32| EyeRay { origin: [x / 1000.0, 0.0, 0.0], origin_valid: true, direction_valid: true, ..Default::default() };
        Sample { seq: i, device_ts_us: i as i64 * 8333, eyes: [e(lx), e(rx)], ipd_mm: f34, ..Default::default() }
    }

    fn strip(s: &str) -> String {
        let mut o = String::new();
        let mut esc = false;
        for c in s.chars() {
            match (esc, c) {
                (false, '\x1b') => esc = true,
                (true, c) if c.is_ascii_alphabetic() => esc = false,
                (false, c) => o.push(c),
                _ => {}
            }
        }
        o
    }

    fn win(f34: f32, dist: f32) -> Window {
        let mut w = Window::default();
        for i in 0..400 {
            w.push(&sample(i, -dist / 2.0, dist / 2.0, f34));
        }
        w
    }

    #[test]
    fn window_gates_and_expires() {
        let mut w = win(63.0, 63.0);
        for i in 400..440 {
            w.push(&sample(i, -31.5, 45.0, 63.0)); // bad right eye
        }
        let v = w.view();
        assert!((v.good_pct[0] - 100.0).abs() < 1e-6 && v.good_pct[1] < 100.0 && v.good_pct[1] > 80.0, "{v:?}");
        let m = v.m.unwrap();
        assert!((m.origin_mm - 63.0).abs() < 0.1 && (m.field34_mm - 63.0).abs() < 1e-3);
        assert!(v.total < 400, "3 s window at 120 Hz");
    }

    #[test]
    fn render_snapshot() {
        let v = win(63.0, 62.6).view();
        let s = strip(&render(&View { eyes: &v, lens: Some(60.0), target: Some(63.0), pending: true, msg: "hello", link: "streaming", samples: 400 }));
        assert!(s.contains("YOUR EYES") && s.contains("LENSES") && s.contains("field 34 63.00 mm"), "{s}");
        assert!(s.contains("target 63.0 (pending)") && s.contains("difference 3.00 mm") && s.contains("L 100.0%   R 100.0%"));
        assert!(s.contains("58      60      62") && s.contains("hello") && s.contains("q quit"));
        // E at (63-58)*4 = 20, L at 8 (columns after the 2-space margin)
        let lines: Vec<&str> = s.lines().collect();
        let li = lines.iter().position(|l| l.contains('v') && l.contains('^')).expect("bar");
        let bar = lines[li];
        assert_eq!(bar.find('v').unwrap() - 2, 20);
        assert_eq!(bar.find('^').unwrap() - 2, 8);
        assert_eq!(lines[li - 1].find('E').unwrap() - 2, 20);
        assert_eq!(big(Some(63.0))[0], "███ ███   ███ ███ ");
    }

    #[test]
    fn diff_colours() {
        assert_eq!(diff_color(0.99), "\x1b[1;32m");
        assert_eq!(diff_color(1.0), "\x1b[1;33m");
        assert_eq!(diff_color(3.0), "\x1b[1;33m");
        assert_eq!(diff_color(3.01), "\x1b[1;31m");
        let v = EyeView::default();
        let s = render(&View { eyes: &v, lens: None, target: None, pending: false, msg: "", link: "x", samples: 0 });
        assert!(strip(&s).contains("difference n/a") && strip(&s).contains("no good eye samples"));
    }

    #[test]
    fn throttle_one_command_per_second() {
        let w = win(63.0, 63.0).view();
        let m = w.m.as_ref();
        let t0 = Instant::now();
        let mut c = Controller::default();
        // + pressed 5 times quickly, nothing before 1 s, then exactly one command for the last target
        for i in 0..5 {
            c.key(Key::Plus, t0 + Duration::from_millis(i * 100), m, Some(60.0));
        }
        assert_eq!(c.target, Some(62.5));
        assert_eq!(c.poll(t0 + Duration::from_millis(1300)), None);
        assert_eq!(c.poll(t0 + Duration::from_millis(1400)), Some(62.5));
        assert_eq!(c.poll(t0 + Duration::from_millis(1500)), None);
        // `a` right after: due now but throttled until 1 s after the last send
        c.key(Key::Apply, t0 + Duration::from_millis(1600), m, Some(62.5));
        assert_eq!(c.poll(t0 + Duration::from_millis(2000)), None);
        assert_eq!(c.poll(t0 + Duration::from_millis(2400)), Some(63.0));
        assert_eq!(c.poll(t0 + Duration::from_secs(9)), None);
    }

    #[test]
    fn keys_clamp_and_refuse() {
        let t0 = Instant::now();
        let mut c = Controller::default();
        c.key(Key::Apply, t0, None, None);
        assert!(c.msg.contains("refused") && c.poll(t0 + Duration::from_secs(5)).is_none());
        c.key(Key::Minus, t0, None, None);
        assert!(c.poll(t0 + Duration::from_secs(5)).is_none());
        c.key(Key::Minus, t0, None, Some(58.2));
        assert_eq!(c.poll(t0 + Duration::from_secs(2)), Some(58.0));
        c.key(Key::Minus, t0 + Duration::from_secs(3), None, Some(58.0));
        assert_eq!(c.target, Some(58.0));
        let mut c = Controller::default();
        c.key(Key::Plus, t0, None, Some(71.9));
        assert_eq!(c.target, Some(72.0));
        // disagreeing measurement is refused like --apply
        let bad = win(63.0, 58.0).view();
        c.key(Key::Apply, t0, bad.m.as_ref(), None);
        assert!(c.msg.contains("disagree"), "{}", c.msg);
        c.key(Key::Quit, t0, None, None);
        assert!(c.quit);
        assert_eq!(key_from_byte(b'+'), Some(Key::Plus));
        assert_eq!(key_from_byte(b'z'), None);
    }
}
