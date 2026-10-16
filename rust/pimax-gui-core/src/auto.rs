//! Auto mode (`pimax-autod`, docs/AUTO-MODE.md): its status and commands over its JSON-lines socket
//! (`$PIMAX_AUTOD_SOCKET`, else `$XDG_RUNTIME_DIR/pimax-autod.sock`). The app does not link the daemon; requests run
//! in background tasks with a timeout, never in demo mode or the UI tests.
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The daemon's socket.
pub fn socket_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("PIMAX_AUTOD_SOCKET").filter(|v| !v.is_empty()) {
        return PathBuf::from(p);
    }
    let run = env("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()).unwrap_or_else(|| "/tmp".into());
    PathBuf::from(run).join("pimax-autod.sock")
}

/// The status object of the daemon.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AutoStatus {
    /// `unplugged`, `plugged`, `worn`, `active(desk|game)`, `idle-dim(desk|game)`, `sleep`.
    pub state: String,
    /// No starts or stops while paused.
    pub paused: bool,
    /// Logs only (`dry_run = true` in auto.toml).
    pub dry_run: bool,
    /// The session was started by auto mode.
    pub owned: bool,
    /// `start`, `stop`, `switch` while one runs.
    pub busy: Option<String>,
    /// Seconds since the daemon started.
    pub uptime_s: u64,
    /// Headset on USB.
    pub plugged: bool,
    /// `unknown`, `still`, `worn`, `moving`.
    pub motion: String,
    /// The session kind, if one runs.
    pub session: Option<String>,
    /// A game runs.
    pub game: bool,
    /// Worn.
    pub worn: bool,
    /// `desk` or `game`.
    pub mode: String,
    /// `monado` or `steamvr`.
    pub game_runtime: String,
    /// `none`, `cinema`, `work`, `assistant`.
    pub preset: String,
    /// Starts on put-on.
    pub auto_start: bool,
}

/// Parse a reply line: the status, or the daemon's error.
pub fn parse_status(line: &str) -> Result<AutoStatus, String> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).map_err(|e| format!("not a reply: {e}"))?;
    if v.get("ok").and_then(|o| o.as_bool()) == Some(false) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("error").to_string());
    }
    let s = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_str()).unwrap_or("").to_string();
    let b = |o: &serde_json::Value, k: &str| o.get(k).and_then(|x| x.as_bool()).unwrap_or(false);
    let sig = v.get("signals").cloned().unwrap_or_default();
    let cfg = v.get("config").cloned().unwrap_or_default();
    Ok(AutoStatus {
        state: s(&v, "state"),
        paused: b(&v, "paused"),
        dry_run: b(&v, "dry_run"),
        owned: b(&v, "owned"),
        busy: v.get("busy").and_then(|x| x.as_str()).map(str::to_string),
        uptime_s: v.get("uptime_s").and_then(|x| x.as_u64()).unwrap_or(0),
        plugged: b(&sig, "plugged"),
        motion: s(&sig, "motion"),
        session: sig.get("session").and_then(|x| x.as_str()).map(str::to_string),
        game: b(&sig, "game"),
        worn: b(&sig, "worn"),
        mode: s(&cfg, "mode"),
        game_runtime: s(&cfg, "game_runtime"),
        preset: s(&cfg, "preset"),
        auto_start: b(&cfg, "auto_start"),
    })
}

/// A request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AutoCmd {
    /// The status.
    Status,
    /// No starts or stops.
    Pause,
    /// Back to work.
    Resume,
    /// "I'm wearing it": start without waiting for the IMU.
    Wake,
    /// Stop our own (non-game) session now.
    Sleep,
    /// Mode, runtime, preset (any subset).
    SetMode {
        /// `desk` / `game`.
        mode: Option<String>,
        /// `monado` / `steamvr`.
        runtime: Option<String>,
        /// `none` / `cinema` / `work` / `assistant`.
        preset: Option<String>,
    },
}

impl AutoCmd {
    /// The request line (without the newline).
    pub fn json(&self) -> String {
        let simple = |c: &str| serde_json::json!({ "cmd": c }).to_string();
        match self {
            AutoCmd::Status => simple("status"),
            AutoCmd::Pause => simple("pause"),
            AutoCmd::Resume => simple("resume"),
            AutoCmd::Wake => simple("wake"),
            AutoCmd::Sleep => simple("sleep"),
            AutoCmd::SetMode { mode, runtime, preset } => {
                let mut m = serde_json::Map::new();
                m.insert("cmd".into(), "set-mode".into());
                for (k, v) in [("mode", mode), ("runtime", runtime), ("preset", preset)] {
                    if let Some(v) = v {
                        m.insert(k.into(), v.clone().into());
                    }
                }
                serde_json::Value::Object(m).to_string()
            }
        }
    }

