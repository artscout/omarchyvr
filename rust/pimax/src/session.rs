//! The session thread and its public handle.
use crate::{discovery, hid, sys, Error, Result};
use pimax_proto::{
    self as proto, ControllerReport, ControllerStatus, ImuSample, LineSplitter, Report, NUM_IF,
    REPORT_SIZE,
};
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering::Relaxed};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Connection state reported to [`Handler::on_state`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// No session.
    Disconnected,
    /// Woken up, keepalive running.
    Connected,
}

/// Callbacks, all run on the session thread: keep them short and never call [`Session::request`] from one.
pub trait Handler: Send + 'static {
    /// Connection state changed.
    fn on_state(&mut self, _state: State) {}
    /// One IMU sample (two per report).
    fn on_imu(&mut self, _s: &ImuSample) {}
    /// Controller report.
    fn on_controller(&mut self, _r: &ControllerReport) {}
    /// Unsolicited controller heartbeat (0x37).
    fn on_status(&mut self, _s: &ControllerStatus) {}
    /// One firmware log line (IF3).
    fn on_log(&mut self, _line: &str) {}
    /// Every report read from IF0.
    fn on_reply(&mut self, _r: &Report) {}
    /// A link ended: how long it lived and how many keepalives it sent (called after `on_state(Disconnected)`).
    fn on_link_closed(&mut self, _lived: Duration, _keepalives: u64) {}
    /// The automatic logic (`Config::auto_attach`) scheduled an attach burst for `slot` (initial or retry).
    fn on_attach(&mut self, _slot: u8, _retry: bool) {}
}

/// Handler that ignores everything (the session still drains and answers requests).
pub struct NoopHandler;
impl Handler for NoopHandler {}

/// Events for the channel interface ([`Session::open_channel`]).
#[derive(Debug, Clone)]
pub enum Event {
    /// State change.
    State(State),
    /// IMU sample.
    Imu(ImuSample),
    /// Controller report.
    Controller(ControllerReport),
    /// Controller heartbeat.
    Status(ControllerStatus),
    /// Firmware log line.
    Log(String),
    /// IF0 report (only when requested).
    Reply(Report),
    /// Automatic attach burst scheduled for a slot (`true` = retry after a 0x37 heartbeat).
    Attach(u8, bool),
}

struct ChannelHandler {
    tx: SyncSender<Event>,
    replies: bool,
    dropped: Arc<AtomicU64>,
}

impl ChannelHandler {
    fn send(&self, e: Event) {
        if self.tx.try_send(e).is_err() {
            self.dropped.fetch_add(1, Relaxed);
        }
    }
}

impl Handler for ChannelHandler {
    fn on_state(&mut self, s: State) {
        self.send(Event::State(s))
    }
    fn on_imu(&mut self, s: &ImuSample) {
        self.send(Event::Imu(*s))
    }
    fn on_controller(&mut self, r: &ControllerReport) {
        self.send(Event::Controller(*r))
    }
    fn on_status(&mut self, s: &ControllerStatus) {
        self.send(Event::Status(*s))
    }
    fn on_log(&mut self, l: &str) {
        self.send(Event::Log(l.to_string()))
    }
    fn on_reply(&mut self, r: &Report) {
        if self.replies {
            self.send(Event::Reply(*r))
        }
    }
    fn on_attach(&mut self, slot: u8, retry: bool) {
        self.send(Event::Attach(slot, retry))
    }
}

/// Where the keepalive gets its video rate byte.
#[derive(Debug, Clone)]
pub enum RateSource {
    /// The value set with [`Session::set_video_rate`] (libpimax behaviour).
    Handle,
    /// Re-read a file on every keepalive; missing or invalid = 0 (`pimax_keepalived.py` behaviour,
    /// `$XDG_RUNTIME_DIR/pimax-rate`).
    File(PathBuf),
}

