//! Process mechanics shared by the process components (moved from the launcher, `omarchyvr-tools/src/vr/{host,desk}.rs`,
//! in P3): the process-table and clock traits, the spawn spec, the detached spawner, the polite stop by pid (SIGINT,
//! never by name, never SIGKILL for a client) and the settle wait. The launcher (`omarchyvr desk|vr`) and the component
//! types in this module tree use the same code.
use std::fs::File;
use std::io;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Monotonic time with an injectable sleep, so waiting loops can be tested without waiting.
pub trait Clock {
    /// Time since an arbitrary start.
    fn now(&self) -> Duration;
    /// Block (or, in tests, advance the fake time).
    fn sleep(&self, d: Duration);
}

/// Wall clock.
pub struct RealClock {
    start: Instant,
}

impl RealClock {
    /// Start counting now.
    pub fn new() -> Self {
        Self { start: Instant::now() }
    }
}

impl Default for RealClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.start.elapsed()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// Signals the stack sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Signal {
    /// Polite stop: Monado shuts the display lease down cleanly.
    Int,
    /// Second termination signal (WayVR exits at once on it once its OpenXR objects are released).
    Term,
    /// Last resort (explicit user stops of Monado only).
    Kill,
}

/// Process table access.
pub trait Procs {
    /// True while the process exists and is not a zombie.
    fn is_alive(&self, pid: u32) -> bool;
    /// `/proc/PID/comm`, if the process exists.
    fn comm(&self, pid: u32) -> Option<String>;
    /// Send a signal.
    fn signal(&self, pid: u32, sig: Signal) -> io::Result<()>;
    /// Pids of processes of this user whose comm is `comm`.
    fn find_by_comm(&self, comm: &str) -> Vec<u32>;
    /// Environment of the process (`/proc/PID/environ`), if readable.
    fn environ(&self, pid: u32) -> Option<Vec<(String, String)>>;
    /// Seconds since the process started, if known.
    fn age_secs(&self, pid: u32) -> Option<f64>;
    /// The executable of the process (`/proc/PID/exe`), for messages.
    fn exe(&self, _pid: u32) -> Option<String> {
        None
    }
    /// Whether the process still has a SIGINT handler installed (`SigCgt` of `/proc/PID/status`). Monado installs its
    /// shutdown handler with SA_RESETHAND: once this is false a SIGINT has already been delivered and a second one
    /// would kill the service without any cleanup. `None` if unknown.
    fn catches_sigint(&self, _pid: u32) -> Option<bool> {
        None
    }
    /// Save evidence of what a hung process is doing (stack dump, thread states, log copy). Returns a sentence naming
    /// the files. Best effort and time-bounded; the default saves nothing.
    fn capture_hang(&self, _pid: u32) -> Option<String> {
        None
    }
}

/// The real process table under a `/proc` root (`PIMAX_PROC_ROOT` in tests). Only processes of `uid` are found by name.
pub struct RealProcs {
    pub proc_root: PathBuf,
    pub uid: u32,
}

impl RealProcs {
    pub fn new(proc_root: &Path) -> Self {
        // SAFETY: getuid has no arguments and cannot fail.
        RealProcs { proc_root: proc_root.to_path_buf(), uid: unsafe { libc::getuid() } }
    }
    fn dir(&self, pid: u32) -> PathBuf {
        self.proc_root.join(pid.to_string())
    }
}

impl Procs for RealProcs {
    fn is_alive(&self, pid: u32) -> bool {
        // /proc/PID/stat: "pid (comm) S ..." where comm may contain spaces and parentheses
        let Ok(s) = std::fs::read_to_string(self.dir(pid).join("stat")) else {
            return false;
        };
        match s.rfind(')').and_then(|i| s[i + 1..].trim_start().chars().next()) {
            Some('Z') | Some('X') | None => false,
            Some(_) => true,
        }
    }

    fn comm(&self, pid: u32) -> Option<String> {
        std::fs::read_to_string(self.dir(pid).join("comm")).ok().map(|s| s.trim().to_string())
    }

    fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
        let signo = match sig {
            Signal::Int => libc::SIGINT,
            Signal::Term => libc::SIGTERM,
            Signal::Kill => libc::SIGKILL,
        };
        // SAFETY: plain syscall; the pid comes from a pid file or a /proc scan and was checked by comm.
        let rc = unsafe { libc::kill(pid as libc::pid_t, signo) };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }

    fn find_by_comm(&self, comm: &str) -> Vec<u32> {
        let Ok(rd) = std::fs::read_dir(&self.proc_root) else {
            return vec![];
        };
        let mut v: Vec<u32> = rd
            .flatten()
            .filter_map(|e| {
                let pid: u32 = e.file_name().to_str()?.parse().ok()?;
                let owner = e.metadata().ok()?.uid();
                // only our own processes; fake /proc trees in tests are owned by the test user as well
                (owner == self.uid && self.is_alive(pid) && self.comm(pid).as_deref() == Some(comm)).then_some(pid)
            })
            .collect();
        v.sort_unstable();
        v
    }

    fn environ(&self, pid: u32) -> Option<Vec<(String, String)>> {
        let raw = std::fs::read(self.dir(pid).join("environ")).ok()?;
        Some(
            raw.split(|b| *b == 0)
                .filter_map(|kv| {
                    let s = String::from_utf8_lossy(kv);
                    let (k, v) = s.split_once('=')?;
                    Some((k.to_string(), v.to_string()))
                })
                .collect(),
        )
    }

    fn exe(&self, pid: u32) -> Option<String> {
        std::fs::read_link(self.dir(pid).join("exe")).ok().map(|p| p.display().to_string())
    }

    fn catches_sigint(&self, pid: u32) -> Option<bool> {
        let status = std::fs::read_to_string(self.dir(pid).join("status")).ok()?;
        let line = status.lines().find(|l| l.starts_with("SigCgt:"))?;
        let mask = u64::from_str_radix(line.split_whitespace().nth(1)?, 16).ok()?;
        // signal n is bit n-1; SIGINT is 2
        Some(mask & (1 << (libc::SIGINT - 1)) != 0)
    }

    fn age_secs(&self, pid: u32) -> Option<f64> {
        let stat = std::fs::read_to_string(self.dir(pid).join("stat")).ok()?;
        let rest = &stat[stat.rfind(')')? + 2..];
        // fields after comm: state is index 0, starttime (field 22 overall) is index 19
        let start_ticks: f64 = rest.split(' ').nth(19)?.parse().ok()?;
        let uptime: f64 =
            std::fs::read_to_string(self.proc_root.join("uptime")).ok()?.split_whitespace().next()?.parse().ok()?;
        // SAFETY: sysconf has no memory arguments.
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        Some((uptime - start_ticks / hz).max(0.0))
    }
}

/// A process to launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProcSpec {
    /// Short name, also the pid file stem (`passthrough`, `wayvr`).
    pub name: &'static str,
    /// Executable.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// Environment added to the caller's.
    pub env: Vec<(String, String)>,
    /// stdout+stderr go here.
    pub log: PathBuf,
}

/// Starts processes (faked in tests).
pub trait Spawner {
    /// Start detached; returns the pid.
    fn spawn(&self, spec: &ProcSpec) -> io::Result<u32>;
}

/// `Command` with its own session, stdin /dev/null and the log as stdout+stderr (the previous log is kept as
/// `<log>.prev`).
pub struct RealSpawner;

impl Spawner for RealSpawner {
    fn spawn(&self, spec: &ProcSpec) -> io::Result<u32> {
        if spec.log.exists() {
            let mut prev = spec.log.clone().into_os_string();
            prev.push(".prev");
            let _ = std::fs::rename(&spec.log, prev);
        }
        let out = File::create(&spec.log)?;
        let err = out.try_clone()?;
        let mut cmd = Command::new(&spec.program);
        cmd.args(&spec.args)
            .envs(spec.env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
            .stdin(Stdio::null())
            .stdout(Stdio::from(out))
            .stderr(Stdio::from(err));
        // SAFETY: setsid(2) and close_range(2) are async-signal-safe.
        unsafe {
            cmd.pre_exec(|| {
                libc::setsid();
                libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32);
                Ok(())
            });
        }
        Ok(cmd.spawn()?.id())
    }
}

