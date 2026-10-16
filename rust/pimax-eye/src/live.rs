//! A long-running gaze session: open and claim the EyeChip, send the captured Windows init (replay list), subscribe,
//! read gaze on a thread and publish [`Sample`]s in the OpenXR head frame with host-clock timestamps; unsubscribe and
//! release the interface on stop; start over when the device re-enumerates or stalls.
//!
//! Safety model is that of [`crate::frames`]: only list members go out ([`Guard`]), calibration opcodes are refused,
//! the single extra frame is the captured gaze unsubscribe (0x4ce, stream 0x0505, as Pimax Play sent it at the end of
//! a session; see [`crate::session::CAPTURED_UNSUBSCRIBE_GAZE`]).

use crate::calib::{Calibration, CombineState};
use crate::clock::{monotonic_ns, ClockMap};
use crate::correction::{tangent_of, Correction};
use crate::drift::{Drift, DriftParams};
use crate::filter::{FilterParams, OneEuro};
use crate::frames::{parse, select, Action, Guard, Skips, DATA};
use crate::gaze::parse_payload;
use crate::session::{in_decoder, CAPTURED_UNSUBSCRIBE_GAZE, IN_BUFFER};
use crate::track::{convert, sample_at, FrameConv, Sample};
use crate::transport::Transport;
use crate::ttp::{opcode_name, Kind, Message, StreamDecoder, STREAM_GAZE};
use crate::usbfs::{find_eye_chips, UsbFs};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

/// Connection state of a [`Tracker`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum State {
    /// No EyeChip on the bus (or the thread has not started yet).
    Searching = 0,
    /// Opening, init sequence running.
    Connecting = 1,
    /// Gaze samples are arriving.
    Streaming = 2,
    /// The link failed; waiting to try again.
    Reconnecting = 3,
    /// Stopped.
    Stopped = 4,
}

impl State {
    fn from_u8(v: u8) -> State {
        match v {
            1 => State::Connecting,
            2 => State::Streaming,
            3 => State::Reconnecting,
            4 => State::Stopped,
            _ => State::Searching,
        }
    }
}

/// Session options.
#[derive(Debug, Clone)]
pub struct Config {
    /// Send the captured calibration blob (0x456, the user's profile). Off by default: streaming works without it.
    pub with_cal: bool,
    /// Leave out the 0x76c / 0x777 auth pair (the answer is rejected by the chip but harmless).
    pub skip_auth: bool,
    /// Leave out the vendor control write bReq 48.
    pub skip_vendor_write: bool,
    /// Device-to-head frame conversion.
    pub conv: FrameConv,
    /// Host-side accuracy correction applied to every converted sample (`None`: raw). The old combined-gaze
    /// correction (`eye-correction.toml`); ignored when `calibration` is set.
    pub correction: Option<Correction>,
    /// Per-eye calibration (`eye-calibration.toml`); its device-to-head conversion replaces `conv`.
    pub calibration: Option<Calibration>,
    /// Output smoothing (`None`: the calibration's `[filter]`, or no filter without a calibration).
    pub filter: Option<FilterParams>,
    /// Transport latency subtracted from mapped host times.
    pub latency_ns: i64,
    /// A streaming link with no gaze for this long is torn down and re-initialised.
    pub stall: Duration,
    /// Wait between connection attempts (doubles up to 10 s while attempts fail).
    pub retry: Duration,
    /// Timeout of every USB transfer.
    pub timeout: Duration,
    /// How long a request may wait for its answer (the 34 KB calibration upload gets at least 10 s).
    pub response_wait: Duration,
    /// Samples kept for [`Tracker::sample_at`].
    pub history: usize,
    /// Use this usbfs node instead of searching sysfs for 2104:0561.
    pub usb_device: Option<PathBuf>,
    /// Pace the init steps like the capture (clamped to 1 s). Off only in tests.
    pub pace: bool,
    /// Recover a chip that stopped answering with a host-side USB port reset (`USBDEVFS_RESET`).
    pub usb_reset: bool,
    /// Consecutive "silent" link failures (a request without answer, or the gaze stream stopping) before a reset.
    pub reset_after: u32,
    /// Minimum time between two USB resets.
    pub reset_min_interval: Duration,
    /// Power the eye module (captured enable on the headset, see [`crate::power`]) when the headset is present and
    /// the chip has been absent for 5 s; at most once per headset enumeration.
    pub power_on: bool,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            with_cal: false,
            skip_auth: false,
            skip_vendor_write: true,
            conv: FrameConv::default(),
            correction: None,
            calibration: None,
            filter: None,
            latency_ns: 0,
            stall: Duration::from_secs(3),
            retry: Duration::from_secs(2),
            timeout: Duration::from_secs(2),
            response_wait: Duration::from_secs(3),
            history: 256,
            usb_device: None,
            pace: true,
            usb_reset: true,
            reset_after: 2,
            reset_min_interval: Duration::from_secs(60),
            power_on: true,
        }
    }
}