/// Session options. `Config::default()` equals the behaviour of the C libpimax.
#[derive(Debug, Clone)]
pub struct Config {
    /// Send the wake `58 06 1a 05 01 00 00` after connecting.
    pub wake: bool,
    /// Send keepalives (disable for passive read-only sessions).
    pub keepalive: bool,
    /// Keepalive period.
    pub period: Duration,
    /// Source of the rate byte.
    pub rate: RateSource,
    /// Attach both controllers at connect and re-attach when a 0x37 heartbeat shows one awake but silent.
    pub auto_attach: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            wake: true,
            keepalive: true,
            period: Duration::from_millis(proto::KEEPALIVE_MS),
            rate: RateSource::Handle,
            auto_attach: false,
        }
    }
}

impl Config {
    /// Read-only: no wake, no keepalive. The headset must be kept alive by someone else.
    pub fn passive() -> Self {
        Self {
            wake: false,
            keepalive: false,
            ..Self::default()
        }
    }
}

/// Counters, readable at any time.
#[derive(Debug, Default, Clone, Copy)]
pub struct Stats {
    /// IMU reports (2 samples each).
    pub imu_reports: u64,
    /// Controller reports.
    pub controller_reports: u64,
    /// 0x37 messages.
    pub status_messages: u64,
    /// Firmware log lines.
    pub log_lines: u64,
    /// Keepalives written.
    pub keepalives: u64,
    /// Sessions started.
    pub connects: u64,
    /// Controller attach bursts scheduled by the automatic logic.
    pub auto_attaches: u64,
    /// Events dropped because a channel was full.
    pub dropped_events: u64,
}

#[derive(Default)]
struct Counters {
    imu: AtomicU64,
    ctrl: AtomicU64,
    status: AtomicU64,
    log: AtomicU64,
    ka: AtomicU64,
    connects: AtomicU64,
    attaches: AtomicU64,
    dropped: Arc<AtomicU64>,
}

/// An open link to the device.
pub struct Link {
    /// Read side of each interface (IF0 included: replies).
    pub reads: [Option<File>; NUM_IF],
    /// Write side of IF0.
    pub ctl: File,
}

/// Source of device links; the real one is [`HidrawBackend`], tests use [`crate::fake`].
pub trait Backend: Send + 'static {
    /// Try to open the device. `Ok(None)` = not present (the session polls every 50 ms).
    fn connect(&mut self) -> io::Result<Option<Link>>;
}

/// hidraw via sysfs.
#[derive(Default)]
pub struct HidrawBackend {
    /// Search roots.
    pub roots: discovery::Roots,
}

impl HidrawBackend {
    /// Defaults plus `PIMAX_SYSFS_ROOT` / `PIMAX_DEV_ROOT`.
    pub fn new() -> Self {
        Self {
            roots: discovery::Roots::from_env(),
        }
    }
}

impl Backend for HidrawBackend {
    fn connect(&mut self) -> io::Result<Option<Link>> {
        let found = discovery::find_interfaces(&self.roots);
        if found[0].is_none() || found[2].is_none() {
            return Ok(None); // need IF0 and IF2
        }
        let mut reads: [Option<File>; NUM_IF] = Default::default();
        for (i, p) in found.iter().enumerate() {
            let Some(p) = p else { continue };
            match hid::open(p, i == 0) {
                Ok(f) => reads[i] = Some(f),
                Err(e) if i == 0 || i == 2 => return Err(e),
                Err(e) => log::warn!("cannot open IF{i} ({}): {e}", p.display()),
            }
        }
        let ctl = reads[0]
            .as_ref()
            .ok_or_else(|| io::Error::from(io::ErrorKind::NotFound))?
            .try_clone()?;
        Ok(Some(Link { reads, ctl }))
    }
}

type Matcher = Box<dyn Fn(&Report) -> bool + Send>;

struct Pending {
    matcher: Matcher,
    done: Option<Report>,
    aborted: bool,
}

