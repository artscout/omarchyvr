//! Control socket `$XDG_RUNTIME_DIR/pimax-autod.sock`: one JSON object per line each way.
//!
//! Requests: `{"cmd":"status"}`, `{"cmd":"events"}` (the connection then streams event lines), `{"cmd":"pause"}`,
//! `{"cmd":"resume"}`, `{"cmd":"wake"}`, `{"cmd":"sleep"}`,
//! `{"cmd":"pair","side":"left|right"}` (reply `{"ok":true,"pairing":"started",...}`, progress as `pair` events),
//! `{"cmd":"set-mode","mode":"desk|game","runtime":"monado|steamvr","preset":"none|cinema|work|assistant"}`.
//! Replies: `{"ok":true,...}` / `{"ok":false,"error":"..."}`. See docs/AUTO-MODE.md for the full schema.
//! A tiny flat-object JSON reader/writer is enough here (no serde).
use crate::config::{Mode, Runtime, PRESETS};
use crate::machine::{Command, Machine, Transition};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// `$PIMAX_AUTOD_SOCKET` or `$XDG_RUNTIME_DIR/pimax-autod.sock` (fallback `/tmp/pimax-autod-<uid>.sock`).
pub fn socket_path() -> PathBuf {
    if let Some(p) = std::env::var_os("PIMAX_AUTOD_SOCKET").filter(|v| !v.is_empty()) {
        return p.into();
    }
    match std::env::var_os("XDG_RUNTIME_DIR").filter(|v| !v.is_empty()) {
        Some(d) => PathBuf::from(d).join("pimax-autod.sock"),
        // SAFETY: getuid has no failure mode.
        None => PathBuf::from(format!("/tmp/pimax-autod-{}.sock", unsafe { libc::getuid() })),
    }
}

