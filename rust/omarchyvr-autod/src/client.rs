//! Client side of the control socket (for `omarchyvr auto` and the GUI; the GUI can also speak the protocol itself).
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::Path;
use std::time::Duration;

/// Send one request line, return the reply line.
pub fn request(sock: &Path, line: &str) -> Result<String, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("omarchyvr-autod not running ({}): {e}", sock.display()))?;
    s.set_read_timeout(Some(Duration::from_secs(5))).ok();
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut r = String::new();
    BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
    Ok(r.trim_end().to_string())
}

/// Subscribe to events; calls `f` per line until it returns false or the daemon goes away.
pub fn events(sock: &Path, mut f: impl FnMut(&str) -> bool) -> Result<(), String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("omarchyvr-autod not running ({}): {e}", sock.display()))?;
    writeln!(s, "{{\"cmd\":\"events\"}}").map_err(|e| e.to_string())?;
    for l in BufReader::new(s).lines() {
        let l = l.map_err(|e| e.to_string())?;
        if !f(&l) {
            break;
        }
    }
    Ok(())
}

/// `pair left|right` for the CLIs: through the daemon when it runs (progress from its event stream), otherwise one
/// standalone attempt in this process. Prints a line per event through `out`; Ok when paired.
pub fn pair_cli(sock: &Path, side: &str, out: &mut dyn FnMut(&str)) -> Result<(), String> {
    let slot = crate::pairing::slot_of(side).ok_or("pair left|right")?;
    out(&format!("{} ({side})", crate::pairing::INSTRUCTION));
    let finish = |state: &str, detail: &str| match state {
        "paired" => Ok(()),
        _ => Err(format!("{state}: {detail}")),
    };
    let Ok(ev) = UnixStream::connect(sock) else {
        out("omarchyvr-autod is not running: pairing from this process");
        let o = crate::pairing::run_real(slot, &mut |l| out(l));
        return match o {
            crate::pairing::Outcome::Paired => Ok(()),
            crate::pairing::Outcome::Timeout => Err("timeout: no pairing within 35 s".into()),
            crate::pairing::Outcome::Failed(e) => Err(format!("failed: {e}")),
        };
    };
    // subscribe first so no event is missed, then ask once
    let mut ev = ev;
    ev.set_read_timeout(Some(Duration::from_secs(60))).ok();
    writeln!(ev, "{{\"cmd\":\"events\"}}").map_err(|e| e.to_string())?;
    let mut lines = BufReader::new(ev).lines();
    lines.next(); // the status object
    let req = request_line(&["pair".into(), side.into()])?;
    let ack = request(sock, &req)?;
    if ack.contains("\"ok\":false") {
        return Err(ack);
    }
    for l in lines {
        let l = l.map_err(|e| format!("event stream: {e}"))?;
        let Ok(m) = crate::ipc::parse_flat(&l) else { continue };
        if m.get("event").map(String::as_str) != Some("pair") || m.get("side").map(String::as_str) != Some(side) {
            continue;
        }
        out(&l);
        let st = m.get("state").map(String::as_str).unwrap_or("");
        if matches!(st, "paired" | "failed" | "timeout") {
            return finish(st, m.get("detail").map(String::as_str).unwrap_or(""));
        }
    }
    Err("daemon closed the event stream before the result".into())
}

/// The request line for a CLI command (`status`, `pause`, `resume`, `wake`, `sleep`, `events`,
/// `set-mode [desk|game] [--runtime monado|steamvr] [--preset NAME]`).
pub fn request_line(args: &[String]) -> Result<String, String> {
    let q = crate::ipc::q;
    let cmd = args.first().map(String::as_str).unwrap_or("status");
    match cmd {
        "pair" => {
            let side = args.get(1).map(String::as_str).unwrap_or("");
            if args.len() != 2 || crate::pairing::slot_of(side).is_none() {
                return Err("pair left|right".into());
            }
            Ok(format!("{{\"cmd\":\"pair\",\"side\":{}}}", q(side)))
        }
        "status" | "pause" | "resume" | "wake" | "sleep" | "events" => Ok(format!("{{\"cmd\":{}}}", q(cmd))),
        "set-mode" => {
            let mut parts = vec!["\"cmd\":\"set-mode\"".to_string()];
            let mut i = 1;
            while i < args.len() {
                match args[i].as_str() {
                    "--runtime" | "--preset" => {
                        let v = args.get(i + 1).ok_or(format!("{} needs a value", args[i]))?;
                        parts.push(format!("{}:{}", q(&args[i][2..]), q(v)));
                        i += 2;
                    }
                    m @ ("desk" | "game") => {
                        parts.push(format!("\"mode\":{}", q(m)));
                        i += 1;
                    }
                    o => return Err(format!("set-mode: unexpected {o}")),
                }
            }
            if parts.len() == 1 {
                return Err("set-mode needs desk|game, --runtime or --preset".into());
            }
            let line = format!("{{{}}}", parts.join(","));
            crate::ipc::parse_request(&line)?;
            Ok(line)
        }
        o => Err(format!("unknown auto command {o} (status, events, pause, resume, wake, sleep, set-mode, pair)")),
    }
}