#[derive(Default)]
struct Inner {
    ctl: Option<File>,
    pending: Option<Pending>,
}

struct Shared {
    stop: AtomicBool,
    connected: AtomicBool,
    rate: AtomicU8,
    inner: Mutex<Inner>,
    cv: Condvar,
    req_lock: Mutex<()>,
    waker: File,
    counters: Counters,
    /// Controller vibration waiting for the session thread (at most one command per hand per 8 ms, newest wins).
    haptic: Mutex<crate::haptics::Limiter>,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|p| p.into_inner())
    }
    fn wake(&self) {
        let _ = (&self.waker).write(&1u64.to_ne_bytes());
    }
    fn write_ctl(&self, r: &Report) -> Result<()> {
        let g = self.lock();
        let Some(f) = g.ctl.as_ref() else {
            return Err(Error::NotConnected);
        };
        hid::write_report(f, r).map_err(|e| {
            if hid::is_gone(&e) {
                Error::NotConnected
            } else {
                Error::Io(e)
            }
        })
    }
}

/// Handle to a running session thread. Dropping it stops the thread.
pub struct Session {
    shared: Arc<Shared>,
    thread: Option<JoinHandle<()>>,
}

impl Session {
    /// Start a session on the real hidraw devices.
    pub fn open(cfg: Config, handler: impl Handler) -> io::Result<Session> {
        Self::open_with_backend(cfg, HidrawBackend::new(), handler)
    }

    /// Start a session and receive events through a bounded channel (events are dropped when it is full).
    pub fn open_channel(
        cfg: Config,
        capacity: usize,
        replies: bool,
    ) -> io::Result<(Session, Receiver<Event>)> {
        Self::open_channel_with_backend(cfg, HidrawBackend::new(), capacity, replies)
    }

    /// Channel variant with a custom backend.
    pub fn open_channel_with_backend(
        cfg: Config,
        backend: impl Backend,
        capacity: usize,
        replies: bool,
    ) -> io::Result<(Session, Receiver<Event>)> {
        let (tx, rx) = std::sync::mpsc::sync_channel(capacity);
        let dropped = Arc::new(AtomicU64::new(0));
        let s = Self::spawn(
            cfg,
            Box::new(backend),
            Box::new(ChannelHandler {
                tx,
                replies,
                dropped: dropped.clone(),
            }),
            Some(dropped),
        )?;
        Ok((s, rx))
    }

    /// Start a session with a custom backend.
    pub fn open_with_backend(
        cfg: Config,
        backend: impl Backend,
        handler: impl Handler,
    ) -> io::Result<Session> {
        Self::spawn(cfg, Box::new(backend), Box::new(handler), None)
    }