/// Poll interval of the waits here.
pub const POLL: Duration = Duration::from_millis(250);
/// Seconds a client gets to exit after SIGINT.
pub const CLIENT_GRACE: Duration = Duration::from_secs(10);

/// Pid file content.
pub fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// `/proc/PID/comm` of a binary: its file name, cut at 15 characters like the kernel does.
/// One exception: `omarchyvr-voice-hud` would be cut to `omarchyvr-voice` (the voice daemon's own comm), so the HUD
/// names itself `omarchyvr-vhud` at start (prctl PR_SET_NAME) and this returns that.
pub fn comm_of(path: &Path) -> String {
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    if name == "omarchyvr-voice-hud" {
        return VOICE_HUD_COMM.to_string();
    }
    name.chars().take(15).collect()
}

/// The comm `omarchyvr-voice-hud` sets for itself (its file name cut at 15 characters would equal `omarchyvr-voice`).
pub const VOICE_HUD_COMM: &str = "omarchyvr-vhud";

/// The last `n` lines of a log.
pub fn log_tail(path: &Path, n: usize) -> Vec<String> {
    let text = std::fs::read_to_string(path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    lines[lines.len().saturating_sub(n)..].iter().map(|s| s.to_string()).collect()
}

/// Wait until `pid` has stayed alive for `settle`; false if it died earlier.
pub fn stays_alive(pid: u32, procs: &dyn Procs, clock: &dyn Clock, settle: Duration) -> bool {
    let t0 = clock.now();
    loop {
        if !procs.is_alive(pid) {
            return false;
        }
        if clock.now().saturating_sub(t0) >= settle {
            return true;
        }
        clock.sleep(POLL);
    }
}

/// What happened to one client's stop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientStop {
    /// No pid file, or the process is gone / is not what the pid file says.
    NotRunning,
    /// Exited after SIGINT.
    Exited,
    /// Still alive after the grace period (no SIGKILL: Monado's teardown copes with a client).
    StillAlive,
    /// The signal failed.
    CannotSignal(String),
}

