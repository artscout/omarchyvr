//! `pimax-autod sim SCRIPT`: replay a scripted signal timeline through the real state machine with a simulated
//! executor (a start takes `start_delay_s`, then the session shows up; a stop takes 2 s) and print the transitions.
//!
//! Script lines (`#` comments):
//! ```text
//! config KEY=VALUE ...          auto.toml keys (e.g. config dim_after_s=60)
//! sim start_delay_s=3 fail_start=0
//! at T KEY=VALUE ...            from time T (seconds): plugged=0|1 motion=unknown|still|worn|moving
//!                               visor=none|0|1 session=none|desk|game game=0|1 input=0|1
//!                               cmd=pause|resume|wake|sleep|mode-desk|mode-game|runtime-steamvr|preset-NAME
//! expect T STATE                state after the tick at T (e.g. active(desk), idle-dim(game), sleep)
//! end T                         stop at T (default: last time + 30)
//! ```
use crate::config::{Config, Mode, Runtime};
use crate::machine::{Action, Command, Input, Kind, Machine, Signals};
use crate::motion::Motion;
use std::path::Path;

#[derive(Debug, Clone)]
enum Ev {
    Set(String, String),
    Cmd(Command),
}

/// Result of a run.
#[derive(Debug, Default)]
pub struct Outcome {
    pub log: Vec<String>,
    pub failures: Vec<String>,
    pub starts: usize,
    pub stops: usize,
}

fn parse_cmd(v: &str) -> Result<Command, String> {
    Ok(match v {
        "pause" => Command::Pause,
        "resume" => Command::Resume,
        "wake" => Command::Wake,
        "sleep" => Command::Sleep,
        "mode-desk" => Command::SetMode { mode: Some(Mode::Desk), runtime: None, preset: None },
        "mode-game" => Command::SetMode { mode: Some(Mode::Game), runtime: None, preset: None },
        "runtime-steamvr" => Command::SetMode { mode: None, runtime: Some(Runtime::SteamVr), preset: None },
        "runtime-monado" => Command::SetMode { mode: None, runtime: Some(Runtime::Monado), preset: None },
        p if p.starts_with("preset-") => Command::SetMode { mode: None, runtime: None, preset: Some(p[7..].into()) },
        _ => return Err(format!("unknown cmd {v}")),
    })
}

fn apply(s: &mut Signals, k: &str, v: &str) -> Result<(), String> {
    let b = |v: &str| match v {
        "1" | "true" => Ok(true),
        "0" | "false" => Ok(false),
        _ => Err(format!("{k}: 0|1, not {v}")),
    };
    match k {
        "plugged" => s.plugged = b(v)?,
        "motion" => s.motion = Motion::parse(v).ok_or(format!("bad motion {v}"))?,
        "visor" => s.visor = if v == "none" { None } else { Some(b(v)?) },
        "session" => {
            s.session = match v {
                "none" => None,
                "desk" => Some(Kind::Desk),
                "game" => Some(Kind::Game),
                _ => return Err(format!("bad session {v}")),
            }
        }
        "game" => s.game = b(v)?,
        "input" => s.input = b(v)?,
        _ => return Err(format!("unknown signal {k}")),
    }
    Ok(())
}

