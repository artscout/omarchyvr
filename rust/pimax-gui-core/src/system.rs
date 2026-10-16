//! The System page: pimaxd's view of the stack (docs/PIMAXD.md, snapshot schema v1).
//!
//! Data, in order (all in the background, never on the UI thread):
//! 1. `{"cmd":"state"}` on `$XDG_RUNTIME_DIR/pimax-autod.sock` (pimaxd is auto mode's daemon);
//! 2. the mirror `$XDG_RUNTIME_DIR/pimaxd/state.json` (`$PIMAXD_DIR`), when it is at most 10 s old;
//! 3. `pimax-health --json` (a read-only one-shot probe, the same schema) when its binary is there;
//! 4. else "pimaxd not running".
//!
//! Recent events come from `{"cmd":"events"}` (a stream, read by its own thread while the page is open). Actions are
//! one request each (`set`, `toggle`, `fix`, `safe-mode`, `ack-gpu`); the ones that stop or restart something go
//! through a confirmation first. In dry-run mode (UI tests) no action is sent; demo mode shows a fixture.
use crate::app::{AppCore, Confirm, Pending};
use serde_json::Value;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

/// The mirror counts as fresh this long.
pub const MIRROR_FRESH: Duration = Duration::from_secs(10);
/// Events kept.
pub const EVENTS_KEPT: usize = 40;
/// How often the state is asked while the page is open.
pub const POLL: Duration = Duration::from_secs(2);

/// A component's health as one of the page's five dot colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dot {
    /// ok.
    Ok,
    /// starting, degraded, blocked.
    Degraded,
    /// down, failed.
    Down,
    /// off.
    Off,
    /// unknown.
    Unknown,
}

impl Dot {
    /// From a `health` value.
    pub fn of(health: &str) -> Dot {
        match health {
            "ok" => Dot::Ok,
            "starting" | "degraded" | "blocked" => Dot::Degraded,
            "down" | "failed" => Dot::Down,
            "off" => Dot::Off,
            _ => Dot::Unknown,
        }
    }
}

/// `restarts`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Restarts {
    /// Used.
    pub used: u32,
    /// "2/15m" (None: pimaxd never restarts it).
    pub budget: Option<String>,
    /// closed | open | half-open.
    pub breaker: String,
}

/// `last_exit`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct LastExit {
    /// expected | collateral | user | crash | unknown.
    pub class: String,
    /// "signal 6".
    pub how: String,
    /// When (daemon clock).
    pub t: f64,
}

/// One row of `components`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Component {
    /// `monado`, `usb.cam`, ...
    pub id: String,
    /// For people.
    pub label: String,
    /// device, process, feature, setting, job.
    pub kind: String,
    /// Wanted on.
    pub wanted: bool,
    /// off | starting | ok | degraded | blocked | down | failed | unknown.
    pub health: String,
    /// Why (a blocking dependency's id, or text).
    pub why: Option<String>,
    /// Pid.
    pub pid: Option<u32>,
    /// Ready.
    pub ready: bool,
    /// Detail.
    pub detail: Option<String>,
    /// core, ...
    pub class: String,
    /// none | live | restart:<id>.
    pub toggle: String,
    /// Restart budget.
    pub restarts: Option<Restarts>,
    /// The last exit.
    pub last_exit: Option<LastExit>,
}

impl Component {
    /// The dot.
    pub fn dot(&self) -> Dot {
        Dot::of(&self.health)
    }

    /// Needs a look: down, failed, an open breaker or an exit nobody explained (a blocked one waits for its
    /// dependency, whose own row has the Fix).
    pub fn finding(&self) -> bool {
        matches!(self.health.as_str(), "down" | "failed")
            || self.restarts.as_ref().is_some_and(|r| r.breaker == "open")
            || self.last_exit.as_ref().is_some_and(|e| matches!(e.class.as_str(), "unknown" | "crash")) && self.health != "ok"
    }

    /// Can be switched live (`toggle`), or only at the next start (`restart:<id>`)?
    pub fn live_toggle(&self) -> bool {
        self.toggle == "live"
    }
}

/// `next_action`.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct NextAction {
    /// wait | notify | start | stop | stop-subtree | reset-headset | reapply.
    pub what: String,
    /// The component.
    pub id: String,
    /// Why.
    pub why: String,
    /// The ladder level.
    pub level: String,
}

