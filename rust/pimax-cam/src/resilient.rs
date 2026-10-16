//! A live [`Source`] that heals itself: it reopens the camera (or reconnects to the broker) after an error or a
//! stall instead of failing, so long-running readers (the broker, passthrough, ctrltrack, roommap, Monado through
//! the C ABI) survive a USB3 hub drop or a broker restart.
//!
//! * An error from the inner source (V4L2 `POLLERR`/`ENODEV` after an unplug, broker socket closed) drops it; the
//!   next [`Source::advance`] calls try to open it again with backoff ([`Policy::backoff_min`] doubling up to
//!   [`Policy::backoff_max`]). Meanwhile `advance` returns `Ok(false)` (a timeout) and honours the caller's timeout,
//!   so callers keep their loops and only see a gap. The camera 34a4:0014 can take about 90 s to come back after a
//!   hub drop (analysis/hang-worn-debug.md): the wrapper simply keeps trying, it never gives up.
//! * A stall (no frame for [`Policy::stall`] while open; live camera only) is handled like an error. A reopen that
//!   never delivers a frame counts as a failure, so a camera that stays dark (headset asleep) is retried with the
//!   long backoff and logged at most every [`Policy::log_every`].
//! * Replays are never wrapped (their end must stay visible), and a broker client connected to a replay broker
//!   passes the broker's end through.
//! * [`Source::generation`] changes on every reopen, so [`crate::camera::Camera`] resets its clock mapping and frame
//!   counter tracking (the device clock may have restarted).
//!
//! Opening sends nothing to the headset: a reopen is the same plain UVC open as the first one (or a socket connect).
//! `PIMAX_CAM_RECONNECT=0` turns the wrapper off (old behaviour: the first error ends the reader); `PIMAX_CAM_STALL_S`
//! sets the camera stall time (default 5 s, 0 = no stall detection).

use crate::source::{RawFrame, Source};
use std::io;
use std::time::{Duration, Instant};

/// Opens (or reopens) the inner source.
pub type Opener = Box<dyn FnMut() -> io::Result<Box<dyn Source>> + Send>;

/// Reopen policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Policy {
    /// No frame for this long while open: treat as lost (None: only errors count).
    pub stall: Option<Duration>,
    /// First retry delay after a loss.
    pub backoff_min: Duration,
    /// Longest retry delay.
    pub backoff_max: Duration,
    /// While down, repeat the "still waiting" log line at most this often.
    pub log_every: Duration,
}

impl Policy {
    /// The live camera: stall after 5 s (`PIMAX_CAM_STALL_S`), retry 0.25 s doubling to 2 s.
    pub fn camera() -> Policy {
        let stall = std::env::var("PIMAX_CAM_STALL_S")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
            .unwrap_or(5.0);
        Policy {
            stall: (stall > 0.0).then(|| Duration::from_secs_f64(stall)),
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(2),
            log_every: Duration::from_secs(30),
        }
    }

    /// A broker client: reconnect on errors only (the broker itself watches the camera), 0.25 s doubling to 1 s.
    pub fn broker() -> Policy {
        Policy {
            stall: None,
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(1),
            log_every: Duration::from_secs(30),
        }
    }

    /// Retry delay after `failures` consecutive failures (>= 1).
    pub fn backoff(&self, failures: u32) -> Duration {
        let shift = failures.saturating_sub(1).min(16);
        self.backoff_min
            .saturating_mul(1u32 << shift)
            .min(self.backoff_max)
    }
}

/// True unless `PIMAX_CAM_RECONNECT=0` (or `off`/`no`/`false`).
pub fn enabled() -> bool {
    !matches!(
        std::env::var("PIMAX_CAM_RECONNECT").ok().as_deref().map(str::trim),
        Some("0" | "off" | "no" | "false")
    )
}

/// Counters of a [`Resilient`] source.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Health {
    /// Inner source open now.
    pub up: bool,
    /// Successful opens, the first one included.
    pub opens: u64,
    /// Times the source was lost (error or stall).
    pub losses: u64,
    /// Of those, stalls.
    pub stalls: u64,
    /// Failed open attempts since the last frame.
    pub failures: u32,
}

/// See the module docs.
pub struct Resilient {
    open: Opener,
    inner: Option<Box<dyn Source>>,
    policy: Policy,
    label: String,
    desc: String,
    last_err: String,
    health: Health,
    /// Frame seen since the last open (resets the failure count).
    framed: bool,
    last_frame: Instant,
    next_try: Instant,
    down_since: Option<Instant>,
    last_log: Option<Instant>,
    corrupt_base: u64,
    log: bool,
}