/// Run a script. `home` only matters for paths in the config.
pub fn run(script: &str, home: &Path) -> Result<Outcome, String> {
    let mut cfg = Config::defaults(home);
    cfg.dry_run = true;
    let mut start_delay = 3.0;
    let mut fail_start = false;
    let mut events: Vec<(f64, Ev)> = Vec::new();
    let mut expects: Vec<(f64, String)> = Vec::new();
    let mut end: Option<f64> = None;
    for (n, raw) in script.lines().enumerate() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        let err = |e: String| format!("line {}: {e}", n + 1);
        let mut w = line.split_whitespace();
        let word = w.next().unwrap_or("");
        let num = |x: Option<&str>| x.and_then(|t| t.parse::<f64>().ok()).ok_or_else(|| err("want a time".into()));
        match word {
            "config" => {
                for kv in w {
                    let (k, v) = kv.split_once('=').ok_or_else(|| err(format!("bad {kv}")))?;
                    cfg.set(k, v, home).map_err(err)?;
                }
            }
            "sim" => {
                for kv in w {
                    match kv.split_once('=') {
                        Some(("start_delay_s", v)) => start_delay = v.parse().map_err(|_| err(kv.into()))?,
                        Some(("fail_start", v)) => fail_start = v == "1",
                        _ => return Err(err(format!("bad sim option {kv}"))),
                    }
                }
            }
            "at" => {
                let t = num(w.next())?;
                for kv in w {
                    let (k, v) = kv.split_once('=').ok_or_else(|| err(format!("bad {kv}")))?;
                    if k == "cmd" {
                        events.push((t, Ev::Cmd(parse_cmd(v).map_err(err)?)));
                    } else {
                        apply(&mut Signals::default(), k, v).map_err(err)?;
                        events.push((t, Ev::Set(k.into(), v.into())));
                    }
                }
            }
            "expect" => {
                let t = num(w.next())?;
                expects.push((t, w.collect::<Vec<_>>().join(" ")));
            }
            "end" => end = Some(num(w.next())?),
            _ => return Err(err(format!("unknown line {line:?}"))),
        }
    }
    let last = events.iter().map(|e| e.0).chain(expects.iter().map(|e| e.0)).fold(0.0, f64::max);
    let end = end.unwrap_or(last + 30.0);
    let step = 1.0_f64.min(cfg.poll_s);
    let mut m = Machine::new(cfg);
    let mut sig = Signals::default();
    let mut o = Outcome::default();
    // simulated executor: (time it finishes, ok, session afterwards)
    let mut pending: Option<(f64, bool, Option<Kind>)> = None;
    let mut sim_session: Option<Kind> = None;
    let mut ti = 0usize;
    events.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut t = 0.0;
    let mut seen = 0;
    while t <= end + 1e-9 {
        let mut acts = Vec::new();
        while ti < events.len() && events[ti].0 <= t + 1e-9 {
            match &events[ti].1 {
                Ev::Set(k, v) => apply(&mut sig, k, v)?,
                Ev::Cmd(c) => {
                    o.log.push(format!("{t:7.1}s  cmd {c:?}"));
                    acts.extend(m.step(t, Input::Cmd(c.clone())));
                }
            }
            ti += 1;
        }
        if let Some((at, ok, sess)) = pending {
            if t + 1e-9 >= at {
                pending = None;
                sim_session = sess;
                acts.extend(m.step(t, Input::Done { ok, detail: if ok { "ok".into() } else { "simulated failure".into() } }));
            }
        }
        let mut s = sig;
        if s.session.is_none() {
            s.session = sim_session;
        }
        acts.extend(m.step(t, Input::Tick(s)));
        for a in acts {
            o.log.push(format!("{t:7.1}s  ACTION {a:?}"));
            match &a {
                Action::Start { mode, .. } | Action::Switch { mode, .. } => {
                    o.starts += 1;
                    let k = if *mode == Mode::Game { Kind::Game } else { Kind::Desk };
                    let ok = !fail_start;
                    pending = Some((t + start_delay, ok, ok.then_some(k)));
                }
                Action::Stop { .. } => {
                    o.stops += 1;
                    pending = Some((t + 2.0, true, None));
                }
                Action::Notify(_) => {}
            }
        }
        for tr in &m.transitions[seen..] {
            o.log.push(format!("{:7.1}s  {} -> {}  ({})", tr.t, tr.from.name(), tr.to.name(), tr.why));
        }
        seen = m.transitions.len();
        for (et, want) in &expects {
            if (et - t).abs() < step / 2.0 {
                let got = m.state.name();
                if &got != want {
                    o.failures.push(format!("at {et}s expected {want}, got {got}"));
                }
            }
        }
        t += step;
    }
    o.log.push(format!("{t:7.1}s  end: {} (starts {}, stops {})", m.state.name(), o.starts, o.stops));
    Ok(o)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check(name: &str, text: &str) {
        let o = run(text, Path::new("/h")).unwrap();
        assert!(o.failures.is_empty(), "{name}: {:?}\n{}", o.failures, o.log.join("\n"));
    }

    #[test]
    fn shipped_scripts_pass() {
        check("put-on", include_str!("../sim/put-on.sim"));
        check("take-off", include_str!("../sim/take-off.sim"));
        check("idle-sleep", include_str!("../sim/idle-sleep.sim"));
        check("game-running", include_str!("../sim/game-running.sim"));
        check("unplug", include_str!("../sim/unplug.sim"));
        check("failed-start", include_str!("../sim/failed-start.sim"));
    }

    #[test]
    fn bad_scripts_are_errors() {
        assert!(run("at x plugged=1", Path::new("/h")).is_err());
        assert!(run("at 1 bogus=1", Path::new("/h")).is_err());
        assert!(run("config nope=1", Path::new("/h")).is_err());
        assert!(run("frobnicate", Path::new("/h")).is_err());
    }

    #[test]
    fn a_wrong_expectation_is_reported() {
        let o = run("at 0 plugged=1\nexpect 10 active(desk)\n", Path::new("/h")).unwrap();
        assert_eq!(o.failures.len(), 1);
    }
}