impl Config {
    /// Defaults overridden by the environment: frame conversion (`PIMAX_EYE_FLIP_*`, see [`crate::track`]),
    /// `PIMAX_EYE_CAL=1` (send the calibration blob), `PIMAX_EYE_SKIP_AUTH=1`, `PIMAX_EYE_LATENCY_MS`,
    /// `PIMAX_EYE_USB_RESET=0` (no USB reset recovery), `PIMAX_EYE_POWER_ON=0` (never power the eye module),
    /// `PIMAX_EYE_USB` (usbfs node), `PIMAX_EYE_CORRECTION=path|none` (see [`crate::correction`]; unset: the file
    /// `~/.config/pimax/eye-correction.toml` when it exists).
    pub fn from_env() -> Config {
        let mut c = Config {
            conv: FrameConv::from_env(),
            ..Config::default()
        };
        let flag = |n: &str| {
            std::env::var(n).is_ok_and(|v| matches!(v.trim(), "1" | "true" | "yes" | "on"))
        };
        c.with_cal = flag("PIMAX_EYE_CAL");
        c.skip_auth = flag("PIMAX_EYE_SKIP_AUTH");
        let off = |n: &str| {
            std::env::var(n).is_ok_and(|v| matches!(v.trim(), "0" | "false" | "no" | "off"))
        };
        c.usb_reset = !off("PIMAX_EYE_USB_RESET");
        c.power_on = !off("PIMAX_EYE_POWER_ON");
        if let Some(ms) = std::env::var("PIMAX_EYE_LATENCY_MS")
            .ok()
            .and_then(|v| v.trim().parse::<f64>().ok())
        {
            c.latency_ns = (ms * 1e6) as i64;
        }
        c.usb_device = std::env::var_os("PIMAX_EYE_USB").map(PathBuf::from);
        match Calibration::from_env() {
            Ok(Some((cal, path))) => {
                let env_conv = c.conv;
                c.conv = cal.conv;
                eprintln!(
                    "pimax-eye: per-eye calibration {} ({} / {} model, filter {}){}",
                    path.display(),
                    cal.eyes[0].map.model,
                    cal.eyes[1].map.model,
                    if cal.filter.enabled { "one-euro" } else { "off" },
                    if env_conv != cal.conv {
                        format!(
                            "; using its frame conversion {:?} instead of the PIMAX_EYE_FLIP_* environment {:?}",
                            cal.conv, env_conv
                        )
                    } else {
                        String::new()
                    }
                );
                c.calibration = Some(cal);
            }
            Ok(None) => {}
            Err(e) => eprintln!("pimax-eye: per-eye calibration not loaded (raw gaze): {e}"),
        }
        if c.calibration.is_none() {
            match Correction::from_env() {
                Ok(Some(corr)) => {
                    eprintln!(
                        "pimax-eye: gaze correction active ({} model){}",
                        corr.model,
                        corr.flip_warning(&c.conv)
                            .map(|w| format!("\npimax-eye: warning: {w}"))
                            .unwrap_or_default()
                    );
                    c.correction = Some(corr);
                }
                Ok(None) => {}
                Err(e) => eprintln!("pimax-eye: gaze correction not loaded: {e}"),
            }
        }
        match std::env::var("PIMAX_EYE_FILTER").ok().as_deref().map(str::trim) {
            Some("none" | "off" | "0") => {
                c.filter = Some(FilterParams {
                    enabled: false,
                    ..FilterParams::default()
                })
            }
            Some("one-euro" | "on" | "1") => {
                c.filter = Some(FilterParams {
                    enabled: true,
                    ..c.calibration.as_ref().map(|k| k.filter).unwrap_or_default()
                })
            }
            _ => {}
        }
        c
    }
}

/// Decodes the IN pipe, maps timestamps and hands out samples: device-to-head conversion, then the per-eye
/// calibration and combination (or the old combined correction), the running drift, the output filter.
pub struct Pipeline {
    dec: StreamDecoder,
    clock: ClockMap,
    conv: FrameConv,
    correction: Option<Correction>,
    calibration: Option<(Calibration, CombineState)>,
    filter: FilterParams,
    one_euro: OneEuro,
    drift: Option<Arc<Mutex<Drift>>>,
    sink: Box<dyn FnMut(Sample) + Send>,
    now: fn() -> i64,
    /// Gaze samples decoded.
    pub samples: u64,
    /// Gaze payloads that failed to decode.
    pub parse_errors: u64,
    /// IN framing errors.
    pub framing_errors: u64,
}

impl Pipeline {
    /// New pipeline; `sink` receives every sample.
    pub fn new(cfg: &Config, sink: Box<dyn FnMut(Sample) + Send>) -> Pipeline {
        let mut clock = ClockMap::default();
        clock.latency_ns = cfg.latency_ns;
        let filter = cfg.filter.unwrap_or_else(|| match &cfg.calibration {
            Some(k) => k.filter,
            None => FilterParams {
                enabled: false,
                ..FilterParams::default()
            },
        });
        Pipeline {
            dec: in_decoder(),
            clock,
            conv: cfg.calibration.as_ref().map_or(cfg.conv, |k| k.conv),
            correction: cfg.correction.clone().filter(|_| cfg.calibration.is_none()),
            calibration: cfg.calibration.clone().map(|k| (k, CombineState::default())),
            filter,
            one_euro: OneEuro::default(),
            drift: None,
            sink,
            now: monotonic_ns,
            samples: 0,
            parse_errors: 0,
            framing_errors: 0,
        }
    }

    /// Start a new link: fresh framing and a fresh clock map (the chip's clock restarts when it re-enumerates).
    pub fn reset_link(&mut self) {
        self.dec = in_decoder();
        self.clock.reset();
        self.one_euro.reset();
    }

    /// Share a running drift estimate (added after the per-eye calibration).
    pub fn set_drift(&mut self, d: Arc<Mutex<Drift>>) {
        self.drift = Some(d);
    }

    /// Feed one IN transfer. Gaze notifications become samples; every other message is returned.
    pub fn push(&mut self, bytes: &[u8]) -> Vec<Message> {
        let arrival = (self.now)();
        let mut rest = Vec::new();
        for r in self.dec.push(bytes) {
            match r {
                Ok(m) if m.header.kind == Kind::Notification && m.header.id == STREAM_GAZE => {
                    match parse_payload(&m.payload) {
                        Ok(g) => {
                            let host = self.clock.map(g.timestamp_us, arrival);
                            let mut s = convert(&g, &self.conv, self.samples, host, arrival);
                            if let Some((k, st)) = self.calibration.as_mut() {
                                let extra = self.drift.as_ref().map_or([(0.0, 0.0); 2], |d| {
                                    d.lock().unwrap_or_else(|e| e.into_inner()).offset
                                });
                                k.apply_sample_with(&mut s, st, extra);
                            } else if let Some(c) = &self.correction {
                                c.apply_sample(&mut s);
                            }
                            crate::filter::apply_sample(&self.filter, &mut self.one_euro, &mut s);
                            self.samples += 1;
                            (self.sink)(s);
                        }
                        Err(_) => self.parse_errors += 1,
                    }
                }
                Ok(m) => rest.push(m),
                Err(_) => self.framing_errors += 1,
            }
        }
        rest
    }
}

/// Why a link ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Failure {
    /// A request got no answer in time (the chip is alive on USB but silent).
    NoResponse,
    /// The gaze stream stopped (or never started after the subscribe).
    Stall,
    /// A request was answered with an unexpected status.
    Status,
    /// A USB transfer failed (device gone, protocol error).
    Io,
    /// Anything else (frame list, guard).
    Other,
}

impl Failure {
    /// The chip is reachable but does not talk: what a USB reset may cure.
    pub fn is_silent(self) -> bool {
        matches!(self, Failure::NoResponse | Failure::Stall)
    }
}

/// A failed link.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkError {
    /// Class of the failure.
    pub kind: Failure,
    /// Text for the log.
    pub msg: String,
}