/// The snapshot (schema v1; fields added later are ignored).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Snapshot {
    /// Schema version.
    pub v: u32,
    /// ok | degraded | down | needs-you | needs-reboot.
    pub overall: String,
    /// Safe mode (nothing runs automatically).
    pub safe_mode: bool,
    /// Why pimaxd put itself into safe mode.
    pub safe_reason: Option<String>,
    /// Observe only (`[restart] enabled` is not set).
    pub observe_only: bool,
    /// ok | caution | needs-reboot.
    pub gpu_gate: String,
    /// Kernel lines behind the gate.
    pub gpu_evidence: Vec<String>,
    /// The session intent: on | off | unset.
    pub intent_session: String,
    /// Who set it.
    pub intent_by: String,
    /// desk | vr.
    pub intent_mode: String,
    /// The running session's mode.
    pub session_mode: Option<String>,
    /// idle | starting | stopping.
    pub launcher: String,
    /// The launcher's watchdog runs.
    pub watchdog: bool,
    /// Components, in table order.
    pub components: Vec<Component>,
    /// What pimaxd does next.
    pub next_action: Option<NextAction>,
    /// What pimaxd is doing now.
    pub busy: Option<String>,
}

impl Snapshot {
    /// A reboot is needed (an Xid or a driver mismatch).
    pub fn needs_reboot(&self) -> bool {
        self.overall == "needs-reboot" || self.gpu_gate == "needs-reboot"
    }

    /// The GPU gate asks for an acknowledgement.
    pub fn gpu_caution(&self) -> bool {
        self.gpu_gate == "caution"
    }

    /// A component by id.
    pub fn component(&self, id: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.id == id)
    }
}

fn s(v: &Value) -> Option<String> {
    match v {
        Value::String(x) => Some(x.clone()),
        Value::Null => None,
        Value::Bool(b) => Some(b.to_string()),
        Value::Number(n) => Some(n.to_string()),
        other => Some(other.to_string()),
    }
}

/// Parse a snapshot line (the socket's reply, state.json, `pimax-health --json`).
pub fn parse_snapshot(text: &str) -> Result<Snapshot, String> {
    let v: Value = serde_json::from_str(text.trim()).map_err(|e| format!("not a snapshot ({e})"))?;
    if let Some(e) = v.get("error").and_then(Value::as_str) {
        return Err(e.to_string());
    }
    let ver = v["v"].as_u64().ok_or("not a snapshot (no schema version)")?;
    let comps = v["components"].as_array().cloned().unwrap_or_default();
    Ok(Snapshot {
        v: ver as u32,
        overall: s(&v["overall"]).unwrap_or_else(|| "unknown".into()),
        safe_mode: v["safe_mode"].as_bool().unwrap_or(false),
        safe_reason: s(&v["safe_reason"]),
        observe_only: v["observe_only"].as_bool().unwrap_or(false),
        gpu_gate: s(&v["gpu_gate"]).unwrap_or_else(|| "ok".into()),
        gpu_evidence: v["gpu_evidence"].as_array().map(|a| a.iter().filter_map(s).collect()).unwrap_or_default(),
        intent_session: s(&v["intent"]["session"]).unwrap_or_else(|| "unset".into()),
        intent_by: s(&v["intent"]["by"]).unwrap_or_default(),
        intent_mode: s(&v["intent"]["mode"]).unwrap_or_else(|| "desk".into()),
        session_mode: s(&v["session"]["mode"]),
        launcher: s(&v["session"]["launcher"]).unwrap_or_default(),
        watchdog: v["session"]["watchdog"].as_bool().unwrap_or(false),
        components: comps
            .iter()
            .map(|c| Component {
                id: s(&c["id"]).unwrap_or_default(),
                label: s(&c["label"]).unwrap_or_default(),
                kind: s(&c["kind"]).unwrap_or_default(),
                wanted: c["wanted"].as_bool().unwrap_or(false),
                health: s(&c["health"]).unwrap_or_else(|| "unknown".into()),
                why: s(&c["why"]),
                pid: c["pid"].as_u64().map(|p| p as u32),
                ready: c["ready"].as_bool().unwrap_or(false),
                detail: s(&c["detail"]),
                class: s(&c["class"]).unwrap_or_default(),
                toggle: s(&c["toggle"]).unwrap_or_else(|| "none".into()),
                restarts: c["restarts"].as_object().map(|r| Restarts {
                    used: r.get("used").and_then(Value::as_u64).unwrap_or(0) as u32,
                    budget: r.get("budget").and_then(s),
                    breaker: r.get("breaker").and_then(s).unwrap_or_else(|| "closed".into()),
                }),
                last_exit: c["last_exit"].as_object().map(|e| LastExit {
                    class: e.get("class").and_then(s).unwrap_or_default(),
                    how: e.get("how").and_then(s).unwrap_or_default(),
                    t: e.get("t").and_then(Value::as_f64).unwrap_or(0.0),
                }),
            })
            .collect(),
        next_action: v["next_action"].as_object().map(|a| NextAction {
            what: a.get("what").and_then(s).unwrap_or_default(),
            id: a.get("id").and_then(s).unwrap_or_default(),
            why: a.get("why").and_then(s).unwrap_or_default(),
            level: a.get("level").and_then(s).unwrap_or_default(),
        }),
        busy: s(&v["busy"]),
    })
}

