//! Learning from clicks and the quick eye check: the engine behind Tracking > Eyes (`omarchyvr eye learn status|forget`,
//! `omarchyvr eye check`). Everything goes over `wayvrctl` (`gaze-learn status --json`, `gaze-learn forget`,
//! `gaze-check --json`). Learning never makes input: it only moves the gaze point WayVR shows (docs/EYE.md).
//! Method ported by WayVR from Frametop (MIT).
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use crate::args::Args;
use crate::{CliError, CliResult};

/// What the headset shows (toast) when the quick check starts.
pub const LOOK_TEXT: &str = "Look at the dot in front of you";

/// `wayvrctl gaze-learn status --json`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LearnStatus {
    /// Learning is on in the running WayVR.
    pub enabled: bool,
    /// Lessons kept (clicks learned from).
    pub lessons: u64,
    /// Lessons taken since start.
    pub taken: u64,
    /// Lessons refused since start.
    pub refused: u64,
    /// Error of the last lesson, degrees.
    pub last_error_deg: Option<f64>,
    /// The correction in use: yaw, pitch (degrees).
    pub offset_deg: Option<(f64, f64)>,
    /// Calibration fingerprint the lessons belong to.
    pub key: String,
}

/// Result of `wayvrctl gaze-check --json`.
#[derive(Debug, Clone, PartialEq)]
pub enum Check {
    /// Measured: error without and with the learned correction (degrees).
    Done { before: f64, after: f64 },
    /// Could not measure: WayVR's reason.
    Failed(String),
}

/// Parse a `gaze-learn status --json` answer.
pub fn parse_status(json: &str) -> Result<LearnStatus, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("WayVR answered something else ({e})"))?;
    let n = |k: &str| v.get(k).and_then(|x| x.as_u64()).unwrap_or(0);
    let off = v.get("offset_deg").and_then(|o| o.as_array()).and_then(|a| Some((a.first()?.as_f64()?, a.get(1)?.as_f64()?)));
    Ok(LearnStatus {
        enabled: v.get("enabled").and_then(|x| x.as_bool()).unwrap_or(false),
        lessons: n("lessons"),
        taken: n("taken"),
        refused: n("refused"),
        last_error_deg: v.get("last_error_deg").and_then(|x| x.as_f64()),
        offset_deg: off,
        key: v.get("key").and_then(|x| x.as_str()).unwrap_or("").to_string(),
    })
}

/// The status line in plain words.
pub fn status_text(s: &LearnStatus) -> String {
    match (s.enabled, s.lessons) {
        (false, _) => "Learning is off".into(),
        (true, 0) => "Nothing learned yet".into(),
        (true, 1) => "Learned from 1 click".into(),
        (true, n) => format!("Learned from {n} clicks"),
    }
}

/// Parse a `gaze-check --json` answer.
pub fn parse_check(json: &str) -> Result<Check, String> {
    let v: serde_json::Value = serde_json::from_str(json.trim()).map_err(|e| format!("WayVR answered something else ({e})"))?;
    if v.get("ok").and_then(|x| x.as_bool()) == Some(true) {
        let f = |k: &str| v.get(k).and_then(|x| x.as_f64()).ok_or_else(|| format!("the answer has no {k}"));
        Ok(Check::Done { before: f("error_before_deg")?, after: f("error_after_deg")? })
    } else {
        Ok(Check::Failed(v.get("reason").and_then(|x| x.as_str()).unwrap_or("unknown").to_string()))
    }
}

/// The check result in plain words.
pub fn check_text(c: &Check) -> String {
    match c {
        Check::Done { before, after } => format!("Your eyes were {before:.1}\u{b0} off; now {after:.1}\u{b0}"),
        Check::Failed(r) => match r.as_str() {
            "no gaze" => "The eye tracker sees no gaze right now: is eye tracking on?".into(),
            "not steady" => "Your eyes were not steady: look at the dot without moving and try again".into(),
            "timed out" => "No steady look in time: try again".into(),
            "learning off" => "Turn on Learn from my clicks first".into(),
            other => format!("The check did not finish ({other})"),
        },
    }
}