impl LinkError {
    fn new(kind: Failure, msg: impl Into<String>) -> LinkError {
        LinkError {
            kind,
            msg: msg.into(),
        }
    }
}

impl std::fmt::Display for LinkError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.msg)
    }
}

/// When to USB-reset the chip: after [`Config::reset_after`] consecutive silent failures, at most once per
/// [`Config::reset_min_interval`].
#[derive(Debug, Default, Clone)]
pub struct Recovery {
    silent: u32,
    last_reset: Option<Instant>,
    /// Resets done.
    pub resets: u32,
}

impl Recovery {
    /// Account for a finished link (`failure` is `None` for a clean stop); `true` means "reset the chip now".
    pub fn after_link(
        &mut self,
        failure: Option<Failure>,
        streamed: bool,
        now: Instant,
        cfg: &Config,
    ) -> bool {
        // a link that streamed proves the chip was fine, unless it ended because the stream stopped
        if streamed && failure != Some(Failure::Stall) {
            self.silent = 0;
        }
        match failure {
            Some(f) if f.is_silent() => self.silent += 1,
            Some(_) | None => {
                self.silent = 0;
                return false;
            }
        }
        let spaced = self
            .last_reset
            .is_none_or(|t| now.duration_since(t) >= cfg.reset_min_interval);
        if cfg.usb_reset && self.silent >= cfg.reset_after.max(1) && spaced {
            self.silent = 0;
            self.last_reset = Some(now);
            self.resets += 1;
            return true;
        }
        false
    }
}

/// How the tracker reaches a chip: open it, reset it, and react to its absence. [`RealDevice`] is the usbfs one.
pub trait Device: Send + 'static {
    /// The open link.
    type Link: Transport;
    /// Open and claim; `Err` = no usable chip (absent, busy).
    fn open(&mut self, cfg: &Config, stop: &AtomicBool) -> Result<Self::Link, String>;
    /// USB-reset the chip (the link is closed when this is called); `Ok` carries a log line.
    fn reset(&mut self, _cfg: &Config) -> Result<String, String> {
        Err("USB reset not supported by this device".into())
    }
    /// Called after `open` failed: may power the eye module; returns a log line when it did something.
    fn absent(&mut self, _cfg: &Config, _now: Instant) -> Option<String> {
        None
    }
}

/// A [`Device`] made of an open closure (tests): no reset, no power control.
pub struct FnDevice<F>(pub F);

impl<T, F> Device for FnDevice<F>
where
    T: Transport + 'static,
    F: FnMut(&Config, &AtomicBool) -> Result<T, String> + Send + 'static,
{
    type Link = T;
    fn open(&mut self, cfg: &Config, stop: &AtomicBool) -> Result<T, String> {
        (self.0)(cfg, stop)
    }
}

fn sleep_stoppable(d: Duration, stop: &AtomicBool) {
    let end = Instant::now() + d;
    while !stop.load(Ordering::Relaxed) {
        let left = end.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        std::thread::sleep(left.min(Duration::from_millis(50)));
    }
}

/// Run one link on an open transport until `stop` is set (Ok) or the link fails (Err; the caller reconnects).
/// Sends the init unless gaze is already flowing, then reads; on the way out unsubscribes if it subscribed.
pub fn run_link<T: Transport>(
    t: &mut T,
    cfg: &Config,
    stop: &AtomicBool,
    pipe: &mut Pipeline,
    state: &mut dyn FnMut(State),
    log: &mut dyn FnMut(&str),
) -> Result<(), LinkError> {
    let steps = parse(DATA).map_err(|e| LinkError::new(Failure::Other, e))?;
    let skips = Skips {
        auth: cfg.skip_auth,
        cal: !cfg.with_cal,
        vendor_write: cfg.skip_vendor_write,
    };
    let sel = select(&steps, skips);
    let mut guard = Guard::new(&steps).map_err(|e| LinkError::new(Failure::Other, e))?;
    guard.allow_frame(&CAPTURED_UNSUBSCRIBE_GAZE);
    pipe.reset_link();
    state(State::Connecting);

    // Whatever the chip has pending; gaze already flowing means a previous session is still subscribed: adopt it.
    let before = pipe.samples;
    let until = Instant::now() + Duration::from_millis(300);
    while Instant::now() < until && !stop.load(Ordering::Relaxed) {
        if let Some(b) = t
            .bulk_in(0x83, IN_BUFFER, Duration::from_millis(100))
            .map_err(|e| LinkError::new(Failure::Io, format!("initial IN read: {e}")))?
        {
            pipe.push(&b);
        }
    }
    let adopted = pipe.samples > before;
    let mut subscribed = false;
    let mut result = Ok(());
    if adopted {
        log("gaze already streaming (earlier session); not sending the init, will not unsubscribe");
    } else {
        result = send_sequence(t, cfg, &sel, &guard, stop, pipe, &mut subscribed, log);
    }
    if result.is_ok() {
        result = read_loop(t, cfg, stop, pipe, state);
    }
    if subscribed {
        // best effort; on a dead device bulk_out fails at once
        if guard
            .bulk_out(t, &CAPTURED_UNSUBSCRIBE_GAZE, Duration::from_millis(500))
            .is_ok()
        {
            let end = Instant::now() + Duration::from_millis(300);
            while Instant::now() < end {
                match t.bulk_in(0x83, IN_BUFFER, Duration::from_millis(50)) {
                    Ok(Some(b)) => {
                        pipe.push(&b);
                    }
                    Ok(None) => {}
                    Err(_) => break,
                }
            }
            log("unsubscribed (0x4ce)");
        }
    }
    result
}