impl Resilient {
    /// Wrap an already open source; `open` reopens it. `label` names it in log lines ("camera", "camera broker").
    pub fn new(first: Box<dyn Source>, open: Opener, policy: Policy, label: &str) -> Resilient {
        let now = Instant::now();
        Resilient {
            desc: first.describe(),
            inner: Some(first),
            open,
            policy,
            label: label.to_string(),
            last_err: String::new(),
            health: Health {
                up: true,
                opens: 1,
                ..Health::default()
            },
            framed: false,
            last_frame: now,
            next_try: now,
            down_since: None,
            last_log: None,
            corrupt_base: 0,
            log: true,
        }
    }

    /// Silence the stderr lines (tests).
    pub fn quiet(mut self) -> Resilient {
        self.log = false;
        self
    }

    /// Counters.
    pub fn health(&self) -> Health {
        Health {
            up: self.inner.is_some(),
            ..self.health
        }
    }

    fn say(&mut self, now: Instant, msg: String) {
        self.last_log = Some(now);
        if self.log {
            eprintln!("pimax-cam: {msg}");
        }
    }

    fn lose(&mut self, now: Instant, why: String, stall: bool) {
        if let Some(s) = self.inner.take() {
            self.corrupt_base += s.corrupt_frames();
        }
        self.health.losses += 1;
        if stall {
            self.health.stalls += 1;
        }
        if !self.framed {
            // reopened but never delivered: back off like a failed open
            self.health.failures = self.health.failures.saturating_add(1);
        }
        let first = self.down_since.is_none();
        self.down_since.get_or_insert(now);
        self.next_try = now + self.policy.backoff(self.health.failures.max(1));
        let quiet = self
            .last_log
            .is_some_and(|t| now.duration_since(t) < self.policy.log_every);
        if first || !quiet {
            self.say(now, format!("{} lost ({why}); reopening", self.label));
        }
        self.last_err = why;
    }

    fn try_open(&mut self, now: Instant) {
        match (self.open)() {
            Ok(s) => {
                let down = self
                    .down_since
                    .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
                self.desc = s.describe();
                self.inner = Some(s);
                self.health.opens += 1;
                self.framed = false;
                self.last_frame = now;
                self.say(
                    now,
                    format!(
                        "{} reopened after {down:.1} s ({} failed tries): {}",
                        self.label, self.health.failures, self.desc
                    ),
                );
            }
            Err(e) => {
                self.health.failures = self.health.failures.saturating_add(1);
                self.next_try = now + self.policy.backoff(self.health.failures);
                let due = self
                    .last_log
                    .is_none_or(|t| now.duration_since(t) >= self.policy.log_every);
                if due {
                    let down = self
                        .down_since
                        .map_or(0.0, |t| now.duration_since(t).as_secs_f64());
                    self.say(
                        now,
                        format!(
                            "{} still gone after {down:.0} s ({} tries): {e}",
                            self.label, self.health.failures
                        ),
                    );
                }
                self.last_err = e.to_string();
            }
        }
    }
}

