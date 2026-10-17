//! The end of Work and the start of Play, in one place (docs/PLAY-WORK-AUDIT.md):
//!
//! * [`stop_ai`]: the local AI (every `omarchyvr-ai` slot: the model, Lara, the speech server on the graphics card, the
//!   agent model, and the VRAM watch), by its pid files. `desk stop` runs it, so Exit Work, the app's Stop and a switch
//!   to Play leave nothing of the AI behind; a restart of the session (`desk restart`, the watchdog) keeps it.
//! * [`clear_for_play`]: before Play starts anything, a crashed Work's leftovers (voice, the desk's clients, the VR
//!   screens' outputs) and the local AI are stopped, by pid only. Nothing of a running session is touched (a running
//!   Monado makes the start refuse anyway), never anything by name, never Sunshine or a process we did not start.
//! * [`survivors`]: after a full stop, what of ours still runs (pid files whose process is alive with its own name).
//! * [`exit`]: "Exit Work" from the headset (the Work menu's and the watch's button): the first press asks for a second
//!   one; the second starts a detached `omarchyvr desk stop` that outlives WayVR's own exit.
use super::config::{comm_of, StartOptions};
use super::desk::{self, DeskOptions};
use super::session::{self, Deps};
use crate::args::Args;
use omarchyvr_ai::{Dirs, Slot};
use std::path::{Path, PathBuf};

/// The order the AI slots go in (the big ones first).
const AI_SLOTS: [Slot; 4] = [Slot::Agent, Slot::Asr, Slot::Router, Slot::Main];

/// An AI slot in plain words.
pub fn slot_words(s: Slot) -> &'static str {
    match s {
        Slot::Main => "the local AI model (llama-server)",
        Slot::Router => "Lara, the command router (laya-serve)",
        Slot::Asr => "the speech server on the graphics card (whisper-server)",
        Slot::Agent => "the agent model (llama-server)",
    }
}

/// What [`stop_ai`] would stop now (dry runs).
pub fn ai_stop_note(dirs: &Dirs) -> String {
    let running: Vec<String> = AI_SLOTS.iter().filter_map(|s| omarchyvr_ai::running_slot(dirs, *s).map(|p| format!("{} pid {}", slot_words(*s), p.pid))).collect();
    if running.is_empty() {
        "nothing runs".into()
    } else {
        running.join(", ")
    }
}

/// Stop the local AI: every `omarchyvr-ai` slot and its VRAM watch (`omarchyvr_ai::twostage::down`: the pid files, the
/// command line checked, SIGTERM, SIGKILL after 5 s for a server that does not exit). Lines in plain words.
pub fn stop_ai(dirs: &Dirs) -> Vec<String> {
    match omarchyvr_ai::twostage::down(dirs) {
        Ok(v) => v.into_iter().map(|(s, pid)| format!("local AI: {} stopped (pid {pid})", slot_words(s))).collect(),
        Err(e) => vec![format!("local AI: could not be stopped: {e}")],
    }
}

/// A desk process from its pid file, when it is alive with the expected name.
fn alive(dir: &Path, name: &str, comm: &str, d: &Deps) -> Option<u32> {
    let pid = session::read_pid(&dir.join(format!("{name}.pid")))?;
    (d.procs.is_alive(pid) && d.procs.comm(pid).as_deref() == Some(comm)).then_some(pid)
}

/// What of ours still runs after a stop: the desk's processes (pid files), the camera broker, Monado, and (unless
/// `keep_ai`) the AI slots. One plain line each, `name (pid N)`; Monado's line starts with `monado`.
pub fn survivors(o: &DeskOptions, d: &Deps, dirs: &Dirs, keep_ai: bool) -> Vec<String> {
    let dir = &o.vr.session_dir;
    let mut v = Vec::new();
    for (name, bin) in desk::stop_order(o) {
        if let Some(pid) = alive(dir, name, &comm_of(bin), d) {
            v.push(format!("{name} (pid {pid})"));
        }
    }
    let cam = o.sixdof.as_ref().map(|s| s.cam_bin.clone()).unwrap_or_else(|| super::config::tools_dir(&PathBuf::from((d.env)("HOME").unwrap_or_default())).join("pimax-cam"));
    if let Some(pid) = alive(dir, "broker", &comm_of(&cam), d) {
        v.push(format!("camera broker (pid {pid})"));
    }
    for pid in session::stop_targets(&o.vr, d.procs).ours {
        v.push(format!("monado-service (pid {pid})"));
    }
    if !keep_ai {
        for s in AI_SLOTS {
            if let Some(p) = omarchyvr_ai::running_slot(dirs, s) {
                v.push(format!("{} (pid {})", slot_words(s), p.pid));
            }
        }
        if let Some(pid) = omarchyvr_ai::twostage::watch_running(dirs) {
            v.push(format!("the local AI's VRAM watch (pid {pid})"));
        }
    }
    v
}