/// Quote a string as JSON.
pub fn q(s: &str) -> String {
    let mut o = String::with_capacity(s.len() + 2);
    o.push('"');
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '\r' => o.push_str("\\r"),
            '\t' => o.push_str("\\t"),
            c if (c as u32) < 0x20 => o.push_str(&format!("\\u{:04x}", c as u32)),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// Parse a flat JSON object whose values are strings, numbers, booleans or null (values kept as text, strings
/// unescaped). Nested objects/arrays are rejected.
pub fn parse_flat(s: &str) -> Result<BTreeMap<String, String>, String> {
    let b: Vec<char> = s.trim().chars().collect();
    let mut i = 0;
    let ws = |i: &mut usize| {
        while *i < b.len() && b[*i].is_whitespace() {
            *i += 1
        }
    };
    let string = |i: &mut usize| -> Result<String, String> {
        if b.get(*i) != Some(&'"') {
            return Err("expected a string".into());
        }
        *i += 1;
        let mut o = String::new();
        while let Some(&c) = b.get(*i) {
            *i += 1;
            match c {
                '"' => return Ok(o),
                '\\' => {
                    let e = *b.get(*i).ok_or("bad escape")?;
                    *i += 1;
                    match e {
                        'n' => o.push('\n'),
                        't' => o.push('\t'),
                        'r' => o.push('\r'),
                        'u' => {
                            let h: String = b.get(*i..*i + 4).ok_or("bad \\u")?.iter().collect();
                            *i += 4;
                            o.push(char::from_u32(u32::from_str_radix(&h, 16).map_err(|_| "bad \\u")?).unwrap_or('?'));
                        }
                        c => o.push(c),
                    }
                }
                c => o.push(c),
            }
        }
        Err("unterminated string".into())
    };
    let mut m = BTreeMap::new();
    ws(&mut i);
    if b.get(i) != Some(&'{') {
        return Err("expected {".into());
    }
    i += 1;
    ws(&mut i);
    if b.get(i) == Some(&'}') {
        return Ok(m);
    }
    loop {
        ws(&mut i);
        let k = string(&mut i)?;
        ws(&mut i);
        if b.get(i) != Some(&':') {
            return Err("expected :".into());
        }
        i += 1;
        ws(&mut i);
        let v = if b.get(i) == Some(&'"') {
            string(&mut i)?
        } else {
            let st = i;
            while i < b.len() && !matches!(b[i], ',' | '}') && !b[i].is_whitespace() {
                i += 1;
            }
            let raw: String = b[st..i].iter().collect();
            if raw.is_empty() || raw.starts_with('{') || raw.starts_with('[') {
                return Err(format!("unsupported value for {k}"));
            }
            raw
        };
        m.insert(k, v);
        ws(&mut i);
        match b.get(i) {
            Some(',') => i += 1,
            Some('}') => return Ok(m),
            _ => return Err("expected , or }".into()),
        }
    }
}

/// A pimaxd (reconciler) request.
#[derive(Debug, Clone, PartialEq)]
pub enum Supervise {
    /// Session on/off (and mode), who asked, per-component toggles for this session.
    Set { session: Option<bool>, mode: Option<pimax_components::model::Mode>, by: pimax_components::model::By, toggles: Vec<(String, bool)>, force: bool },
    Toggle { id: String, on: bool },
    Fix { id: String },
    SafeMode(bool),
    AckGpu,
}

/// A parsed request.
#[derive(Debug, Clone, PartialEq)]
pub enum Request {
    Status,
    /// The pimaxd snapshot (schema v1, docs/PIMAXD.md).
    State,
    /// Reply when the condition holds (`overall=ok`, `health:monado=ok`, `intent=off`) or after `timeout_s`.
    Wait { what: String, timeout_s: f64 },
    Supervise(Supervise),
    Events,
    /// Controller pairing (slot 2 left, 3 right): one StartPair, progress on the event stream.
    Pair(u8),
    Cmd(Command),
}

pub fn parse_request(line: &str) -> Result<Request, String> {
    let m = parse_flat(line)?;
    let cmd = m.get("cmd").ok_or("missing cmd")?;
    Ok(match cmd.as_str() {
        "status" => Request::Status,
        "state" => Request::State,
        "events" => Request::Events,
        "wait" => {
            let what = m.get("for").cloned().ok_or("wait needs for (overall=ok | health:ID=H | intent=on|off)")?;
            if !what.contains('=') {
                return Err(format!("bad wait condition {what}"));
            }
            let timeout_s = m.get("timeout_s").and_then(|v| v.parse::<f64>().ok()).unwrap_or(120.0).clamp(0.0, 3600.0);
            Request::Wait { what, timeout_s }
        }
        "set" => {
            use pimax_components::model::{By, Mode as PMode};
            let flag = |v: &str| -> Result<bool, String> {
                match v {
                    "on" | "true" | "1" => Ok(true),
                    "off" | "false" | "0" => Ok(false),
                    _ => Err(format!("bad on/off value {v}")),
                }
            };
            let session = m.get("session").map(|v| flag(v)).transpose()?;
            let mode = m.get("mode").map(|v| PMode::parse(v).ok_or(format!("bad mode {v}"))).transpose()?;
            let force = matches!(m.get("force").map(String::as_str), Some("true" | "1" | "on"));
            let by = m.get("by").map(|v| By::parse(v).ok_or(format!("bad by {v}"))).transpose()?.unwrap_or(By::User);
            let mut toggles = Vec::new();
            for (k, v) in &m {
                if matches!(k.as_str(), "cmd" | "session" | "mode" | "by" | "force") {
                    continue;
                }
                let key = k.strip_prefix("components.").unwrap_or(k);
                if pimax_components::spec::toggle_ids(key).is_empty() && pimax_components::spec::spec(key).is_none() {
                    return Err(format!("unknown component {key}"));
                }
                toggles.push((key.to_string(), flag(v)?));
            }
            if session.is_none() && toggles.is_empty() && mode.is_none() {
                return Err("set needs session, mode or component toggles".into());
            }
            Request::Supervise(Supervise::Set { session, mode, by, toggles, force })
        }
        "toggle" => {
            let id = m.get("id").cloned().ok_or("toggle needs id")?;
            let on = matches!(m.get("on").map(String::as_str), Some("true" | "on" | "1"));
            Request::Supervise(Supervise::Toggle { id, on })
        }
        "fix" => Request::Supervise(Supervise::Fix { id: m.get("id").cloned().ok_or("fix needs id")? }),
        "safe-mode" => Request::Supervise(Supervise::SafeMode(matches!(m.get("on").map(String::as_str), Some("true" | "on" | "1")))),
        "ack-gpu" => Request::Supervise(Supervise::AckGpu),
        "pause" => Request::Cmd(Command::Pause),
        "resume" => Request::Cmd(Command::Resume),
        "wake" => Request::Cmd(Command::Wake),
        "sleep" => Request::Cmd(Command::Sleep),
        "pair" => {
            let side = m.get("side").ok_or("pair needs side left|right")?;
            Request::Pair(crate::pairing::slot_of(side).ok_or(format!("bad side {side} (left|right)"))?)
        }
        "set-mode" => {
            let mode = m.get("mode").map(|v| Mode::parse(v).ok_or(format!("bad mode {v}"))).transpose()?;
            let runtime = m.get("runtime").map(|v| Runtime::parse(v).ok_or(format!("bad runtime {v}"))).transpose()?;
            let preset = m.get("preset").cloned();
            if let Some(p) = &preset {
                if !PRESETS.contains(&p.as_str()) {
                    return Err(format!("bad preset {p}"));
                }
            }
            if mode.is_none() && runtime.is_none() && preset.is_none() {
                return Err("set-mode needs mode, runtime or preset".into());
            }
            Request::Cmd(Command::SetMode { mode, runtime, preset })
        }
        other => return Err(format!("unknown cmd {other}")),
    })
}

/// Does the snapshot meet a wait condition (`overall=ok`, `intent=off`, `health:monado=ok`)?
pub fn wait_met(state: &str, what: &str) -> bool {
    let Ok(v) = pimax_components::json::parse(state) else { return false };
    let Some((k, want)) = what.split_once('=') else { return false };
    let wants: Vec<&str> = want.split('|').collect();
    let got = if k == "overall" {
        v.s("overall").map(str::to_string)
    } else if k == "intent" {
        v.get("intent").and_then(|i| i.s("session")).map(str::to_string)
    } else if let Some(id) = k.strip_prefix("health:") {
        v.get("components").and_then(|c| c.as_arr().iter().find(|x| x.s("id") == Some(id)).and_then(|x| x.s("health")).map(str::to_string))
    } else {
        None
    };
    got.is_some_and(|g| wants.contains(&g.as_str()))
}

/// Apply a pimaxd request; the reply line.
pub fn apply_supervise(sup: &mut crate::supervisor::Supervisor, c: &Supervise, safe_flag: &std::path::Path, pimax_bin: &std::path::Path, session_dir: &std::path::Path) -> String {
    // desk captions are a live flag (`pimax desk captions on|off`), not a process stop: an explicit user command,
    // it runs in safe mode too
    if let Supervise::Toggle { id, on } = c {
        if id == "captions" || id == "voice.captions" {
            let r = std::process::Command::new(pimax_bin)
                .args(["desk", "captions", if *on { "on" } else { "off" }])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            return match r {
                Ok(st) if st.success() => format!("{{\"ok\":true,\"captions\":{on}}}"),
                Ok(st) => error_line(&format!("pimax desk captions: {st}")),
                Err(e) => error_line(&format!("{}: {e}", pimax_bin.display())),
            };
        }
    }
    let r: Result<String, String> = (|| match c {
        Supervise::Set { session, mode, by, toggles, force } => {
            let reboot = sup.w.gate.effective() == pimax_components::model::GateLevel::NeedsReboot;
            if *session == Some(true) && reboot && !*force {
                return Err("the GPU needs a reboot (known NVIDIA issue): no VR start; add force=1 to start anyway (the display will likely fail)".into());
            }
            if let Some(m) = mode {
                sup.w.intent.mode = *m;
            }
            if let Some(on) = session {
                sup.set_session(*on, *by, *mode);
                // a launcher start (P6): its recorded argv says which components this session runs
                if *on && *by == pimax_components::model::By::Cli && toggles.is_empty() {
                    let argv = pimax_components::probe::session_facts_argv(session_dir);
                    sup.apply_start_argv(argv);
                }
                if *on && *force && reboot {
                    sup.w.intent.overrides.insert("__force".into(), true);
                }
            }
            let mut ids = Vec::new();
            for (k, on) in toggles {
                ids.extend(sup.toggle(k, *on)?);
            }
            Ok(format!("\"intent\":{},\"toggled\":[{}]", q(sup.w.intent.session_name()), ids.iter().map(|i| q(i)).collect::<Vec<_>>().join(",")))
        }
        Supervise::Toggle { id, on } => {
            let ids = sup.toggle(id, *on)?;
            Ok(format!("\"toggled\":[{}]", ids.iter().map(|i| q(i)).collect::<Vec<_>>().join(",")))
        }
        Supervise::Fix { id } => {
            if id == "pimaxd" || id == "safe-mode" {
                sup.w.safe_auto = None;
                return Ok("\"fixed\":\"pimaxd\"".into());
            }
            sup.fix(id)?;
            Ok(format!("\"fixed\":{}", q(id)))
        }
        Supervise::SafeMode(on) => {
            if *on {
                pimax_components::util::write_atomic(safe_flag, b"on\n").map_err(|e| format!("{}: {e}", safe_flag.display()))?;
            } else {
                let _ = std::fs::remove_file(safe_flag);
                sup.w.safe_auto = None;
            }
            sup.w.safe_mode = *on;
            Ok(format!("\"safe_mode\":{on}"))
        }
        Supervise::AckGpu => {
            if sup.w.gate.ack() {
                Ok("\"gpu_gate\":\"ok\"".into())
            } else if sup.w.gate.level == pimax_components::model::GateLevel::NeedsReboot {
                Err("needs-reboot cannot be acknowledged: reboot (a start with --force is allowed, with a warning)".into())
            } else {
                Ok("\"gpu_gate\":\"ok\"".into())
            }
        }
    })();
    match r {
        Ok(body) => format!("{{\"ok\":true,{body}}}"),
        Err(e) => error_line(&e),
    }
}

pub fn error_line(e: &str) -> String {
    format!("{{\"ok\":false,\"error\":{}}}", q(e))
}

/// The status object.
pub fn status_line(m: &Machine, uptime_s: f64) -> String {
    let s = m.signals();
    let opt_b = |v: Option<bool>| v.map_or("null".to_string(), |b| b.to_string());
    let busy = match &m.busy {
        None => "null".to_string(),
        Some(a) => q(action_name(a)),
    };
    format!(
        concat!(
            "{{\"ok\":true,\"state\":{},\"paused\":{},\"dry_run\":{},\"owned\":{},\"busy\":{},\"uptime_s\":{:.0},",
            "\"signals\":{{\"plugged\":{},\"motion\":{},\"visor\":{},\"session\":{},\"game\":{},\"input\":{},\"worn\":{}}},",
            "\"config\":{{\"mode\":{},\"game_runtime\":{},\"preset\":{},\"auto_start\":{},\"sleep_games\":{},",
            "\"dim_after_s\":{},\"sleep_after_s\":{}}}}}"
        ),
        q(&m.state.name()),
        m.paused,
        m.cfg.dry_run,
        m.owned,
        busy,
        uptime_s,
        s.plugged,
        q(s.motion.name()),
        opt_b(s.visor),
        s.session.map_or("null".to_string(), |k| q(k.name())),
        s.game,
        s.input,
        s.worn(),
        q(m.cfg.mode.name()),
        q(m.cfg.game_runtime.name()),
        q(&m.cfg.preset),
        m.cfg.auto_start,
        m.cfg.sleep_games,
        m.cfg.dim_after_s,
        m.cfg.sleep_after_s,
    )
}

pub fn action_name(a: &crate::machine::Action) -> &'static str {
    use crate::machine::Action::*;
    match a {
        Start { .. } => "start",
        Stop { .. } => "stop",
        Switch { .. } => "switch",
        Notify(_) => "notify",
    }
}