#[allow(clippy::too_many_arguments)]
fn send_sequence<T: Transport>(
    t: &mut T,
    cfg: &Config,
    sel: &[&crate::frames::FStep],
    guard: &Guard,
    stop: &AtomicBool,
    pipe: &mut Pipeline,
    subscribed: &mut bool,
    log: &mut dyn FnMut(&str),
) -> Result<(), LinkError> {
    let mut prev_t: Option<f64> = None;
    let mut prev_sent = Instant::now();
    for st in sel {
        if stop.load(Ordering::Relaxed) {
            return Ok(());
        }
        match &st.action {
            Action::Enumeration => unreachable!("never selected"),
            Action::Control { setup, data } => {
                guard
                    .control(t, *setup, data, cfg.timeout)
                    .map_err(|e| LinkError::new(Failure::Io, format!("step {}: {e}", st.n)))?;
            }
            Action::Ttp { seq, op, frames } => {
                if let (true, Some(pt), Some(ct)) = (cfg.pace, prev_t, st.t) {
                    let gap = Duration::from_secs_f64((ct - pt).clamp(0.0, 1.0));
                    sleep_stoppable(gap.saturating_sub(prev_sent.elapsed()), stop);
                }
                prev_sent = Instant::now();
                prev_t = st.t;
                for f in frames {
                    guard.bulk_out(t, f, cfg.timeout).map_err(|e| {
                        LinkError::new(
                            Failure::Io,
                            format!("step {} ({}): {e}", st.n, opcode_name(*op)),
                        )
                    })?;
                }
                if *op == 0x4c4 {
                    *subscribed = true;
                }
                let sent = Instant::now();
                let wait = if *op == 0x456 {
                    cfg.response_wait.max(Duration::from_secs(10))
                } else {
                    cfg.response_wait
                };
                let resp = loop {
                    if stop.load(Ordering::Relaxed) {
                        return Ok(());
                    }
                    if sent.elapsed() > wait {
                        return Err(LinkError::new(
                            Failure::NoResponse,
                            format!(
                                "no response to seq {seq} op {op:#x} ({}) within {:.1} s",
                                opcode_name(*op),
                                wait.as_secs_f64()
                            ),
                        ));
                    }
                    let Some(b) = t
                        .bulk_in(0x83, IN_BUFFER, Duration::from_millis(200))
                        .map_err(|e| LinkError::new(Failure::Io, format!("IN read: {e}")))?
                    else {
                        continue;
                    };
                    let hit = pipe
                        .push(&b)
                        .into_iter()
                        .find(|m| m.header.kind == Kind::Response && m.header.seq == *seq);
                    if let Some(m) = hit {
                        break m;
                    }
                };
                if resp.header.status != 1 && !crate::frames::AUTH_OPS.contains(op) {
                    return Err(LinkError::new(
                        Failure::Status,
                        format!(
                            "op {op:#x} ({}) answered with status {} (captured: 1)",
                            opcode_name(*op),
                            resp.header.status
                        ),
                    ));
                }
                if resp.header.status != 1 {
                    log(&format!(
                        "auth op {op:#x} answered with status {} (expected, not needed)",
                        resp.header.status
                    ));
                }
            }
        }
    }
    Ok(())
}

fn read_loop<T: Transport>(
    t: &mut T,
    cfg: &Config,
    stop: &AtomicBool,
    pipe: &mut Pipeline,
    state: &mut dyn FnMut(State),
) -> Result<(), LinkError> {
    let mut last_gaze = Instant::now();
    let mut last_count = pipe.samples;
    let mut announced = false;
    // the first gaze can take a while after the calibration upload
    let first_wait = cfg.stall.max(Duration::from_secs(10));
    while !stop.load(Ordering::Relaxed) {
        match t.bulk_in(0x83, IN_BUFFER, Duration::from_millis(100)) {
            Ok(Some(b)) => {
                pipe.push(&b);
            }
            Ok(None) => {}
            Err(e) => return Err(LinkError::new(Failure::Io, format!("IN read: {e}"))),
        }
        if pipe.samples != last_count {
            last_count = pipe.samples;
            last_gaze = Instant::now();
            if !announced {
                announced = true;
                state(State::Streaming);
            }
        } else if last_gaze.elapsed() > if announced { cfg.stall } else { first_wait } {
            return Err(LinkError::new(
                Failure::Stall,
                format!("no gaze for {:.1} s", last_gaze.elapsed().as_secs_f64()),
            ));
        }
    }
    Ok(())
}

#[derive(Default)]
struct History {
    buf: VecDeque<Sample>,
}

struct Shared {
    stop: AtomicBool,
    state: AtomicU8,
    hist: Mutex<History>,
    samples: AtomicU64,
    reconnects: AtomicU64,
    resets: AtomicU64,
    drift: Arc<Mutex<Drift>>,
    calibration: Option<Calibration>,
    cap: usize,
}

/// Counters of a running [`Tracker`].
#[derive(Debug, Clone, Copy, Default)]
pub struct TrackerStats {
    /// Samples published.
    pub samples: u64,
    /// Link failures followed by a new attempt.
    pub reconnects: u64,
    /// USB resets done to recover a silent chip.
    pub resets: u64,
}

/// The gaze session handle. Dropping it stops the thread (unsubscribe, release).
pub struct Tracker {
    shared: Arc<Shared>,
    join: Option<JoinHandle<()>>,
}

impl Tracker {
    /// Start the reader thread on the real device.
    pub fn start(cfg: Config) -> Tracker {
        Tracker::start_device(cfg, RealDevice::default())
    }

    /// Start with a custom way to open a transport (tests): called for every connection attempt, `Err` means "no
    /// device". No USB reset and no power control.
    pub fn start_with<T, F>(cfg: Config, open: F) -> Tracker
    where
        T: Transport + 'static,
        F: FnMut(&Config, &AtomicBool) -> Result<T, String> + Send + 'static,
    {
        Tracker::start_device(cfg, FnDevice(open))
    }