    fn spawn(
        cfg: Config,
        backend: Box<dyn Backend>,
        handler: Box<dyn Handler>,
        dropped: Option<Arc<AtomicU64>>,
    ) -> io::Result<Session> {
        let mut counters = Counters::default();
        if let Some(d) = dropped {
            counters.dropped = d;
        }
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            connected: AtomicBool::new(false),
            rate: AtomicU8::new(0),
            inner: Mutex::new(Inner::default()),
            cv: Condvar::new(),
            req_lock: Mutex::new(()),
            waker: sys::eventfd()?,
            counters,
            haptic: Mutex::new(crate::haptics::Limiter::default()),
        });
        let sh = shared.clone();
        let thread = std::thread::Builder::new()
            .name("pimax-session".into())
            .spawn(move || thread_main(sh, backend, handler, cfg))?;
        Ok(Session {
            shared,
            thread: Some(thread),
        })
    }

    /// True while a device session is up.
    pub fn is_connected(&self) -> bool {
        self.shared.connected.load(Relaxed)
    }

    /// Block until connected.
    pub fn wait_connected(&self, timeout: Duration) -> Result<()> {
        let deadline = Instant::now() + timeout;
        let mut g = self.shared.lock();
        while !self.shared.connected.load(Relaxed) {
            let now = Instant::now();
            if now >= deadline {
                return Err(Error::Timeout);
            }
            g = self
                .shared
                .cv
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        }
        Ok(())
    }

    /// Video rate carried by the keepalive (with [`RateSource::Handle`]); 0 = none.
    pub fn set_video_rate(&self, fps: u8) {
        self.shared.rate.store(fps, Relaxed);
    }

    /// Snapshot of the counters.
    pub fn stats(&self) -> Stats {
        let c = &self.shared.counters;
        Stats {
            imu_reports: c.imu.load(Relaxed),
            controller_reports: c.ctrl.load(Relaxed),
            status_messages: c.status.load(Relaxed),
            log_lines: c.log.load(Relaxed),
            keepalives: c.ka.load(Relaxed),
            connects: c.connects.load(Relaxed),
            auto_attaches: c.attaches.load(Relaxed),
            dropped_events: c.dropped.load(Relaxed),
        }
    }

    /// Fire and forget `58 LL cmd opt data`.
    pub fn send(&self, cmd: u8, opt: u8, data: &[u8]) -> Result<()> {
        let r = proto::build_command(cmd, opt, data).map_err(|_| Error::TooLong)?;
        self.shared.write_ctl(&r)
    }

    /// Send a prebuilt report.
    pub fn send_report(&self, r: &Report) -> Result<()> {
        self.shared.write_ctl(r)
    }

    /// Send and wait for the `4c LL cmd opt ..` reply; returns its data without the check byte.
    pub fn request(&self, cmd: u8, opt: u8, data: &[u8], timeout: Duration) -> Result<Vec<u8>> {
        let r = proto::build_command(cmd, opt, data).map_err(|_| Error::TooLong)?;
        let rep = self.request_with(&r, timeout, move |d| d[2] == cmd && d[3] == opt)?;
        Ok(proto::parse_reply(&rep)
            .map(|p| p.data.to_vec())
            .unwrap_or_default())
    }

    /// Send `report` and wait for the first IF0 `4c` report accepted by `matcher`.
    pub fn request_with(
        &self,
        report: &Report,
        timeout: Duration,
        matcher: impl Fn(&Report) -> bool + Send + 'static,
    ) -> Result<Report> {
        let sh = &*self.shared;
        let _serial = sh.req_lock.lock().unwrap_or_else(|p| p.into_inner());
        sh.lock().pending = Some(Pending {
            matcher: Box::new(matcher),
            done: None,
            aborted: false,
        });
        if let Err(e) = sh.write_ctl(report) {
            sh.lock().pending = None;
            return Err(e);
        }
        let deadline = Instant::now() + timeout;
        let mut g = sh.lock();
        let res = loop {
            match g.pending.as_ref() {
                Some(Pending { done: Some(r), .. }) => break Ok(*r),
                Some(Pending { aborted: true, .. }) | None => break Err(Error::NotConnected),
                _ => {}
            }
            let now = Instant::now();
            if now >= deadline {
                break Err(Error::Timeout);
            }
            g = sh
                .cv
                .wait_timeout(g, deadline - now)
                .unwrap_or_else(|p| p.into_inner())
                .0;
        };
        g.pending = None;
        res
    }

    /// Serial number (cmd 0x16) of the headset (`opt` 0) or a controller (`opt` = slot).
    pub fn read_serial(&self, opt: u8, timeout: Duration) -> Result<String> {
        let d = self.request(proto::cmd::SERIAL, opt, &[0x1e], timeout)?;
        Ok(String::from_utf8_lossy(proto::parse_serial(&d)).into_owned())
    }

    /// Version bytes (cmd 0x03).
    pub fn read_version(&self, opt: u8, timeout: Duration) -> Result<Vec<u8>> {
        self.request(proto::cmd::VERSION, opt, &[], timeout)
    }

    /// Read the 1344-byte calibration blob over IF0 (84 reads, up to 3 tries each) and return it.
    /// Use [`proto::calib::inflate_blob`] for the XML.
    pub fn read_calibration_blob(&self, per_try: Duration) -> Result<Vec<u8>> {
        use proto::calib;
        let mut asm = calib::CalibAssembler::new();
        for addr in (0..calib::TOTAL).step_by(calib::BLOCK) {
            let addr = addr as u16;
            let req = calib::read_block_request(addr);
            let mut ok = false;
            for _ in 0..3 {
                match self.request_with(&req, per_try, move |d| {
                    calib::parse_block_reply(d).is_some_and(|(a, _)| a == addr)
                }) {
                    Ok(r) => {
                        asm.feed(&r);
                        ok = true;
                        break;
                    }
                    Err(Error::Timeout) => continue,
                    Err(e) => return Err(e),
                }
            }
            if !ok {
                return Err(Error::Timeout);
            }
        }
        Ok(asm.blob().to_vec())
    }

    /// Per-slot attach burst (0x16, 0x50, 0x0a, 0x03), 10 ms apart, blocking about 30 ms.
    pub fn controller_attach(&self, slot: u8) -> Result<()> {
        for (i, c) in proto::attach_commands(slot).iter().enumerate() {
            self.shared.write_ctl(c)?;
            if i < 3 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        Ok(())
    }

    /// Switch the panel: `5b(02 02)`, `29(01)`, `1a(02 FPS)`, `1a(05 00)` and set the keepalive rate.
    /// Never called by the test suite.
    pub fn video_prepare(&self, fps: u8) -> Result<()> {
        self.set_video_rate(fps);
        for c in proto::video_prepare_commands(fps) {
            self.shared.write_ctl(&c)?;
        }
        Ok(())
    }

    /// Vibrate a controller (`slot` 2 left / 3 right, `amp` percent 1..100, `dur_ms` milliseconds): queued for the
    /// session thread, which writes `58 09 07 SS AMP DUR` (Windows capture 2026-10-15) at most once per 8 ms per hand;
    /// a newer request replaces a waiting one. `None` drops a waiting pulse (a stop sends nothing, as on Windows).
    /// Never blocks on the device. Returns false for an unknown slot.
    pub fn haptic(&self, slot: u8, pulse: Option<(u8, u16)>) -> bool {
        let ok = self
            .shared
            .haptic
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .offer(slot, pulse);
        if ok && pulse.is_some() {
            self.shared.wake();
        }
        ok
    }

    /// Vibration commands written so far and requests replaced before they went out.
    pub fn haptic_counts(&self) -> (u64, u64) {
        let l = self.shared.haptic.lock().unwrap_or_else(|p| p.into_inner());
        (l.sent, l.replaced)
    }

    /// Stop the thread and wait for it.
    pub fn close(mut self) {
        self.stop_and_join();
    }

    fn stop_and_join(&mut self) {
        self.shared.stop.store(true, Relaxed);
        self.shared.wake();
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.stop_and_join();
    }
}