/// Send one request with a reply timeout.
pub fn request_timeout(sock: &Path, line: &str, t: Duration) -> Result<String, String> {
    let mut s = UnixStream::connect(sock).map_err(|e| format!("omarchyvrd not running ({}): {e}", sock.display()))?;
    s.set_read_timeout(Some(t)).ok();
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut r = String::new();
    BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
    Ok(r.trim_end().to_string())
}

/// The request line of a omarchyvrd CLI command (`state`, `set ...`, `toggle K on`, `fix ID`, `safe-mode on`,
/// `ack-gpu`, `wait COND [--timeout S]`).
pub fn supervise_line(cmd: &str, a: &mut Vec<String>) -> Result<String, String> {
    let q = crate::ipc::q;
    let line = match cmd {
        "state" | "ack-gpu" => format!("{{\"cmd\":{}}}", q(cmd)),
        "set" => {
            let mut parts = vec!["\"cmd\":\"set\"".to_string()];
            for kv in a.iter() {
                let (k, v) = kv.split_once('=').ok_or(format!("set: want KEY=VALUE, not {kv}"))?;
                parts.push(format!("{}:{}", q(k), q(v)));
            }
            format!("{{{}}}", parts.join(","))
        }
        "toggle" => {
            let (k, v) = (a.first().ok_or("toggle KEY on|off")?, a.get(1).ok_or("toggle KEY on|off")?);
            format!("{{\"cmd\":\"toggle\",\"id\":{},\"on\":{}}}", q(k), v == "on")
        }
        "fix" => format!("{{\"cmd\":\"fix\",\"id\":{}}}", q(a.first().ok_or("fix ID")?)),
        "safe-mode" => {
            let v = a.first().ok_or("safe-mode on|off")?;
            format!("{{\"cmd\":\"safe-mode\",\"on\":{}}}", v == "on")
        }
        "wait" => {
            let mut timeout = 120.0;
            if let Some(i) = a.iter().position(|x| x == "--timeout") {
                timeout = a.get(i + 1).and_then(|v| v.parse().ok()).ok_or("--timeout S")?;
                a.drain(i..i + 2);
            }
            format!("{{\"cmd\":\"wait\",\"for\":{},\"timeout_s\":{timeout}}}", q(a.first().ok_or("wait CONDITION")?))
        }
        o => return Err(format!("unknown command {o}")),
    };
    crate::ipc::parse_request(&line)?;
    Ok(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn cli_lines() {
        assert_eq!(request_line(&[]).unwrap(), r#"{"cmd":"status"}"#);
        assert_eq!(request_line(&s(&["pause"])).unwrap(), r#"{"cmd":"pause"}"#);
        assert_eq!(
            request_line(&s(&["set-mode", "game", "--runtime", "steamvr"])).unwrap(),
            r#"{"cmd":"set-mode","mode":"game","runtime":"steamvr"}"#
        );
        assert!(request_line(&s(&["set-mode", "--preset", "disco"])).is_err());
        assert!(request_line(&s(&["set-mode"])).is_err());
        assert!(request_line(&s(&["nuke"])).is_err());
        assert_eq!(request_line(&s(&["pair", "right"])).unwrap(), r#"{"cmd":"pair","side":"right"}"#);
        assert!(request_line(&s(&["pair"])).is_err() && request_line(&s(&["pair", "up"])).is_err());
    }
}
