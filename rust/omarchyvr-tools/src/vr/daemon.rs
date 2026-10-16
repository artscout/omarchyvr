//! The launcher as a omarchyvrd client (resilience plan P6; protocol in docs/PIMAXD.md).
//!
//! * `desk|vr start`: when omarchyvrd answers on its socket and executes (not observe-only, not in safe mode), the
//!   launcher records the start arguments (`start-argv.txt`, as always), sends
//!   `{"cmd":"set","session":"on","mode":...,"by":"cli"}` and follows the snapshot until the session is up (overall
//!   ok, or every started part ok with something optional degraded), fails (needs-reboot, needs-you, a core part
//!   failed) or 120 s pass. omarchyvrd itself starts the session with `pimax <kind> start <recorded args>` under
//!   `PIMAX_WATCHDOG=1`, which always takes the direct path. No session watchdog is spawned: omarchyvrd owns restarts.
//! * Direct path (byte-identical to before P6): omarchyvrd absent, `--no-daemon` / `PIMAX_NO_DAEMON=1`, `--dry-run`,
//!   `--wait` (the foreground desktop entry), `--force` (omarchyvrd refuses past its GPU gate), or inside omarchyvrd /
//!   the watchdog (`PIMAX_WATCHDOG=1`). With an observe-only omarchyvrd the direct start or stop also tells it the
//!   intent (`by=cli`), so the session counts as the user's, not as "adopted".
//! * `stop` always runs directly (an explicit user stop is the only path that may escalate to SIGKILL), after telling
//!   omarchyvrd `session=off`, so it does not restore what the user stopped.
//! * `status` adds one omarchyvrd line when it answers.
use omarchyvr_components::json::{self, Value};
use std::path::PathBuf;
use std::time::Duration;

/// One request, one reply line.
pub trait Link {
    fn request(&self, line: &str, timeout: Duration) -> Result<String, String>;
}

/// The real socket (`$PIMAX_AUTOD_SOCKET` or `$XDG_RUNTIME_DIR/omarchyvr-autod.sock`).
pub struct SockLink(pub PathBuf);

impl SockLink {
    pub fn from_env() -> Self {
        SockLink(omarchyvr_autod::ipc::socket_path())
    }
}

impl Link for SockLink {
    fn request(&self, line: &str, timeout: Duration) -> Result<String, String> {
        if !self.0.exists() {
            return Err("no socket".into());
        }
        omarchyvr_autod::client::request_timeout(&self.0, line, timeout)
    }
}

/// What omarchyvrd can do for this command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Daemon {
    /// Not running (or not answering, or not a omarchyvrd).
    Absent,
    /// Running, but it executes nothing (observe-only or safe mode): the launcher acts, omarchyvrd watches.
    Observing(String),
    /// Running and executing: it starts and restarts the session.
    Executing,
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(2);