// ---------------------------------------------------------------- session thread

fn sleep_interruptible(sh: &Shared, d: Duration) {
    if sh.stop.load(Relaxed) {
        return;
    }
    let mut p = [sys::pollin(sh.waker.as_raw_fd())];
    let _ = sys::ppoll(&mut p, Some(d));
    drain_waker(sh);
}

fn drain_waker(sh: &Shared) {
    let mut b = [0u8; 8];
    let _ = (&sh.waker).read(&mut b);
}

fn thread_main(
    sh: Arc<Shared>,
    mut backend: Box<dyn Backend>,
    mut handler: Box<dyn Handler>,
    cfg: Config,
) {
    let mut warned = false;
    while !sh.stop.load(Relaxed) {
        match backend.connect() {
            Ok(Some(link)) => {
                warned = false;
                run_link(&sh, link, &mut *handler, &cfg);
                sleep_interruptible(&sh, Duration::from_millis(100));
            }
            Ok(None) => sleep_interruptible(&sh, Duration::from_millis(50)),
            Err(e) => {
                if !warned {
                    log::warn!(
                        "cannot open the headset: {e}{}",
                        if e.kind() == io::ErrorKind::PermissionDenied {
                            " (udev rule 70-pimax.rules missing?)"
                        } else {
                            ""
                        }
                    );
                    warned = true;
                }
                sleep_interruptible(
                    &sh,
                    Duration::from_secs(if e.kind() == io::ErrorKind::PermissionDenied {
                        2
                    } else {
                        1
                    }),
                );
            }
        }
    }
}

