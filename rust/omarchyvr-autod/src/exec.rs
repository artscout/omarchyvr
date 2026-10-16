//! Turning [`Action`]s into launcher commands. Auto mode never starts or signals Monado / WayVR / the broker itself:
//! it runs `omarchyvr desk|vr start|stop` (the launcher stops by pid only, with `--no-kill`: never SIGKILL) and
//! `omarchyvr-roommap relocalize --source broker --name <map> --apply` (the only offset writer of an auto session: the
//! AprilTag `omarchyvr-anchor apply` is never run by auto mode).
use crate::config::{Config, Mode, Runtime};
use crate::machine::{Action, Kind};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// One command line.
#[derive(Debug, Clone, PartialEq)]
pub struct CmdSpec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// A failure of this step is reported but does not fail the action (relocalisation).
    pub optional: bool,
}

impl CmdSpec {
    fn new(program: &Path, args: &[&str]) -> Self {
        Self { program: program.to_path_buf(), args: args.iter().map(|s| s.to_string()).collect(), env: vec![], optional: false }
    }
    pub fn line(&self) -> String {
        let env: String = self.env.iter().map(|(k, v)| format!("{k}={v} ")).collect();
        format!("{env}{} {}", self.program.display(), self.args.join(" "))
    }
}

/// The room map exists (`<base>/<name>/map.bin`, base = `$PIMAX_ROOMMAP_DIR` or `~/.local/share/omarchyvr/roommap`).
pub fn room_map_exists(home: &Path, name: &str) -> bool {
    let base = std::env::var_os("PIMAX_ROOMMAP_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".local/share/omarchyvr/roommap"));
    base.join(name).join("map.bin").is_file()
}

fn start_cmds(c: &Config, mode: Mode, runtime: Runtime, preset: &str, map: bool) -> Vec<CmdSpec> {
    if mode == Mode::Game && runtime == Runtime::SteamVr {
        return vec![]; // phase 1: the user starts SteamVR from Steam
    }
    let mut args: Vec<&str> = match mode {
        Mode::Desk => vec!["desk", "start"],
        Mode::Game => vec!["vr", "start"],
    };
    if c.sixdof {
        args.push("--6dof");
        if c.ctrl_track {
            args.push("--ctrl-track");
        }
    }
    match mode {
        Mode::Desk => {
            if !c.perf {
                args.push("--no-perf");
            }
            if preset != "none" {
                args.extend(["--preset", preset]);
            }
        }
        Mode::Game => {
            if c.perf {
                args.push("--perf");
            }
        }
    }
    args.push("--no-notify");
    let mut start = CmdSpec::new(&c.pimax_bin, &args);
    let reloc = c.relocalize && c.sixdof && map;
    if reloc {
        // Monado applies $XDG_RUNTIME_DIR/pimax-anchor.offset (and writes the head ring roommap reads) only with this
        start.env.push(("PIMAX_ANCHOR".into(), "1".into()));
    }
    let mut v = vec![start];
    if reloc {
        let mut r = CmdSpec::new(&c.roommap_bin, &["relocalize", "--source", "broker", "--name", &c.room_map, "--apply"]);
        r.optional = true;
        v.push(r);
    }
    v
}

fn stop_cmd(c: &Config, kind: Kind) -> CmdSpec {
    match kind {
        // automatic stops never SIGKILL Monado (P0: a hung one is left alive and latches the GPU gate)
        Kind::Desk => CmdSpec::new(&c.pimax_bin, &["desk", "stop", "--grace", "30", "--no-kill"]),
        Kind::Game => CmdSpec::new(&c.pimax_bin, &["vr", "stop", "--grace", "30", "--no-kill"]),
    }
}

/// The command lines for an action (empty for Notify and the SteamVR start).
pub fn commands(c: &Config, a: &Action, map_exists: bool) -> Vec<CmdSpec> {
    match a {
        Action::Start { mode, runtime, preset } => start_cmds(c, *mode, *runtime, preset, map_exists),
        Action::Stop { kind, .. } => vec![stop_cmd(c, *kind)],
        Action::Switch { from, mode, runtime, preset } => {
            let mut v = vec![stop_cmd(c, *from)];
            v.extend(start_cmds(c, *mode, *runtime, preset, map_exists));
            v
        }
        Action::Notify(_) => vec![],
    }
}

/// Where the launcher's output goes (a file, not a pipe: the processes it detaches could keep a pipe open forever).
pub fn cmd_log_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("omarchyvr-autod-cmd.log")
}