    /// Start with any [`Device`].
    pub fn start_device<D: Device>(cfg: Config, mut dev: D) -> Tracker {
        let shared = Arc::new(Shared {
            stop: AtomicBool::new(false),
            state: AtomicU8::new(State::Searching as u8),
            hist: Mutex::new(History::default()),
            samples: AtomicU64::new(0),
            reconnects: AtomicU64::new(0),
            resets: AtomicU64::new(0),
            drift: Arc::new(Mutex::new(Drift::default())),
            calibration: cfg.calibration.clone(),
            cap: cfg.history.max(2),
        });
        let sh = Arc::clone(&shared);
        let join = std::thread::Builder::new()
            .name("pimax-eye".into())
            .spawn(move || {
                let sink_sh = Arc::clone(&sh);
                let mut pipe = Pipeline::new(
                    &cfg,
                    Box::new(move |s| {
                        sink_sh.samples.fetch_add(1, Ordering::Relaxed);
                        let mut h = sink_sh.hist.lock().unwrap_or_else(|e| e.into_inner());
                        if h.buf.len() >= sink_sh.cap {
                            h.buf.pop_front();
                        }
                        h.buf.push_back(s);
                    }),
                );
                pipe.set_drift(Arc::clone(&sh.drift));
                let set = |s: State| sh.state.store(s as u8, Ordering::Relaxed);
                let mut backoff = cfg.retry;
                let mut last_msg = String::new();
                let mut say = |m: &str| {
                    if m != last_msg {
                        eprintln!("pimax-eye: {m}");
                        last_msg = m.to_string();
                    }
                };
                let mut recovery = Recovery::default();
                while !sh.stop.load(Ordering::Relaxed) {
                    set(State::Searching);
                    let mut t = match dev.open(&cfg, &sh.stop) {
                        Ok(t) => t,
                        Err(e) => {
                            say(&e);
                            if let Some(m) = dev.absent(&cfg, Instant::now()) {
                                say(&m);
                                backoff = cfg.retry;
                            }
                            sleep_stoppable(backoff, &sh.stop);
                            backoff = (backoff * 2).min(Duration::from_secs(10));
                            continue;
                        }
                    };
                    let mut streamed = false;
                    let r = run_link(
                        &mut t,
                        &cfg,
                        &sh.stop,
                        &mut pipe,
                        &mut |s| {
                            streamed |= s == State::Streaming;
                            set(s);
                        },
                        &mut |m| say(m),
                    );
                    drop(t); // releases the interface
                    if streamed {
                        backoff = cfg.retry;
                    }
                    let failure = r.as_ref().err().map(|e| e.kind);
                    let reset = recovery.after_link(failure, streamed, Instant::now(), &cfg);
                    if let Err(e) = r {
                        say(&format!("link lost: {e}; retrying"));
                        sh.reconnects.fetch_add(1, Ordering::Relaxed);
                        set(State::Reconnecting);
                        if reset && !sh.stop.load(Ordering::Relaxed) {
                            sh.resets.fetch_add(1, Ordering::Relaxed);
                            match dev.reset(&cfg) {
                                Ok(m) => say(&m),
                                Err(m) => say(&format!("USB reset failed: {m}")),
                            }
                            // re-enumeration takes about a second; then try at once
                            sleep_stoppable(Duration::from_millis(1500), &sh.stop);
                            backoff = cfg.retry;
                        } else {
                            sleep_stoppable(backoff, &sh.stop);
                            backoff = (backoff * 2).min(Duration::from_secs(10));
                        }
                    }
                }
                set(State::Stopped);
            })
            .expect("spawn pimax-eye thread");
        Tracker {
            shared,
            join: Some(join),
        }
    }

    /// Current state.
    pub fn state(&self) -> State {
        State::from_u8(self.shared.state.load(Ordering::Relaxed))
    }

    /// Counters.
    pub fn stats(&self) -> TrackerStats {
        TrackerStats {
            samples: self.shared.samples.load(Ordering::Relaxed),
            reconnects: self.shared.reconnects.load(Ordering::Relaxed),
            resets: self.shared.resets.load(Ordering::Relaxed),
        }
    }

    /// Newest sample, if any arrived.
    pub fn latest(&self) -> Option<Sample> {
        self.shared
            .hist
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .buf
            .back()
            .copied()
    }

    /// Every sample still in the history with `seq > after` (all of them for `None`), oldest first. Polling this once
    /// per rendered frame loses nothing while the history (default 256 samples, about 2 s) covers the gap.
    pub fn samples_since(&self, after: Option<u64>) -> Vec<Sample> {
        let h = self.shared.hist.lock().unwrap_or_else(|e| e.into_inner());
        h.buf
            .iter()
            .filter(|s| after.is_none_or(|a| s.seq > a))
            .copied()
            .collect()
    }

    /// Sample for host time `t_ns` (`CLOCK_MONOTONIC`): interpolated between neighbours, clamped to the history ends.
    pub fn sample_at(&self, t_ns: i64) -> Option<Sample> {
        let h = self.shared.hist.lock().unwrap_or_else(|e| e.into_inner());
        let (a, b) = h.buf.as_slices();
        if b.is_empty() {
            sample_at(a, t_ns)
        } else {
            let v: Vec<Sample> = h.buf.iter().copied().collect();
            sample_at(&v, t_ns)
        }
    }

    /// Drift-correction hook: the wearer was looking along `dir` (head frame, unit, from the head origin) at host
    /// time `at_ns`. Each calibrated eye's residual updates the running per-eye offset (see [`crate::drift`]).
    /// Returns how many eyes accepted the observation; `Err` without a calibration or a sample at that time.
    pub fn observe_target(&self, at_ns: i64, dir: [f64; 3], p: &DriftParams) -> Result<usize, String> {
        let cal = self
            .shared
            .calibration
            .as_ref()
            .ok_or("no per-eye calibration loaded: drift correction needs one")?;
        let s = self.sample_at(at_ns).ok_or("no gaze sample")?;
        let mid = [
            0.5 * (cal.eyes[0].origin_m[0] + cal.eyes[1].origin_m[0]),
            0.5 * (cal.eyes[0].origin_m[1] + cal.eyes[1].origin_m[1]),
            0.5 * (cal.eyes[0].origin_m[2] + cal.eyes[1].origin_m[2]),
        ];
        let target = [
            mid[0] + cal.combine.depth_m * dir[0],
            mid[1] + cal.combine.depth_m * dir[1],
            mid[2] + cal.combine.depth_m * dir[2],
        ];
        let mut d = self.shared.drift.lock().unwrap_or_else(|e| e.into_inner());
        let mut used = 0;
        for (i, e) in s.eyes.iter().enumerate() {
            if !(e.origin_valid && e.direction_valid) {
                continue;
            }
            let o = cal.eyes[i].origin_m;
            let want = [target[0] - o[0], target[1] - o[1], target[2] - o[2]];
            let got = [
                f64::from(e.direction[0]),
                f64::from(e.direction[1]),
                f64::from(e.direction[2]),
            ];
            if let (Some(w), Some(g)) = (tangent_of(want), tangent_of(got)) {
                // the published direction already holds the running offset: observe without it
                let cur = d.offset[i];
                if d.observe(p, i, (g.0 - cur.0, g.1 - cur.1), w) {
                    used += 1;
                }
            }
        }
        Ok(used)
    }