fn set_state(sh: &Shared, h: &mut dyn Handler, s: State) {
    let publish = |sh: &Shared| {
        let _g = sh.lock();
        sh.connected.store(s == State::Connected, Relaxed);
        sh.cv.notify_all();
    };
    if s == State::Connected {
        // Handler first: when `wait_connected` returns (or `is_connected` reads true) the application has already
        // been told `on_state(Connected)`, so it never reads a stale state right after waiting.
        h.on_state(s);
        publish(sh);
    } else {
        publish(sh);
        h.on_state(s);
    }
}

const STALE_AFTER: Duration = Duration::from_millis(1500);
const RETRY_MIN: Duration = Duration::from_millis(2500);
/// A headset that just restarted ignores the first wake (Windows capture 2026-10-15 take 2: no ack, re-sent after
/// 0.5 s and acked). Without a `4c .. 1a` reply this long after the wake, send it once more.
pub const WAKE_RESEND_AFTER: Duration = Duration::from_millis(500);

struct Attach {
    /// The wake is acked (any `4c 04 1a ..` reply seen on IF0).
    wake_acked: bool,
    /// When to send the wake once more if it is still not acked (`None`: not waiting / already re-sent).
    wake_resend_at: Option<Instant>,
    last_rx: [Option<Instant>; 4],
    seen37: [Option<Instant>; 4],
    tried: [Option<Instant>; 4],
}

fn schedule_burst(q: &mut VecDeque<(Instant, Report)>, at: Instant, slot: u8) {
    for (i, c) in proto::attach_commands(slot).into_iter().enumerate() {
        q.push_back((at + Duration::from_millis(10 * i as u64), c));
    }
}