/// Run the steps of an action in order; the first non-optional failure ends it. Returns (ok, detail).
pub fn run_steps(steps: &[CmdSpec], log: &dyn Fn(&str)) -> (bool, String) {
    let out_path = cmd_log_path();
    for s in steps {
        log(&format!("run: {}", s.line()));
        let out = std::fs::OpenOptions::new().create(true).append(true).open(&out_path);
        let start_len = std::fs::metadata(&out_path).map(|m| m.len()).unwrap_or(0);
        let pair = out.and_then(|f| Ok((f.try_clone()?, f)));
        let (o1, o2) = match pair {
            Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
            Err(_) => (Stdio::null(), Stdio::null()),
        };
        let r = Command::new(&s.program)
            .args(&s.args)
            .envs(s.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(o1)
            .stderr(o2)
            .status();
        let (ok, why) = match r {
            Ok(st) if st.success() => (true, String::new()),
            Ok(st) => {
                let text = std::fs::read(&out_path).unwrap_or_default();
                let tail = String::from_utf8_lossy(text.get(start_len as usize..).unwrap_or(&[])).into_owned();
                let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                (false, format!("{} exited with {st}: {last}", s.program.display()))
            }
            Err(e) => (false, format!("{}: {e}", s.program.display())),
        };
        if !ok {
            log(&format!("failed: {why}"));
            if !s.optional {
                return (false, why);
            }
        }
    }
    (true, "ok".into())
}

/// After a failed start the launcher may have left Monado running ("Monado is still running: omarchyvr desk stop"):
/// the stop of that mode, run by the worker before it reports the failure.
pub fn cleanup_after_failed_start(c: &Config, a: &Action) -> Option<CmdSpec> {
    match a {
        Action::Start { mode, runtime, .. } | Action::Switch { mode, runtime, .. } => {
            if *mode == Mode::Game && *runtime == Runtime::SteamVr {
                return None;
            }
            Some(stop_cmd(c, if *mode == Mode::Game { Kind::Game } else { Kind::Desk }))
        }
        _ => None,
    }
}

/// Desktop notification (best effort, never blocks the loop for long).
pub fn notify(msg: &str) {
    let _ = Command::new("notify-send")
        .args(["-a", "omarchyvr", "omarchyvr auto mode", msg])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|mut c| std::thread::spawn(move || c.wait()));
}