    /// For people.
    pub fn label(&self) -> &'static str {
        match self {
            AutoCmd::Status => "status",
            AutoCmd::Pause => "pause",
            AutoCmd::Resume => "resume",
            AutoCmd::Wake => "start now",
            AutoCmd::Sleep => "sleep now",
            AutoCmd::SetMode { .. } => "set mode",
        }
    }
}

/// A pairing attempt's state (the daemon's `pair` events).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairState {
    /// StartPair sent; press and hold the controller's pairing buttons.
    Waiting,
    /// Paired (Monado attaches it within about 2 s).
    Paired,
    /// 35 s passed (or the firmware cancelled).
    Timeout,
    /// Could not send (nothing was sent).
    Failed,
}

impl PairState {
    /// From the event's `state`.
    pub fn parse(s: &str) -> Option<PairState> {
        Some(match s {
            "waiting" => PairState::Waiting,
            "paired" => PairState::Paired,
            "timeout" => PairState::Timeout,
            "failed" => PairState::Failed,
            _ => return None,
        })
    }

    /// Final?
    pub fn done(self) -> bool {
        self != PairState::Waiting
    }
}

/// How long the controller has to answer.
pub const PAIR_SECS: u64 = 35;

/// What the user does while pairing (the button combination is in the Pimax manual).
pub const PAIR_HINT: &str = "Put the controller in pairing mode first (hold its pairing buttons, see the Pimax manual), then press Pair for that hand only and wait up to 60 s. The headset briefly drops the other controller while it listens.";

/// A `pair` event line for `side`: (state, detail).
pub fn parse_pair_event(line: &str, side: &str) -> Option<(PairState, String)> {
    let v: serde_json::Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("event")?.as_str()? != "pair" || v.get("side")?.as_str()? != side {
        return None;
    }
    let st = PairState::parse(v.get("state")?.as_str()?)?;
    Some((st, v.get("detail").and_then(|d| d.as_str()).unwrap_or("").to_string()))
}

/// Pair one controller through the daemon (background tasks only): send `pair`, then follow the event stream until
/// the attempt ends (at most 45 s). Exactly one StartPair per call; never retried.
pub fn pair(socket: &Path, side: &str) -> Result<(PairState, String), String> {
    crate::cmd::allow_start("controller pairing")?;
    let connect = || -> Result<std::os::unix::net::UnixStream, String> {
        let s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| match e.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => "not running".to_string(),
            _ => format!("cannot reach the daemon: {e}"),
        })?;
        s.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
        Ok(s)
    };
    // the event stream first, so no event of this attempt is missed
    let ev = connect()?;
    ev.set_read_timeout(Some(Duration::from_secs(45))).map_err(|e| e.to_string())?;
    (&ev).write_all(b"{\"cmd\":\"events\"}\n").map_err(|e| format!("write: {e}"))?;
    let mut events = BufReader::new(&ev);
    let mut first = String::new();
    events.read_line(&mut first).map_err(|e| format!("no reply: {e}"))?;
    let req = connect()?;
    req.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    (&req).write_all(format!("{}\n", serde_json::json!({ "cmd": "pair", "side": side })).as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut reply = String::new();
    BufReader::new(&req).read_line(&mut reply).map_err(|e| format!("no reply: {e}"))?;
    let v: serde_json::Value = serde_json::from_str(reply.trim()).map_err(|e| format!("not a reply: {e}"))?;
    if v.get("ok").and_then(|o| o.as_bool()) != Some(true) {
        return Err(v.get("error").and_then(|e| e.as_str()).unwrap_or("refused").to_string());
    }
    let t0 = std::time::Instant::now();
    let mut last = (PairState::Waiting, String::new());
    while t0.elapsed() < Duration::from_secs(PAIR_SECS + 10) {
        let mut line = String::new();
        match events.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                if let Some(e) = parse_pair_event(&line, side) {
                    last = e;
                    if last.0.done() {
                        return Ok(last);
                    }
                }
            }
            Err(_) => break,
        }
    }
    Ok((PairState::Timeout, if last.1.is_empty() { "no answer from the daemon".into() } else { last.1 }))
}

/// How long a request may take.
pub const TIMEOUT: Duration = Duration::from_secs(2);