impl Source for Resilient {
    fn advance(&mut self, timeout_ms: i32) -> io::Result<bool> {
        let deadline =
            (timeout_ms >= 0).then(|| Instant::now() + Duration::from_millis(timeout_ms as u64));
        loop {
            let now = Instant::now();
            if self.inner.is_none() && now >= self.next_try {
                self.try_open(now);
            }
            let Some(src) = self.inner.as_mut() else {
                let wake = deadline.map_or(self.next_try, |d| d.min(self.next_try));
                std::thread::sleep(wake.saturating_duration_since(now));
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Ok(false);
                }
                continue;
            };
            // waiting forever: still come back now and then so a stall can be noticed
            let left = deadline.map_or(500, |d| {
                d.saturating_duration_since(now).as_millis().min(i32::MAX as u128) as i32
            });
            match src.advance(left) {
                Ok(true) => {
                    self.last_frame = Instant::now();
                    if !self.framed {
                        self.framed = true;
                        self.health.failures = 0;
                        self.down_since = None;
                    }
                    return Ok(true);
                }
                Ok(false) => {
                    if src.finished() {
                        return Ok(false);
                    }
                    let now = Instant::now();
                    if let Some(stall) = self.policy.stall {
                        if !src.is_replay() && now.duration_since(self.last_frame) >= stall {
                            let why = format!("no frame for {:.1} s", stall.as_secs_f64());
                            self.lose(now, why, true);
                        }
                    }
                    if deadline.is_some() {
                        return Ok(false);
                    }
                }
                Err(e) => {
                    if src.is_replay() {
                        return Err(e); // the end of a replay broker: not ours to hide
                    }
                    self.lose(Instant::now(), e.to_string(), false);
                    if deadline.is_some_and(|d| Instant::now() >= d) {
                        return Ok(false);
                    }
                }
            }
        }
    }

    fn current(&self) -> Option<RawFrame<'_>> {
        self.inner.as_ref().and_then(|s| s.current())
    }

    fn describe(&self) -> String {
        match &self.inner {
            Some(s) => s.describe(),
            None => format!("{} (reopening: {})", self.desc, self.last_err),
        }
    }

    fn finished(&self) -> bool {
        self.inner.as_ref().is_some_and(|s| s.finished())
    }

    fn corrupt_frames(&self) -> u64 {
        self.corrupt_base + self.inner.as_ref().map_or(0, |s| s.corrupt_frames())
    }

    fn is_replay(&self) -> bool {
        self.inner.as_ref().is_some_and(|s| s.is_replay())
    }

    fn generation(&self) -> u64 {
        let inner = self.inner.as_ref().map_or(0, |s| s.generation());
        (self.health.opens.saturating_sub(1) << 32) | (inner & 0xffff_ffff)
    }

    fn up(&self) -> bool {
        self.inner.as_ref().is_some_and(|s| s.up())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};
    use std::sync::{Arc, Mutex};

    /// What the fake camera does on each advance.
    #[derive(Clone, Copy, Debug)]
    enum Step {
        Frame,
        Timeout,
        Fail,
    }

    struct FakeCam {
        script: Vec<Step>,
        pos: usize,
        id: u32,
        k: u32,
        buf: Vec<u8>,
    }

    impl Source for FakeCam {
        fn advance(&mut self, t: i32) -> io::Result<bool> {
            let s = self.script.get(self.pos).copied().unwrap_or(Step::Timeout);
            self.pos += 1;
            match s {
                Step::Frame => {
                    self.k += 1;
                    Ok(true)
                }
                Step::Timeout => {
                    std::thread::sleep(Duration::from_millis(t.clamp(0, 5) as u64));
                    Ok(false)
                }
                Step::Fail => Err(io::Error::other("camera device error (unplugged?)")),
            }
        }
        fn current(&self) -> Option<RawFrame<'_>> {
            Some(RawFrame {
                data: &self.buf,
                stride: 4,
                host_ts_ns: self.k as u64,
                sequence: self.id * 1000 + self.k,
            })
        }
        fn describe(&self) -> String {
            format!("fake cam #{}", self.id)
        }
        fn corrupt_frames(&self) -> u64 {
            1
        }
    }

    /// An opener that fails `absent` times, then hands out cams with the given scripts.
    fn opener(absent: u32, scripts: Vec<Vec<Step>>) -> (Opener, Arc<AtomicU32>) {
        let calls = Arc::new(AtomicU32::new(0));
        let c = calls.clone();
        let scripts = Arc::new(Mutex::new(scripts.into_iter()));
        let mut id = 1;
        let o: Opener = Box::new(move || {
            let n = c.fetch_add(1, Ordering::SeqCst);
            if n < absent {
                return Err(io::Error::new(io::ErrorKind::NotFound, "camera 34a4:0014 not found"));
            }
            id += 1;
            let script = scripts.lock().unwrap().next().unwrap_or_default();
            Ok(Box::new(FakeCam { script, pos: 0, id, k: 0, buf: vec![0; 4] }) as Box<dyn Source>)
        });
        (o, calls)
    }

    fn cam(script: Vec<Step>) -> Box<dyn Source> {
        Box::new(FakeCam { script, pos: 0, id: 1, k: 0, buf: vec![0; 4] })
    }

    fn fast() -> Policy {
        Policy {
            stall: Some(Duration::from_millis(60)),
            backoff_min: Duration::from_millis(5),
            backoff_max: Duration::from_millis(20),
            log_every: Duration::from_secs(30),
        }
    }

    /// Advance until a frame arrives or `limit` passes; returns the frame's sequence.
    fn next_frame(r: &mut Resilient, limit: Duration) -> Option<u32> {
        let t0 = Instant::now();
        while t0.elapsed() < limit {
            match r.advance(10) {
                Ok(true) => return r.current().map(|f| f.sequence),
                Ok(false) => {}
                Err(e) => panic!("resilient source returned an error: {e}"),
            }
        }
        None
    }

    #[test]
    fn backoff_doubles_and_caps() {
        let p = Policy {
            stall: None,
            backoff_min: Duration::from_millis(250),
            backoff_max: Duration::from_secs(2),
            log_every: Duration::from_secs(30),
        };
        let v: Vec<u64> = (1..=6).map(|n| p.backoff(n).as_millis() as u64).collect();
        assert_eq!(v, vec![250, 500, 1000, 2000, 2000, 2000]);
        assert_eq!(p.backoff(0), Duration::from_millis(250));
        assert_eq!(p.backoff(u32::MAX), Duration::from_secs(2));
    }

    #[test]
    fn error_then_absent_then_back_reopens_and_bumps_the_generation() {
        // frame, then an unplug error; the device is absent for 4 tries, then returns
        let (o, calls) = opener(4, vec![vec![Step::Frame, Step::Frame]]);
        let mut r = Resilient::new(cam(vec![Step::Frame, Step::Fail]), o, fast(), "camera").quiet();
        let g0 = r.generation();
        assert_eq!(next_frame(&mut r, Duration::from_secs(1)), Some(1001));
        // the error is hidden: only timeouts until the camera is back
        let back = next_frame(&mut r, Duration::from_secs(2)).expect("reopened");
        assert_eq!(back, 2001, "first frame of the reopened camera");
        assert_eq!(calls.load(Ordering::SeqCst), 5, "4 failed opens + 1 good");
        let h = r.health();
        assert_eq!((h.up, h.opens, h.losses, h.stalls, h.failures), (true, 2, 1, 0, 0));
        assert_ne!(r.generation(), g0);
        assert!(r.up());
        assert_eq!(r.corrupt_frames(), 2, "counts of the dropped source are kept");
    }

    #[test]
    fn stall_reopens_a_camera_that_stops_sending() {
        let (o, calls) = opener(0, vec![vec![Step::Frame]]);
        let mut r = Resilient::new(cam(vec![Step::Frame]), o, fast(), "camera").quiet();
        assert_eq!(next_frame(&mut r, Duration::from_secs(1)), Some(1001));
        let t0 = Instant::now();
        assert_eq!(next_frame(&mut r, Duration::from_secs(2)), Some(2001));
        assert!(t0.elapsed() >= Duration::from_millis(60), "waited for the stall time");
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert_eq!(r.health().stalls, 1);
    }

    #[test]
    fn a_dark_camera_backs_off_instead_of_spinning() {
        // every reopen succeeds but never delivers a frame (headset asleep): failures grow, so the retry slows down
        let (o, calls) = opener(0, vec![vec![]; 100]);
        let mut r = Resilient::new(cam(vec![]), o, fast(), "camera").quiet();
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(500) {
            assert!(!r.advance(10).unwrap());
        }
        let n = calls.load(Ordering::SeqCst);
        // stall 60 ms + backoff up to 20 ms: at most ~8 reopens in 500 ms, at least 3
        assert!((3..=9).contains(&n), "{n} reopens");
        assert!(r.health().failures >= 3);
    }

    #[test]
    fn respects_the_callers_timeout_while_down() {
        let (o, _) = opener(u32::MAX, vec![]);
        let mut p = fast();
        p.backoff_min = Duration::from_secs(5);
        p.backoff_max = Duration::from_secs(5);
        let mut r = Resilient::new(cam(vec![Step::Fail]), o, p, "camera").quiet();
        let t0 = Instant::now();
        assert!(!r.advance(50).unwrap());
        assert!(!r.advance(50).unwrap());
        let took = t0.elapsed();
        assert!(took >= Duration::from_millis(90) && took < Duration::from_millis(400), "{took:?}");
        assert!(!r.up() && r.current().is_none());
        assert!(r.describe().contains("reopening"), "{}", r.describe());
        assert!(!r.finished());
    }

    #[test]
    fn broker_policy_does_not_stall_out() {
        let (o, calls) = opener(0, vec![]);
        let mut r = Resilient::new(cam(vec![Step::Frame]), o, Policy::broker(), "camera broker").quiet();
        assert!(next_frame(&mut r, Duration::from_secs(1)).is_some());
        let t0 = Instant::now();
        while t0.elapsed() < Duration::from_millis(150) {
            assert!(!r.advance(10).unwrap());
        }
        assert_eq!(calls.load(Ordering::SeqCst), 0, "a quiet broker is not reconnected");
    }
}