fn snapshot(link: &dyn Link) -> Option<Value> {
    let r = link.request(r#"{"cmd":"state"}"#, PROBE_TIMEOUT).ok()?;
    let v = json::parse(&r).ok()?;
    // a omarchyvrd snapshot (schema v1), not an old omarchyvr-autod without the reconciler
    (v.get("v").and_then(Value::as_f64) == Some(1.0) && v.s("overall").is_some()).then_some(v)
}

/// Ask the daemon what it is.
pub fn probe(link: &dyn Link) -> Daemon {
    let Some(v) = snapshot(link) else { return Daemon::Absent };
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    if flag("safe_mode") {
        let why = v.s("safe_reason").map(|r| format!(" ({r})")).unwrap_or_default();
        Daemon::Observing(format!("omarchyvrd is in safe mode{why}"))
    } else if flag("observe_only") {
        Daemon::Observing("omarchyvrd is observe-only ([restart] enabled = false)".into())
    } else {
        Daemon::Executing
    }
}

/// omarchyvrd is not even asked: `--no-daemon`, `PIMAX_NO_DAEMON=1`, a dry run (its output is the golden files'), or a
/// run by omarchyvrd itself or the watchdog (`PIMAX_WATCHDOG=1`).
pub fn bypass(no_daemon_flag: bool, dry_run: bool) -> bool {
    no_daemon_flag
        || dry_run
        || super::supervise::in_watchdog()
        || std::env::var("PIMAX_NO_DAEMON").is_ok_and(|v| !v.is_empty() && v != "0")
}

/// The socket and what omarchyvrd is ([`Daemon::Absent`] without asking when [`bypass`]).
pub fn contact(no_daemon_flag: bool, dry_run: bool) -> (SockLink, Daemon) {
    let link = SockLink::from_env();
    let d = if bypass(no_daemon_flag, dry_run) { Daemon::Absent } else { probe(&link) };
    (link, d)
}

/// `start` goes through omarchyvrd: it executes, and the start is not `--wait` (foreground) or `--force` (omarchyvrd will
/// not go past its GPU gate).
pub fn start_through(d: &Daemon, wait: bool, force: bool) -> bool {
    *d == Daemon::Executing && !wait && !force
}

/// After a direct start or before a direct stop: tell a running omarchyvrd the intent (best effort, one line of output).
pub fn tell_direct(link: &dyn Link, d: &Daemon, on: bool, kind: &str, say: &mut dyn FnMut(&str)) {
    if *d == Daemon::Absent {
        return;
    }
    let what = if on { "on" } else { "off" };
    match tell(link, on, kind) {
        Ok(_) => say(&format!("told omarchyvrd: session {what}")),
        Err(e) => say(&format!("WARNING: could not tell omarchyvrd the session is {what}: {e}")),
    }
}

/// The recorded start arguments without `--no-daemon` (omarchyvrd replays them under PIMAX_WATCHDOG=1 anyway).
pub fn argv_for_record(argv: &[String]) -> Vec<String> {
    argv.iter().filter(|a| *a != "--no-daemon").cloned().collect()
}

/// `set session=on|off mode=KIND by=cli`; Ok with the reply.
pub fn tell(link: &dyn Link, on: bool, kind: &str) -> Result<String, String> {
    let line = format!(
        r#"{{"cmd":"set","session":"{}","mode":"{kind}","by":"cli"}}"#,
        if on { "on" } else { "off" }
    );
    let r = link.request(&line, PROBE_TIMEOUT)?;
    match json::parse(&r) {
        Ok(v) if v.get("ok").and_then(Value::as_bool) == Some(true) => Ok(r),
        Ok(v) => Err(v.s("error").unwrap_or("refused").to_string()),
        Err(e) => Err(format!("bad reply: {e}")),
    }
}

/// How long `start` follows omarchyvrd.
pub const START_TIMEOUT: Duration = Duration::from_secs(120);
/// A degraded overall state with the core parts ok counts as started after this long.
pub const DEGRADED_SETTLE: Duration = Duration::from_secs(10);
const POLL: Duration = Duration::from_secs(1);

/// Components whose state is shown while following a start.
const SHOWN: &[&str] = &["usb.hid", "usb.cam", "display.dp1", "gpu.gate", "broker", "monado", "passthrough", "wayvr", "ctrltrack", "perf", "voice.listen", "voicehud"];
/// They must be ok (when wanted) for the session to count as started.
const CORE: &[&str] = &["monado", "wayvr", "passthrough", "broker"];

fn comp<'a>(v: &'a Value, id: &str) -> Option<&'a Value> {
    v.get("components")?.as_arr().iter().find(|c| c.s("id") == Some(id))
}

