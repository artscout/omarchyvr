//! `omarchyvr desk captions on|off|status`: the live subtitles of a running desk session.
//!
//! The same switch as the voice intents "captions on/off" (omarchyvr-voice `@captions`): the flag file
//! `$XDG_RUNTIME_DIR/omarchyvr-voice-captions.off`, which `omarchyvr-voice captions` checks twice a second. `on` also starts
//! `omarchyvr-voice listen` and `omarchyvr-voice captions` into the running session when they are missing (pid files in the
//! session dir, so `desk stop` stops them), the way `desk start --voice` does. The captions panel is part of WayVR's
//! config only in sessions started with voice (`--voice` or the assistant preset): in another session `on` says so
//! and changes nothing else.
use super::config::comm_of;
use super::desk::{plan, stays_alive, DeskOptions, Spawner};
use super::session::{self, Deps};
use crate::{CliError, CliResult};
use std::path::PathBuf;

/// The flag file (same path as omarchyvr-voice's `act::captions_off_path`).
pub fn off_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    env("XDG_RUNTIME_DIR").filter(|p| !p.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir).join("omarchyvr-voice-captions.off")
}

/// Does the running session's WayVR have the captions panel (written at desk start with voice)?
fn panel_configured(o: &DeskOptions) -> bool {
    std::fs::read_to_string(o.wayvr_home.join("config/wayvr/conf.d/40-panels.yaml")).is_ok_and(|t| t.lines().any(|l| l.trim() == "- captions"))
}

fn alive(o: &DeskOptions, name: &str, bin: &std::path::Path, d: &Deps) -> Option<u32> {
    let pid = session::read_pid(&o.pid_file(name))?;
    (d.procs.is_alive(pid) && d.procs.comm(pid).as_deref() == Some(comm_of(bin).as_str())).then_some(pid)
}

/// The status lines.
pub fn status_lines(o: &DeskOptions, d: &Deps) -> Vec<String> {
    let flag = off_path(d.env);
    let mut v = vec![format!("captions: {}", if flag.exists() { "off (flag file)" } else { "on" })];
    for name in ["voice", "voicecap"] {
        v.push(match alive(o, name, &o.voice_bin, d) {
            Some(p) => format!("{name}: running, pid {p}"),
            None => format!("{name}: not running"),
        });
    }
    v.push(format!("captions panel in this session's WayVR: {}", if panel_configured(o) { "yes" } else { "no (desk start without voice)" }));
    v
}