/// One line of the `events` stream.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Event {
    /// log | notice | critical | would | step | done | uevent | exit | kernel | health (auto mode's own events too).
    pub event: String,
    /// Daemon clock.
    pub t: f64,
    /// Component.
    pub id: Option<String>,
    /// Text (or, for `health`, "health (why)").
    pub text: String,
}

/// Parse an event line (None: not one).
pub fn parse_event(line: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    let event = v["event"].as_str()?.to_string();
    let text = if event == "health" {
        let h = s(&v["health"]).unwrap_or_default();
        match s(&v["why"]) {
            Some(w) => format!("{h} ({w})"),
            None => h,
        }
    } else {
        s(&v["text"]).or_else(|| s(&v["state"])).or_else(|| s(&v["detail"])).unwrap_or_default()
    };
    Some(Event { event, t: v["t"].as_f64().unwrap_or(0.0), id: s(&v["id"]), text })
}

/// One request line, one reply line.
pub fn request(socket: &Path, line: &str, timeout: Duration) -> Result<String, String> {
    let mut st = UnixStream::connect(socket).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => "pimaxd not running".to_string(),
        _ => format!("cannot reach pimaxd: {e}"),
    })?;
    st.set_read_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    st.set_write_timeout(Some(timeout)).map_err(|e| e.to_string())?;
    st.write_all(format!("{line}\n").as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut r = String::new();
    BufReader::new(&st).read_line(&mut r).map_err(|e| format!("no reply: {e}"))?;
    if r.trim().is_empty() {
        return Err("empty reply".into());
    }
    Ok(r.trim_end().to_string())
}

/// Where the data came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// The daemon's socket.
    Socket,
    /// state.json, this many seconds old.
    Mirror(u64),
    /// `pimax-health --json` (pimaxd does not run).
    Health,
    /// Nothing.
    None,
}

impl Source {
    /// For people.
    pub fn label(&self) -> String {
        match self {
            Source::Socket => "pimaxd live".into(),
            Source::Mirror(a) => format!("pimaxd state.json ({a} s old)"),
            Source::Health => "pimaxd not running: a one-shot probe (pimax-health)".into(),
            Source::None => "pimaxd not running".into(),
        }
    }
}

/// Where to look.
#[derive(Debug, Clone)]
pub struct Where {
    /// The socket.
    pub socket: PathBuf,
    /// state.json.
    pub mirror: PathBuf,
    /// pimax-health candidates.
    pub health: Vec<PathBuf>,
}

/// The age of a file.
fn age(p: &Path) -> Option<Duration> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(SystemTime::now().duration_since(m).unwrap_or_default())
}

/// `pimax-health --json` with a deadline (left to finish by itself when it hangs).
fn run_health(bin: &Path) -> Result<Snapshot, String> {
    crate::cmd::allow_start("pimax-health")?;
    let mut child = std::process::Command::new(bin)
        .arg("--json")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("{}: {e}", bin.display()))?;
    let out = child.stdout.take();
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(mut o) = out {
            use std::io::Read;
            let _ = o.read_to_string(&mut s);
        }
        let _ = child.wait();
        let _ = tx.send(s);
    });
    let text = rx.recv_timeout(Duration::from_secs(15)).map_err(|_| "pimax-health did not answer within 15 s".to_string())?;
    parse_snapshot(text.lines().rev().find(|l| l.trim_start().starts_with('{')).unwrap_or(""))
}

