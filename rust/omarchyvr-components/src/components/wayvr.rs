//! WayVR, the desk's XR overlay client (moved from the launcher's `desk.rs`/`perf.rs` in P3): its spawn spec (the
//! desk one with screens, and the game-mode one that carries only the wrist panel) and its stop, which waits until it
//! has really exited before the caller goes on to Monado (resilience plan G9: Monado's teardown under a WayVR still
//! tearing down its swapchains raised Xid 31).
use super::proc::{stop_client, ClientStop, Clock, ProcSpec, Procs, POLL};
use crate::component::{Component, Env};
use crate::spec::Spec;
use std::path::Path;
use std::time::Duration;

/// First wait for WayVR's exit after SIGINT.
pub const WAYVR_GRACE: Duration = Duration::from_secs(15);
/// The second wait, after the "slow to exit" notice.
pub const WAYVR_EXTRA: Duration = Duration::from_secs(15);
/// [`end_hung_wayvr`]: wait after SIGTERM before SIGKILL.
pub const WAYVR_TERM_WAIT: Duration = Duration::from_secs(5);
/// [`end_hung_wayvr`]: wait after SIGKILL.
pub const WAYVR_KILL_WAIT: Duration = Duration::from_secs(3);

/// The XDG variables handed over as `OMARCHYVR_USER_<NAME>` (the same lists as `omarchyvr_config::paths::XDG_VARS` and
/// `USER_XDG_PREFIX`; a test in omarchyvr-tools keeps them equal).
pub const USER_XDG_VARS: [&str; 4] = ["XDG_CONFIG_HOME", "XDG_DATA_HOME", "XDG_STATE_HOME", "XDG_CACHE_HOME"];
pub const USER_XDG_PREFIX: &str = "OMARCHYVR_USER_";

/// What a WayVR process needs.
pub struct Params<'a> {
    pub bin: &'a Path,
    /// The isolated `XDG_*_HOME` root (`config/`, `data/`, `state/`).
    pub home: &'a Path,
    /// Directory with `libopenvr_api.so`, prepended to `LD_LIBRARY_PATH`.
    pub openvr_lib: Option<&'a Path>,
    pub runtime_json: &'a Path,
    pub session_dir: &'a Path,
    /// The directory of the `omarchyvr` tools, put first on WayVR's PATH: the watch's and the Work menu's buttons run
    /// `omarchyvr ...` / `omarchyvr-voice ...` through `sh -c`.
    pub tools_dir: Option<&'a Path>,
}

fn env_of(p: &Params) -> Vec<(String, String)> {
    let h = p.home;
    let mut env = super::proc::xr_env(p.runtime_json);
    env.extend([
        ("XDG_CONFIG_HOME".into(), h.join("config").to_string_lossy().into_owned()),
        ("XDG_DATA_HOME".into(), h.join("data").to_string_lossy().into_owned()),
        ("XDG_STATE_HOME".into(), h.join("state").to_string_lossy().into_owned()),
        ("RUST_LOG".into(), "info".into()),
    ]);
    // the user's own XDG directories, for the commands WayVR's buttons run (they inherit WayVR's isolated ones above;
    // `omarchyvr_config::paths::user_xdg_fix` puts these back): an empty value means the user had none
    for k in USER_XDG_VARS {
        env.push((format!("{USER_XDG_PREFIX}{k}"), std::env::var(k).unwrap_or_default()));
    }
    if let Some(t) = p.tools_dir {
        let old = std::env::var("PATH").unwrap_or_default();
        let val = if old.is_empty() { t.to_string_lossy().into_owned() } else { format!("{}:{old}", t.display()) };
        env.push(("PATH".into(), val));
    }
    if let Some(l) = p.openvr_lib {
        let old = std::env::var("LD_LIBRARY_PATH").unwrap_or_default();
        let val = if old.is_empty() { l.to_string_lossy().into_owned() } else { format!("{}:{old}", l.display()) };
        env.push(("LD_LIBRARY_PATH".into(), val));
    }
    env
}