/// `start` through omarchyvrd. `say` prints progress; Err is the failure message.
pub fn start_via(
    link: &dyn Link,
    kind: &str,
    clock: &dyn super::host::Clock,
    say: &mut dyn FnMut(&str),
) -> Result<(), String> {
    tell(link, true, kind).map_err(|e| format!("omarchyvrd refused the start: {e} (omarchyvr {kind} start --no-daemon starts it directly)"))?;
    say(&format!("omarchyvrd starts the session (follow: omarchyvr-health --watch; direct start: omarchyvr {kind} start --no-daemon)"));
    let t0 = clock.now();
    let mut shown: Vec<(String, String)> = Vec::new();
    let mut degraded_since: Option<Duration> = None;
    loop {
        clock.sleep(POLL);
        let now = clock.now().saturating_sub(t0);
        let Some(v) = snapshot(link) else {
            return Err("omarchyvrd stopped answering during the start; see omarchyvr-health, or start with --no-daemon".into());
        };
        for id in SHOWN {
            let Some(c) = comp(&v, id) else { continue };
            if c.get("wanted").and_then(Value::as_bool) != Some(true) {
                continue;
            }
            let h = c.s("health").unwrap_or("unknown").to_string();
            let line = match c.s("why") {
                Some(w) if !w.is_empty() => format!("{h} ({w})"),
                _ => h.clone(),
            };
            match shown.iter_mut().find(|(i, _)| i == id) {
                Some((_, old)) if *old == line => {}
                Some((_, old)) => {
                    *old = line.clone();
                    say(&format!("{id}: {line}"));
                }
                None => {
                    shown.push((id.to_string(), line.clone()));
                    say(&format!("{id}: {line}"));
                }
            }
        }
        let overall = v.s("overall").unwrap_or("unknown");
        if v.get("intent").and_then(|i| i.s("session")) == Some("off") {
            return Err("the session intent went off during the start (a stop, or auto mode)".into());
        }
        match overall {
            "ok" => return Ok(()),
            "needs-reboot" => {
                let ev = v.get("gpu_evidence").map(|e| e.as_arr().iter().filter_map(Value::as_str).collect::<Vec<_>>().join("; ")).unwrap_or_default();
                return Err(format!("omarchyvrd will not start: the GPU needs a reboot ({ev}). omarchyvr {kind} start --force goes past it at your own risk"));
            }
            "needs-you" => {
                let why = v.get("next_action").and_then(|a| a.s("why")).unwrap_or("see omarchyvr-health");
                return Err(format!("omarchyvrd needs you: {why}"));
            }
            _ => {}
        }
        let core_ok = CORE.iter().all(|id| match comp(&v, id) {
            Some(c) if c.get("wanted").and_then(Value::as_bool) == Some(true) => c.s("health") == Some("ok"),
            _ => true,
        });
        let failed: Vec<&str> = CORE
            .iter()
            .copied()
            .filter(|id| comp(&v, id).is_some_and(|c| c.get("wanted").and_then(Value::as_bool) == Some(true) && c.s("health") == Some("failed")))
            .collect();
        if !failed.is_empty() {
            return Err(format!("omarchyvrd gave up on {} (breaker open): see omarchyvr-health; Fix there, or omarchyvr {kind} start --no-daemon", failed.join(", ")));
        }
        if overall == "degraded" && core_ok {
            let since = *degraded_since.get_or_insert(now);
            if now - since >= DEGRADED_SETTLE {
                say("the session is up; something optional is degraded (omarchyvr-health shows what)");
                return Ok(());
            }
        } else {
            degraded_since = None;
        }
        if now >= START_TIMEOUT {
            return Err(format!(
                "omarchyvrd did not bring the session up within {:.0} s (overall {overall}); see omarchyvr-health",
                START_TIMEOUT.as_secs_f64()
            ));
        }
    }
}