/// Ask the socket, then the mirror, then the probe (background only).
pub fn fetch(w: &Where, probe: bool) -> (Option<Snapshot>, Source, Option<String>) {
    let sock_err = match request(&w.socket, r#"{"cmd":"state"}"#, Duration::from_secs(2)).and_then(|r| parse_snapshot(&r)) {
        Ok(s) => return (Some(s), Source::Socket, None),
        Err(e) => e,
    };
    if let Some(a) = age(&w.mirror).filter(|a| *a <= MIRROR_FRESH) {
        if let Ok(s) = std::fs::read_to_string(&w.mirror).map_err(|e| e.to_string()).and_then(|t| parse_snapshot(&t)) {
            return (Some(s), Source::Mirror(a.as_secs()), None);
        }
    }
    if probe {
        if let Some(bin) = w.health.iter().find(|p| p.is_file()) {
            if let Ok(s) = run_health(bin) {
                return (Some(s), Source::Health, Some(sock_err));
            }
        }
    }
    (None, Source::None, Some(sock_err))
}

/// The events stream: a thread that reads `{"cmd":"events"}` lines and reconnects every 5 s, until stopped.
pub struct EventStream {
    rx: Receiver<Event>,
    stop: Arc<AtomicBool>,
}

impl EventStream {
    /// Start reading.
    pub fn start(socket: PathBuf, wake: Arc<dyn Fn() + Send + Sync>) -> EventStream {
        let (tx, rx) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let st = stop.clone();
        let _ = std::thread::Builder::new().name("pimax-pimaxd-events".into()).spawn(move || {
            while !st.load(Ordering::SeqCst) {
                if let Ok(mut s) = UnixStream::connect(&socket) {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(1)));
                    if s.write_all(b"{\"cmd\":\"events\"}\n").is_ok() {
                        let mut r = BufReader::new(s);
                        let mut line = String::new();
                        loop {
                            if st.load(Ordering::SeqCst) {
                                return;
                            }
                            line.clear();
                            match r.read_line(&mut line) {
                                Ok(0) => break,
                                Ok(_) => {
                                    if let Some(e) = parse_event(&line) {
                                        if tx.send(e).is_err() {
                                            return;
                                        }
                                        wake();
                                    }
                                }
                                Err(e) if matches!(e.kind(), std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut) => {}
                                Err(_) => break,
                            }
                        }
                    }
                }
                for _ in 0..50 {
                    if st.load(Ordering::SeqCst) {
                        return;
                    }
                    std::thread::sleep(Duration::from_millis(100));
                }
            }
        });
        EventStream { rx, stop }
    }

    /// New events.
    pub fn drain(&self) -> Vec<Event> {
        self.rx.try_iter().collect()
    }
}

impl Drop for EventStream {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

impl std::fmt::Debug for EventStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("EventStream")
    }
}

/// The System page's state.
#[derive(Debug, Default)]
pub struct SystemView {
    /// The last snapshot.
    pub snap: Option<Snapshot>,
    /// Where it came from.
    pub source: Option<Source>,
    /// Why the daemon was not reached.
    pub error: Option<String>,
    /// Recent events, newest first.
    pub events: VecDeque<Event>,
    /// The last action's answer.
    pub reply: String,
    at: Option<Instant>,
    probed_at: Option<Instant>,
    stream: Option<EventStream>,
    /// The page was drawn in the last few seconds (the stream stops when it is not).
    seen: Option<Instant>,
}