/// Desk mode: `wayvr --openxr --show --no-autostart` with the screens.
pub fn desk_spec(p: &Params) -> ProcSpec {
    let dir = p.session_dir;
    ProcSpec {
        name: "wayvr",
        program: p.bin.to_path_buf(),
        args: vec![
            "--openxr".into(),
            "--show".into(),
            "--no-autostart".into(),
            "--log-to".into(),
            dir.join("wayvr.log").to_string_lossy().into_owned(),
        ],
        env: env_of(p),
        log: dir.join("wayvr.stdout"),
    }
}

/// Game mode (`vr start --perf`): the minimal headless WayVR that carries only the wrist panel.
pub fn perf_spec(p: &Params) -> ProcSpec {
    let dir = p.session_dir;
    ProcSpec {
        name: "perfwayvr",
        program: p.bin.to_path_buf(),
        args: vec![
            "--openxr".into(),
            "--headless".into(),
            "--show".into(),
            "--no-autostart".into(),
            "--log-to".into(),
            dir.join("perfwayvr.log").to_string_lossy().into_owned(),
        ],
        env: env_of(p),
        log: dir.join("perfwayvr.stdout"),
    }
}

/// SIGINT to WayVR (by pid) and wait until it has actually exited: up to [`WAYVR_GRACE`], then `slow` is told (a
/// notification) and it gets [`WAYVR_EXTRA`] more. Never SIGKILL.
pub fn stop_wayvr(
    name: &str,
    pid_file: &Path,
    comm: &str,
    procs: &dyn Procs,
    clock: &dyn Clock,
    info: &mut dyn FnMut(&str),
    slow: &mut dyn FnMut(&str),
) -> ClientStop {
    let r = stop_client(name, pid_file, comm, procs, clock, WAYVR_GRACE, info);
    if r != ClientStop::StillAlive {
        return r;
    }
    slow(&format!(
        "{name} is still running {:.0} s after SIGINT: waiting {:.0} s more before Monado is stopped",
        WAYVR_GRACE.as_secs_f64(),
        WAYVR_EXTRA.as_secs_f64()
    ));
    let Some(pid) = super::proc::read_pid(pid_file) else { return ClientStop::Exited };
    let t0 = clock.now();
    while clock.now().saturating_sub(t0) < WAYVR_EXTRA {
        if !procs.is_alive(pid) {
            info(&format!("{name}: exited after {:.1} s", (WAYVR_GRACE + clock.now().saturating_sub(t0)).as_secs_f64()));
            return ClientStop::Exited;
        }
        clock.sleep(POLL);
    }
    if procs.is_alive(pid) {
        ClientStop::StillAlive
    } else {
        ClientStop::Exited
    }
}

/// How [`end_hung_wayvr`] ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HungEnd {
    /// Already gone (or the pid is now another program): nothing sent.
    Gone,
    /// Exited on SIGTERM.
    Terminated,
    /// Exited on SIGKILL.
    Killed,
    /// Still there after SIGKILL (uninterruptible sleep in the driver): only a reboot clears it.
    Stuck,
    CannotSignal(String),
}