/// Run `wayvrctl ARGS` with a time limit: (success, stdout, last stderr line).
pub fn run_ctl(ctl: &Path, args: &[&str], limit: Duration) -> Result<(bool, String, String), String> {
    let mut child = Command::new(ctl)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", ctl.display()))?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                let se = String::from_utf8_lossy(&out.stderr);
                return Ok((st.success(), String::from_utf8_lossy(&out.stdout).into_owned(), se.lines().last().unwrap_or("").to_string()));
            }
            Ok(None) if t0.elapsed() < limit => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err("WayVR did not answer in time".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn answer(r: (bool, String, String)) -> Result<String, String> {
    match r {
        (_, out, _) if !out.trim().is_empty() => Ok(out),
        (true, out, _) => Ok(out),
        (false, _, err) => Err(format!("wayvrctl: {}", if err.is_empty() { "failed (is Work running?)" } else { &err })),
    }
}

/// `gaze-learn status --json`.
pub fn status(ctl: &Path) -> Result<LearnStatus, String> {
    parse_status(&answer(run_ctl(ctl, &["gaze-learn", "status", "--json"], Duration::from_secs(3))?)?)
}

/// `gaze-learn forget`.
pub fn forget(ctl: &Path) -> Result<(), String> {
    answer(run_ctl(ctl, &["gaze-learn", "forget"], Duration::from_secs(3))?).map(|_| ())
}

/// The quick eye check: tells the person in the headset (toast), then `gaze-check --json` (up to 8 s; 12 s limit here).
/// Returns WayVR's raw JSON answer.
pub fn check_raw(ctl: &Path) -> Result<String, String> {
    super::work::toast("Quick eye check", LOOK_TEXT);
    answer(run_ctl(ctl, &["gaze-check", "--json"], Duration::from_secs(12))?)
}

/// The quick eye check, parsed.
pub fn check(ctl: &Path) -> Result<Check, String> {
    parse_check(&check_raw(ctl)?)
}

const USAGE: &str = "usage: omarchyvr eye learn status [--json] | omarchyvr eye learn forget | omarchyvr eye check [--json]
  Learning from clicks and the quick eye check of the running Work session (WayVR); the same as the Tracking page.";

/// `omarchyvr eye ...`.
pub fn run(a: Args) -> CliResult {
    let home = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/root".into()));
    let ctl = omarchyvr_config::paths::wayvrctl_bin(&home);
    let words: Vec<&str> = a.0.iter().map(String::as_str).filter(|w| *w != "--json").collect();
    let json = a.0.iter().any(|w| w == "--json");
    match words.as_slice() {
        ["learn", "status"] => {
            let s = status(&ctl).map_err(CliError::new)?;
            if json {
                println!("{}", serde_json::json!({"enabled": s.enabled, "lessons": s.lessons, "taken": s.taken, "refused": s.refused,
                    "last_error_deg": s.last_error_deg, "offset_deg": s.offset_deg.map(|(y, p)| [y, p]), "key": s.key}));
            } else {
                println!("{}", status_text(&s));
            }
            Ok(())
        }
        ["learn", "forget"] => {
            forget(&ctl).map_err(CliError::new)?;
            println!("Forgot what it learned");
            Ok(())
        }
        ["check"] => {
            let raw = check_raw(&ctl).map_err(CliError::new)?;
            let c = parse_check(&raw).map_err(CliError::new)?;
            if json {
                println!("{}", raw.trim());
            } else {
                println!("{}", check_text(&c));
            }
            match c {
                Check::Done { .. } => Ok(()),
                Check::Failed(_) => Err(CliError::new("")),
            }
        }
        _ => Err(CliError::usage(USAGE)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_parses_and_reads_in_plain_words() {
        let s = parse_status(r#"{"enabled":true,"lessons":12,"taken":14,"refused":2,"last_error_deg":1.25,"offset_deg":[0.4,-0.2],"key":"abc"}"#).unwrap();
        assert_eq!((s.lessons, s.taken, s.refused, s.offset_deg), (12, 14, 2, Some((0.4, -0.2))));
        assert_eq!(status_text(&s), "Learned from 12 clicks");
        assert_eq!(status_text(&LearnStatus { enabled: true, ..Default::default() }), "Nothing learned yet");
        assert_eq!(status_text(&LearnStatus::default()), "Learning is off");
        assert!(parse_status("nope").is_err());
    }

    #[test]
    fn check_parses_and_reads_in_plain_words() {
        let c = parse_check(r#"{"ok":true,"error_before_deg":2.14,"error_after_deg":0.42}"#).unwrap();
        assert_eq!(check_text(&c), "Your eyes were 2.1\u{b0} off; now 0.4\u{b0}");
        for (r, w) in [("no gaze", "no gaze"), ("not steady", "not steady"), ("timed out", "try again"), ("learning off", "Learn from my clicks")] {
            let c = parse_check(&format!(r#"{{"ok":false,"reason":"{r}"}}"#)).unwrap();
            assert!(check_text(&c).contains(w), "{r}");
        }
    }

    #[test]
    fn run_ctl_returns_output_and_times_out() {
        let ok = run_ctl(Path::new("/bin/echo"), &["hi"], Duration::from_secs(2)).unwrap();
        assert!(ok.0 && ok.1.trim() == "hi");
        assert!(run_ctl(Path::new("/nonexistent/wayvrctl"), &[], Duration::from_secs(1)).is_err());
        assert!(run_ctl(Path::new("/bin/sleep"), &["5"], Duration::from_millis(100)).unwrap_err().contains("in time"));
    }
}