/// Execute an action in a worker thread (real) or log it (dry run). The result comes back on `done`.
pub fn execute(c: &Config, a: &Action, home: &Path, done: Box<dyn FnOnce(bool, String) + Send>, log: fn(&str)) {
    if let Action::Notify(m) = a {
        log(&format!("notify: {m}"));
        if !c.dry_run {
            notify(m);
        }
        return;
    }
    let steps = commands(c, a, room_map_exists(home, &c.room_map));
    if c.dry_run {
        for s in &steps {
            log(&format!("dry-run: would run: {}", s.line()));
        }
        done(true, "dry-run".into());
        return;
    }
    let cleanup = cleanup_after_failed_start(c, a);
    std::thread::spawn(move || {
        let (ok, detail) = run_steps(&steps, &|m| log(m));
        if let (false, Some(cl)) = (ok, &cleanup) {
            log("start failed: stopping whatever it left running");
            run_steps(std::slice::from_ref(cl), &|m| log(m));
        }
        done(ok, detail);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(c: &Config, a: &Action, map: bool) -> Vec<String> {
        commands(c, a, map).iter().map(CmdSpec::line).collect()
    }

    fn start(mode: Mode, runtime: Runtime, preset: &str) -> Action {
        Action::Start { mode, runtime, preset: preset.into() }
    }

    #[test]
    fn default_desk_start_with_and_without_a_room_map() {
        let c = Config::defaults(Path::new("/h"));
        assert_eq!(
            lines(&c, &start(Mode::Desk, Runtime::Monado, "none"), false),
            ["/h/.local/bin/omarchyvr desk start --6dof --ctrl-track --no-notify"]
        );
        assert_eq!(
            lines(&c, &start(Mode::Desk, Runtime::Monado, "cinema"), true),
            [
                "PIMAX_ANCHOR=1 /h/.local/bin/omarchyvr desk start --6dof --ctrl-track --preset cinema --no-notify",
                "/h/pimax-re/rust/target/release/omarchyvr-roommap relocalize --source broker --name home --apply",
            ]
        );
        assert!(commands(&c, &start(Mode::Desk, Runtime::Monado, "none"), true)[1].optional);
    }

    #[test]
    fn game_start_stop_and_switch() {
        let mut c = Config::defaults(Path::new("/h"));
        c.relocalize = false;
        assert_eq!(
            lines(&c, &start(Mode::Game, Runtime::Monado, "work"), true),
            ["/h/.local/bin/omarchyvr vr start --6dof --ctrl-track --perf --no-notify"]
        );
        assert!(lines(&c, &start(Mode::Game, Runtime::SteamVr, "none"), true).is_empty());
        assert_eq!(
            lines(&c, &Action::Stop { kind: Kind::Desk, reason: "idle" }, true),
            ["/h/.local/bin/omarchyvr desk stop --grace 30 --no-kill"]
        );
        let sw = Action::Switch { from: Kind::Desk, mode: Mode::Game, runtime: Runtime::Monado, preset: "none".into() };
        let l = lines(&c, &sw, false);
        assert_eq!(l[0], "/h/.local/bin/omarchyvr desk stop --grace 30 --no-kill");
        assert!(l[1].contains("vr start"));
    }

    #[test]
    fn no_6dof_means_no_ctrl_track_and_no_relocalisation() {
        let mut c = Config::defaults(Path::new("/h"));
        c.sixdof = false;
        c.perf = false;
        assert_eq!(
            lines(&c, &start(Mode::Desk, Runtime::Monado, "none"), true),
            ["/h/.local/bin/omarchyvr desk start --no-perf --no-notify"]
        );
    }

    #[test]
    fn steps_stop_at_the_first_required_failure() {
        let ok = CmdSpec::new(Path::new("true"), &[]);
        let bad = CmdSpec::new(Path::new("false"), &[]);
        let mut opt_bad = bad.clone();
        opt_bad.optional = true;
        assert!(run_steps(&[ok.clone(), opt_bad, ok.clone()], &|_| {}).0);
        let (r, why) = run_steps(&[bad, ok], &|_| {});
        assert!(!r && why.contains("false"));
        assert!(!run_steps(&[CmdSpec::new(Path::new("/nonexistent/x"), &[])], &|_| {}).0);
        // the reason is the last line the command printed
        let sh = CmdSpec::new(Path::new("sh"), &["-c", "echo first; echo 'omarchyvr: no headset' >&2; exit 3"]);
        let (r, why) = run_steps(&[sh], &|_| {});
        assert!(!r && why.ends_with("omarchyvr: no headset"), "{why}");
    }

    #[test]
    fn a_failed_start_is_cleaned_up_with_the_matching_stop() {
        let c = Config::defaults(Path::new("/h"));
        let l = |a: &Action| cleanup_after_failed_start(&c, a).map(|s| s.line());
        assert_eq!(l(&start(Mode::Desk, Runtime::Monado, "none")).unwrap(), "/h/.local/bin/omarchyvr desk stop --grace 30 --no-kill");
        assert_eq!(l(&start(Mode::Game, Runtime::Monado, "none")).unwrap(), "/h/.local/bin/omarchyvr vr stop --grace 30 --no-kill");
        assert!(l(&start(Mode::Game, Runtime::SteamVr, "none")).is_none());
        assert!(l(&Action::Stop { kind: Kind::Desk, reason: "x" }).is_none());
    }
}