fn run_link(sh: &Arc<Shared>, link: Link, h: &mut dyn Handler, cfg: &Config) {
    let Link { reads, ctl } = link;
    sh.lock().ctl = Some(ctl);
    sh.counters.connects.fetch_add(1, Relaxed);
    if cfg.wake {
        if let Err(e) = sh.write_ctl(&proto::wake()) {
            log::warn!("wake failed: {e}");
            teardown(sh, h, false);
            return;
        }
    }
    set_state(sh, h, State::Connected);

    let mut log_split = LineSplitter::new();
    let mut sched: VecDeque<(Instant, Report)> = VecDeque::new();
    let mut att = Attach {
        wake_acked: false,
        wake_resend_at: cfg.wake.then(|| Instant::now() + WAKE_RESEND_AFTER),
        last_rx: [None; 4],
        seen37: [None; 4],
        tried: [None; 4],
    };
    let start = Instant::now();
    if cfg.auto_attach {
        for slot in [proto::SLOT_LEFT, proto::SLOT_RIGHT] {
            schedule_burst(&mut sched, start, slot);
            att.tried[slot as usize] = Some(start);
            h.on_attach(slot, false);
        }
    }
    let mut next_ka = start;
    let mut ka_sent = 0u64;
    let mut pf = [sys::pollin(0); NUM_IF + 1];
    let mut map = [usize::MAX; NUM_IF + 1];
    let mut gone = false;

    'main: while !sh.stop.load(Relaxed) {
        let now = Instant::now();
        if cfg.keepalive && now >= next_ka {
            next_ka += cfg.period;
            if next_ka < now {
                next_ka = now + cfg.period;
            }
            let rate = match &cfg.rate {
                RateSource::Handle => sh.rate.load(Relaxed),
                RateSource::File(p) => std::fs::read_to_string(p)
                    .ok()
                    .and_then(|s| s.trim().parse::<i64>().ok())
                    .unwrap_or(0) as u8,
            };
            match sh.write_ctl(&proto::keepalive(rate)) {
                Ok(()) => {
                    ka_sent += 1;
                    sh.counters.ka.fetch_add(1, Relaxed);
                }
                Err(Error::NotConnected) => break,
                Err(Error::Io(e)) if e.kind() == io::ErrorKind::WouldBlock => {}
                Err(e) => log::debug!("keepalive write: {e}"),
            }
        }
        if let Some(t) = att.wake_resend_at {
            if att.wake_acked {
                att.wake_resend_at = None;
            } else if now >= t {
                att.wake_resend_at = None;
                log::info!("wake not acked after {} ms: sending it once more", WAKE_RESEND_AFTER.as_millis());
                if matches!(sh.write_ctl(&proto::wake()), Err(Error::NotConnected)) {
                    break 'main;
                }
            }
        }
        while sched.front().is_some_and(|(t, _)| *t <= now) {
            if let Some((_, c)) = sched.pop_front() {
                if matches!(sh.write_ctl(&c), Err(Error::NotConnected)) {
                    break 'main;
                }
            }
        }
        let due = sh
            .haptic
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take_due(now);
        for (slot, amp, ms) in due {
            match sh.write_ctl(&proto::haptic_command(slot, amp, ms)) {
                Err(Error::NotConnected) => break 'main,
                Err(e) => log::debug!("haptic write: {e}"),
                Ok(()) => {}
            }
        }
        if cfg.auto_attach {
            for slot in [proto::SLOT_LEFT, proto::SLOT_RIGHT] {
                let s = slot as usize;
                let stale = att.last_rx[s].is_none_or(|t| now.duration_since(t) > STALE_AFTER);
                let awake = matches!((att.seen37[s], att.tried[s]), (Some(a), Some(t)) if a > t)
                    || (att.seen37[s].is_some() && att.tried[s].is_none());
                let rate_ok = att.tried[s].is_none_or(|t| now.duration_since(t) >= RETRY_MIN);
                if awake && stale && rate_ok {
                    log::info!(
                        "controller slot {slot} awake (0x37) but silent: re-sending attach burst"
                    );
                    schedule_burst(&mut sched, now, slot);
                    att.tried[s] = Some(now);
                    sh.counters.attaches.fetch_add(1, Relaxed);
                    h.on_attach(slot, true);
                }
            }
        }

        // poll: waker + interfaces
        let mut n = 0;
        pf[n] = sys::pollin(sh.waker.as_raw_fd());
        map[n] = usize::MAX;
        n += 1;
        for (i, f) in reads.iter().enumerate() {
            if let Some(f) = f {
                pf[n] = sys::pollin(f.as_raw_fd());
                map[n] = i;
                n += 1;
            }
        }
        let mut wake_at = if cfg.keepalive {
            next_ka
        } else {
            now + Duration::from_secs(1)
        };
        if let Some((t, _)) = sched.front() {
            wake_at = wake_at.min(*t);
        }
        if let Some(t) = att.wake_resend_at {
            wake_at = wake_at.min(t);
        }
        if let Some(t) = sh
            .haptic
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .next_due()
        {
            wake_at = wake_at.min(t);
        }
        let wait = wake_at.saturating_duration_since(Instant::now());
        match sys::ppoll(&mut pf[..n], Some(wait)) {
            Ok(_) => {}
            Err(_) => break,
        }
        if pf[0].revents != 0 {
            drain_waker(sh);
        }
        for k in 1..n {
            let re = pf[k].revents;
            let idx = map[k];
            if re & sys::POLLIN != 0 {
                let Some(f) = reads[idx].as_ref() else {
                    continue;
                };
                // One read per ready fd and cycle: a second read would only return EAGAIN almost every time
                // (reports arrive one by one) and cost a syscall; level-triggered ppoll hands back anything still queued.
                let mut buf = [0u8; REPORT_SIZE];
                match (&*f).read(&mut buf) {
                    Ok(0) => {}
                    Ok(len) => dispatch(sh, h, cfg, idx, &buf, len, &mut log_split, &mut att),
                    Err(e)
                        if e.kind() == io::ErrorKind::WouldBlock
                            || e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        log::debug!("IF{idx} read: {e}");
                        gone = true;
                    }
                }
            }
            if re & (sys::POLLERR | sys::POLLHUP | sys::POLLNVAL) != 0 {
                gone = true;
            }
        }
        if gone {
            break;
        }
    }
    teardown(sh, h, true);
    h.on_link_closed(start.elapsed(), ka_sent);
}