/// Send one request and read the reply (background tasks only). `Err("not running")` when there is no socket.
pub fn request(socket: &Path, c: &AutoCmd) -> Result<AutoStatus, String> {
    if crate::cmd::dry_run() {
        return Err("dry run: the daemon is not asked".into());
    }
    let mut s = std::os::unix::net::UnixStream::connect(socket).map_err(|e| match e.kind() {
        std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused => "not running".to_string(),
        _ => format!("cannot reach the daemon: {e}"),
    })?;
    s.set_read_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    s.set_write_timeout(Some(TIMEOUT)).map_err(|e| e.to_string())?;
    s.write_all(format!("{}\n", c.json()).as_bytes()).map_err(|e| format!("write: {e}"))?;
    let mut line = String::new();
    BufReader::new(&s).read_line(&mut line).map_err(|e| format!("no reply: {e}"))?;
    parse_status(&line)
}

/// Demo data (screenshots, tests): a daemon in dry run with the headset worn.
pub fn demo_status() -> AutoStatus {
    AutoStatus {
        state: "active(desk)".into(),
        dry_run: true,
        owned: true,
        uptime_s: 812,
        plugged: true,
        motion: "worn".into(),
        session: Some("desk".into()),
        worn: true,
        mode: "desk".into(),
        game_runtime: "monado".into(),
        preset: "none".into(),
        auto_start: true,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_commands_and_socket() {
        let line = r#"{"ok":true,"state":"active(desk)","paused":false,"dry_run":true,"owned":true,"busy":null,"uptime_s":812,"signals":{"plugged":true,"motion":"worn","visor":null,"session":"desk","game":false,"input":false,"worn":true},"config":{"mode":"desk","game_runtime":"monado","preset":"none","auto_start":true,"sleep_games":false,"dim_after_s":60,"sleep_after_s":600}}"#;
        let s = parse_status(line).unwrap();
        assert_eq!(s, demo_status());
        assert_eq!(parse_status(r#"{"ok":false,"error":"busy"}"#).unwrap_err(), "busy");
        assert!(parse_status("x").is_err());
        assert_eq!(AutoCmd::Pause.json(), r#"{"cmd":"pause"}"#);
        let m = AutoCmd::SetMode { mode: Some("game".into()), runtime: None, preset: Some("cinema".into()) }.json();
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["cmd"], "set-mode");
        assert_eq!(v["mode"], "game");
        assert_eq!(v["preset"], "cinema");
        assert!(v.get("runtime").is_none());
        assert_eq!(socket_path(&|k| (k == "XDG_RUNTIME_DIR").then(|| "/run/user/7".to_string())), PathBuf::from("/run/user/7/pimax-autod.sock"));
        assert_eq!(socket_path(&|k| (k == "PIMAX_AUTOD_SOCKET").then(|| "/s".to_string())), PathBuf::from("/s"));
    }

    #[test]
    fn pair_events() {
        let l = r#"{"event":"pair","side":"left","state":"waiting","detail":"0/35 s: Press and hold"}"#;
        assert_eq!(parse_pair_event(l, "left"), Some((PairState::Waiting, "0/35 s: Press and hold".into())));
        assert_eq!(parse_pair_event(l, "right"), None);
        assert_eq!(parse_pair_event(r#"{"event":"transition"}"#, "left"), None);
        assert!(PairState::Paired.done() && !PairState::Waiting.done());
    }

    #[test]
    fn talks_to_a_socket_with_one_reply_per_line() {
        let d = std::env::temp_dir().join(format!("pimax-auto-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let sock = d.join("a.sock");
        assert_eq!(request(&sock, &AutoCmd::Status).unwrap_err(), "not running");
        let l = std::os::unix::net::UnixListener::bind(&sock).unwrap();
        let t = std::thread::spawn(move || {
            let (c, _) = l.accept().unwrap();
            let mut r = BufReader::new(&c);
            let mut q = String::new();
            r.read_line(&mut q).unwrap();
            assert_eq!(q.trim(), r#"{"cmd":"pause"}"#);
            (&c).write_all(b"{\"ok\":true,\"state\":\"plugged\",\"paused\":true}\n").unwrap();
        });
        let s = request(&sock, &AutoCmd::Pause).unwrap();
        assert!(s.paused && s.state == "plugged");
        t.join().unwrap();
        let _ = std::fs::remove_dir_all(d);
    }
}

/// A VR session that owns the headset (monado-service or vrserver) runs: its name. Pair and unpair are refused then.
pub fn vr_session_owner() -> Option<&'static str> {
    for e in std::fs::read_dir("/proc").ok()?.flatten() {
        if !e.file_name().to_string_lossy().bytes().all(|b| b.is_ascii_digit()) {
            continue;
        }
        match std::fs::read_to_string(e.path().join("comm")).as_deref().map(str::trim) {
            Ok("monado-service") => return Some("Monado"),
            Ok("vrserver") => return Some("SteamVR"),
            _ => {}
        }
    }
    None
}