/// The last resort for a WayVR that outlived [`stop_wayvr`]'s wait (exit hang 2026-10-15: 100 % CPU in its
/// teardown, deaf to signals). Call it only once Monado is gone: its GPU objects can no longer race the
/// runtime's then. SIGTERM by pid (WayVR's second termination signal: it exits at once once its OpenXR objects
/// are released), [`WAYVR_TERM_WAIT`], then SIGKILL by pid, [`WAYVR_KILL_WAIT`]. Only `pid` while its comm is
/// still `comm`; never anything by name, never Monado. `warn` gets every step, for the user.
pub fn end_hung_wayvr(
    name: &str,
    pid: u32,
    comm: &str,
    procs: &dyn Procs,
    clock: &dyn Clock,
    warn: &mut dyn FnMut(&str),
) -> HungEnd {
    use super::proc::Signal;
    let ours = |p: u32| procs.is_alive(p) && procs.comm(p).as_deref() == Some(comm);
    if !ours(pid) {
        return HungEnd::Gone;
    }
    let wait = |d: Duration| {
        let t0 = clock.now();
        while clock.now().saturating_sub(t0) < d {
            if !procs.is_alive(pid) {
                return true;
            }
            clock.sleep(POLL);
        }
        !procs.is_alive(pid)
    };
    warn(&format!(
        "{name} (pid {pid}) is still running after its stop wait and Monado is gone: hung at exit; sending SIGTERM"
    ));
    match procs.signal(pid, Signal::Term) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return HungEnd::Gone,
        Err(e) => return HungEnd::CannotSignal(format!("SIGTERM to {name} pid {pid}: {e}")),
    }
    if wait(WAYVR_TERM_WAIT) {
        warn(&format!("{name} (pid {pid}) exited on SIGTERM"));
        return HungEnd::Terminated;
    }
    if !ours(pid) {
        return HungEnd::Gone;
    }
    warn(&format!(
        "{name} (pid {pid}) ignored SIGTERM for {:.0} s: sending SIGKILL (by pid)",
        WAYVR_TERM_WAIT.as_secs_f64()
    ));
    match procs.signal(pid, Signal::Kill) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return HungEnd::Gone,
        Err(e) => return HungEnd::CannotSignal(format!("SIGKILL to {name} pid {pid}: {e}")),
    }
    if wait(WAYVR_KILL_WAIT) {
        warn(&format!("{name} (pid {pid}) killed"));
        HungEnd::Killed
    } else {
        warn(&format!(
            "{name} (pid {pid}) is still there {:.0} s after SIGKILL (stuck in the kernel): reboot before the next session",
            WAYVR_KILL_WAIT.as_secs_f64()
        ));
        HungEnd::Stuck
    }
}