/// The demo snapshot: a desk session with the camera missing, a GPU caution and a few events.
pub const DEMO_SNAPSHOT: &str = r#"{"v":1,"t":812.0,"unix":1792032882.8,"overall":"degraded","safe_mode":false,"safe_reason":null,"observe_only":true,
"gpu_gate":"caution","gpu_evidence":["kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel 0x00000016"],"boot_id":"3bfa",
"intent":{"session":"on","by":"adopted","mode":"desk","overrides":{}},
"session":{"mode":"desk","launcher":"idle","watchdog":true},
"components":[
{"id":"usb.hid","label":"Headset (USB 34a4:0044)","kind":"device","wanted":true,"health":"ok","why":null,"pid":null,"ready":true,"detail":"bus 1 dev 7","class":"device","toggle":"none","restarts":null,"last_exit":null},
{"id":"usb.cam","label":"Headset camera (USB 34a4:0014)","kind":"device","wanted":true,"health":"down","why":"gone 14 s ago","pid":null,"ready":false,"detail":"waiting 20 s for it to return by itself","class":"device","toggle":"none","restarts":{"used":0,"budget":"1/30m","breaker":"closed"},"last_exit":null},
{"id":"keepalive","label":"Keepalive (user service)","kind":"process","wanted":true,"health":"ok","why":null,"pid":1201,"ready":true,"detail":"active","class":"core","toggle":"none","restarts":{"used":0,"budget":"5/10m","breaker":"closed"},"last_exit":null},
{"id":"broker","label":"Camera broker","kind":"process","wanted":true,"health":"blocked","why":"usb.cam","pid":null,"ready":false,"detail":null,"class":"core","toggle":"none","restarts":{"used":1,"budget":"3/10m","breaker":"closed"},"last_exit":{"class":"collateral","how":"exit 1","t":800.0}},
{"id":"monado","label":"VR runtime (monado-service)","kind":"process","wanted":true,"health":"ok","why":null,"pid":372460,"ready":true,"detail":"pid 372460","class":"core","toggle":"restart:monado","restarts":{"used":0,"budget":"2/15m","breaker":"closed"},"last_exit":null},
{"id":"wayvr","label":"Desktop overlay (WayVR)","kind":"process","wanted":true,"health":"ok","why":null,"pid":378947,"ready":true,"detail":"pid 378947","class":"client","toggle":"live","restarts":{"used":0,"budget":"3/10m","breaker":"closed"},"last_exit":null},
{"id":"passthrough","label":"Passthrough","kind":"process","wanted":true,"health":"degraded","why":"no camera frames","pid":372563,"ready":true,"detail":"pid 372563","class":"client","toggle":"live","restarts":{"used":0,"budget":"3/10m","breaker":"closed"},"last_exit":null},
{"id":"voice.captions","label":"Subtitles (pimax-voice captions)","kind":"process","wanted":false,"health":"off","why":null,"pid":null,"ready":false,"detail":null,"class":"leaf","toggle":"live","restarts":{"used":0,"budget":"5/10m","breaker":"closed"},"last_exit":null},
{"id":"ai.server","label":"Local AI (pimax-ai)","kind":"process","wanted":false,"health":"off","why":null,"pid":null,"ready":false,"detail":null,"class":"leaf","toggle":"live","restarts":{"used":0,"budget":"5/10m","breaker":"closed"},"last_exit":null},
{"id":"sixdof","label":"6DoF tracking","kind":"feature","wanted":true,"health":"ok","why":null,"pid":null,"ready":true,"detail":null,"class":"feature","toggle":"restart:monado","restarts":null,"last_exit":null},
{"id":"eye.laser","label":"Eye laser","kind":"setting","wanted":false,"health":"off","why":null,"pid":null,"ready":false,"detail":null,"class":"setting","toggle":"live","restarts":null,"last_exit":null}],
"next_action":{"what":"wait","id":"usb.cam","why":"waiting 20 s for the camera to return by itself","at":832.0,"level":"L0"},
"busy":null}"#;

/// Demo events (newest first).
pub fn demo_events() -> Vec<Event> {
    [
        r#"{"event":"health","t":810.0,"id":"usb.cam","health":"down","why":"gone"}"#,
        r#"{"event":"would","t":810.5,"id":"usb.cam","text":"wait 20 s for the camera to return by itself"}"#,
        r#"{"event":"uevent","t":798.0,"id":"usb.cam","text":"remove 1-5.4"}"#,
        r#"{"event":"notice","t":700.0,"id":"gpu.gate","text":"GPU caution: a WayVR Xid 31 this boot (acknowledge on the System page)"}"#,
    ]
    .iter()
    .rev()
    .filter_map(|l| parse_event(l))
    .collect()
}