pub fn transition_line(t: &Transition) -> String {
    format!(
        "{{\"event\":\"transition\",\"t\":{:.1},\"from\":{},\"to\":{},\"why\":{}}}",
        t.t,
        q(&t.from.name()),
        q(&t.to.name()),
        q(&t.why)
    )
}

pub fn action_line(t: f64, a: &crate::machine::Action, dry_run: bool) -> String {
    format!(
        "{{\"event\":\"action\",\"t\":{t:.1},\"action\":{},\"detail\":{},\"dry_run\":{dry_run}}}",
        q(action_name(a)),
        q(&format!("{a:?}"))
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn flat_json_round_trip() {
        let m = parse_flat(r#" {"cmd" : "set-mode", "mode":"game","n":3, "b":true, "s":"a\"b\\cA"} "#).unwrap();
        assert_eq!(m["cmd"], "set-mode");
        assert_eq!(m["n"], "3");
        assert_eq!(m["b"], "true");
        assert_eq!(m["s"], "a\"b\\cA");
        assert_eq!(parse_flat(&format!("{{\"x\":{}}}", q("l1\nl2\t\"\u{1}"))).unwrap()["x"], "l1\nl2\t\"\u{1}");
        assert!(parse_flat("{}").unwrap().is_empty());
        for bad in ["", "[]", "{\"a\":{}}", "{\"a\":[1]}", "{\"a\" 1}", "{\"a\":\"x", "{a:1}"] {
            assert!(parse_flat(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn requests() {
        assert_eq!(parse_request(r#"{"cmd":"status"}"#).unwrap(), Request::Status);
        assert_eq!(parse_request(r#"{"cmd":"pause"}"#).unwrap(), Request::Cmd(Command::Pause));
        assert_eq!(
            parse_request(r#"{"cmd":"set-mode","runtime":"steamvr"}"#).unwrap(),
            Request::Cmd(Command::SetMode { mode: None, runtime: Some(Runtime::SteamVr), preset: None })
        );
        assert_eq!(parse_request(r#"{"cmd":"pair","side":"left"}"#).unwrap(), Request::Pair(2));
        assert_eq!(parse_request(r#"{"cmd":"pair","side":"right"}"#).unwrap(), Request::Pair(3));
        assert_eq!(parse_request(r#"{"cmd":"state"}"#).unwrap(), Request::State);
        assert_eq!(parse_request(r#"{"cmd":"wait","for":"overall=ok","timeout_s":5}"#).unwrap(), Request::Wait { what: "overall=ok".into(), timeout_s: 5.0 });
        assert_eq!(
            parse_request(r#"{"cmd":"set","session":"on","mode":"desk","voice":true,"by":"gui"}"#).unwrap(),
            Request::Supervise(Supervise::Set {
                session: Some(true),
                mode: Some(pimax_components::model::Mode::Desk),
                by: pimax_components::model::By::User,
                toggles: vec![("voice".into(), true)],
                force: false
            })
        );
        assert_eq!(parse_request(r#"{"cmd":"toggle","id":"perf","on":false}"#).unwrap(), Request::Supervise(Supervise::Toggle { id: "perf".into(), on: false }));
        assert_eq!(parse_request(r#"{"cmd":"safe-mode","on":true}"#).unwrap(), Request::Supervise(Supervise::SafeMode(true)));
        assert_eq!(parse_request(r#"{"cmd":"ack-gpu"}"#).unwrap(), Request::Supervise(Supervise::AckGpu));
        assert_eq!(parse_request(r#"{"cmd":"fix","id":"wayvr"}"#).unwrap(), Request::Supervise(Supervise::Fix { id: "wayvr".into() }));
        for bad in [r#"{"cmd":"set"}"#, r#"{"cmd":"set","bogus":true}"#, r#"{"cmd":"set","session":"maybe"}"#, r#"{"cmd":"wait","for":"x"}"#, r#"{"cmd":"fix"}"#] {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
        for bad in [r#"{"cmd":"pair"}"#, r#"{"cmd":"pair","side":"both"}"#, r#"{"cmd":"set-mode"}"#, r#"{"cmd":"set-mode","mode":"x"}"#, r#"{"cmd":"set-mode","preset":"x"}"#, r#"{"cmd":"nuke"}"#, "{}"] {
            assert!(parse_request(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn status_is_valid_flat_enough_json() {
        let m = Machine::new(crate::config::Config::defaults(Path::new("/h")));
        let s = status_line(&m, 12.0);
        assert!(s.starts_with("{\"ok\":true,\"state\":\"unplugged\""), "{s}");
        assert!(s.contains("\"dry_run\":true") && s.contains("\"motion\":\"unknown\"") && s.contains("\"visor\":null"));
        assert_eq!(s.matches('{').count(), s.matches('}').count());
    }
}