/// The `wayvr` component: its stop is [`stop_wayvr`] (the `grace` argument is ignored: WayVR always gets 15 + 15 s).
pub struct Wayvr(pub &'static Spec);

impl Component for Wayvr {
    fn spec(&self) -> &'static Spec {
        self.0
    }
    fn stop(&self, env: &dyn Env, _grace: Duration) -> crate::component::StopOutcome {
        use crate::component::StopOutcome as O;
        let r = env.roots();
        let Some(ps) = self.0.pid else { return O::Stopped };
        let pf = crate::procs::pid_path(r, &ps);
        let procs = super::proc::RealProcs::new(&r.proc_);
        let clock = super::proc::RealClock::new();
        let mut log = |m: &str| env.log(m);
        match stop_wayvr("wayvr", &pf, ps.comm, &procs, &clock, &mut |m| env.log(m), &mut log) {
            ClientStop::NotRunning | ClientStop::Exited => {
                let _ = std::fs::remove_file(&pf);
                O::Stopped
            }
            ClientStop::StillAlive => O::Hung { pid: super::proc::read_pid(&pf).unwrap_or(0) },
            ClientStop::CannotSignal(m) => O::Failed(m),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::proc::fake::*;
    use super::super::proc::Signal;
    use super::*;

    #[test]
    fn wayvr_is_waited_for_until_it_has_exited() {
        let d = std::env::temp_dir().join(format!("pimax-comp-wayvr-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        for (dies, want, slow) in [(Some(5), ClientStop::Exited, 0), (Some(22), ClientStop::Exited, 1), (None, ClientStop::StillAlive, 1)] {
            let clock = FakeClock::new();
            let procs = FakeProcs::new(&clock);
            procs.add(100, "wayvr", dies);
            let w = d.join("wayvr.pid");
            std::fs::write(&w, "100\n").unwrap();
            let mut said = 0;
            let r = stop_wayvr("wayvr", &w, "wayvr", &procs, &clock, &mut |_| {}, &mut |_| said += 1);
            assert_eq!(r, want, "{dies:?}");
            assert_eq!(said, slow, "{dies:?}");
            match dies {
                Some(t) => assert!(clock.now() >= Duration::from_secs(t) && clock.now() < Duration::from_secs(t + 1)),
                None => assert!(clock.now() >= WAYVR_GRACE + WAYVR_EXTRA),
            }
            assert_eq!(*procs.log.borrow(), vec![(100, Signal::Int)], "never SIGKILL");
        }
    }

    #[test]
    fn a_hung_wayvr_gets_sigterm_then_sigkill_by_pid() {
        // (SIGTERM effect, SIGKILL effect, outcome, signals sent)
        let cases: [(Option<u64>, Option<u64>, HungEnd, Vec<Signal>); 4] = [
            (Some(1), None, HungEnd::Terminated, vec![Signal::Term]),
            // the incident: deaf to SIGTERM, dies on SIGKILL
            (None, Some(0), HungEnd::Killed, vec![Signal::Term, Signal::Kill]),
            // D state: not even SIGKILL
            (None, None, HungEnd::Stuck, vec![Signal::Term, Signal::Kill]),
            (Some(0), None, HungEnd::Terminated, vec![Signal::Term]),
        ];
        for (term, kill, want, sent) in cases {
            let clock = FakeClock::new();
            let procs = FakeProcs::new(&clock);
            procs.add(100, "wayvr", None);
            procs.add(200, "monado-service", None);
            if let Some(t) = term {
                procs.dies_on(100, Signal::Term, t);
            }
            if let Some(k) = kill {
                procs.dies_on(100, Signal::Kill, k);
            }
            let mut said = vec![];
            let r = end_hung_wayvr("wayvr", 100, "wayvr", &procs, &clock, &mut |m| said.push(m.to_string()));
            assert_eq!(r, want, "{term:?} {kill:?}");
            let log = procs.log.borrow();
            assert!(log.iter().all(|(p, _)| *p == 100), "only the wayvr pid, never Monado: {log:?}");
            assert_eq!(log.iter().map(|x| x.1).collect::<Vec<_>>(), sent);
            assert!(said[0].contains("hung at exit"), "{said:?}");
            assert!(clock.now() <= WAYVR_TERM_WAIT + WAYVR_KILL_WAIT + POLL * 2, "{:?}", clock.now());
        }
        // gone already, or the pid is another program now: nothing is sent
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        procs.add(100, "bash", None);
        assert_eq!(end_hung_wayvr("wayvr", 100, "wayvr", &procs, &clock, &mut |_| {}), HungEnd::Gone);
        assert_eq!(end_hung_wayvr("wayvr", 101, "wayvr", &procs, &clock, &mut |_| {}), HungEnd::Gone);
        assert!(procs.log.borrow().is_empty());
    }

    #[test]
    fn specs() {
        let p = Params {
            bin: Path::new("/b/wayvr"),
            home: Path::new("/h"),
            openvr_lib: None,
            runtime_json: Path::new("/r.json"),
            session_dir: Path::new("/s"),
            tools_dir: Some(Path::new("/t/bin")),
        };
        let d = desk_spec(&p);
        assert_eq!(d.args, ["--openxr", "--show", "--no-autostart", "--log-to", "/s/wayvr.log"]);
        assert_eq!(d.env[0], ("XR_RUNTIME_JSON".into(), "/r.json".into()));
        assert_eq!(d.env[2], ("XDG_CONFIG_HOME".into(), "/h/config".into()));
        // the buttons' commands find the tools first and get the user's own XDG directories handed over
        let path = d.env.iter().find(|(k, _)| k == "PATH").map(|(_, v)| v.clone()).unwrap();
        assert!(path.starts_with("/t/bin"), "{path}");
        for k in USER_XDG_VARS {
            assert!(d.env.iter().any(|(e, _)| *e == format!("OMARCHYVR_USER_{k}")), "{k}");
        }
        assert_eq!(perf_spec(&p).args[1], "--headless");
        assert_eq!(perf_spec(&p).log, Path::new("/s/perfwayvr.stdout"));
    }
}