/// `desk captions on|off|status`.
pub fn run(o: &DeskOptions, what: &str, d: &Deps, sp: &dyn Spawner) -> CliResult {
    let flag = off_path(d.env);
    match what {
        "off" => {
            std::fs::write(&flag, "off\n").map_err(|e| CliError::new(format!("{}: {e}", flag.display())))?;
            vout!("omarchyvr desk: captions off (the listener keeps running for voice commands)");
            Ok(())
        }
        "on" => {
            match std::fs::remove_file(&flag) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => return Err(CliError::new(format!("{}: {e}", flag.display()))),
                _ => {}
            }
            if alive(o, "wayvr", &o.wayvr_bin, d).is_none() {
                vout!("omarchyvr desk: captions on; no desk session runs (start it with --voice to show them)");
                return Ok(());
            }
            if !panel_configured(o) {
                return Err(CliError::new(
                    "this desk session was started without voice: its WayVR has no captions panel. Restart desk mode with --voice (or the assistant preset)",
                ));
            }
            let mut with_voice = o.clone();
            with_voice.voice = true;
            for s in plan(&with_voice).into_iter().filter(|s| matches!(s.name, "voice" | "voicecap")) {
                if let Some(p) = alive(o, s.name, &s.program, d) {
                    vout!("omarchyvr desk: {} already running, pid {p}", s.name);
                    continue;
                }
                vout!("omarchyvr desk: starting {} ({})", s.name, s.program.display());
                let pid = sp.spawn(&s).map_err(|e| CliError::new(format!("cannot start {}: {e}", s.program.display())))?;
                let _ = std::fs::write(o.pid_file(s.name), format!("{pid}\n"));
                if !stays_alive(pid, d.procs, d.clock, o.settle) {
                    let _ = std::fs::remove_file(o.pid_file(s.name));
                    return Err(CliError::new(format!("{} ended during start-up (log {})", s.name, s.log.display())));
                }
                vout!("omarchyvr desk: {} running, pid {pid}, log {}", s.name, s.log.display());
            }
            vout!("omarchyvr desk: captions on");
            Ok(())
        }
        "status" => {
            for l in status_lines(o, d) {
                vout!("{l}");
            }
            Ok(())
        }
        other => Err(CliError::usage(format!("usage: omarchyvr desk captions on|off|status (not {other})"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::args::Args;
    use crate::vr::desk::{parse_desk, ProcSpec};
    use crate::vr::host::{Clock, NotifySend, Procs, RealHost, Signal};
    use std::cell::{Cell, RefCell};
    use std::time::Duration;

    struct Clk(Cell<Duration>);
    impl Clock for Clk {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    #[derive(Default)]
    struct Table(RefCell<Vec<(u32, String)>>);
    impl Procs for Table {
        fn is_alive(&self, pid: u32) -> bool {
            self.0.borrow().iter().any(|(p, _)| *p == pid)
        }
        fn comm(&self, pid: u32) -> Option<String> {
            self.0.borrow().iter().find(|(p, _)| *p == pid).map(|(_, c)| c.clone())
        }
        fn signal(&self, _: u32, _: Signal) -> std::io::Result<()> {
            Ok(())
        }
        fn find_by_comm(&self, _: &str) -> Vec<u32> {
            vec![]
        }
        fn environ(&self, _: u32) -> Option<Vec<(String, String)>> {
            None
        }
        fn age_secs(&self, _: u32) -> Option<f64> {
            None
        }
    }

    /// Records the specs and puts a live process with the binary's name in the table.
    struct Spawns<'a>(&'a Table, RefCell<Vec<String>>);
    impl Spawner for Spawns<'_> {
        fn spawn(&self, spec: &ProcSpec) -> std::io::Result<u32> {
            let pid = 500 + self.1.borrow().len() as u32;
            self.1.borrow_mut().push(format!("{} {}", spec.name, spec.args.first().cloned().unwrap_or_default()));
            self.0 .0.borrow_mut().push((pid, comm_of(&spec.program)));
            Ok(pid)
        }
    }

    fn never() -> bool {
        false
    }

    #[test]
    fn captions_flip_the_flag_and_start_the_listener_into_a_voice_session() {
        let d = crate::vr::test_dir("captions");
        let (run_dir, session, home) = (d.join("run"), d.join("session"), d.join("wayvr-home"));
        for x in [&run_dir, &session, &home] {
            std::fs::create_dir_all(x).unwrap();
        }
        let vars = [("HOME", "/home/u".to_string()), ("XDG_RUNTIME_DIR", run_dir.display().to_string()), ("PIMAX_VR_SESSION_DIR", session.display().to_string())];
        let env = move |k: &str| vars.iter().find(|(n, _)| *n == k).map(|(_, v)| v.clone());
        let mut a = Args(vec!["--wayvr-home".into(), home.display().to_string()]);
        let o = parse_desk(&mut a, &env).unwrap();
        let (host, notifier, clock, table) = (RealHost::from_env(), NotifySend::new(false), Clk(Cell::new(Duration::ZERO)), Table::default());
        let deps = Deps { host: &host, procs: &table, notifier: &notifier, clock: &clock, env: &env, stop_requested: &never };
        let sp = Spawns(&table, RefCell::new(vec![]));
        let flag = run_dir.join("omarchyvr-voice-captions.off");
        // off: the flag; on without a session: the flag goes, nothing starts
        run(&o, "off", &deps, &sp).unwrap();
        assert_eq!(std::fs::read_to_string(&flag).unwrap(), "off\n");
        run(&o, "on", &deps, &sp).unwrap();
        assert!(!flag.exists() && sp.1.borrow().is_empty());
        // a session without voice: WayVR runs but has no captions panel
        table.0.borrow_mut().push((42, comm_of(&o.wayvr_bin)));
        std::fs::write(o.pid_file("wayvr"), "42\n").unwrap();
        assert!(run(&o, "on", &deps, &sp).unwrap_err().msg.contains("without voice"));
        // a voice session: the listener and the captions start, with their pid files; a second on starts nothing
        let cd = home.join("config/wayvr/conf.d");
        std::fs::create_dir_all(&cd).unwrap();
        std::fs::write(cd.join("40-panels.yaml"), "custom_panels:\n  - captions\n  - perf\n").unwrap();
        run(&o, "on", &deps, &sp).unwrap();
        assert_eq!(*sp.1.borrow(), vec!["voice listen".to_string(), "voicecap captions".to_string()]);
        assert!(o.pid_file("voice").exists() && o.pid_file("voicecap").exists());
        run(&o, "on", &deps, &sp).unwrap();
        assert_eq!(sp.1.borrow().len(), 2);
        let st = status_lines(&o, &deps);
        assert!(st[0] == "captions: on" && st[1].starts_with("voice: running") && st[3].ends_with("yes"), "{st:?}");
        assert!(run(&o, "maybe", &deps, &sp).is_err());
        let _ = std::fs::remove_dir_all(d);
    }
}