/// What [`clear_for_play`] did: lines for the start's output, and the entries of the app's "Stopped for Play" line
/// (`ai`, `voice`, `desk`; see `gamemode::record_stopped`).
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Cleared {
    pub lines: Vec<String>,
    pub entries: Vec<(&'static str, String)>,
}

/// Before Play starts: nothing of Work may run next to the game. When no monado-service runs (a running one is a
/// session: never touched here, and the start refuses it), the desk's leftovers of a crashed Work (voice, the leaves,
/// WayVR, passthrough) get the desk's own polite stop by pid, the local AI goes ([`stop_ai`]), and the VR screens'
/// outputs left behind are removed. Sunshine and processes we did not start are never touched.
pub fn clear_for_play(vr: &StartOptions, d: &Deps, dirs: &Dirs, hypr: &mut dyn super::screens::Hypr) -> Cleared {
    let mut c = Cleared::default();
    if !d.procs.find_by_comm(&vr.service_comm()).is_empty() {
        return c;
    }
    // the desk's processes with the desk's own names and stop order (a default desk start's binaries)
    if let Ok(mut o) = desk::parse_desk(&mut Args(Vec::new()), d.env) {
        o.vr.session_dir = vr.session_dir.clone();
        let dir = o.vr.session_dir.clone();
        let found: Vec<(&'static str, u32)> = desk::stop_order(&o).into_iter().filter_map(|(n, b)| alive(&dir, n, &comm_of(b), d).map(|p| (n, p))).collect();
        if !found.is_empty() {
            let (problems, hung) = desk::stop_clients(&o, d);
            // no Monado runs: a WayVR hung at exit is ended now (SIGTERM, then SIGKILL, by pid)
            desk::end_hung_wayvrs(&hung, &[], &vr.service_comm(), "omarchyvr vr", d);
            for (n, pid) in &found {
                let kind = if n.starts_with("voice") { "voice" } else { "desk" };
                c.entries.push((kind, format!("{n} pid {pid}")));
                c.lines.push(format!("stopped {n} (pid {pid}), left from Work"));
            }
            c.lines.extend(problems.into_iter().map(|p| format!("WARNING: {p}")));
        }
    }
    for s in AI_SLOTS {
        if let Some(p) = omarchyvr_ai::running_slot(dirs, s) {
            c.entries.push(("ai", format!("{} pid {}", slot_words(s), p.pid)));
        }
    }
    c.lines.extend(stop_ai(dirs));
    let r = super::screens::remove_all(hypr, false, false);
    if !r.removed.is_empty() {
        c.lines.push(format!("removed the VR screens left from Work: {}", r.removed.join(", ")));
        c.entries.push(("desk", format!("VR screens {}", r.removed.join(","))));
    }
    if let Some(e) = r.error.filter(|_| !r.removed.is_empty()) {
        c.lines.push(format!("WARNING: VR screens: {e}"));
    }
    c
}

/// The marker of a first Exit Work press (`<session dir>/exit-armed`, unix milliseconds).
pub const EXIT_ARMED: &str = "exit-armed";
/// The second press must come within this.
pub const EXIT_CONFIRM_MS: u64 = 6000;

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// A toast in the headset (WayVR's XSOverlay listener, UDP 127.0.0.1:42069); best effort.
pub fn toast(title: &str, text: &str) {
    let msg = serde_json::json!({ "messageType": 1, "title": title, "content": text, "timeout": 5.0, "volume": -1.0 }).to_string();
    if let Ok(s) = std::net::UdpSocket::bind("127.0.0.1:0") {
        let _ = s.send_to(msg.as_bytes(), "127.0.0.1:42069");
    }
}

/// `desk exit [--now]`: Exit Work from the headset. Only while a Work session runs. The first press arms (a toast:
/// "press again"); a second press within [`EXIT_CONFIRM_MS`] (or `now`) runs `spawn` with the session dir, which starts
/// the full stop detached from WayVR ([`spawn_detached_stop`]). Returns the line for the output.
pub fn exit(o: &DeskOptions, now: bool, d: &Deps, spawn: &mut dyn FnMut(&Path) -> std::io::Result<u32>) -> Result<String, String> {
    let dir = &o.vr.session_dir;
    let kind = super::supervise::read_argv(dir).map(|k| k.0);
    let wayvr_up = alive(dir, "wayvr", &comm_of(&o.wayvr_bin), d).is_some();
    let monado_up = !session::stop_targets(&o.vr, d.procs).ours.is_empty();
    if kind.as_deref() != Some("desk") || !(wayvr_up || monado_up) {
        return Err("No Work session is running: nothing to exit".into());
    }
    let marker = dir.join(EXIT_ARMED);
    let t = unix_ms();
    let armed = std::fs::read_to_string(&marker).ok().and_then(|x| x.trim().parse::<u64>().ok()).is_some_and(|at| t.saturating_sub(at) <= EXIT_CONFIRM_MS);
    if !now && !armed {
        let _ = std::fs::write(&marker, format!("{t}\n"));
        toast("Exit Work?", "Press Exit Work again to stop everything: screens, voice, AI.");
        return Ok("Exit Work: press it again within 6 s to stop everything".into());
    }
    let _ = std::fs::remove_file(&marker);
    toast("Exiting Work", "Stopping everything: screens, voice, AI. The headset goes dark in a few seconds.");
    let pid = spawn(dir).map_err(|e| format!("could not start the stop: {e}"))?;
    Ok(format!("Exit Work: stopping everything (pid {pid}, log {})", dir.join("exit.log").display()))
}

/// The real `spawn` of [`exit`]: `omarchyvr desk stop --grace 30` in a session of its own (it must outlive WayVR, which
/// ran this command from its button and is the first thing the stop ends), output appended to `<dir>/exit.log`, the
/// user's own XDG directories (a WayVR button gives WayVR's isolated ones), no watchdog flag.
pub fn spawn_detached_stop(dir: &Path) -> std::io::Result<u32> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    std::fs::create_dir_all(dir)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("exit.log"))?;
    let log2 = log.try_clone()?;
    let mut c = std::process::Command::new(exe);
    c.args(["desk", "stop", "--grace", "30", "--session-dir"]).arg(dir);
    c.stdin(std::process::Stdio::null()).stdout(log).stderr(log2).env_remove("PIMAX_WATCHDOG");
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    for (k, v) in omarchyvr_config::paths::user_xdg_fix(&|k| std::env::var(k).ok(), &home) {
        match v {
            Some(v) => {
                c.env(k, v);
            }
            None => {
                c.env_remove(k);
            }
        }
    }
    // SAFETY: setsid is async-signal-safe.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    Ok(c.spawn()?.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::host::{NotifySend, RealClock, RealHost};
    use crate::vr::screens::Hypr;
    use std::process::{Child, Command};
    use std::time::Duration;

    fn bin_of(names: &[&str]) -> String {
        names.iter().find(|p| Path::new(p).exists()).map(|s| s.to_string()).expect("coreutils")
    }

    /// Our own test children: killed and reaped at the end whatever happens.
    struct Kids(Vec<Child>);
    impl Drop for Kids {
        fn drop(&mut self) {
            for c in &mut self.0 {
                let _ = c.kill();
                let _ = c.wait();
            }
        }
    }

    /// `hyprctl` with the VR screens of a Work session; records every call (nothing reaches the real compositor).
    struct FakeHypr(Vec<String>);
    impl Hypr for FakeHypr {
        fn run(&mut self, args: &[&str]) -> Result<String, String> {
            self.0.push(args.join(" "));
            let m = |name: &str, x: i64| serde_json::json!({"name": name, "width": 1920, "height": 1080, "x": x, "y": 0, "focused": x == 0, "disabled": false, "mirrorOf": "none", "activeWorkspace": {"id": 1, "name": "1"}});
            match args {
                ["-j", "monitors", "all"] => Ok(serde_json::json!([m("SUNSHINE", 0), m("PIMAX-LEFT", -1920), m("PIMAX-RIGHT", 1920)]).to_string()),
                ["output", "remove", _] => Ok("ok".into()),
                _ => Err(format!("unexpected hyprctl {args:?}")),
            }
        }
        fn sleep(&mut self, _d: Duration) {}
    }

    /// A fake Work session: processes named like the real binaries (symlinks to sleep / tail) with their pid files in a
    /// temp session dir, the AI slots' pid files in temp AI dirs, and two processes we did NOT start (no pid file).
    struct World {
        base: PathBuf,
        sess: PathBuf,
        home: PathBuf,
        dirs: Dirs,
        _kids: Kids,
        /// (what, pid) of every process the stop must end
        ours: Vec<(String, u32)>,
        /// processes of the user that are not ours (must survive)
        foreign: Vec<u32>,
        host: RealHost,
    }

    impl World {
        fn new(tag: &str, desk_clients: bool, monado: bool) -> World {
            let base = crate::vr::test_dir(tag);
            let (sess, home, bin) = (base.join("sess"), base.join("home"), base.join("bin"));
            for d in [&sess, &home, &bin] {
                std::fs::create_dir_all(d).unwrap();
            }
            let dirs = Dirs { data: base.join("ai-data"), state: base.join("ai-state"), logs: base.join("ai-logs") };
            std::fs::create_dir_all(&dirs.state).unwrap();
            let (sleep, tail) = (bin_of(&["/usr/bin/sleep", "/bin/sleep"]), bin_of(&["/usr/bin/tail", "/bin/tail"]));
            std::fs::write(base.join("watch"), "").unwrap();
            let mut kids = Kids(Vec::new());
            let mut ours = Vec::new();
            let mut spawn = |link: &str, target: &str, args: &[&str]| -> u32 {
                let p = bin.join(link);
                if !p.exists() {
                    std::os::unix::fs::symlink(target, &p).unwrap();
                }
                let c = Command::new(&p).args(args).current_dir(&base).spawn().unwrap();
                let pid = c.id();
                kids.0.push(c);
                pid
            };
            let put = |file: &Path, text: String| std::fs::write(file, text).unwrap();
            if monado {
                // like Monado, it catches SIGINT (the stop checks for the handler before it sends one)
                let sh = bin_of(&["/usr/bin/sh", "/bin/sh"]);
                let pid = spawn("monado-service", &sh, &["-c", "trap 'exit 0' INT TERM; while :; do sleep 0.2; done"]);
                put(&sess.join("monado.pid"), format!("{pid}\n"));
                ours.push(("monado".to_string(), pid));
            }
            if desk_clients {
                for (file, link) in [
                    ("wayvr", "wayvr"),
                    ("passthrough", "omarchyvr-passthrough"),
                    ("voice", "omarchyvr-voice"),
                    ("voicecap", "omarchyvr-voice"),
                    ("voiceact", "omarchyvr-voice"),
                    // the HUD names itself omarchyvr-vhud (components::proc::VOICE_HUD_COMM)
                    ("voicehud", "omarchyvr-vhud"),
                    ("perf", "omarchyvr-perf"),
                    ("screens", "omarchyvr"),
                    ("ctrltrack", "pimax-ctrltrack"),
                    ("broker", "pimax-cam"),
                ] {
                    let pid = spawn(link, &sleep, &["300"]);
                    put(&sess.join(format!("{file}.pid")), format!("{pid}\n"));
                    ours.push((file.to_string(), pid));
                }
            }
            // the local AI: the model, Lara, the speech server on the GPU, the agent model, the VRAM watch
            for (slot, link) in [(Slot::Main, "llama-server"), (Slot::Router, "laya-serve"), (Slot::Asr, "whisper-server"), (Slot::Agent, "llama-server")] {
                let pid = spawn(link, &sleep, &["300"]);
                put(&dirs.slot_pid(slot), format!("{pid}\nport={}\nmode=gpu (all layers)\nmodel=m.gguf\n", slot.port()));
                ours.push((slot.id().to_string(), pid));
            }
            let pid = spawn("omarchyvr-ai", &tail, &["-f", "watch"]);
            put(&omarchyvr_ai::twostage::watch_pid(&dirs), format!("{pid}\n"));
            ours.push(("vram-watch".to_string(), pid));
            // not ours: a streaming server and a WayVR the user runs by hand (no pid files): never touched
            let foreign = vec![spawn("sunshine", &sleep, &["300"]), spawn("wayvr", &sleep, &["301"])];
            std::fs::write(sess.join("start-argv.txt"), "desk\n--6dof\n--voice\n").unwrap();
            let journal = base.join("journalctl");
            std::fs::write(&journal, "#!/bin/sh\nexit 0\n").unwrap();
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&journal, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let host = RealHost {
                sysfs: base.join("sys"),
                proc_root: "/proc".into(),
                // SAFETY: getuid cannot fail.
                uid: unsafe { libc::getuid() },
                service_comm: "monado-service".into(),
                journalctl: journal.to_string_lossy().into_owned(),
                session_dir: sess.clone(),
            };
            World { base, sess, home, dirs, _kids: kids, ours, foreign, host }
        }

        fn env(&self) -> impl Fn(&str) -> Option<String> + '_ {
            move |k: &str| match k {
                "HOME" => Some(self.home.to_string_lossy().into_owned()),
                "PIMAX_VR_SESSION_DIR" => Some(self.sess.to_string_lossy().into_owned()),
                _ => None,
            }
        }

        fn desk(&self, env: &dyn Fn(&str) -> Option<String>) -> DeskOptions {
            let mut a = Args(vec!["--session-dir".into(), self.sess.to_string_lossy().into_owned()]);
            desk::parse_desk(&mut a, env).unwrap()
        }

        /// Alive and not a zombie (a dead child this test has not reaped yet counts as gone).
        fn alive(&self, pid: u32) -> bool {
            std::fs::read_to_string(format!("/proc/{pid}/stat")).is_ok_and(|s| !s.rsplit(") ").next().is_some_and(|r| r.starts_with('Z')))
        }
    }

    /// The rule for every test that runs processes: never next to a live VR session. It also serialises these tests:
    /// a fake monado-service of one test is a running session for another (the cleanup looks by name for that).
    fn no_vr_session() -> Option<std::sync::MutexGuard<'static, ()>> {
        static ONE: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let g = ONE.lock().unwrap_or_else(|e| e.into_inner());
        Command::new("pgrep").args(["-x", "monado-service"]).output().map(|o| o.stdout.is_empty()).unwrap_or(true).then_some(g)
    }

    #[test]
    fn exit_work_stops_every_tracked_component_and_nothing_else() {
        let Some(_one) = no_vr_session() else {
            eprintln!("skipped: a monado-service runs on this machine");
            return;
        };
        let w = World::new("work-full-stop", true, true);
        let env = w.env();
        let (clock, notifier) = (RealClock::new(), NotifySend::new(false));
        let deps = Deps { host: &w.host, procs: &w.host, notifier: &notifier, clock: &clock, env: &env, stop_requested: &|| false };
        let o = w.desk(&env);
        let before = survivors(&o, &deps, &w.dirs, false);
        assert_eq!(before.len(), w.ours.len(), "every fake process is seen as ours: {before:?}");
        let mut hypr = FakeHypr(Vec::new());
        desk::stop_with(&o, Duration::from_secs(5), true, false, &deps, &w.dirs, &mut hypr).unwrap();
        for (what, pid) in &w.ours {
            assert!(!w.alive(*pid), "{what} (pid {pid}) still runs after Exit Work");
        }
        for pid in &w.foreign {
            assert!(w.alive(*pid), "pid {pid} was not started by us and must not be touched");
        }
        assert!(survivors(&o, &deps, &w.dirs, false).is_empty());
        for s in AI_SLOTS {
            assert!(!w.dirs.slot_pid(s).exists(), "{} pid file left", s.id());
        }
        assert!(hypr.0.iter().any(|c| c == "output remove PIMAX-LEFT") && hypr.0.iter().any(|c| c == "output remove PIMAX-RIGHT"), "{:?}", hypr.0);
        assert!(!hypr.0.iter().any(|c| c.starts_with("output") && c.contains("SUNSHINE")), "the desktop's own output is never removed");
        let _ = std::fs::remove_dir_all(&w.base);
    }

    #[test]
    fn a_restart_keeps_the_local_ai_loaded() {
        let Some(_one) = no_vr_session() else { return };
        let w = World::new("work-restart", true, true);
        let env = w.env();
        let (clock, notifier) = (RealClock::new(), NotifySend::new(false));
        let deps = Deps { host: &w.host, procs: &w.host, notifier: &notifier, clock: &clock, env: &env, stop_requested: &|| false };
        let o = w.desk(&env);
        desk::stop_with(&o, Duration::from_secs(5), true, true, &deps, &w.dirs, &mut FakeHypr(Vec::new())).unwrap();
        for (what, pid) in &w.ours {
            let ai = ["main", "router", "asr", "agent", "vram-watch"].contains(&what.as_str());
            assert_eq!(w.alive(*pid), ai, "{what}: the AI stays across a restart, the rest stops");
        }
        let _ = std::fs::remove_dir_all(&w.base);
    }

    #[test]
    fn play_clears_a_crashed_works_leftovers_and_the_ai_but_never_a_running_session() {
        let Some(_one) = no_vr_session() else { return };
        // a crashed Work: its Monado is gone; voice, WayVR, the leaves and the AI still run
        let w = World::new("work-clear-play", true, false);
        let env = w.env();
        let (clock, notifier) = (RealClock::new(), NotifySend::new(false));
        let deps = Deps { host: &w.host, procs: &w.host, notifier: &notifier, clock: &clock, env: &env, stop_requested: &|| false };
        let o = w.desk(&env);
        let mut hypr = FakeHypr(Vec::new());
        let c = clear_for_play(&o.vr, &deps, &w.dirs, &mut hypr);
        for (what, pid) in &w.ours {
            // the camera broker is not Work's (a tool may use it): the preflight names it instead
            if what != "broker" {
                assert!(!w.alive(*pid), "{what} (pid {pid}) must not run next to a game");
            }
        }
        for pid in &w.foreign {
            assert!(w.alive(*pid), "not ours: untouched");
        }
        let kinds: Vec<&str> = c.entries.iter().map(|e| e.0).collect();
        assert!(kinds.contains(&"ai") && kinds.contains(&"voice") && kinds.contains(&"desk"), "{c:?}");
        assert!(c.lines.iter().any(|l| l.contains("Lara")) && c.lines.iter().any(|l| l.contains("whisper-server")), "{:?}", c.lines);
        assert!(hypr.0.iter().any(|x| x == "output remove PIMAX-LEFT"));
        let _ = std::fs::remove_dir_all(&w.base);

        // a running session (its Monado is alive): nothing is touched, the start refuses it by itself
        let w = World::new("work-clear-play-running", true, true);
        let env = w.env();
        let deps = Deps { host: &w.host, procs: &w.host, notifier: &notifier, clock: &clock, env: &env, stop_requested: &|| false };
        let o = w.desk(&env);
        let mut hypr = FakeHypr(Vec::new());
        assert_eq!(clear_for_play(&o.vr, &deps, &w.dirs, &mut hypr), Cleared::default());
        assert!(w.ours.iter().all(|(_, p)| w.alive(*p)), "a running session is never touched");
        assert!(hypr.0.is_empty());
        let _ = std::fs::remove_dir_all(&w.base);
    }

    #[test]
    fn exit_work_asks_for_a_second_press_and_only_in_work() {
        let Some(_one) = no_vr_session() else { return };
        let w = World::new("work-exit", true, true);
        let env = w.env();
        let (clock, notifier) = (RealClock::new(), NotifySend::new(false));
        let deps = Deps { host: &w.host, procs: &w.host, notifier: &notifier, clock: &clock, env: &env, stop_requested: &|| false };
        let o = w.desk(&env);
        let mut spawned: Vec<PathBuf> = Vec::new();
        let mut spawn = |d: &Path| {
            spawned.push(d.to_path_buf());
            Ok(4242)
        };
        let l = exit(&o, false, &deps, &mut spawn).unwrap();
        assert!(l.contains("again"), "{l}");
        assert!(w.sess.join(EXIT_ARMED).exists());
        let l = exit(&o, false, &deps, &mut spawn).unwrap();
        assert!(l.contains("stopping everything") && l.contains("4242"), "{l}");
        assert!(!w.sess.join(EXIT_ARMED).exists());
        // --now: no question
        exit(&o, true, &deps, &mut spawn).unwrap();
        // an old first press (more than 6 s ago) asks again
        std::fs::write(w.sess.join(EXIT_ARMED), "1000\n").unwrap();
        assert!(exit(&o, false, &deps, &mut spawn).unwrap().contains("again"));
        // Play is not Work: refused
        std::fs::write(w.sess.join("start-argv.txt"), "vr\n--6dof\n").unwrap();
        assert!(exit(&o, true, &deps, &mut spawn).unwrap_err().contains("No Work session"));
        assert_eq!(spawned, vec![w.sess.clone(), w.sess.clone()], "the stop is started on the second press and with --now only");
        let _ = std::fs::remove_dir_all(&w.base);
    }

    #[test]
    fn the_handover_lists_match_the_config_crate() {
        use omarchyvr_components::components::wayvr::{USER_XDG_PREFIX, USER_XDG_VARS};
        assert_eq!(USER_XDG_VARS, omarchyvr_config::paths::XDG_VARS);
        assert_eq!(USER_XDG_PREFIX, omarchyvr_config::paths::USER_XDG_PREFIX);
    }
}