fn teardown(sh: &Shared, h: &mut dyn Handler, was_connected_hint: bool) {
    sh.haptic.lock().unwrap_or_else(|p| p.into_inner()).clear();
    {
        let mut g = sh.lock();
        g.ctl = None;
        if let Some(p) = g.pending.as_mut() {
            p.aborted = true;
        }
        sh.cv.notify_all();
    }
    if was_connected_hint && sh.connected.load(Relaxed) {
        set_state(sh, h, State::Disconnected);
    }
}

#[allow(clippy::too_many_arguments)]
fn dispatch(
    sh: &Shared,
    h: &mut dyn Handler,
    cfg: &Config,
    idx: usize,
    d: &Report,
    len: usize,
    log_split: &mut LineSplitter,
    att: &mut Attach,
) {
    if len < 4 {
        return;
    }
    // hidraw delivers whole reports; anything shorter was zero padded by `buf`
    match idx {
        3 => {
            log_split.push(&d[..len], |l| {
                sh.counters.log.fetch_add(1, Relaxed);
                match std::str::from_utf8(l) {
                    Ok(s) => h.on_log(s),
                    Err(_) => h.on_log(&String::from_utf8_lossy(l)),
                }
            });
        }
        0 => dispatch_if0(sh, h, cfg, d, att),
        _ => {
            if let Some(s) = proto::decode_imu(d) {
                sh.counters.imu.fetch_add(1, Relaxed);
                h.on_imu(&s[0]);
                h.on_imu(&s[1]);
            } else if let Some(c) = proto::decode_controller(d) {
                sh.counters.ctrl.fetch_add(1, Relaxed);
                att.last_rx[c.slot as usize] = Some(Instant::now());
                h.on_controller(&c);
            }
        }
    }
}

fn dispatch_if0(sh: &Shared, h: &mut dyn Handler, cfg: &Config, d: &Report, att: &mut Attach) {
    h.on_reply(d);
    if d[0] != 0x4c {
        return;
    }
    if d[2] == proto::cmd::ANNOUNCE {
        att.wake_acked = true;
    }
    let st = proto::parse_status(d);
    if st.iter().any(Option::is_some) {
        for s in st.iter().flatten() {
            sh.counters.status.fetch_add(1, Relaxed);
            if cfg.auto_attach {
                let now = Instant::now();
                let stale = att.last_rx[s.slot as usize]
                    .is_none_or(|t| now.duration_since(t) > STALE_AFTER);
                if stale {
                    att.seen37[s.slot as usize] = Some(now); // awake but not attached
                }
            }
            h.on_status(s);
        }
        if st[0].is_some() {
            return;
        }
    }
    let mut g = sh.lock();
    if let Some(p) = g.pending.as_mut() {
        if p.done.is_none() && (p.matcher)(d) {
            p.done = Some(*d);
            sh.cv.notify_all();
        }
    }
}