/// One line for `status`, when omarchyvrd answers.
pub fn status_line(link: &dyn Link) -> Option<String> {
    let v = snapshot(link)?;
    let flag = |k: &str| v.get(k).and_then(Value::as_bool).unwrap_or(false);
    let mode = if flag("safe_mode") {
        ", safe mode"
    } else if flag("observe_only") {
        ", observe only"
    } else {
        ""
    };
    Some(format!(
        "omarchyvrd: overall {}, GPU gate {}, intent {} (by {}){mode}",
        v.s("overall").unwrap_or("?"),
        v.s("gpu_gate").unwrap_or("?"),
        v.get("intent").and_then(|i| i.s("session")).unwrap_or("?"),
        v.get("intent").and_then(|i| i.s("by")).unwrap_or("?"),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Clk(Cell<Duration>);
    impl super::super::host::Clock for Clk {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// Replies from a script: the state reply for poll n is `states[min(n, len-1)]`.
    struct Fake {
        states: Vec<String>,
        polls: Cell<usize>,
        sent: RefCell<Vec<String>>,
        set_reply: String,
    }
    impl Fake {
        fn new(states: &[String]) -> Self {
            Fake { states: states.to_vec(), polls: Cell::new(0), sent: RefCell::new(vec![]), set_reply: r#"{"ok":true,"intent":"on","toggled":[]}"#.into() }
        }
    }
    impl Link for Fake {
        fn request(&self, line: &str, _t: Duration) -> Result<String, String> {
            self.sent.borrow_mut().push(line.to_string());
            if line.contains(r#""cmd":"set""#) {
                return Ok(self.set_reply.clone());
            }
            if self.states.is_empty() {
                return Err("no socket".into());
            }
            let n = self.polls.get();
            self.polls.set(n + 1);
            Ok(self.states[n.min(self.states.len() - 1)].clone())
        }
    }

    fn st(overall: &str, observe: bool, safe: bool, comps: &[(&str, bool, &str)]) -> String {
        let c: Vec<String> = comps.iter().map(|(id, w, h)| format!(r#"{{"id":"{id}","wanted":{w},"health":"{h}","why":null}}"#)).collect();
        format!(
            r#"{{"v":1,"t":1.0,"overall":"{overall}","safe_mode":{safe},"safe_reason":null,"observe_only":{observe},"gpu_gate":"ok","gpu_evidence":[],"intent":{{"session":"on","by":"cli","mode":"desk"}},"components":[{}],"next_action":null}}"#,
            c.join(",")
        )
    }

    #[test]
    fn probe_tells_absent_observing_and_executing_apart() {
        assert_eq!(probe(&Fake::new(&[])), Daemon::Absent);
        // an old omarchyvr-autod status reply is not a omarchyvrd
        assert_eq!(probe(&Fake::new(&[r#"{"state":"idle"}"#.into()])), Daemon::Absent);
        assert!(matches!(probe(&Fake::new(&[st("ok", true, false, &[])])), Daemon::Observing(w) if w.contains("observe-only")));
        assert!(matches!(probe(&Fake::new(&[st("ok", false, true, &[])])), Daemon::Observing(w) if w.contains("safe mode")));
        assert_eq!(probe(&Fake::new(&[st("ok", false, false, &[])])), Daemon::Executing);
    }

    #[test]
    fn start_via_sets_the_intent_and_follows_until_ok() {
        let states = [
            st("down", false, false, &[("monado", true, "starting"), ("wayvr", true, "blocked")]),
            st("degraded", false, false, &[("monado", true, "ok"), ("wayvr", true, "starting")]),
            st("ok", false, false, &[("monado", true, "ok"), ("wayvr", true, "ok")]),
        ];
        let f = Fake::new(&states);
        let c = Clk(Cell::new(Duration::ZERO));
        let mut said = vec![];
        start_via(&f, "desk", &c, &mut |m| said.push(m.to_string())).unwrap();
        assert_eq!(f.sent.borrow()[0], r#"{"cmd":"set","session":"on","mode":"desk","by":"cli"}"#);
        assert!(said.contains(&"monado: starting".to_string()) && said.contains(&"wayvr: ok".to_string()), "{said:?}");
    }

    #[test]
    fn start_via_accepts_a_settled_degraded_session_and_reports_failures() {
        // core ok, voice degraded: up after the settle time
        let f = Fake::new(&[st("degraded", false, false, &[("monado", true, "ok"), ("wayvr", true, "ok"), ("voice.listen", true, "down")])]);
        let c = Clk(Cell::new(Duration::ZERO));
        start_via(&f, "desk", &c, &mut |_| {}).unwrap();
        assert!(c.0.get() >= DEGRADED_SETTLE && c.0.get() < DEGRADED_SETTLE + Duration::from_secs(3));
        // the gate
        let f = Fake::new(&[st("needs-reboot", false, false, &[])]);
        let e = start_via(&f, "desk", &c, &mut |_| {}).unwrap_err();
        assert!(e.contains("reboot") && e.contains("--force"), "{e}");
        // breaker open on Monado
        let f = Fake::new(&[st("down", false, false, &[("monado", true, "failed")])]);
        assert!(start_via(&f, "vr", &c, &mut |_| {}).unwrap_err().contains("gave up on monado"));
        // never comes up
        let c = Clk(Cell::new(Duration::ZERO));
        let f = Fake::new(&[st("down", false, false, &[("monado", true, "starting")])]);
        assert!(start_via(&f, "desk", &c, &mut |_| {}).unwrap_err().contains("within 120 s"));
        assert!(c.0.get() >= START_TIMEOUT);
        // refused set
        let mut f = Fake::new(&[st("ok", false, false, &[])]);
        f.set_reply = r#"{"ok":false,"error":"busy"}"#.into();
        assert!(start_via(&f, "desk", &c, &mut |_| {}).unwrap_err().contains("busy"));
        // the daemon goes away
        let f = Fake::new(&[]);
        let f2 = Fake { set_reply: f.set_reply.clone(), ..f };
        assert!(start_via(&f2, "desk", &c, &mut |_| {}).unwrap_err().contains("stopped answering"));
    }

    #[test]
    fn status_line_names_the_mode() {
        let l = status_line(&Fake::new(&[st("ok", true, false, &[])])).unwrap();
        assert_eq!(l, "omarchyvrd: overall ok, GPU gate ok, intent on (by cli), observe only");
        assert!(status_line(&Fake::new(&[])).is_none());
    }
}