impl AppCore {
    /// Where pimaxd's data is.
    pub fn system_where(&self) -> Where {
        let dir = (self.env)("PIMAXD_DIR").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| self.paths.runtime_dir.join("pimaxd"));
        Where {
            socket: self.auto_socket.clone(),
            mirror: dir.join("state.json"),
            health: vec![self.paths.bin("pimax-health"), self.paths.home.join(".cargo/bin/pimax-health"), self.paths.home.join(".local/bin/pimax-health"), PathBuf::from("/usr/bin/pimax-health")],
        }
    }

    /// Keep the page fresh: the state every 2 s (the probe at most every 10 s), the events stream while it is open.
    pub fn want_system(&mut self) {
        self.system.seen = Some(Instant::now());
        if self.demo {
            if self.system.snap.is_none() {
                self.system.snap = parse_snapshot(DEMO_SNAPSHOT).ok();
                self.system.source = Some(Source::Socket);
                self.system.events = demo_events().into();
            }
            return;
        }
        if let Some(st) = &self.system.stream {
            for e in st.drain() {
                self.system.events.push_front(e);
            }
            self.system.events.truncate(EVENTS_KEPT);
        } else if !crate::cmd::dry_run() {
            self.system.stream = Some(EventStream::start(self.auto_socket.clone(), self.wake.clone()));
        }
        if self.system.at.is_some_and(|t| t.elapsed() < POLL) || self.task_busy("system") {
            return;
        }
        self.system.at = Some(Instant::now());
        let probe = self.system.probed_at.is_none_or(|t| t.elapsed() >= Duration::from_secs(10));
        if probe {
            self.system.probed_at = Some(Instant::now());
        }
        let w = self.system_where();
        self.task(Some("system"), move || {
            let (snap, source, err) = fetch(&w, probe);
            Box::new(move |c: &mut AppCore| {
                // a probe's answer stays until the next probe; the socket or the mirror replace it at once
                let keep = source == Source::None && !probe && matches!(c.system.source, Some(Source::Health));
                if !keep {
                    c.system.snap = snap;
                    c.system.source = Some(source);
                }
                c.system.error = err;
            })
        });
    }

    /// The page closed: stop the events stream (called by the window when another page shows).
    pub fn system_closed(&mut self) {
        if self.system.seen.is_some_and(|t| t.elapsed() > Duration::from_secs(3)) {
            self.system.stream = None;
        }
    }

    /// Send a request (background), then ask the state again.
    pub fn system_send(&mut self, line: String) {
        if self.demo {
            self.system.reply = format!("demo mode: would send {line}");
            return;
        }
        if let Err(e) = crate::cmd::allow_start("a pimaxd request") {
            self.system.reply = e;
            return;
        }
        let sock = self.auto_socket.clone();
        log::info!("pimaxd request: {line}");
        self.task(None, move || {
            let r = request(&sock, &line, Duration::from_secs(5));
            Box::new(move |c: &mut AppCore| {
                c.system.reply = match r {
                    Ok(reply) => match serde_json::from_str::<Value>(&reply) {
                        Ok(v) if v["ok"].as_bool() == Some(false) => format!("pimaxd: {}", v["error"].as_str().unwrap_or("refused")),
                        _ => format!("pimaxd: {reply}"),
                    },
                    Err(e) => e,
                };
                c.system.at = None;
            })
        });
    }

    fn system_confirm(&mut self, title: &str, detail: &str, line: String) {
        self.pending = Some(Pending::Confirm { title: title.into(), detail: detail.into(), command: line.clone(), what: Confirm::Daemon(line) });
    }

    /// The session on or off (off stops it: confirmed first).
    pub fn system_session(&mut self, on: bool) {
        let mode = self.system.snap.as_ref().map(|s| s.intent_mode.clone()).filter(|m| m == "desk" || m == "vr").unwrap_or_else(|| "desk".into());
        let line = format!(r#"{{"cmd":"set","session":"{}","mode":"{mode}","by":"gui"}}"#, if on { "on" } else { "off" });
        if on {
            self.system_send(line);
        } else {
            self.system_confirm("Stop the session?", "pimaxd stops the session in order (clients by pid, then Monado with a 30 s grace) and keeps it off.", line);
        }
    }

    /// A component on or off for this session (off stops it: confirmed first).
    pub fn system_toggle(&mut self, id: &str, on: bool) {
        let line = format!(r#"{{"cmd":"toggle","id":"{}","on":{on}}}"#, id.replace(['"', '\\'], ""));
        if on {
            self.system_send(line);
        } else {
            let label = self.system.snap.as_ref().and_then(|s| s.component(id)).map(|c| c.label.clone()).unwrap_or_else(|| id.to_string());
            self.system_confirm(&format!("Turn off {label}?"), "pimaxd stops it for this session and does not restart it until you turn it on again.", line);
        }
    }

    /// Fix: reset that component's breaker so pimaxd tries once more (it may restart it: confirmed first).
    pub fn system_fix(&mut self, id: &str) {
        let line = format!(r#"{{"cmd":"fix","id":"{}"}}"#, id.replace(['"', '\\'], ""));
        self.system_confirm(&format!("Fix {id}?"), "pimaxd forgets this component's failures and tries its next repair step once (it may restart it). With observe only, it only says what it would do.", line);
    }

    /// Safe mode on (nothing runs automatically) or off (automatic repairs again: confirmed first).
    pub fn system_safe_mode(&mut self, on: bool) {
        let line = format!(r#"{{"cmd":"safe-mode","on":{on}}}"#);
        if on {
            self.system_send(line);
        } else {
            self.system_confirm("Leave safe mode?", "pimaxd may then start, stop and restart parts of the stack by itself again (unless it only observes).", line);
        }
    }

    /// Start a session once past the reboot gate (`set session=on force`; user decision 9; confirmed first).
    pub fn system_force_start(&mut self) {
        let mode = self.system.snap.as_ref().map(|s| s.intent_mode.clone()).filter(|m| m == "desk" || m == "vr").unwrap_or_else(|| "desk".into());
        let line = format!(r#"{{"cmd":"set","session":"on","mode":"{mode}","by":"gui","force":true}}"#);
        self.system_confirm("Start once without the reboot?", "The GPU asked for a reboot. Starting anyway can hang the display engine; pimaxd lets this one start through.", line);
    }

    /// Acknowledge the GPU caution (confirmed first: it lets starts go ahead again).
    pub fn system_ack_gpu(&mut self) {
        self.system_confirm("Acknowledge the GPU warning?", "You saw the GPU kernel errors of this boot. Sessions may start again; a reboot clears them.", r#"{"cmd":"ack-gpu"}"#.into());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::net::UnixListener;

    #[test]
    fn the_snapshot_and_events_parse() {
        let s = parse_snapshot(DEMO_SNAPSHOT).unwrap();
        assert_eq!((s.v, s.overall.as_str(), s.gpu_gate.as_str(), s.observe_only), (1, "degraded", "caution", true));
        assert!(s.gpu_caution() && !s.needs_reboot());
        assert_eq!((s.intent_session.as_str(), s.intent_by.as_str(), s.session_mode.as_deref()), ("on", "adopted", Some("desk")));
        let cam = s.component("usb.cam").unwrap();
        assert!(cam.finding() && cam.dot() == Dot::Down);
        let broker = s.component("broker").unwrap();
        assert_eq!((broker.dot(), broker.why.as_deref()), (Dot::Degraded, Some("usb.cam")));
        assert!(!broker.finding(), "blocked: it waits for usb.cam, which has the Fix");
        assert!(s.component("wayvr").unwrap().live_toggle() && !s.component("monado").unwrap().live_toggle());
        assert_eq!(s.component("voice.captions").unwrap().dot(), Dot::Off);
        assert_eq!(s.next_action.as_ref().unwrap().id, "usb.cam");
        let reboot = parse_snapshot(r#"{"v":1,"overall":"needs-reboot","gpu_gate":"needs-reboot","components":[]}"#).unwrap();
        assert!(reboot.needs_reboot() && !reboot.gpu_caution());
        assert!(parse_snapshot(r#"{"ok":false,"error":"unknown cmd state"}"#).unwrap_err().contains("unknown cmd"));
        assert!(parse_snapshot("{}").is_err());
        let e = parse_event(r#"{"event":"health","t":5.0,"id":"monado","health":"down","why":"usb.hid"}"#).unwrap();
        assert_eq!((e.event.as_str(), e.text.as_str()), ("health", "down (usb.hid)"));
        assert_eq!(parse_event(r#"{"event":"would","t":1,"id":"wayvr","text":"restart wayvr"}"#).unwrap().text, "restart wayvr");
        assert!(parse_event("garbage").is_none());
        assert_eq!(Dot::of("starting"), Dot::Degraded);
        assert_eq!(Dot::of("weird"), Dot::Unknown);
    }

    /// A fake pimaxd: answers each request line with `reply(line)`, records the lines.
    fn fake(dir: &Path, reply: fn(&str) -> String) -> (PathBuf, Arc<std::sync::Mutex<Vec<String>>>) {
        let sock = dir.join("pimax-autod.sock");
        let _ = std::fs::remove_file(&sock);
        let l = UnixListener::bind(&sock).unwrap();
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let s2 = seen.clone();
        std::thread::spawn(move || {
            for c in l.incoming().flatten() {
                let mut r = BufReader::new(c.try_clone().unwrap());
                let mut line = String::new();
                if r.read_line(&mut line).is_ok() {
                    s2.lock().unwrap().push(line.trim().to_string());
                    let mut w = c;
                    let _ = w.write_all(format!("{}\n", reply(line.trim())).as_bytes());
                }
            }
        });
        (sock, seen)
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pimax-system-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_socket_then_the_fresh_mirror_then_not_running() {
        let d = tmp("fetch");
        let (sock, seen) = fake(&d, |l| if l.contains("\"state\"") { DEMO_SNAPSHOT.replace('\n', "") } else { r#"{"ok":true}"#.into() });
        let w = Where { socket: sock.clone(), mirror: d.join("pimaxd/state.json"), health: vec![] };
        let (s, src, err) = fetch(&w, false);
        assert_eq!((src, err), (Source::Socket, None));
        assert_eq!(s.unwrap().overall, "degraded");
        assert_eq!(seen.lock().unwrap()[0], r#"{"cmd":"state"}"#);
        // no daemon: a fresh mirror counts, a stale one does not
        let w = Where { socket: d.join("none.sock"), mirror: d.join("pimaxd/state.json"), health: vec![] };
        std::fs::create_dir_all(d.join("pimaxd")).unwrap();
        std::fs::write(&w.mirror, DEMO_SNAPSHOT).unwrap();
        let (s, src, _) = fetch(&w, false);
        assert!(matches!(src, Source::Mirror(a) if a <= 10) && s.is_some());
        let old = std::fs::File::options().write(true).open(&w.mirror).unwrap();
        old.set_modified(SystemTime::now() - Duration::from_secs(30)).unwrap();
        let (s, src, err) = fetch(&w, false);
        assert_eq!((s, src), (None, Source::None));
        assert_eq!(err.as_deref(), Some("pimaxd not running"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn actions_send_one_request_and_destructive_ones_are_confirmed_first() {
        let d = tmp("act");
        let (sock, seen) = fake(&d, |l| if l.contains("\"state\"") { DEMO_SNAPSHOT.replace('\n', "") } else if l.contains("monado") { r#"{"ok":false,"error":"monado needs a restart"}"#.into() } else { r#"{"ok":true}"#.into() });
        let mut a = AppCore::demo(false, pimax_config::Store::new(d.join("a"), d.join("b")));
        a.demo = false;
        a.auto_socket = sock;
        a.want_system();
        assert_eq!(a.system.snap.as_ref().unwrap().overall, "degraded");
        // on: sent at once
        a.system_toggle("voice.captions", true);
        // off: a confirmation first, nothing sent yet
        a.system_toggle("wayvr", false);
        assert!(matches!(&a.pending, Some(Pending::Confirm { what: Confirm::Daemon(l), .. }) if l.contains("\"wayvr\"")));
        let n = seen.lock().unwrap().len();
        a.cancel_pending();
        assert_eq!(seen.lock().unwrap().len(), n, "cancelled: nothing sent");
        a.system_toggle("wayvr", false);
        a.confirm_step();
        a.system_fix("usb.cam");
        a.confirm_step();
        a.system_safe_mode(true);
        a.system_session(false);
        a.confirm_step();
        a.system_ack_gpu();
        a.confirm_step();
        a.system_toggle("monado", true);
        assert_eq!(a.system.reply, "pimaxd: monado needs a restart");
        let lines: Vec<String> = seen.lock().unwrap().iter().filter(|l| !l.contains("\"state\"") && !l.contains("\"events\"")).cloned().collect();
        assert_eq!(
            lines,
            vec![
                r#"{"cmd":"toggle","id":"voice.captions","on":true}"#,
                r#"{"cmd":"toggle","id":"wayvr","on":false}"#,
                r#"{"cmd":"fix","id":"usb.cam"}"#,
                r#"{"cmd":"safe-mode","on":true}"#,
                r#"{"cmd":"set","session":"off","mode":"desk","by":"gui"}"#,
                r#"{"cmd":"ack-gpu"}"#,
                r#"{"cmd":"toggle","id":"monado","on":true}"#,
            ]
        );
        a.system.stream = None;
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn the_events_stream_reconnects_and_delivers() {
        let d = tmp("events");
        let sock = d.join("ev.sock");
        let l = UnixListener::bind(&sock).unwrap();
        std::thread::spawn(move || {
            if let Some(Ok(mut c)) = l.incoming().next() {
                let mut line = String::new();
                let _ = BufReader::new(c.try_clone().unwrap()).read_line(&mut line);
                assert_eq!(line.trim(), r#"{"cmd":"events"}"#);
                let _ = c.write_all(b"{\"event\":\"notice\",\"t\":1.0,\"id\":\"wayvr\",\"text\":\"WayVR restarted\"}\n");
                std::thread::sleep(Duration::from_millis(300));
            }
        });
        let st = EventStream::start(sock, Arc::new(|| {}));
        let t0 = Instant::now();
        let mut got = Vec::new();
        while got.is_empty() && t0.elapsed() < Duration::from_secs(5) {
            got = st.drain();
            std::thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(got[0].text, "WayVR restarted");
        drop(st);
        let _ = std::fs::remove_dir_all(d);
    }
}
