//! The command line of pimaxd / pimax-autod (one program, two names).
use crate::{client, config, daemon, dconfig, ipc, motion, scenario, sim, visor};
use std::path::PathBuf;
use std::time::Duration;

const USAGE: &str = "pimaxd - the Pimax stack's owner: auto mode + the component reconciler (docs/PIMAXD.md, docs/AUTO-MODE.md)
(pimax-autod is the same program)

usage: pimaxd <command>
  run [--config FILE] [--live|--dry-run]   the daemon. Auto mode: dry run unless pimaxd.toml [auto] / auto.toml say
                                           dry_run = false or --live. Reconciler: observe only unless pimaxd.toml
                                           [restart] enabled = true and ~/.config/pimax/safe-mode does not exist
  health [--json] [--deep] [--probe] [--watch]   the component table (read-only; pimaxd's snapshot when it runs)
  state                                    the snapshot from the running daemon (JSON, schema v1)
  set session=on|off [mode=desk|vr] [KEY=on|off...]   intent and per-session toggles (KEY: voice, perf, ...)
  toggle KEY on|off | fix ID | safe-mode on|off | ack-gpu
  wait overall=ok|health:ID=H|intent=off [--timeout S]
  scenario SCRIPT...                       replay reconciler scenarios (rust/pimax-autod/sim/scenarios/*.scn)
  config [--config FILE]                   print the effective settings and config warnings
  sim SCRIPT...                            replay scripted signal timelines (rust/pimax-autod/sim/*.sim)
  replay-imu DATA.csv [--window S]         wear classes of a EuRoC imu0/data.csv recording
  status | events | pause | resume | wake | sleep
  set-mode [desk|game] [--runtime monado|steamvr] [--preset none|cinema|work|assistant]
                                           control a running daemon (same as `pimax auto ...`)
  visor-probe [--count N] [--every S]      LIVE TEST ONLY: send the Windows 0x2c visor-status read, print replies
  pair left|right                          controller pairing: sends StartPair once (approved 2026-10-14), waits up to 35 s";

fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| "/".into())
}

fn opt(a: &mut Vec<String>, name: &str) -> Option<String> {
    let i = a.iter().position(|x| x == name)?;
    if i + 1 >= a.len() {
        return None;
    }
    let v = a.remove(i + 1);
    a.remove(i);
    Some(v)
}

fn flag(a: &mut Vec<String>, name: &str) -> bool {
    let i = a.iter().position(|x| x == name);
    if let Some(i) = i {
        a.remove(i);
    }
    i.is_some()
}

fn load(a: &mut Vec<String>) -> (config::Config, Vec<String>, String, dconfig::DaemonConfig) {
    let h = home();
    let explicit = opt(a, "--config").map(PathBuf::from);
    let (d, mut dw, dp) = dconfig::DaemonConfig::load(&h);
    for w in dw.iter_mut() {
        *w = format!("{}: {w}", dp.display());
    }
    let (c, w, from) = d.auto_config(&h, explicit.as_deref());
    dw.extend(w.into_iter().map(|x| format!("{from}: {x}")));
    (c, dw, from, d)
}

fn real_main() -> Result<(), String> {
    let mut a: Vec<String> = std::env::args().skip(1).collect();
    if a.is_empty() || matches!(a[0].as_str(), "-h" | "--help" | "help") {
        println!("{USAGE}");
        return Ok(());
    }
    let cmd = a.remove(0);
    match cmd.as_str() {
        "run" => {
            let (mut c, warn, _, d) = load(&mut a);
            for w in &warn {
                daemon::log(w);
            }
            if flag(&mut a, "--live") {
                c.dry_run = false;
            }
            if flag(&mut a, "--dry-run") {
                c.dry_run = true;
            }
            if let Some(x) = a.first() {
                return Err(format!("unexpected {x}"));
            }
            daemon::run(c, d, home(), ipc::socket_path())
        }
        "config" => {
            let (c, warn, from, d) = load(&mut a);
            println!("# pimaxd.toml: {}", dconfig::path(&home()).display());
            println!("{d:#?}");
            println!("# auto mode: {from}");
            println!("{c:#?}");
            for w in warn {
                println!("warning: {w}");
            }
            Ok(())
        }
        "sim" => {
            if a.is_empty() {
                return Err("sim needs script files".into());
            }
            let mut failed = 0;
            for f in &a {
                let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
                let o = sim::run(&text, &home()).map_err(|e| format!("{f}: {e}"))?;
                println!("== {f}");
                for l in &o.log {
                    println!("{l}");
                }
                for x in &o.failures {
                    println!("FAIL {x}");
                }
                failed += o.failures.len();
            }
            if failed > 0 {
                return Err(format!("{failed} expectation(s) failed"));
            }
            Ok(())
        }
        "replay-imu" => {
            let w: f64 = opt(&mut a, "--window").and_then(|v| v.parse().ok()).unwrap_or(0.5);
            let f = a.first().ok_or("replay-imu needs a data.csv")?;
            let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
            let (c, _, _, _) = load(&mut Vec::new());
            let rows = motion::replay_euroc(&text, w, &mut motion::Classifier::new(c.still_dps, c.worn_dps));
            let mut counts = std::collections::BTreeMap::new();
            let mut last = None;
            for (t, s, m) in &rows {
                *counts.entry(m.name()).or_insert(0) += 1;
                if last != Some(*m) {
                    println!("{t:8.1}s  {:7}  gyro std {s:.3} deg/s", m.name());
                    last = Some(*m);
                }
            }
            println!("windows: {counts:?}");
            Ok(())
        }
        "status" | "events" | "pause" | "resume" | "wake" | "sleep" | "set-mode" => {
            let mut args = vec![cmd.clone()];
            args.extend(a);
            let line = client::request_line(&args)?;
            let sock = ipc::socket_path();
            if cmd == "events" {
                client::events(&sock, |l| {
                    println!("{l}");
                    true
                })
            } else {
                println!("{}", client::request(&sock, &line)?);
                Ok(())
            }
        }
        "health" => {
            let code = pimax_components::health::main(&a);
            if code != 0 && code != 1 {
                std::process::exit(code);
            }
            Ok(())
        }
        "scenario" => {
            if a.is_empty() {
                return Err("scenario needs script files".into());
            }
            let mut failed = 0;
            for f in &a {
                let text = std::fs::read_to_string(f).map_err(|e| format!("{f}: {e}"))?;
                let o = scenario::run(&text).map_err(|e| format!("{f}: {e}"))?;
                println!("== {f}");
                for l in &o.log {
                    println!("{l}");
                }
                for x in &o.failures {
                    println!("FAIL {x}");
                }
                failed += o.failures.len();
            }
            if failed > 0 {
                return Err(format!("{failed} expectation(s) failed"));
            }
            Ok(())
        }
        "state" | "set" | "toggle" | "fix" | "safe-mode" | "ack-gpu" | "wait" => {
            let line = client::supervise_line(&cmd, &mut a)?;
            let sock = ipc::socket_path();
            let reply = client::request_timeout(&sock, &line, Duration::from_secs(if cmd == "wait" { 3700 } else { 5 }))?;
            println!("{reply}");
            if reply.contains("\"ok\":false") {
                return Err("refused".into());
            }
            Ok(())
        }
        "visor-probe" => {
            let n: u32 = opt(&mut a, "--count").and_then(|v| v.parse().ok()).unwrap_or(10);
            let every: f64 = opt(&mut a, "--every").and_then(|v| v.parse().ok()).unwrap_or(1.0);
            visor::probe(n.min(1000), Duration::from_secs_f64(every.clamp(0.1, 60.0)))
        }
        "pair" => {
            let side = a.first().cloned().unwrap_or_default();
            client::pair_cli(&ipc::socket_path(), &side, &mut |l| println!("{l}"))
        }
        other => Err(format!("unknown command {other}\n{USAGE}")),
    }
}

pub fn main() {
    if let Err(e) = real_main() {
        eprintln!("pimaxd: {e}");
        std::process::exit(1);
    }
}
