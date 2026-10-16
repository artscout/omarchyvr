//! Executes reconcile steps (never in observe-only mode). Hard limits (plan section 4.4):
//! * Monado is never SIGKILLed: the subtree stop is `pimax desk|vr stop --grace 30 --no-kill` (Lane A's P0: WayVR
//!   exits before Monado gets SIGINT; a hung Monado is left alive and latches needs-reboot).
//! * No DRM, hotplug, sysfs writes or HID bytes from pimaxd. Device recovery is `pimax vr reset-headset` (the
//!   approved exit path), only after the subtree stopped.
//! * Leaves stop through their component module (`pimax_components::components`, SIGINT by pid, WayVR waited
//!   for); without one, SIGINT then SIGTERM by pid. Never by name, never SIGKILL.
//! * Children run with `PIMAX_WATCHDOG=1`: the launcher then leaves no user-stop marker and spawns no second
//!   watchdog (pimaxd is the owner).
//!
//! Starts: Monado through the launcher with the recorded arguments (`start-argv.txt`, or flags derived from what
//! the session ran); a leaf by respawning the argv/env/cwd captured from /proc while it ran (detached with setsid,
//! output appended to `<session dir>/<name>.log`, pid file written); ai.server through `pimax-ai serve`; the
//! keepalive through `systemctl --user restart`.
use crate::supervisor::Captured;
use pimax_components::model::Mode;
use pimax_components::procs;
use pimax_components::reconcile::{Act, Step};
use pimax_components::spec::spec;
use pimax_components::state::Outcome;
use pimax_components::util::Roots;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::Sender;
use std::time::{Duration, Instant};

/// Everything a step needs, copied out of the supervisor (the executor runs in a worker thread).
#[derive(Debug, Clone)]
pub struct Ctx {
    pub roots: Roots,
    pub pimax_bin: PathBuf,
    pub mode: Mode,
    /// `start-argv.txt` (kind, args).
    pub argv: Option<(String, Vec<String>)>,
    /// What this session ran (for flags when there is no recorded argv).
    pub seen: Vec<&'static str>,
    pub captured: Option<Captured>,
    pub preset: String,
    pub log: fn(&str),
}

/// A child pimaxd spawned exited: (pid, code, signal).
pub type Reaped = (u32, Option<i32>, Option<i32>);