/// SIGINT one client by pid (never by name), wait up to `grace`.
pub fn stop_client(
    name: &str,
    pid_file: &Path,
    comm: &str,
    procs: &dyn Procs,
    clock: &dyn Clock,
    grace: Duration,
    info: &mut dyn FnMut(&str),
) -> ClientStop {
    let Some(pid) = read_pid(pid_file) else {
        info(&format!("{name}: no pid file, not running"));
        return ClientStop::NotRunning;
    };
    if !procs.is_alive(pid) || procs.comm(pid).as_deref() != Some(comm) {
        info(&format!("{name}: pid {pid} is gone (or is not {comm}), not touching it"));
        return ClientStop::NotRunning;
    }
    info(&format!("{name}: SIGINT to pid {pid}, waiting up to {:.0} s", grace.as_secs_f64()));
    match procs.signal(pid, Signal::Int) {
        Ok(()) => {}
        Err(e) if e.raw_os_error() == Some(libc::ESRCH) => return ClientStop::Exited,
        Err(e) => return ClientStop::CannotSignal(format!("SIGINT to {name} pid {pid}: {e}")),
    }
    let t0 = clock.now();
    while clock.now().saturating_sub(t0) < grace {
        if !procs.is_alive(pid) {
            info(&format!("{name}: exited after {:.1} s", clock.now().saturating_sub(t0).as_secs_f64()));
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

/// Spawn, record the pid file `<session_dir>/<name>.pid`, wait `settle`. `Err((pid, tail))` if it died during the
/// settle (pid file removed, the log tail for the message); `Err((0, [error]))` if it could not be spawned.
pub fn start_client(
    spec: &ProcSpec,
    session_dir: &Path,
    sp: &dyn Spawner,
    procs: &dyn Procs,
    clock: &dyn Clock,
    settle: Duration,
) -> Result<u32, StartError> {
    let pid = sp.spawn(spec).map_err(|e| StartError::Spawn(e.to_string()))?;
    let pf = session_dir.join(format!("{}.pid", spec.name));
    let _ = std::fs::write(&pf, format!("{pid}\n"));
    if !stays_alive(pid, procs, clock, settle) {
        let _ = std::fs::remove_file(&pf);
        return Err(StartError::DiedEarly { pid, tail: log_tail(&spec.log, 12) });
    }
    Ok(pid)
}

/// Why [`start_client`] failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartError {
    Spawn(String),
    DiedEarly { pid: u32, tail: Vec<String> },
}

/// `XR_RUNTIME_JSON` + `IPC_IGNORE_VERSION`: the environment every XR client of the session gets.
pub fn xr_env(runtime_json: &Path) -> Vec<(String, String)> {
    vec![
        ("XR_RUNTIME_JSON".into(), runtime_json.to_string_lossy().into_owned()),
        ("IPC_IGNORE_VERSION".into(), "1".into()),
    ]
}

/// [`crate::Component::stop`] of a client process: SIGINT by the pid of its table row's pid file (checked by comm),
/// wait `grace`, never SIGKILL.
pub fn component_stop(spec: &'static crate::spec::Spec, env: &dyn crate::component::Env, grace: Duration) -> crate::component::StopOutcome {
    use crate::component::StopOutcome as O;
    let r = env.roots();
    let Some(ps) = spec.pid else { return O::Failed(format!("{}: not a process", spec.id)) };
    let pf = crate::procs::pid_path(r, &ps);
    let procs = RealProcs::new(&r.proc_);
    let clock = RealClock::new();
    match stop_client(spec.id, &pf, ps.comm, &procs, &clock, grace, &mut |m| env.log(m)) {
        ClientStop::NotRunning | ClientStop::Exited => {
            let _ = std::fs::remove_file(&pf);
            O::Stopped
        }
        ClientStop::StillAlive => O::Hung { pid: read_pid(&pf).unwrap_or(0) },
        ClientStop::CannotSignal(m) => O::Failed(m),
    }
}

/// A client process whose module adds only the stop (the start needs the launcher's options).
macro_rules! client_component {
    ($ty:ident) => {
        pub struct $ty(pub &'static $crate::spec::Spec);
        impl $crate::component::Component for $ty {
            fn spec(&self) -> &'static $crate::spec::Spec {
                self.0
            }
            fn stop(&self, env: &dyn $crate::component::Env, grace: std::time::Duration) -> $crate::component::StopOutcome {
                $crate::components::proc::component_stop(self.0, env, grace)
            }
        }
    };
}
pub(crate) use client_component;

#[cfg(test)]
pub(crate) mod fake {
    //! A fake process table and clock for the component tests.
    use super::*;
    use std::cell::{Cell, RefCell};

    pub struct FakeClock(pub Cell<Duration>);
    impl FakeClock {
        pub fn new() -> Self {
            FakeClock(Cell::new(Duration::ZERO))
        }
    }
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// (pid, comm, dies this long after SIGINT (None: ignores it), SIGINT time)
    pub type FakeProc = (u32, String, Option<Duration>, Option<Duration>);

    pub struct FakeProcs<'a> {
        pub clock: &'a FakeClock,
        pub procs: RefCell<Vec<FakeProc>>,
        pub log: RefCell<Vec<(u32, Signal)>>,
        /// (pid, signal, dies this long after it): overrides the SIGINT behaviour for that signal.
        pub on_signal: RefCell<Vec<(u32, Signal, Duration)>>,
    }
    impl<'a> FakeProcs<'a> {
        pub fn new(clock: &'a FakeClock) -> Self {
            FakeProcs { clock, procs: RefCell::new(vec![]), log: RefCell::new(vec![]), on_signal: RefCell::new(vec![]) }
        }
        fn is_alive_at(&self, x: &FakeProc) -> bool {
            match (x.2, x.3) {
                (Some(d), Some(t)) => self.clock.now() < t + d,
                _ => true,
            }
        }
        /// `pid` dies `secs` after it receives `sig`.
        pub fn dies_on(&self, pid: u32, sig: Signal, secs: u64) {
            self.on_signal.borrow_mut().push((pid, sig, Duration::from_secs(secs)));
        }
        pub fn add(&self, pid: u32, comm: &str, dies_after: Option<u64>) {
            self.procs.borrow_mut().push((pid, comm.into(), dies_after.map(Duration::from_secs), None));
        }
    }
    impl Procs for FakeProcs<'_> {
        fn is_alive(&self, pid: u32) -> bool {
            self.procs.borrow().iter().any(|x| x.0 == pid && self.is_alive_at(x))
        }
        fn comm(&self, pid: u32) -> Option<String> {
            self.procs.borrow().iter().find(|x| x.0 == pid).map(|x| x.1.clone())
        }
        fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
            self.log.borrow_mut().push((pid, sig));
            let after = self.on_signal.borrow().iter().find(|o| o.0 == pid && o.1 == sig).map(|o| o.2);
            for x in self.procs.borrow_mut().iter_mut().filter(|x| x.0 == pid) {
                if !self.is_alive_at(x) {
                    continue;
                }
                if let Some(d) = after {
                    x.2 = Some(d);
                }
                x.3 = Some(self.clock.now());
            }
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
}

#[cfg(test)]
mod tests {
    use super::fake::*;
    use super::*;

    fn dir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pimax-comp-proc-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn stop_client_signals_by_pid_only_and_never_kills() {
        let d = dir("stop");
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        procs.add(10, "omarchyvr-voice", Some(2));
        procs.add(11, "stubborn", None);
        procs.add(12, "other", None);
        std::fs::write(d.join("a.pid"), "10\n").unwrap();
        std::fs::write(d.join("b.pid"), "11\n").unwrap();
        std::fs::write(d.join("c.pid"), "12\n").unwrap();
        let mut said = vec![];
        let mut info = |m: &str| said.push(m.to_string());
        assert_eq!(stop_client("a", &d.join("a.pid"), "omarchyvr-voice", &procs, &clock, CLIENT_GRACE, &mut info), ClientStop::Exited);
        assert_eq!(stop_client("b", &d.join("b.pid"), "stubborn", &procs, &clock, CLIENT_GRACE, &mut info), ClientStop::StillAlive);
        // the pid file names another program: not touched
        assert_eq!(stop_client("c", &d.join("c.pid"), "omarchyvr-voice", &procs, &clock, CLIENT_GRACE, &mut info), ClientStop::NotRunning);
        assert_eq!(stop_client("x", &d.join("x.pid"), "x", &procs, &clock, CLIENT_GRACE, &mut info), ClientStop::NotRunning);
        assert_eq!(*procs.log.borrow(), vec![(10, Signal::Int), (11, Signal::Int)]);
    }

    struct Sp(u32);
    impl Spawner for Sp {
        fn spawn(&self, _: &ProcSpec) -> io::Result<u32> {
            Ok(self.0)
        }
    }

    #[test]
    fn start_client_writes_the_pid_file_and_notices_an_early_exit() {
        let d = dir("start");
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock);
        procs.add(20, "x", None);
        let spec = ProcSpec { name: "voice", program: "/bin/true".into(), args: vec![], env: vec![], log: d.join("voice.log") };
        assert_eq!(start_client(&spec, &d, &Sp(20), &procs, &clock, Duration::from_secs(3)), Ok(20));
        assert_eq!(read_pid(&d.join("voice.pid")), Some(20));
        std::fs::write(&spec.log, "a\nlast words\n").unwrap();
        match start_client(&spec, &d, &Sp(21), &procs, &clock, Duration::from_secs(3)) {
            Err(StartError::DiedEarly { pid: 21, tail }) => assert_eq!(tail.last().unwrap(), "last words"),
            o => panic!("{o:?}"),
        }
        assert!(!d.join("voice.pid").exists());
    }

    #[test]
    fn comm_is_cut_like_the_kernel() {
        assert_eq!(comm_of(Path::new("/x/omarchyvr-passthrough")), "omarchyvr-passt");
        assert_eq!(comm_of(Path::new("/x/omarchyvr-voice-hud")), "omarchyvr-vhud", "not the voice daemon's comm");
        assert_eq!(comm_of(Path::new("/x/omarchyvr-roommap")), "omarchyvr-roomm");
        assert_eq!(comm_of(Path::new("/x/omarchyvr-record")), "omarchyvr-recor");
    }
}