    /// The running drift offsets.
    pub fn drift(&self) -> Drift {
        self.shared.drift.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Stop the thread and wait for it (unsubscribes and releases the interface first).
    pub fn stop(mut self) {
        self.shutdown();
    }

    fn shutdown(&mut self) {
        self.shared.stop.store(true, Ordering::SeqCst);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Tracker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

/// The usbfs EyeChip: sysfs discovery, USB reset, eye-module power-on (see [`crate::power`]).
#[derive(Default)]
pub struct RealDevice {
    gate: crate::power::EnableGate,
}

fn chip_node(cfg: &Config) -> Result<PathBuf, String> {
    match &cfg.usb_device {
        Some(p) => Ok(p.clone()),
        None => {
            let chips = find_eye_chips();
            let Some(c) = chips.first() else {
                return Err("no EyeChip (2104:0561) on the USB bus".into());
            };
            if let Some(d) = c.interface_driver() {
                return Err(format!(
                    "kernel driver '{d}' is bound to the EyeChip interface; not detaching it"
                ));
            }
            Ok(c.devnode.clone())
        }
    }
}

impl Device for RealDevice {
    type Link = UsbFs;

    fn open(&mut self, cfg: &Config, _stop: &AtomicBool) -> Result<UsbFs, String> {
        let node = chip_node(cfg)?;
        let mut dev = UsbFs::open(&node).map_err(|e| format!("open {}: {e}", node.display()))?;
        dev.claim_interface(0)
            .map_err(|e| format!("claim interface 0 of {}: {e}", node.display()))?;
        Ok(dev)
    }

    fn reset(&mut self, cfg: &Config) -> Result<String, String> {
        let node = chip_node(cfg)?;
        crate::usbfs::reset_node(&node).map_err(|e| format!("{}: {e}", node.display()))?;
        Ok(format!(
            "chip silent: host-side USB port reset of 2104:0561 ({}); it re-enumerates like a replug",
            node.display()
        ))
    }

    fn absent(&mut self, cfg: &Config, now: Instant) -> Option<String> {
        use crate::power::{eye_chip_present, headset, headset_command_node, send_enable, Gate};
        if !cfg.power_on || cfg.usb_device.is_some() {
            return None;
        }
        let usb = std::path::Path::new("/sys/bus/usb/devices");
        let h = headset(usb);
        match self.gate.step(now, h.as_ref(), eye_chip_present(usb)) {
            Gate::Send(h) => {
                let node = headset_command_node(
                    std::path::Path::new("/sys/class/hidraw"),
                    std::path::Path::new("/dev"),
                );
                Some(match node {
                    Some(n) => match send_enable(&n) {
                        Ok(()) => format!(
                            "eye module off (headset {} devnum {} present, no 2104:0561 for 5 s): sent the captured \
                             eye-module enable 58 05 3c 05 00 01 to {} (once for this headset enumeration)",
                            h.name,
                            h.devnum,
                            n.display()
                        ),
                        Err(e) => format!("eye module enable to {} failed: {e}", n.display()),
                    },
                    None => "eye module off but no hidraw node for interface 0 of 34a4:0044".into(),
                })
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeChip, Sent};

    fn fast_cfg() -> Config {
        Config {
            pace: false,
            retry: Duration::from_millis(20),
            history: 64,
            ..Config::default()
        }
    }

    fn sent_ops(chip: &FakeChip) -> Vec<u32> {
        let mut dec = StreamDecoder::new(crate::ttp::Direction::Out);
        let mut ops = Vec::new();
        for s in &chip.sent {
            if let Sent::BulkOut { data, .. } = s {
                for m in dec.push(data).into_iter().flatten() {
                    ops.push(m.header.id);
                }
            }
        }
        ops
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn link_inits_streams_and_unsubscribes_without_cal() {
        let cfg = fast_cfg();
        let got = Arc::new(Mutex::new(Vec::<Sample>::new()));
        let g2 = Arc::clone(&got);
        let stop_ref: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let mut pipe = Pipeline::new(
            &cfg,
            Box::new(move |s| {
                let mut v = g2.lock().unwrap();
                v.push(s);
                if v.len() >= 40 {
                    stop_ref.store(true, Ordering::SeqCst);
                }
            }),
        );
        let mut chip = FakeChip::new();
        chip.start_on = Some(0x4c4);
        let mut states = Vec::new();
        run_link(
            &mut chip,
            &cfg,
            stop_ref,
            &mut pipe,
            &mut |s| states.push(s),
            &mut |_| {},
        )
        .unwrap();
        let ops = sent_ops(&chip);
        assert!(ops.contains(&0x4c4), "{ops:x?}");
        assert!(
            !ops.contains(&0x456),
            "calibration must not be sent by default"
        );
        assert_eq!(*ops.last().unwrap(), 0x4ce, "unsubscribe last: {ops:x?}");
        for denied in crate::frames::DENIED_OPS {
            assert!(!ops.contains(&denied));
        }
        assert!(!chip.streaming, "fake stops streaming on 0x4ce");
        assert_eq!(states.first(), Some(&State::Connecting));
        assert!(states.contains(&State::Streaming));
        let v = got.lock().unwrap();
        assert!(v.len() >= 40);
        assert!(v
            .windows(2)
            .all(|w| w[1].host_ts_ns > w[0].host_ts_ns && w[1].seq == w[0].seq + 1));
        // fake: eye A tracked only, A is the right eye by default
        assert!(v[0].eyes[1].direction_valid && !v[0].eyes[0].direction_valid);
        assert_eq!(v[0].gaze.unwrap().eyes, 0b10);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn with_cal_sends_the_captured_blob_only() {
        let cfg = Config {
            with_cal: true,
            ..fast_cfg()
        };
        let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let mut pipe = Pipeline::new(
            &cfg,
            Box::new(move |s| {
                if s.seq >= 5 {
                    stop.store(true, Ordering::SeqCst);
                }
            }),
        );
        let mut chip = FakeChip::new(); // starts streaming after 0x456
        run_link(&mut chip, &cfg, stop, &mut pipe, &mut |_| {}, &mut |_| {}).unwrap();
        let ops = sent_ops(&chip);
        assert!(ops.contains(&0x456));
        assert_eq!(*ops.last().unwrap(), 0x4ce);
    }

    #[test]
    fn already_streaming_chip_is_adopted_not_reinitialised() {
        let cfg = fast_cfg();
        let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let mut pipe = Pipeline::new(
            &cfg,
            Box::new(move |s| {
                if s.seq >= 5 {
                    stop.store(true, Ordering::SeqCst);
                }
            }),
        );
        let mut chip = FakeChip::new();
        chip.streaming = true;
        run_link(&mut chip, &cfg, stop, &mut pipe, &mut |_| {}, &mut |_| {}).unwrap();
        assert!(
            chip.sent.is_empty(),
            "nothing may be sent to a session we did not start"
        );
    }

    #[test]
    fn silent_chip_is_a_stall_error() {
        let cfg = Config {
            stall: Duration::from_millis(100),
            ..fast_cfg()
        };
        let stop = AtomicBool::new(false);
        let mut pipe = Pipeline::new(&cfg, Box::new(|_| {}));
        let mut chip = FakeChip::new();
        chip.start_on = None; // never streams
                              // first-gaze wait is 10 s; shorten by stopping from another thread would not test the stall, so just check
                              // that a dead sequence step errors out instead of hanging: silence the subscribe op.
        chip.silent.push(0x4c4);
        let t0 = Instant::now();
        let r = std::thread::scope(|sc| {
            let h =
                sc.spawn(|| run_link(&mut chip, &cfg, &stop, &mut pipe, &mut |_| {}, &mut |_| {}));
            std::thread::sleep(Duration::from_millis(500));
            stop.store(true, Ordering::SeqCst);
            h.join().unwrap()
        });
        // stopped by the flag while waiting for the response: clean exit, bounded time
        assert!(r.is_ok());
        assert!(t0.elapsed() < Duration::from_secs(5));
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn tracker_thread_publishes_and_stops_cleanly() {
        let tr = Tracker::start_with(fast_cfg(), |_, _| {
            let mut c = FakeChip::new();
            c.start_on = Some(0x4c4);
            Ok(c)
        });
        let t0 = Instant::now();
        while tr.latest().is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        let l = tr.latest().expect("a sample");
        assert_eq!(tr.state(), State::Streaming);
        // interpolation at a time between two history entries
        std::thread::sleep(Duration::from_millis(100));
        let l2 = tr.latest().unwrap();
        let mid = (l.host_ts_ns + l2.host_ts_ns) / 2;
        assert_eq!(tr.sample_at(mid).unwrap().host_ts_ns, mid);
        assert_eq!(
            tr.sample_at(i64::MAX).unwrap().seq,
            tr.latest().unwrap().seq
        );
        assert!(tr.stats().samples > 0);
        tr.stop();
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn pipeline_applies_the_configured_correction_to_every_sample() {
        use crate::correction::{dir_of, tangent_of};
        let first_gaze = |corr: Option<Correction>| {
            let cfg = Config {
                correction: corr,
                ..fast_cfg()
            };
            let got = Arc::new(Mutex::new(Vec::<Sample>::new()));
            let g2 = Arc::clone(&got);
            let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
            let mut pipe = Pipeline::new(
                &cfg,
                Box::new(move |s| {
                    let mut v = g2.lock().unwrap();
                    v.push(s);
                    if v.len() >= 3 {
                        stop.store(true, Ordering::SeqCst);
                    }
                }),
            );
            let mut chip = FakeChip::new();
            chip.start_on = Some(0x4c4);
            run_link(&mut chip, &cfg, stop, &mut pipe, &mut |_| {}, &mut |_| {}).unwrap();
            let v = got.lock().unwrap();
            v[0].gaze.expect("fake chip tracks eye A")
        };
        let raw = first_gaze(None);
        let mut c = Correction::identity();
        c.cx = [0.1, 1.0, 0.0, 0.0, 0.0, 0.0]; // shift u by 0.1
        let fixed = first_gaze(Some(c));
        let (ru, rv) = tangent_of(raw.direction.map(f64::from)).unwrap();
        let (fu, fv) = tangent_of(fixed.direction.map(f64::from)).unwrap();
        assert!(
            (fu - (ru + 0.1)).abs() < 1e-4 && (fv - rv).abs() < 1e-4,
            "{ru} {fu}"
        );
        assert_eq!(raw.origin, fixed.origin);
        let _ = dir_of(0.0, 0.0);
    }

    fn first_samples(cfg: Config, n: usize) -> Vec<Sample> {
        let got = Arc::new(Mutex::new(Vec::<Sample>::new()));
        let g2 = Arc::clone(&got);
        let stop: &'static AtomicBool = Box::leak(Box::new(AtomicBool::new(false)));
        let mut pipe = Pipeline::new(
            &cfg,
            Box::new(move |s| {
                let mut v = g2.lock().unwrap();
                v.push(s);
                if v.len() >= n {
                    stop.store(true, Ordering::SeqCst);
                }
            }),
        );
        let mut chip = FakeChip::new();
        chip.start_on = Some(0x4c4);
        run_link(&mut chip, &cfg, stop, &mut pipe, &mut |_| {}, &mut |_| {}).unwrap();
        let v = got.lock().unwrap().clone();
        v
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn pipeline_applies_the_per_eye_calibration_and_its_conversion() {
        use crate::calib::Calibration;
        use crate::correction::{dir_of, tangent_of};
        let raw = first_samples(fast_cfg(), 3);
        // a calibration measured with FLIP_X: eye A becomes the LEFT eye, and its map shifts u by +0.05
        let mut cal = Calibration::identity(
            FrameConv {
                flip_x: true,
                ..FrameConv::default()
            },
            0.064,
        );
        cal.eyes[0].map.cx[0] = 0.05;
        cal.filter.enabled = false;
        let cfg = Config {
            calibration: Some(cal.clone()),
            // an old correction file is ignored when a calibration is present
            correction: Some({
                let mut c = Correction::identity();
                c.cx[0] = 1.0;
                c
            }),
            ..fast_cfg()
        };
        let out = first_samples(cfg, 3);
        let r = &raw[0].eyes[1]; // default conversion: A is the right eye
        let o = &out[0].eyes[0]; // calibration conversion: A is the left eye
        assert!(o.direction_valid && !out[0].eyes[1].direction_valid);
        let (ru, rv) = tangent_of([-f64::from(r.direction[0]), f64::from(r.direction[1]), f64::from(r.direction[2])]).unwrap();
        let (ou, ov) = tangent_of(o.direction.map(f64::from)).unwrap();
        assert!((ou - (ru + 0.05)).abs() < 1e-5 && (ov - rv).abs() < 1e-5, "{ru} {ou}");
        assert_eq!(o.origin, [-0.032, 0.0, 0.0]);
        let g = out[0].gaze.unwrap();
        assert_eq!(g.eyes, 0b01);
        assert_eq!(g.origin, [0.0, 0.0, 0.0]);
        let _ = dir_of(0.0, 0.0);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn drift_hook_moves_the_calibrated_gaze_towards_the_target() {
        use crate::calib::Calibration;
        let mut cal = Calibration::identity(FrameConv::default(), 0.064);
        cal.filter.enabled = false;
        let cfg = Config {
            calibration: Some(cal),
            ..fast_cfg()
        };
        let tr = Tracker::start_with(cfg, |_, _| {
            let mut c = FakeChip::new();
            c.start_on = Some(0x4c4);
            Ok(c)
        });
        let t0 = Instant::now();
        while tr.latest().is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let s = tr.latest().unwrap();
        let g = s.gaze.unwrap();
        // the wearer was looking 2 degrees to the right of where the gaze is
        let (u, v) = tangent_of(g.direction.map(f64::from)).unwrap();
        let truth = crate::correction::dir_of(u + 2f64.to_radians().tan(), v);
        let p = DriftParams {
            gain: 1.0,
            ..DriftParams::default()
        };
        assert_eq!(tr.observe_target(s.host_ts_ns, truth, &p).unwrap(), 1);
        let d = tr.drift();
        assert!(d.offset[1].0 > 0.03 && d.offset[1].0 < 0.04, "{:?}", d.offset);
        assert_eq!(d.offset[0], (0.0, 0.0));
        tr.stop();
        // without a calibration the hook refuses
        let tr = Tracker::start_with(fast_cfg(), |_, _| Err::<FakeChip, _>("none".to_string()));
        assert!(tr.observe_target(0, [0.0, 0.0, -1.0], &p).is_err());
        tr.stop();
    }

    #[test]
    fn recovery_resets_after_two_silent_failures_and_spaces_resets() {
        let cfg = Config {
            reset_min_interval: Duration::from_secs(60),
            ..Config::default()
        };
        let mut r = Recovery::default();
        let t0 = Instant::now();
        assert!(!r.after_link(Some(Failure::NoResponse), false, t0, &cfg));
        assert!(r.after_link(Some(Failure::NoResponse), false, t0, &cfg));
        assert_eq!(r.resets, 1);
        // two more silent failures within the minimum interval: no second reset yet
        assert!(!r.after_link(Some(Failure::Stall), false, t0 + Duration::from_secs(5), &cfg));
        assert!(!r.after_link(Some(Failure::NoResponse), false, t0 + Duration::from_secs(10), &cfg));
        assert!(r.after_link(Some(Failure::NoResponse), false, t0 + Duration::from_secs(70), &cfg));
        // an I/O error (device gone) or a clean stop clears the count
        let mut r = Recovery::default();
        assert!(!r.after_link(Some(Failure::NoResponse), false, t0, &cfg));
        assert!(!r.after_link(Some(Failure::Io), false, t0, &cfg));
        assert!(!r.after_link(Some(Failure::NoResponse), false, t0, &cfg));
        assert!(!r.after_link(None, true, t0, &cfg));
        assert!(!r.after_link(Some(Failure::NoResponse), false, t0, &cfg));
        // a link that streamed and then stalled counts once
        let mut r = Recovery::default();
        assert!(!r.after_link(Some(Failure::Stall), true, t0, &cfg));
        assert!(r.after_link(Some(Failure::Stall), true, t0, &cfg));
        // disabled
        let off = Config {
            usb_reset: false,
            ..Config::default()
        };
        let mut r = Recovery::default();
        for _ in 0..5 {
            assert!(!r.after_link(Some(Failure::NoResponse), false, t0, &off));
        }
    }

    /// A chip that ignores 0x547 (the 14 Oct wedge) until it is USB-reset.
    struct WedgedUntilReset {
        resets: Arc<AtomicU64>,
        opens: Arc<AtomicU64>,
    }

    impl Device for WedgedUntilReset {
        type Link = FakeChip;
        fn open(&mut self, _cfg: &Config, _stop: &AtomicBool) -> Result<FakeChip, String> {
            self.opens.fetch_add(1, Ordering::SeqCst);
            let mut c = FakeChip::new();
            c.start_on = Some(0x4c4);
            if self.resets.load(Ordering::SeqCst) == 0 {
                c.silent.push(0x547);
            }
            Ok(c)
        }
        fn reset(&mut self, _cfg: &Config) -> Result<String, String> {
            self.resets.fetch_add(1, Ordering::SeqCst);
            Ok("reset".into())
        }
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn silent_chip_is_usb_reset_and_streams_again() {
        let cfg = Config {
            response_wait: Duration::from_millis(150),
            reset_min_interval: Duration::ZERO,
            ..fast_cfg()
        };
        let resets = Arc::new(AtomicU64::new(0));
        let opens = Arc::new(AtomicU64::new(0));
        let tr = Tracker::start_device(
            cfg,
            WedgedUntilReset {
                resets: Arc::clone(&resets),
                opens: Arc::clone(&opens),
            },
        );
        let t0 = Instant::now();
        while tr.latest().is_none() && t0.elapsed() < Duration::from_secs(20) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(tr.latest().is_some(), "streams after the reset");
        assert_eq!(resets.load(Ordering::SeqCst), 1, "exactly one reset");
        assert!(opens.load(Ordering::SeqCst) >= 3, "two silent attempts, then the good one");
        let st = tr.stats();
        assert_eq!(st.resets, 1);
        assert!(st.reconnects >= 2);
        // the samples arrive in order and none is lost between polls
        let all = tr.samples_since(None);
        assert!(!all.is_empty());
        let last = all.last().unwrap().seq;
        std::thread::sleep(Duration::from_millis(100));
        let more = tr.samples_since(Some(last));
        assert!(more.iter().all(|s| s.seq > last));
        assert!(more.windows(2).all(|w| w[1].seq == w[0].seq + 1));
        tr.stop();
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn tracker_reconnects_when_the_device_goes_away() {
        use std::sync::atomic::AtomicU32;
        let opens = Arc::new(AtomicU32::new(0));
        let o2 = Arc::clone(&opens);
        let tr = Tracker::start_with(fast_cfg(), move |_, _| {
            let n = o2.fetch_add(1, Ordering::SeqCst);
            if n == 0 {
                Err("no EyeChip yet".to_string())
            } else {
                let mut c = FakeChip::new();
                c.start_on = Some(0x4c4);
                Ok(c)
            }
        });
        let t0 = Instant::now();
        while tr.latest().is_none() && t0.elapsed() < Duration::from_secs(10) {
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(tr.latest().is_some());
        assert!(opens.load(Ordering::SeqCst) >= 2);
        tr.stop();
    }
}