/// Run a command to completion with a deadline; Err with the last output line on failure.
fn run(argv: &[String], timeout: Duration, ctx: &Ctx) -> Result<(), String> {
    (ctx.log)(&format!("run: {}", argv.join(" ")));
    let out = crate::exec::cmd_log_path();
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&out);
    let start_len = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    let (o1, o2) = match f.and_then(|f| Ok((f.try_clone()?, f))) {
        Ok((a, b)) => (Stdio::from(a), Stdio::from(b)),
        Err(_) => (Stdio::null(), Stdio::null()),
    };
    let mut c = Command::new(&argv[0]).args(&argv[1..]).env("PIMAX_WATCHDOG", "1").stdin(Stdio::null()).stdout(o1).stderr(o2).spawn().map_err(|e| format!("{}: {e}", argv[0]))?;
    let t0 = Instant::now();
    loop {
        match c.try_wait() {
            Ok(Some(st)) if st.success() => return Ok(()),
            Ok(Some(st)) => {
                let text = std::fs::read(&out).unwrap_or_default();
                let tail = String::from_utf8_lossy(text.get(start_len as usize..).unwrap_or(&[])).into_owned();
                let last = tail.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("").to_string();
                return Err(format!("{} exited with {st}: {last}", argv[..2.min(argv.len())].join(" ")));
            }
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(200)),
            Ok(None) => {
                // our own child (the launcher), not Monado: a TERM is fine, it never SIGKILLs Monado itself
                // SAFETY: signalling our own child.
                unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
                let _ = c.wait();
                return Err(format!("{} timed out after {:.0} s", argv[..2.min(argv.len())].join(" "), timeout.as_secs_f64()));
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

fn kind_word(m: Mode) -> &'static str {
    match m {
        Mode::Desk => "desk",
        Mode::Vr => "vr",
    }
}

/// The launcher start command of this session.
pub fn start_argv(ctx: &Ctx) -> Vec<String> {
    let bin = ctx.pimax_bin.display().to_string();
    let mut v = vec![bin];
    match &ctx.argv {
        Some((kind, args)) => {
            v.push(kind.clone());
            v.push("start".into());
            v.extend(args.iter().filter(|a| *a != "--wait").cloned());
        }
        None => {
            v.push(kind_word(ctx.mode).into());
            v.push("start".into());
            let has = |id: &str| ctx.seen.contains(&id);
            if has("broker") {
                v.push("--6dof".into());
                if has("ctrltrack") {
                    v.push("--ctrl-track".into());
                }
                if has("hands") {
                    v.push("--hands".into());
                }
            }
            if ctx.mode == Mode::Desk {
                if !has("passthrough") {
                    v.push("--no-passthrough".into());
                }
                if !has("perf") {
                    v.push("--no-perf".into());
                }
                if !has("screens") {
                    v.push("--screens".into());
                    v.push("1".into());
                }
                if has("voice.listen") {
                    v.push("--voice".into());
                    if has("voicehud") {
                        v.push("--voice-hud".into());
                    }
                }
            } else if has("perf") {
                v.push("--perf".into());
            }
        }
    }
    for f in ["--no-watchdog", "--no-notify"] {
        if !v.iter().any(|a| a == f) {
            v.push(f.into());
        }
    }
    v
}

/// The launcher stop command (never kills Monado).
pub fn stop_argv(ctx: &Ctx) -> Vec<String> {
    let kind = ctx.argv.as_ref().map(|a| a.0.clone()).unwrap_or_else(|| kind_word(ctx.mode).into());
    vec![ctx.pimax_bin.display().to_string(), kind, "stop".into(), "--grace".into(), "30".into(), "--no-kill".into()]
}

fn monado_alive(ctx: &Ctx) -> Option<u32> {
    procs::from_pid_file(&ctx.roots, &spec("monado").unwrap().pid.unwrap()).map(|i| i.pid)
}

fn stop_subtree(ctx: &Ctx) -> Outcome {
    let r = run(&stop_argv(ctx), Duration::from_secs(120), ctx);
    if let Some(pid) = monado_alive(ctx) {
        return Outcome::Hung(format!("monado-service pid {pid} still alive after `stop --no-kill`"));
    }
    match r {
        Ok(()) => Outcome::Ok,
        Err(e) => Outcome::Failed(e),
    }
}

/// Signal by pid and wait: SIGINT, then SIGTERM. Never SIGKILL.
fn stop_pid(pid: u32, grace: Duration, ctx: &Ctx, proc_: &std::path::Path) -> Outcome {
    let gone = |d: Duration| {
        let t0 = Instant::now();
        while t0.elapsed() < d {
            if !procs::same_alive(proc_, pid, None) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
        !procs::same_alive(proc_, pid, None)
    };
    for (sig, d) in [(libc::SIGINT, grace), (libc::SIGTERM, Duration::from_secs(5))] {
        (ctx.log)(&format!("signal {sig} to pid {pid}"));
        // SAFETY: signalling a pid we read from its pid file with the right comm.
        unsafe { libc::kill(pid as i32, sig) };
        if gone(d) {
            return Outcome::Ok;
        }
    }
    Outcome::Hung(format!("pid {pid} still alive after SIGINT and SIGTERM"))
}

/// Respawn a captured leaf, detached. Returns the pid.
fn respawn(id: &str, c: &Captured, ctx: &Ctx, reaped: &Sender<Reaped>) -> Result<u32, String> {
    let s = spec(id).ok_or("unknown")?;
    let ps = s.pid.ok_or("not a process")?;
    let stem = ps.file.trim_end_matches(".pid");
    let log = ctx.roots.session_dir.join(format!("{stem}.log"));
    let f = std::fs::OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let f2 = f.try_clone().map_err(|e| e.to_string())?;
    let (prog, args) = c.argv.split_first().ok_or("empty argv")?;
    let mut cmd = Command::new(prog);
    cmd.args(args).env_clear().envs(c.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::null()).stdout(f).stderr(f2);
    if let Some(d) = &c.cwd {
        cmd.current_dir(d);
    }
    // SAFETY: setsid in the child before exec (async-signal-safe).
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    let mut child = cmd.spawn().map_err(|e| format!("{prog}: {e}"))?;
    let pid = child.id();
    let _ = pimax_components::util::write_atomic(&procs::pid_path(&ctx.roots, &ps), format!("{pid}\n").as_bytes());
    (ctx.log)(&format!("respawned {id}: pid {pid}: {}", c.argv.join(" ")));
    let tx = reaped.clone();
    std::thread::spawn(move || {
        if let Ok(st) = child.wait() {
            let _ = tx.send((pid, st.code(), st.signal()));
        }
    });
    Ok(pid)
}

/// Execute one step (blocking; call from a worker thread).
pub fn execute(st: &Step, ctx: &Ctx, reaped: &Sender<Reaped>) -> Outcome {
    let res = |r: Result<(), String>| match r {
        Ok(()) => Outcome::Ok,
        Err(e) => Outcome::Failed(e),
    };
    match (&st.act, st.id) {
        (Act::Start, "monado") => {
            if monado_alive(ctx).is_some() {
                return Outcome::Ok;
            }
            // clients and the broker Monado's exit left behind go first (the launcher's own ordered stop, no kill)
            if let Err(e) = run(&stop_argv(ctx), Duration::from_secs(120), ctx) {
                (ctx.log)(&format!("pre-start cleanup: {e}"));
            }
            res(run(&start_argv(ctx), Duration::from_secs(240), ctx))
        }
        (Act::Start, "keepalive") => res(run(&["systemctl".into(), "--user".into(), "restart".into(), "pimax-keepalived.service".into()], Duration::from_secs(30), ctx)),
        (Act::Start, "ai.server") => {
            // `up` starts what ai.toml asks for: the simple model (same mode as last time) or, with two stages, the
            // router plus the action model fitted to the free VRAM next to this session (docs/AI.md 16)
            let mode = std::fs::read_to_string(ctx.roots.ai_state.join("llama-server.pid")).unwrap_or_default();
            // --wait 100: returns ("still loading") before this step's 120 s deadline
            let mut a = vec!["pimax-ai".to_string(), "up".into(), "--wait".into(), "100".into()];
            if mode.lines().any(|l| l.trim() == "mode=cpu") {
                a.push("--cpu".into());
            }
            res(run(&a, Duration::from_secs(120), ctx))
        }
        // the router and the VRAM watch go with the model
        (Act::Stop, "ai.server") => res(run(&["pimax-ai".to_string(), "down".into()], Duration::from_secs(30), ctx)),
        (Act::Start, id) => match &ctx.captured {
            Some(c) => match respawn(id, c, ctx, reaped) {
                Ok(pid) => {
                    std::thread::sleep(Duration::from_secs(1));
                    if procs::same_alive(&ctx.roots.proc_, pid, None) {
                        Outcome::Ok
                    } else {
                        Outcome::Failed(format!("{id} exited at once (see its log)"))
                    }
                }
                Err(e) => Outcome::Failed(e),
            },
            None => Outcome::Failed(format!("{id}: no captured command line (it never ran under pimaxd's eyes)")),
        },
        (Act::Stop, "monado") => stop_subtree(ctx),
        (Act::Stop, id) => {
            // the component module's own stop (Lane A, P3: SIGINT by pid, WayVR waited for, never SIGKILL)
            if let Some(c) = pimax_components::components::registry().into_iter().find(|c| c.id() == id) {
                let env = pimax_components::RealEnv::new(ctx.roots.clone());
                match c.stop(&env, Duration::from_secs(10)) {
                    pimax_components::StopOutcome::Stopped => return Outcome::Ok,
                    pimax_components::StopOutcome::Hung { pid } => return Outcome::Hung(format!("{id} pid {pid} still alive after SIGINT")),
                    pimax_components::StopOutcome::Failed(m) if !m.contains("no stop in this component module") => return Outcome::Failed(m),
                    pimax_components::StopOutcome::Failed(_) => {}
                }
            }
            let Some(ps) = spec(id).and_then(|s| s.pid) else { return Outcome::Failed("not a process".into()) };
            match procs::from_pid_file(&ctx.roots, &ps) {
                Some(i) => stop_pid(i.pid, Duration::from_secs(if id == "wayvr" { 15 } else { 10 }), ctx, &ctx.roots.proc_),
                None => Outcome::Ok,
            }
        }
        (Act::StopSubtree, _) => stop_subtree(ctx),
        (Act::ResetHeadset, _) => {
            if monado_alive(ctx).is_some() {
                match stop_subtree(ctx) {
                    Outcome::Ok => {}
                    other => return other,
                }
            }
            res(run(&[ctx.pimax_bin.display().to_string(), "vr".into(), "reset-headset".into()], Duration::from_secs(240), ctx))
        }
        (Act::Reapply, "preset") => {
            let p = ctx
                .argv
                .as_ref()
                .and_then(|(_, a)| a.iter().position(|x| x == "--preset").and_then(|i| a.get(i + 1).cloned()))
                .unwrap_or_else(|| ctx.preset.clone());
            if p == "none" {
                return Outcome::Ok;
            }
            res(run(&[ctx.pimax_bin.display().to_string(), "desk".into(), "preset".into(), p], Duration::from_secs(30), ctx))
        }
        _ => Outcome::Ok,
    }
}

/// Desktop notification plus, while WayVR runs, a headset toast (WayVR's XSOverlay-format UDP listener, the same
/// channel pimax-voice uses).
pub fn notify(text: &str, critical: bool, desktop: bool, headset: bool) {
    if desktop {
        let mut a = vec!["-a", "Pimax", "Pimax"];
        if critical {
            a.splice(0..0, ["-u", "critical"]);
        }
        a.push(text);
        let _ = Command::new("notify-send").args(&a).stdin(Stdio::null()).stdout(Stdio::null()).stderr(Stdio::null()).spawn().map(|mut c| std::thread::spawn(move || c.wait()));
    }
    if headset {
        let msg = format!(
            "{{\"messageType\":1,\"title\":\"Pimax\",\"content\":{},\"timeout\":{},\"volume\":-1.0}}",
            pimax_components::json::q(text),
            if critical { 8.0 } else { 4.0 }
        );
        if let Ok(s) = std::net::UdpSocket::bind("127.0.0.1:0") {
            let _ = s.send_to(msg.as_bytes(), "127.0.0.1:42069");
        }
    }
}

/// Sleep inhibitor: a `systemd-inhibit --what=sleep:idle --mode=block sleep infinity` child while a session runs.
#[derive(Default)]
pub struct Inhibit {
    child: Option<std::process::Child>,
}

impl Inhibit {
    pub fn set(&mut self, on: bool, log: fn(&str)) {
        if let Some(c) = &mut self.child {
            if !matches!(c.try_wait(), Ok(None)) {
                self.child = None;
            }
        }
        match (on, self.child.is_some()) {
            (true, false) => {
                let r = Command::new("systemd-inhibit")
                    .args(["--what=sleep:idle", "--who=pimaxd", "--why=VR session running", "--mode=block", "sleep", "infinity"])
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .spawn();
                match r {
                    Ok(c) => {
                        log(&format!("sleep inhibited while the session runs (systemd-inhibit pid {})", c.id()));
                        self.child = Some(c);
                    }
                    Err(e) => log(&format!("systemd-inhibit: {e}")),
                }
            }
            (false, true) => {
                if let Some(mut c) = self.child.take() {
                    // SAFETY: our own child.
                    unsafe { libc::kill(c.id() as i32, libc::SIGTERM) };
                    let _ = c.wait();
                    log("sleep inhibit released");
                }
            }
            _ => {}
        }
    }
    pub fn active(&self) -> bool {
        self.child.is_some()
    }
}

impl Drop for Inhibit {
    fn drop(&mut self) {
        self.set(false, |_| {});
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_components::util::TempDir;

    fn ctx(r: Roots) -> Ctx {
        Ctx { roots: r, pimax_bin: "/h/.local/bin/pimax".into(), mode: Mode::Desk, argv: None, seen: vec![], captured: None, preset: "cinema".into(), log: |_| {} }
    }

    #[test]
    fn launcher_commands() {
        let t = TempDir::new("exec");
        let mut c = ctx(Roots::under(t.path()));
        c.seen = vec!["broker", "ctrltrack", "passthrough", "voice.listen"];
        assert_eq!(start_argv(&c).join(" "), "/h/.local/bin/pimax desk start --6dof --ctrl-track --no-perf --screens 1 --voice --no-watchdog --no-notify");
        c.seen = vec!["broker", "hands", "passthrough", "perf", "screens"];
        assert_eq!(start_argv(&c).join(" "), "/h/.local/bin/pimax desk start --6dof --hands --no-watchdog --no-notify");
        c.seen = vec!["broker", "ctrltrack", "passthrough", "voice.listen"];
        c.argv = Some(("desk".into(), vec!["--6dof".into(), "--preset".into(), "work".into(), "--no-notify".into()]));
        assert_eq!(start_argv(&c).join(" "), "/h/.local/bin/pimax desk start --6dof --preset work --no-notify --no-watchdog");
        assert_eq!(stop_argv(&c).join(" "), "/h/.local/bin/pimax desk stop --grace 30 --no-kill");
        c.argv = None;
        c.mode = Mode::Vr;
        c.seen = vec!["perf"];
        assert_eq!(start_argv(&c).join(" "), "/h/.local/bin/pimax vr start --perf --no-watchdog --no-notify");
        assert_eq!(stop_argv(&c)[1], "vr");
    }

    #[test]
    fn respawn_writes_the_pid_file_and_reaps() {
        let t = TempDir::new("respawn");
        let r = Roots::under(t.path());
        std::fs::create_dir_all(&r.session_dir).unwrap();
        let c = ctx(r.clone());
        let cap = Captured { argv: vec!["sh".into(), "-c".into(), "echo hi; exit 3".into()], env: vec![("PATH".into(), std::env::var("PATH").unwrap_or_default())], cwd: None };
        let (tx, rx) = std::sync::mpsc::channel();
        let pid = respawn("perf", &cap, &c, &tx).unwrap();
        assert_eq!(procs::read_pid(&r.session_dir.join("perf.pid")), Some(pid));
        let (p, code, sig) = rx.recv_timeout(Duration::from_secs(5)).unwrap();
        assert_eq!((p, code, sig), (pid, Some(3), None));
        assert!(std::fs::read_to_string(r.session_dir.join("perf.log")).unwrap().contains("hi"));
    }

    #[test]
    fn stop_pid_uses_sigint_then_sigterm_only() {
        let t = TempDir::new("stoppid");
        let c = ctx(Roots::under(t.path()));
        // a child that ignores SIGINT but dies on SIGTERM
        let mut ch = Command::new("sh").args(["-c", "trap '' INT; sleep 30"]).spawn().unwrap();
        let pid = ch.id();
        let reaper = std::thread::spawn(move || ch.wait());
        std::thread::sleep(Duration::from_millis(300)); // the trap is installed
        let o = stop_pid(pid, Duration::from_millis(300), &c, std::path::Path::new("/proc"));
        assert_eq!(o, Outcome::Ok);
        assert_eq!(reaper.join().unwrap().unwrap().signal(), Some(libc::SIGTERM));
    }
}
