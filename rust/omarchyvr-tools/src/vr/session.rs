//! The flows behind `omarchyvr vr start|stop|status`: spawn a detached monado-service, wait for the gyro calibration,
//! stop it with SIGINT (SIGKILL after a grace period) and check the kernel log afterwards.
//!
//! The waiting loops take a [`Clock`] and closures instead of files and processes, so the unit tests below run them
//! with fake time; `tests/vr_cli.rs` drives the real thing against a fake service script.
use super::config::{build_env, inherited_overrides, StartOptions, DEFAULT_STOP_GRACE_SECS};
use super::gate;
use super::host::{Clock, Notifier, Procs, Signal};
use super::logs::{LineBuffer, Summary};
use super::preflight::{self, Host, Report};
use super::runtime_link;
use crate::CliError;
use std::collections::VecDeque;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::MetadataExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::Duration;

/// How often the waiting loops look again.
pub const POLL: Duration = Duration::from_millis(100);
/// Lines of log kept for error messages.
const TAIL_LINES: usize = 12;

/// Incremental reader of the service log (which keeps growing while we wait).
pub struct LogTail {
    path: PathBuf,
    offset: u64,
    buf: LineBuffer,
}

impl LogTail {
    /// Start at the beginning of `path` (which may not exist yet).
    pub fn new(path: &Path) -> Self {
        Self {
            path: path.to_path_buf(),
            offset: 0,
            buf: LineBuffer::default(),
        }
    }

    /// Complete lines written since the last call.
    pub fn poll(&mut self) -> Vec<String> {
        let Ok(mut f) = File::open(&self.path) else {
            return vec![];
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // truncated or replaced: start over
            self.offset = 0;
            self.buf = LineBuffer::default();
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return vec![];
        }
        let mut bytes = Vec::new();
        if f.read_to_end(&mut bytes).is_err() {
            return vec![];
        }
        self.offset += bytes.len() as u64;
        self.buf.push_bytes(&bytes)
    }
}

/// Result of waiting for the service to come up.
#[derive(Debug)]
pub enum ReadyOutcome {
    /// `gyro bias done` was logged.
    Ready {
        /// Everything the log said until then.
        summary: Box<Summary>,
        /// Time it took.
        waited: Duration,
    },
    /// The process ended first.
    Exited {
        /// Exit status description.
        status: String,
        /// Last log lines.
        tail: Vec<String>,
    },
    /// The timeout passed with the process still running.
    TimedOut {
        /// Last log lines.
        tail: Vec<String>,
    },
}

/// Wait until the log shows the gyro calibration, the process dies, or `timeout` passes.
/// `new_lines` returns the lines logged since its last call; `exited` returns a description once the process is gone.
pub fn wait_ready(
    new_lines: &mut dyn FnMut() -> Vec<String>,
    exited: &mut dyn FnMut() -> Option<String>,
    clock: &dyn Clock,
    timeout: Duration,
) -> ReadyOutcome {
    let t0 = clock.now();
    let mut summary = Summary::default();
    let mut tail: VecDeque<String> = VecDeque::new();
    loop {
        let mut absorb = |lines: Vec<String>, summary: &mut Summary| {
            for l in lines {
                summary.feed(&l);
                if tail.len() == TAIL_LINES {
                    tail.pop_front();
                }
                tail.push_back(l);
            }
        };
        absorb(new_lines(), &mut summary);
        if summary.ready() {
            return ReadyOutcome::Ready {
                summary: Box::new(summary),
                waited: clock.now().saturating_sub(t0),
            };
        }
        if let Some(status) = exited() {
            // the last words of a dying process are often the explanation
            absorb(new_lines(), &mut summary);
            return ReadyOutcome::Exited {
                status,
                tail: tail.into_iter().collect(),
            };
        }
        if clock.now().saturating_sub(t0) >= timeout {
            return ReadyOutcome::TimedOut {
                tail: tail.into_iter().collect(),
            };
        }
        clock.sleep(POLL);
    }
}

pub use omarchyvr_components::components::monado::{sigint_already_consumed, stop_pid, stop_pid_with, StopOutcome};

/// What `vr stop` acts on.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StopTargets {
    /// The service of the pid file (still alive and still named like the service).
    pub ours: Vec<u32>,
    /// Other processes of this user with the service's name (another build, another agent, an old script).
    pub others: Vec<u32>,
}

/// Find what to stop. Only `ours` is stopped by default: other monado-service processes may belong to somebody else.
pub fn stop_targets(opts: &StartOptions, procs: &dyn Procs) -> StopTargets {
    let comm = opts.service_comm();
    let ours: Vec<u32> = read_pid(&opts.pid_path())
        .into_iter()
        .filter(|p| procs.is_alive(*p) && procs.comm(*p).as_deref() == Some(comm.as_str()))
        .collect();
    let others = procs
        .find_by_comm(&comm)
        .into_iter()
        .filter(|p| !ours.contains(p))
        .collect();
    StopTargets { ours, others }
}

/// Every running process named like the service plus the pid-file one (used by `status`, which only reports).
pub fn service_pids(opts: &StartOptions, procs: &dyn Procs) -> Vec<u32> {
    let comm = opts.service_comm();
    let mut v = procs.find_by_comm(&comm);
    if let Some(p) = read_pid(&opts.pid_path()) {
        if procs.is_alive(p) && procs.comm(p).as_deref() == Some(comm.as_str()) && !v.contains(&p) {
            v.push(p);
        }
    }
    v.sort_unstable();
    v
}

/// Pid file content.
pub fn read_pid(path: &Path) -> Option<u32> {
    std::fs::read_to_string(path).ok()?.trim().parse().ok()
}

/// Description of how a process ended.
pub fn describe_exit(st: ExitStatus) -> String {
    if let Some(sig) = st.signal() {
        format!("killed by signal {sig}")
    } else {
        format!("exit code {}", st.code().unwrap_or(-1))
    }
}

/// Spawn monado-service detached (own session, stdin /dev/null, stdout+stderr into the log file).
pub fn spawn_service(opts: &StartOptions, env: &[(String, String)]) -> Result<Child, CliError> {
    ensure_private_dir(&opts.session_dir)?;
    let log = opts.log_path();
    if log.exists() {
        let _ = std::fs::rename(&log, opts.session_dir.join("monado.log.prev"));
    }
    let out = File::create(&log)
        .map_err(|e| CliError::new(format!("cannot create {}: {e}", log.display())))?;
    let err = out
        .try_clone()
        .map_err(|e| CliError::new(format!("cannot duplicate the log handle: {e}")))?;
    let bin = opts.service_path();
    let mut cmd = Command::new(&bin);
    cmd.envs(env.iter().map(|(k, v)| (k.as_str(), v.as_str())))
        .stdin(Stdio::null())
        .stdout(Stdio::from(out))
        .stderr(Stdio::from(err));
    // SAFETY: the closure runs between fork and exec and only makes two raw system calls, which are
    // async-signal-safe: setsid(2) gives the service its own session, so it survives the terminal or the launcher that
    // started us; close_range(3, ~0, CLOSE_RANGE_CLOEXEC) (best effort, Linux 5.11+) keeps the caller's other
    // descriptors (a `flock(1)` lock, pipes) from leaking into a process that lives for hours.
    unsafe {
        cmd.pre_exec(|| {
            libc::setsid();
            libc::syscall(libc::SYS_close_range, 3u32, u32::MAX, 4u32);
            Ok(())
        });
    }
    cmd.spawn()
        .map_err(|e| CliError::new(format!("cannot start {}: {e}", bin.display())))
}

/// Write the pid file and the session summary.
pub fn write_session_files(opts: &StartOptions, pid: u32) {
    let _ = std::fs::write(opts.pid_path(), format!("{pid}\n"));
    let started = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let dump = opts
        .dump_dir
        .as_ref()
        .map(|d| d.display().to_string())
        .unwrap_or_default();
    let _ = std::fs::write(
        opts.summary_path(),
        format!(
            "pid={pid}\nfps={}\nmode={}\nstarted_unix={started}\nlog={}\ndump_dir={dump}\nservice={}\n",
            opts.fps.hz(),
            opts.mode_override.unwrap_or_else(|| opts.fps.desired_mode()),
            opts.log_path().display(),
            opts.service_path().display(),
        ),
    );
}

/// `key=value` lines of the session summary.
pub fn read_session_file(path: &Path) -> Vec<(String, String)> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .filter_map(|l| {
            l.split_once('=')
                .map(|(k, v)| (k.to_string(), v.to_string()))
        })
        .collect()
}

/// `1h 02m 03s` style duration.
pub fn fmt_age(secs: f64) -> String {
    let s = secs.max(0.0) as u64;
    if s >= 3600 {
        format!("{}h {:02}m {:02}s", s / 3600, (s / 60) % 60, s % 60)
    } else if s >= 60 {
        format!("{}m {:02}s", s / 60, s % 60)
    } else {
        format!("{s}s")
    }
}

/// Create `dir` if needed and make sure it is a real directory (not a symlink) owned by us: the log, pid file and locks
/// live in a predictable /tmp path and must not follow somebody else's symlinks.
pub fn ensure_private_dir(dir: &Path) -> Result<(), CliError> {
    std::fs::create_dir_all(dir)
        .map_err(|e| CliError::new(format!("cannot create {}: {e}", dir.display())))?;
    let md = std::fs::symlink_metadata(dir)
        .map_err(|e| CliError::new(format!("cannot inspect {}: {e}", dir.display())))?;
    // SAFETY: getuid cannot fail.
    let me = unsafe { libc::getuid() };
    if !md.is_dir() || md.uid() != me {
        return Err(CliError::new(format!(
            "{} is not a directory owned by you (somebody else created it?): remove it or use --session-dir",
            dir.display()
        )));
    }
    Ok(())
}

/// Why a lock could not be taken.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LockError {
    /// Somebody else holds it.
    Busy,
    /// Anything else (permissions, a foreign directory).
    Failed(String),
}

pub(crate) fn flock_file(dir: &Path, name: &str, blocking: bool) -> Result<File, LockError> {
    ensure_private_dir(dir).map_err(|e| LockError::Failed(e.msg))?;
    let path = dir.join(name);
    let file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| LockError::Failed(format!("cannot open {}: {e}", path.display())))?;
    let how = if blocking {
        libc::LOCK_EX
    } else {
        libc::LOCK_EX | libc::LOCK_NB
    };
    loop {
        // SAFETY: flock(2) on a descriptor that stays open for the lifetime of `file`.
        if unsafe { libc::flock(file.as_raw_fd(), how) } == 0 {
            return Ok(file);
        }
        let e = io::Error::last_os_error();
        match e.raw_os_error() {
            Some(libc::EINTR) => continue,
            Some(libc::EWOULDBLOCK) => return Err(LockError::Busy),
            _ => {
                return Err(LockError::Failed(format!(
                    "cannot lock {}: {e}",
                    path.display()
                )))
            }
        }
    }
}

/// Exclusive, non-blocking lock so that two `vr start` (a double click on the desktop entry) cannot race each other.
/// The descriptor is close-on-exec, so the service that gets spawned does not inherit and keep the lock.
pub struct StartLock {
    _file: File,
}

impl StartLock {
    /// Take the lock in `dir`, or fail with [`LockError::Busy`] if another `vr start` holds it.
    pub fn acquire(dir: &Path) -> Result<Self, LockError> {
        flock_file(dir, "start.lock", false).map(|f| Self { _file: f })
    }
}

/// Exclusive lock around `vr stop` (blocking): Monado installs its SIGINT/SIGTERM handlers with SA_RESETHAND, so two
/// concurrent stops (a double click on the Stop action) would deliver a second signal during the teardown, which kills
/// the service without cleanup. The second stop waits here and then finds nothing left to stop.
pub struct StopLock {
    _file: File,
}

impl StopLock {
    /// Wait for and take the lock in `dir`.
    pub fn acquire(dir: &Path) -> Result<Self, CliError> {
        flock_file(dir, "stop.lock", true)
            .map(|f| Self { _file: f })
            .map_err(|e| match e {
                LockError::Busy => CliError::new("stop lock busy"),
                LockError::Failed(m) => CliError::new(m),
            })
    }
}

/// Everything `start` needs from the machine.
pub struct Deps<'a> {
    /// Preflight probes.
    pub host: &'a dyn Host,
    /// Process table.
    pub procs: &'a dyn Procs,
    /// Desktop notifications.
    pub notifier: &'a dyn Notifier,
    /// Time.
    pub clock: &'a dyn Clock,
    /// The caller's environment.
    pub env: &'a dyn Fn(&str) -> Option<String>,
    /// True once SIGINT/SIGTERM reached the launcher (the real `vr start` installs the handler).
    pub stop_requested: &'a dyn Fn() -> bool,
}

/// Run the preflight checks of `opts` against the machine behind `d`.
pub fn preflight_report(opts: &StartOptions, d: &Deps) -> Report {
    preflight::run_for(d.host, &opts.connector, Some(&opts.service_path()))
}

/// `vr start`. Returns once the service is ready (or, with `opts.wait`, once it has exited again).
pub fn start(opts: &StartOptions, d: &Deps) -> Result<(), CliError> {
    let _lock = if opts.dry_run {
        None
    } else {
        match StartLock::acquire(&opts.session_dir) {
            Ok(l) => Some(l),
            Err(LockError::Busy) => {
                let e = CliError::new(
                    "another omarchyvr vr start is in progress (or supervising the service with --wait)",
                );
                d.notifier
                    .notify(false, "Pimax VR: already starting", &e.msg);
                return Err(e);
            }
            Err(LockError::Failed(m)) => {
                d.notifier.notify(true, "Pimax VR: not started", &m);
                return Err(CliError::new(m));
            }
        }
    };
    let mut report = preflight_report(opts, d);
    let home = PathBuf::from((d.env)("HOME").unwrap_or_default());
    let want = opts.runtime_json();
    // Games run with: SteamVR (the app's switch) and Monado started anyway (Work / desk): the global OpenXR runtime
    // keeps the user's games choice; this session's own OpenXR clients (WayVR, passthrough, the HUDs) get
    // XR_RUNTIME_JSON for this build explicitly (config::build_env, the desk plan), so nothing here needs the link.
    let keep_link = super::steamvr::games_runtime(d.env) == super::steamvr::Games::SteamVr;
    // doctor check: active_runtime.json must point at the build being started (start fixes it unless --no-set-runtime)
    if let Some(detail) = runtime_link::describe(&runtime_link::inspect(&home, &want), &home, &want).filter(|_| !keep_link) {
        report.checks.push(preflight::Check {
            name: "openxr runtime",
            level: preflight::Level::Warn,
            detail: if opts.set_runtime && !opts.dry_run { format!("{detail}; fixing it") } else { detail },
        });
    }
    vout!("omarchyvr vr: preflight\n{}", report.render());
    let env = build_env(opts, d.env);
    let inherited = inherited_overrides(&env, d.env);
    if !inherited.is_empty() {
        let list: Vec<String> = inherited.iter().map(|(k, v)| format!("{k}={v}")).collect();
        vout!(
            "omarchyvr vr: note: passing on from your environment: {}",
            list.join(" ")
        );
    }
    if opts.dry_run {
        vout!("\nwould start: {}", opts.service_path().display());
        for (k, v) in &env {
            vout!("  {k}={v}");
        }
        vout!("  log: {}", opts.log_path().display());
        return if report.passed() {
            Ok(())
        } else {
            Err(CliError::new("preflight failed (dry run, nothing started)"))
        };
    }
    if !report.passed() {
        if !opts.force {
            d.notifier
                .notify(true, "Pimax VR: not started", &report.failure_summary());
            return Err(CliError::new(format!(
                "preflight failed, not starting ({})",
                report.failure_summary()
            )));
        }
        verr!("omarchyvr vr: --force given, starting despite failed checks");
    }
    if opts.set_runtime && keep_link {
        vout!(
            "omarchyvr vr: OpenXR runtime left as it is (Games run with: SteamVR): games go to SteamVR from Play; this session's apps use {} directly",
            want.display()
        );
    }
    if opts.set_runtime && !keep_link {
        match runtime_link::ensure(&home, &want) {
            Ok(Some(line)) => vout!("omarchyvr vr: OpenXR runtime set: {line}"),
            Ok(None) => {}
            Err(e) => verr!("omarchyvr vr: WARNING: could not set the OpenXR runtime ({e}); games need XR_RUNTIME_JSON={}", want.display()),
        }
        // OpenVR games go to Monado through xrizer: put it back if SteamVR rewrote openvrpaths.vrpath (LESSONS row 20)
        let xrizer = super::openvr_link::xrizer_dir(d.env, &home);
        match super::openvr_link::ensure_xrizer(&home, &xrizer) {
            Ok(Some(line)) => vout!("omarchyvr vr: OpenVR runtime set: {line}"),
            Ok(None) => {}
            Err(e) => verr!("omarchyvr vr: WARNING: could not set the OpenVR runtime to xrizer ({e})"),
        }
    }
    if !opts.service_path().exists() {
        let msg = format!(
            "{} does not exist (build Monado first)",
            opts.service_path().display()
        );
        d.notifier.notify(true, "Pimax VR: not started", &msg);
        return Err(CliError::new(msg));
    }
    if let Some(dir) = &opts.dump_dir {
        if let Err(e) = std::fs::create_dir_all(dir) {
            let msg = format!("cannot create dump dir {}: {e}", dir.display());
            d.notifier.notify(true, "Pimax VR: not started", &msg);
            return Err(CliError::new(msg));
        }
        verr!(
            "omarchyvr vr: frame dumps go to {} (about 109 MB per frame at 7104x3840)",
            dir.display()
        );
    }
    vout!(
        "omarchyvr vr: starting monado-service at {} Hz, log {}",
        opts.fps.hz(),
        opts.log_path().display()
    );
    d.notifier.notify(
        false,
        &format!("Pimax VR starting ({} Hz)", opts.fps.hz()),
        "Keep the headset still for 3 seconds (gyro calibration)",
    );
    let mut child = match spawn_service(opts, &env) {
        Ok(c) => c,
        Err(e) => {
            d.notifier.notify(true, "Pimax VR: not started", &e.msg);
            return Err(e);
        }
    };
    let pid = child.id();
    write_session_files(opts, pid);
    vout!("omarchyvr vr: keep the headset still for 3 s (gyro calibration), pid {pid}");

    let mut tail = LogTail::new(&opts.log_path());
    let outcome = wait_ready(
        &mut || tail.poll(),
        &mut || {
            if (d.stop_requested)() {
                return Some("aborted by a signal".to_string());
            }
            match child.try_wait() {
                Ok(Some(st)) => Some(describe_exit(st)),
                Ok(None) => None,
                // ECHILD (for example SIGCHLD ignored by the caller): ask the process table instead
                Err(_) => (!d.procs.is_alive(pid)).then(|| "no longer running".to_string()),
            }
        },
        d.clock,
        opts.ready_timeout,
    );
    match outcome {
        ReadyOutcome::Ready { summary, waited } => {
            vout!(
                "omarchyvr vr: ready after {:.1} s (pid {pid})",
                waited.as_secs_f64()
            );
            for l in summary.report() {
                vout!("  {l}");
            }
            let warnings = summary.rate_warnings(opts.fps.hz());
            for w in &warnings {
                verr!("omarchyvr vr: WARNING: {w}");
            }
            vout!("omarchyvr vr: start your game (Steam launch options: omarchyvr vr steam-options); stop with: omarchyvr vr stop");
            if warnings.is_empty() {
                d.notifier.notify(
                    false,
                    &format!("Pimax VR ready ({} Hz)", opts.fps.hz()),
                    "Start your game. Stop with: omarchyvr vr stop",
                );
            } else {
                d.notifier.notify(
                    true,
                    "Pimax VR started, but the rates disagree",
                    &warnings.join("; "),
                );
            }
            if opts.wait {
                return supervise(opts, d, child);
            }
            // dropping the handle neither kills nor waits; the service lives on in its own session
            drop(child);
            Ok(())
        }
        ReadyOutcome::Exited { .. } if (d.stop_requested)() => {
            // interrupted while the service was still starting: stop it politely instead of leaving it behind
            verr!("omarchyvr vr: interrupted while starting, stopping monado-service (pid {pid})");
            let out = stop_pid_with(
                pid,
                d.procs,
                d.clock,
                Duration::from_secs_f64(DEFAULT_STOP_GRACE_SECS),
                !super::supervise::in_watchdog(),
                &mut |m| vout!("omarchyvr vr: {m}"),
                &mut |m| verr!("omarchyvr vr: WARNING: {m}"),
            );
            let _ = child.try_wait(); // reap
            let _ = std::fs::remove_file(opts.pid_path());
            d.notifier.notify(
                matches!(out, StopOutcome::Killed | StopOutcome::Stuck | StopOutcome::Hung),
                "Pimax VR start aborted",
                "monado-service was stopped",
            );
            Err(CliError::new(
                "start aborted by a signal, monado-service stopped",
            ))
        }
        ReadyOutcome::Exited { status, tail } => {
            let _ = std::fs::remove_file(opts.pid_path());
            verr!("omarchyvr vr: monado-service ended before it was ready ({status}). Last log lines:");
            for l in &tail {
                verr!("  {l}");
            }
            d.notifier.notify(
                true,
                "Pimax VR failed to start",
                &format!(
                    "monado-service ended ({status}). See {}",
                    opts.log_path().display()
                ),
            );
            Err(CliError::new(format!(
                "monado-service ended during start-up ({status})"
            )))
        }
        ReadyOutcome::TimedOut { tail } => {
            verr!(
                "omarchyvr vr: no 'gyro bias done' after {:.0} s, but monado-service (pid {pid}) is still running. \
                 Last log lines:",
                opts.ready_timeout.as_secs_f64()
            );
            for l in &tail {
                verr!("  {l}");
            }
            d.notifier.notify(
                true,
                "Pimax VR not ready",
                "Gyro calibration did not finish (headset moving or no IMU data?). omarchyvr vr stop, then try again",
            );
            Err(CliError::new(
                "timed out waiting for the gyro calibration (service left running; stop it with: omarchyvr vr stop)",
            ))
        }
    }
}

/// `--wait`: stay in the foreground while the service runs, forward SIGINT/SIGTERM to it, clean up when it ends.
fn supervise(opts: &StartOptions, d: &Deps, mut child: Child) -> Result<(), CliError> {
    let pid = child.id();
    let grace = Duration::from_secs_f64(DEFAULT_STOP_GRACE_SECS);
    let mut asked: Option<Duration> = None;
    let status: Option<ExitStatus> = loop {
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => {}
            Err(_) if !d.procs.is_alive(pid) => break None, // status unknown (ECHILD)
            Err(_) => {}
        }
        let now = d.clock.now();
        if (d.stop_requested)() && asked.is_none() {
            if sigint_already_consumed(pid, d.procs) {
                // systemd/uwsm may have signalled the whole cgroup already: a second SIGINT would kill the service half way
                verr!("omarchyvr vr: stop requested, monado-service is already shutting down");
            } else {
                verr!("omarchyvr vr: stop requested, sending SIGINT to monado-service");
                let _ = d.procs.signal(pid, Signal::Int);
            }
            asked = Some(now);
        }
        if let Some(t) = asked {
            if now.saturating_sub(t) > grace {
                let evidence = d.procs.capture_hang(pid);
                let note = evidence.as_deref().unwrap_or("");
                verr!("omarchyvr vr: monado-service ignored the stop request for {DEFAULT_STOP_GRACE_SECS:.0} s, sending SIGKILL. {note}");
                d.notifier.notify(
                    true,
                    "Pimax VR: forced kill",
                    &format!("monado-service ignored SIGINT for {DEFAULT_STOP_GRACE_SECS:.0} s and was killed. {note}"),
                );
                let _ = d.procs.signal(pid, Signal::Kill);
                asked = Some(now + Duration::from_secs(3600));
            }
        }
        d.clock.sleep(Duration::from_millis(200));
    };
    let _ = std::fs::remove_file(opts.pid_path());
    let desc = status
        .map(describe_exit)
        .unwrap_or_else(|| "exit status unknown".to_string());
    vout!("omarchyvr vr: monado-service ended ({desc})");
    // Monado exits with code 0 after a handled SIGINT; death by a signal means the teardown did not run
    let clean = status.is_none_or(|s| s.success());
    d.clock.sleep(Duration::from_millis(1500)); // a late "Idling display engine" line may still be on its way
    finish_kernel_check(d)?;
    d.notifier.notify(!clean, "Pimax VR stopped", &desc);
    if clean {
        Ok(())
    } else {
        Err(CliError::new(format!(
            "monado-service ended abnormally ({desc})"
        )))
    }
}

/// Print the kernel log verdict; `Err` (exit code 2) if there is an Xid / display engine error.
pub fn finish_kernel_check(d: &Deps) -> Result<(), CliError> {
    match d.host.kernel_errors() {
        Ok(lines) if lines.is_empty() => {
            vout!("omarchyvr vr: kernel log of this boot: no Xid / display engine errors");
            Ok(())
        }
        Ok(lines) => {
            verr!("omarchyvr vr: KERNEL LOG HAS GPU/DISPLAY ERRORS, reboot before the next session:");
            for l in lines.iter().take(8) {
                verr!("  {l}");
            }
            d.notifier.notify(
                true,
                "Pimax VR: GPU/display error in the kernel log",
                "Reboot before the next VR session (Xid or 'Idling display engine')",
            );
            let mut e = CliError::new("kernel log shows an Xid / display engine error: reboot");
            e.code = 2;
            Err(e)
        }
        Err(e) => {
            verr!("omarchyvr vr: could not read the kernel log ({e}); check journalctl -k -b | grep -E 'Xid|Idling'");
            Ok(())
        }
    }
}

/// Seconds Monado needs for its teardown with and without clients: it waits up to 3 s for connected clients and
/// joins their threads, so a stop with a game attached must not be cut off after 5 s (a SIGKILL half way through is
/// what leaves the NVIDIA display engine stuck).
const GRACE_NO_CLIENTS_SECS: f64 = 5.0;
const GRACE_WITH_CLIENTS_SECS: f64 = 15.0;

/// Default grace period: longer when the log shows a connected client.
pub fn default_grace(opts: &StartOptions) -> (Duration, bool) {
    let attached = File::open(opts.log_path())
        .map(|f| Summary::from_reader(std::io::BufReader::new(f)))
        .map(|s| s.clients_connected.len() as u32 > s.clients_disconnected)
        .unwrap_or(false);
    let secs = if attached {
        GRACE_WITH_CLIENTS_SECS
    } else {
        GRACE_NO_CLIENTS_SECS
    };
    (Duration::from_secs_f64(secs), attached)
}

/// `vr stop`. `grace`: explicit `--grace`, otherwise 5 s (15 s with a client attached). `all`: also stop monado-service
/// processes that this tool did not start. `kill`: SIGKILL after the grace (an explicit user stop only); automatic
/// stops pass false (`--no-kill`, or inside the watchdog) and leave a hung service alive, latching the GPU gate.
pub fn stop(
    opts: &StartOptions,
    grace: Option<Duration>,
    all: bool,
    kill: bool,
    d: &Deps,
) -> Result<(), CliError> {
    // one stop at a time; the one that waited finds nothing left to stop
    let _lock = StopLock::acquire(&opts.session_dir)?;
    // the recorder first (desk stop already did it): it finishes its mp4 while Monado still runs
    if !opts.dry_run {
        let _ = omarchyvr_components::components::record::stop_first(&opts.session_dir, d.procs, d.clock, &mut |m| vout!("omarchyvr vr: {m}"));
    }
    let targets = stop_targets(opts, d.procs);
    let mut pids = targets.ours.clone();
    if all {
        pids.extend(targets.others.iter().copied());
    }
    if pids.is_empty() {
        vout!("omarchyvr vr: monado-service is not running");
        let _ = std::fs::remove_file(opts.pid_path());
        if !targets.others.is_empty() {
            let list: Vec<String> = targets
                .others
                .iter()
                .map(|p| {
                    format!(
                        "pid {p} ({})",
                        d.procs.exe(*p).unwrap_or_else(|| "exe unknown".into())
                    )
                })
                .collect();
            let msg = format!(
                "none was started by `omarchyvr vr start`, but other monado-service processes of yours are running: {}; \
                 `omarchyvr vr stop --all` stops them too",
                list.join(", ")
            );
            verr!("omarchyvr vr: {msg}");
            return Err(CliError::new(msg));
        }
    }
    let (grace, attached) = match grace {
        Some(g) => (g, false),
        None => default_grace(opts),
    };
    if attached && !pids.is_empty() {
        vout!(
            "omarchyvr vr: a client is still connected: giving monado-service up to {:.0} s for its teardown",
            grace.as_secs_f64()
        );
    }
    let mut worst: Option<StopOutcome> = None;
    for pid in &pids {
        if kill {
            vout!(
                "omarchyvr vr: stopping monado-service (pid {pid}): SIGINT, then SIGKILL after {:.0} s",
                grace.as_secs_f64()
            );
        } else {
            vout!(
                "omarchyvr vr: stopping monado-service (pid {pid}): SIGINT, waiting up to {:.0} s (no SIGKILL)",
                grace.as_secs_f64()
            );
        }
        let out = stop_pid_with(
            *pid,
            d.procs,
            d.clock,
            grace,
            kill,
            &mut |m| vout!("omarchyvr vr: {m}"),
            &mut |m| {
                verr!("omarchyvr vr: WARNING: {m}");
                let title = if kill { "Pimax VR: forced kill" } else { "Pimax VR: monado-service hung, reboot needed" };
                d.notifier.notify(true, title, m);
            },
        );
        match &out {
            StopOutcome::Graceful(t) => vout!(
                "omarchyvr vr: pid {pid} exited cleanly after {:.1} s",
                t.as_secs_f64()
            ),
            StopOutcome::Killed => vout!("omarchyvr vr: pid {pid} killed"),
            StopOutcome::Stuck => verr!(
                "omarchyvr vr: pid {pid} is STILL running after SIGKILL (uninterruptible: GPU hang?)"
            ),
            StopOutcome::Hung => verr!(
                "omarchyvr vr: pid {pid} is still running {:.0} s after SIGINT; left alive (no SIGKILL from an automatic stop)",
                grace.as_secs_f64()
            ),
            StopOutcome::CannotSignal(m) => verr!("omarchyvr vr: cannot stop pid {pid}: {m}"),
        }
        let why = match &out {
            StopOutcome::Killed => Some("monado-service was SIGKILLed"),
            StopOutcome::Stuck => Some("monado-service survived SIGKILL (uninterruptible)"),
            StopOutcome::Hung => Some("monado-service hung after SIGINT"),
            _ => None,
        };
        if let Some(why) = why {
            gate::latch_env(d.env, gate::Level::NeedsReboot, &format!("{why} (pid {pid})"));
        }
        let rank = |o: &StopOutcome| match o {
            StopOutcome::Graceful(_) => 0,
            StopOutcome::Killed => 1,
            StopOutcome::Hung => 2,
            StopOutcome::Stuck => 3,
            StopOutcome::CannotSignal(_) => 4,
        };
        if worst.as_ref().is_none_or(|w| rank(&out) > rank(w)) {
            worst = Some(out);
        }
    }
    if !pids.is_empty() {
        // a hung service keeps its pid file: it is still running (the watchdog and `vr status` must see it)
        if worst != Some(StopOutcome::Hung) {
            let _ = std::fs::remove_file(opts.pid_path());
        }
        // the driver may log a display engine timeout a moment after the process is gone
        d.clock.sleep(Duration::from_millis(1500));
    }
    let kernel = finish_kernel_check(d);
    match &worst {
        Some(StopOutcome::Stuck) => {
            return Err(CliError::new(
                "monado-service did not die; reboot is probably needed",
            ))
        }
        Some(StopOutcome::Hung) => {
            return Err(CliError::new(
                "monado-service did not exit and was not killed (automatic stop); reboot before the next session",
            ))
        }
        Some(StopOutcome::CannotSignal(m)) => return Err(CliError::new(m.clone())),
        _ => {}
    }
    kernel?;
    if worst == Some(StopOutcome::Killed) {
        return Err(CliError::new(
            "monado-service had to be killed (SIGKILL): look at the kernel log and the headset before the next start",
        ));
    }
    if !pids.is_empty() {
        d.notifier
            .notify(false, "Pimax VR stopped", "Kernel log clean");
    }
    Ok(())
}

/// One status block as text lines; the first line says whether the service runs. Returns (lines, running).
pub fn status_lines(
    opts: &StartOptions,
    d: &Deps,
    log_override: Option<&Path>,
) -> (Vec<String>, bool) {
    let pids = service_pids(opts, d.procs);
    let mut out = Vec::new();
    let running = !pids.is_empty();
    let sess = read_session_file(&opts.summary_path());
    let sess_get = |k: &str| sess.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
    if let Some(pid) = pids.first() {
        let age = d
            .procs
            .age_secs(*pid)
            .map(fmt_age)
            .unwrap_or_else(|| "?".into());
        out.push(format!("service: running, pid {pid}, up {age}"));
        let env = d.procs.environ(*pid).unwrap_or_default();
        let ev = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        out.push(format!(
            "rate: {} Hz (PIMAX_FPS), display mode index {}",
            ev("PIMAX_FPS").unwrap_or_else(|| "90 (default)".into()),
            ev("XRT_COMPOSITOR_DESIRED_MODE").unwrap_or_else(|| "?".into())
        ));
        if let Some(dd) = ev("XRT_COMPOSITOR_DUMP_DIR") {
            out.push(format!("frame dumps: {dd}"));
        }
        if pids.len() > 1 {
            out.push(format!(
                "WARNING: {} monado-service processes: {pids:?}",
                pids.len()
            ));
        }
    } else {
        out.push("service: not running".into());
        if let (Some(p), Some(f)) = (sess_get("pid"), sess_get("fps")) {
            out.push(format!("last session: pid {p}, {f} Hz"));
        }
    }
    let log = log_override
        .map(Path::to_path_buf)
        .unwrap_or_else(|| opts.log_path());
    out.push(format!("log: {}", log.display()));
    if let Ok(f) = File::open(&log) {
        for l in Summary::from_reader(std::io::BufReader::new(f)).report() {
            out.push(format!("  {l}"));
        }
    }
    let host_report = preflight::run_for(d.host, &opts.connector, Some(&opts.service_path()));
    for c in host_report
        .checks
        .iter()
        .filter(|c| c.name != "service" && c.name != "wayland")
    {
        out.push(format!("{}: {}", c.name, c.detail));
    }
    (out, running)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::config::tests::opts;
    use crate::vr::preflight::tests::Mock;
    use std::cell::{Cell, RefCell};
    use std::collections::HashMap;
    use std::io;

    /// Virtual time: `sleep` advances `now`.
    struct FakeClock(Cell<Duration>);
    impl FakeClock {
        fn new() -> Self {
            Self(Cell::new(Duration::ZERO))
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

    const GYRO: &str = " INFO [bias_finish] gyro bias done: 0.2 0.7 0.4 dps over 2000 samples (0 restarts); accel 0 1 0 g";

    #[test]
    fn ready_when_the_gyro_line_arrives() {
        let clock = FakeClock::new();
        let n = Cell::new(0u32);
        let mut lines = || {
            n.set(n.get() + 1);
            match n.get() {
                1 => vec![" INFO [pimax_hmd_create] pimax_video_prepare(90) -> 0".to_string()],
                20 => vec![GYRO.to_string()],
                _ => vec![],
            }
        };
        let out = wait_ready(&mut lines, &mut || None, &clock, Duration::from_secs(30));
        match out {
            ReadyOutcome::Ready { summary, waited } => {
                assert!(summary.ready());
                assert_eq!(summary.video_prepare, Some((90, 0)));
                // 19 sleeps of 100 ms before the 20th poll
                assert_eq!(waited, Duration::from_millis(1900));
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn exit_before_ready_returns_the_tail_including_last_words() {
        let clock = FakeClock::new();
        let polls = Cell::new(0u32);
        let mut lines = || {
            polls.set(polls.get() + 1);
            match polls.get() {
                1 => (1..=20).map(|i| format!("line {i}")).collect(),
                // the final burst is only visible after the exit was noticed
                3 => vec!["ERROR [x] lease denied".to_string()],
                _ => vec![],
            }
        };
        let ticks = Cell::new(0u32);
        let mut exited = || {
            ticks.set(ticks.get() + 1);
            (ticks.get() >= 2).then(|| "exit code 1".to_string())
        };
        match wait_ready(&mut lines, &mut exited, &clock, Duration::from_secs(30)) {
            ReadyOutcome::Exited { status, tail } => {
                assert_eq!(status, "exit code 1");
                assert_eq!(tail.len(), TAIL_LINES);
                assert_eq!(tail.last().unwrap(), "ERROR [x] lease denied");
                assert_eq!(tail[0], "line 10"); // only the newest lines are kept (20 + 1 - 12 + 1)
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn timeout_when_the_process_lives_but_never_calibrates() {
        let clock = FakeClock::new();
        let mut lines = || vec![];
        let out = wait_ready(&mut lines, &mut || None, &clock, Duration::from_secs(3));
        assert!(matches!(out, ReadyOutcome::TimedOut { .. }));
        assert_eq!(clock.now(), Duration::from_secs(3));
    }

    #[test]
    fn ready_wins_over_a_simultaneous_exit() {
        // a service that logged the calibration and exited right away is still "ready" (it was)
        let clock = FakeClock::new();
        let mut lines = || vec![GYRO.to_string()];
        let out = wait_ready(
            &mut lines,
            &mut || Some("exit code 0".into()),
            &clock,
            Duration::from_secs(1),
        );
        assert!(matches!(out, ReadyOutcome::Ready { .. }));
    }

    /// A process table with one process that dies at a chosen virtual time after SIGINT (or never).
    struct FakeProcs<'a> {
        clock: &'a FakeClock,
        die_after_int: Option<Duration>,
        die_after_kill: Option<Duration>,
        int_at: Cell<Option<Duration>>,
        kill_at: Cell<Option<Duration>>,
        signals: RefCell<Vec<Signal>>,
        pids_signalled: RefCell<Vec<u32>>,
        gone: bool,
        /// What `catches_sigint` answers (None = unknown).
        sigint_caught: Option<bool>,
        age: Option<f64>,
        /// Absolute virtual time at which the process ends on its own.
        dies_at: Option<Duration>,
        /// What `find_by_comm` lists.
        listed: Vec<u32>,
        /// errno that every `signal` call fails with.
        signal_errno: Option<i32>,
        /// Pids for which `capture_hang` ran (it answers a fixed sentence).
        captured: RefCell<Vec<u32>>,
    }
    impl<'a> FakeProcs<'a> {
        fn new(
            clock: &'a FakeClock,
            die_after_int: Option<Duration>,
            die_after_kill: Option<Duration>,
        ) -> Self {
            Self {
                clock,
                die_after_int,
                die_after_kill,
                int_at: Cell::new(None),
                kill_at: Cell::new(None),
                signals: RefCell::new(vec![]),
                pids_signalled: RefCell::new(vec![]),
                gone: false,
                sigint_caught: None,
                age: Some(125.0),
                dies_at: None,
                listed: vec![4242],
                signal_errno: None,
                captured: RefCell::new(vec![]),
            }
        }
    }
    impl Procs for FakeProcs<'_> {
        fn capture_hang(&self, pid: u32) -> Option<String> {
            self.captured.borrow_mut().push(pid);
            Some("evidence saved: /tmp/x/hang-1.txt".into())
        }
        fn is_alive(&self, _pid: u32) -> bool {
            if self.gone {
                return false;
            }
            let now = self.clock.now();
            if self.dies_at.is_some_and(|t| now >= t) {
                return false;
            }
            if let (Some(t), Some(d)) = (self.int_at.get(), self.die_after_int) {
                if now >= t + d {
                    return false;
                }
            }
            if let (Some(t), Some(d)) = (self.kill_at.get(), self.die_after_kill) {
                if now >= t + d {
                    return false;
                }
            }
            true
        }
        fn comm(&self, _pid: u32) -> Option<String> {
            Some("monado-service".into())
        }
        fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
            if self.gone {
                return Err(io::Error::from_raw_os_error(libc::ESRCH));
            }
            if let Some(e) = self.signal_errno {
                return Err(io::Error::from_raw_os_error(e));
            }
            self.pids_signalled.borrow_mut().push(pid);
            self.signals.borrow_mut().push(sig);
            match sig {
                Signal::Int => self.int_at.set(Some(self.clock.now())),
                Signal::Term => {}
                Signal::Kill => self.kill_at.set(Some(self.clock.now())),
            }
            Ok(())
        }
        fn find_by_comm(&self, _comm: &str) -> Vec<u32> {
            if self.gone {
                vec![]
            } else {
                self.listed.clone()
            }
        }
        fn environ(&self, _pid: u32) -> Option<Vec<(String, String)>> {
            Some(vec![
                ("PIMAX_FPS".into(), "90".into()),
                ("XRT_COMPOSITOR_DESIRED_MODE".into(), "3".into()),
            ])
        }
        fn age_secs(&self, _pid: u32) -> Option<f64> {
            self.age
        }
        fn catches_sigint(&self, _pid: u32) -> Option<bool> {
            self.sigint_caught
        }
    }

    #[test]
    fn sigint_is_enough_for_a_polite_service() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, Some(Duration::from_millis(1300)), None);
        let mut warnings = vec![];
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(5),
            &mut |_| {},
            &mut |m| warnings.push(m.to_string()),
        );
        match out {
            StopOutcome::Graceful(t) => assert_eq!(t, Duration::from_millis(1300)),
            o => panic!("{o:?}"),
        }
        assert_eq!(*procs.signals.borrow(), vec![Signal::Int]);
        assert!(warnings.is_empty());
        assert!(procs.captured.borrow().is_empty());
    }

    #[test]
    fn a_stubborn_service_gets_sigkill_after_the_grace_period_with_a_warning() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(200)));
        let mut warnings = vec![];
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(5),
            &mut |_| {},
            &mut |m| warnings.push(m.to_string()),
        );
        assert_eq!(out, StopOutcome::Killed);
        assert_eq!(*procs.signals.borrow(), vec![Signal::Int, Signal::Kill]);
        assert_eq!(warnings.len(), 1);
        assert!(
            warnings[0].contains("did not exit 5 s after SIGINT"),
            "{}",
            warnings[0]
        );
        assert!(warnings[0].contains("SIGKILL"));
        // the evidence was collected (once) and named in the warning
        assert_eq!(*procs.captured.borrow(), vec![4242]);
        assert!(warnings[0].contains("evidence saved: /tmp/x/hang-1.txt"));
        // SIGKILL was sent only after the full grace period
        assert!(procs.kill_at.get().unwrap() >= Duration::from_secs(5));
    }

    #[test]
    fn an_unkillable_process_is_reported_as_stuck() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(1),
            &mut |_| {},
            &mut |_| {},
        );
        assert_eq!(out, StopOutcome::Stuck);
    }

    #[test]
    fn a_vanished_process_counts_as_stopped() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, None, None);
        procs.gone = true;
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(5),
            &mut |_| panic!("no note expected"),
            &mut |_| panic!("no warning expected"),
        );
        assert_eq!(out, StopOutcome::Graceful(Duration::ZERO));
    }

    #[derive(Default)]
    struct Recorder(RefCell<Vec<(bool, String, String)>>);
    impl Notifier for Recorder {
        fn notify(&self, critical: bool, summary: &str, body: &str) {
            self.0
                .borrow_mut()
                .push((critical, summary.into(), body.into()));
        }
    }

    fn deps<'a>(
        host: &'a Mock,
        procs: &'a FakeProcs<'a>,
        rec: &'a Recorder,
        clock: &'a FakeClock,
        env: &'a dyn Fn(&str) -> Option<String>,
    ) -> Deps<'a> {
        Deps {
            host,
            procs,
            notifier: rec,
            clock,
            env,
            stop_requested: &never,
        }
    }

    fn never() -> bool {
        false
    }

    fn scratch(name: &str) -> PathBuf {
        crate::vr::test_dir(&format!("session-{name}"))
    }

    #[test]
    fn stop_with_nothing_running_is_a_clean_noop_that_still_checks_the_kernel() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, None, None);
        procs.gone = true;
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-noop");
        assert!(stop(
            &o,
            Some(Duration::from_secs(5)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env)
        )
        .is_ok());
        assert!(
            rec.0.borrow().is_empty(),
            "nothing stopped, nothing to announce"
        );
        // with an Xid in the log the same call fails with exit code 2 and a critical notification
        let mut bad = Mock::healthy();
        bad.kernel = Ok(vec!["NVRM: Xid (PCI:0000:01:00): 31".into()]);
        let e = stop(
            &o,
            Some(Duration::from_secs(5)),
            false, true,
            &deps(&bad, &procs, &rec, &clock, &env),
        )
        .unwrap_err();
        assert_eq!(e.code, 2);
        assert!(rec
            .0
            .borrow()
            .iter()
            .any(|(crit, s, _)| *crit && s.contains("kernel log")));
    }

    #[test]
    fn stop_reports_success_and_removes_the_pid_file() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, Some(Duration::from_millis(500)), None);
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-ok");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        stop(
            &o,
            Some(Duration::from_secs(5)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap();
        assert!(!o.pid_path().exists());
        let n = rec.0.borrow();
        assert_eq!(n.len(), 1);
        assert_eq!(n[0].1, "Pimax VR stopped");
        assert!(!n[0].0);
    }

    #[test]
    fn stop_with_a_stuck_process_fails() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-stuck");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let e = stop(
            &o,
            Some(Duration::from_secs(1)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap_err();
        assert!(e.msg.contains("reboot"), "{}", e.msg);
        assert!(rec
            .0
            .borrow()
            .iter()
            .any(|(crit, s, _)| *crit && s.contains("forced kill")));
    }

    #[test]
    fn start_refuses_when_the_preflight_fails_and_force_overrides_the_refusal_only_up_to_the_binary(
    ) {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let mut host = Mock::healthy();
        host.kernel = Ok(vec!["NVRM: Xid (PCI:0000:01:00): 31".into()]);
        host.pids = vec![9];
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("start-refuse");
        o.service_bin = Some(PathBuf::from("/nonexistent/monado-service"));
        let e = start(&o, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("preflight failed"), "{}", e.msg);
        assert!(
            e.msg.contains("kernel:") && e.msg.contains("service:"),
            "{}",
            e.msg
        );
        assert!(rec.0.borrow()[0].0, "critical notification");
        assert!(!o.log_path().exists(), "nothing was started");
        // forced: the preflight is ignored, the missing binary is still caught before anything is spawned
        o.force = true;
        let e = start(&o, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("does not exist"), "{}", e.msg);
    }

    #[test]
    fn a_second_start_while_one_is_in_progress_is_refused_without_touching_anything() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("start-lock");
        o.service_bin = Some(PathBuf::from("/nonexistent/monado-service"));
        let first = StartLock::acquire(&o.session_dir).unwrap();
        assert!(StartLock::acquire(&o.session_dir).is_err());
        let e = start(&o, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("in progress"), "{}", e.msg);
        assert_eq!(rec.0.borrow()[0].1, "Pimax VR: already starting");
        assert!(!o.log_path().exists());
        drop(first);
        // lock released: the start now gets as far as the missing binary
        let e = start(&o, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("does not exist"), "{}", e.msg);
        // a dry run never needs the lock
        let _held = StartLock::acquire(&o.session_dir).unwrap();
        o.dry_run = true;
        assert!(start(&o, &deps(&host, &procs, &rec, &clock, &env)).is_ok());
    }

    #[test]
    fn dry_run_prints_the_plan_and_reports_the_preflight_result() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.dry_run = true;
        o.session_dir = scratch("dry");
        let host = Mock::healthy();
        assert!(start(&o, &deps(&host, &procs, &rec, &clock, &env)).is_ok());
        let mut bad = Mock::healthy();
        bad.usb = false;
        assert!(start(&o, &deps(&bad, &procs, &rec, &clock, &env)).is_err());
        assert!(rec.0.borrow().is_empty(), "a dry run never notifies");
        assert!(!o.session_dir.join("monado.log").exists());
    }

    #[test]
    fn status_lines_describe_a_running_service() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("status");
        std::fs::write(
            o.log_path(),
            format!(" INFO [pimax_hmd_create] pimax_video_prepare(90) -> 0\n{GYRO}\n"),
        )
        .unwrap();
        let (lines, running) = status_lines(&o, &deps(&host, &procs, &rec, &clock, &env), None);
        let text = lines.join("\n");
        assert!(running);
        assert!(
            text.contains("service: running, pid 4242, up 2m 05s"),
            "{text}"
        );
        assert!(
            text.contains("rate: 90 Hz (PIMAX_FPS), display mode index 3"),
            "{text}"
        );
        assert!(text.contains("gyro calibration done"), "{text}");
        assert!(text.contains("headset: 34a4:0044 on USB"), "{text}");
        assert!(text.contains("kernel: no Xid"), "{text}");
        assert!(!text.contains("wayland"), "{text}");
        let mut gone = FakeProcs::new(&clock, None, None);
        gone.gone = true;
        let (lines, running) = status_lines(&o, &deps(&host, &gone, &rec, &clock, &env), None);
        assert!(!running);
        assert_eq!(lines[0], "service: not running");
    }

    #[test]
    fn a_second_sigint_is_not_sent_when_monados_single_shot_handler_is_already_used_up() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, None, None);
        procs.sigint_caught = Some(false); // SA_RESETHAND: the first SIGINT/SIGTERM-triggered shutdown consumed it
        procs.dies_at = Some(Duration::from_secs(2));
        let mut notes = vec![];
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(5),
            &mut |m| notes.push(m.to_string()),
            &mut |m| panic!("no warning expected: {m}"),
        );
        assert_eq!(out, StopOutcome::Graceful(Duration::from_secs(2)));
        assert!(procs.signals.borrow().is_empty(), "no signal may be sent");
        assert!(notes[0].contains("already received a SIGINT"), "{notes:?}");
        // a process that is only a second old has not installed its handler yet: the SIGINT goes out
        let mut young = FakeProcs::new(&clock, Some(Duration::from_millis(300)), None);
        young.sigint_caught = Some(false);
        young.age = Some(1.0);
        let out = stop_pid(
            4242,
            &young,
            &clock,
            Duration::from_secs(5),
            &mut |_| {},
            &mut |_| {},
        );
        assert!(matches!(out, StopOutcome::Graceful(_)));
        assert_eq!(*young.signals.borrow(), vec![Signal::Int]);
        // unknown handler state: also sent
        let unknown = FakeProcs::new(&clock, Some(Duration::from_millis(300)), None);
        stop_pid(
            4242,
            &unknown,
            &clock,
            Duration::from_secs(5),
            &mut |_| {},
            &mut |_| {},
        );
        assert_eq!(*unknown.signals.borrow(), vec![Signal::Int]);
    }

    #[test]
    fn a_signal_error_other_than_esrch_is_not_reported_as_a_clean_exit() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, None, None);
        procs.signal_errno = Some(libc::EPERM);
        let out = stop_pid(
            4242,
            &procs,
            &clock,
            Duration::from_secs(5),
            &mut |_| {},
            &mut |_| {},
        );
        match out {
            StopOutcome::CannotSignal(m) => assert!(m.contains("SIGINT to pid 4242"), "{m}"),
            o => panic!("{o:?}"),
        }
    }

    #[test]
    fn stop_only_touches_the_service_of_the_pid_file_unless_all_is_given() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, Some(Duration::from_millis(200)), None);
        procs.listed = vec![7, 4242, 9001];
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-ours");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let t = stop_targets(&o, &procs);
        assert_eq!(t.ours, vec![4242]);
        assert_eq!(t.others, vec![7, 9001]);
        stop(
            &o,
            Some(Duration::from_secs(5)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap();
        assert_eq!(
            *procs.pids_signalled.borrow(),
            vec![4242],
            "the others are left alone"
        );
        // --all takes the others as well
        let mut procs = FakeProcs::new(&clock, Some(Duration::from_millis(200)), None);
        procs.listed = vec![7, 4242, 9001];
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        stop(
            &o,
            Some(Duration::from_secs(5)),
            true, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap();
        assert_eq!(*procs.pids_signalled.borrow(), vec![4242, 7, 9001]);
    }

    #[test]
    fn stop_without_a_pid_file_names_the_other_services_and_asks_for_all() {
        let clock = FakeClock::new();
        let mut procs = FakeProcs::new(&clock, Some(Duration::from_millis(200)), None);
        procs.listed = vec![555];
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-others");
        let e = stop(&o, None, false, true, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(
            e.msg.contains("pid 555") && e.msg.contains("--all"),
            "{}",
            e.msg
        );
        assert!(procs.pids_signalled.borrow().is_empty());
        stop(&o, None, true, true, &deps(&host, &procs, &rec, &clock, &env)).unwrap();
        assert_eq!(*procs.pids_signalled.borrow(), vec![555]);
    }

    #[test]
    fn a_connected_client_buys_monado_more_time_before_the_sigkill() {
        let mut o = opts();
        o.session_dir = scratch("grace");
        assert_eq!(
            default_grace(&o),
            (Duration::from_secs(5), false),
            "no log yet"
        );
        std::fs::write(
            o.log_path(),
            " INFO [client_connected] Client 1 connected\n INFO [client_disconnected] Client 1 disconnected\n",
        )
        .unwrap();
        assert_eq!(
            default_grace(&o),
            (Duration::from_secs(5), false),
            "client gone"
        );
        std::fs::write(
            o.log_path(),
            " INFO [client_connected] Client 1 connected\n INFO [client_disconnected] Client 1 disconnected\n INFO [client_connected] Client 2 connected\n",
        )
        .unwrap();
        assert_eq!(default_grace(&o), (Duration::from_secs(15), true));
        // and stop() really waits that long: a stubborn service is killed only after 15 virtual seconds
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(100)));
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let e = stop(&o, None, false, true, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("had to be killed"), "{}", e.msg);
        assert!(procs.kill_at.get().unwrap() >= Duration::from_secs(15));
        // an explicit --grace wins over the log
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(100)));
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let before = clock.now();
        let _ = stop(
            &o,
            Some(Duration::from_secs(2)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        );
        assert!(procs.kill_at.get().unwrap() - before < Duration::from_secs(4));
    }

    #[test]
    fn an_automatic_stop_never_sends_sigkill_and_latches_the_gpu_gate() {
        let clock = FakeClock::new();
        // would die on SIGKILL: it must never get one
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(100)));
        let host = Mock::healthy();
        let rec = Recorder::default();
        let base = scratch("stop-nokill");
        let proc_root = base.join("proc");
        std::fs::create_dir_all(proc_root.join("sys/kernel/random")).unwrap();
        std::fs::write(proc_root.join("sys/kernel/random/boot_id"), "boot-x\n").unwrap();
        let (run, pr) = (base.join("run").display().to_string(), proc_root.display().to_string());
        let env = move |k: &str| match k {
            "XDG_RUNTIME_DIR" => Some(run.clone()),
            "PIMAX_PROC_ROOT" => Some(pr.clone()),
            _ => None,
        };
        let mut o = opts();
        o.session_dir = base.join("session");
        std::fs::create_dir_all(&o.session_dir).unwrap();
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let e = stop(&o, Some(Duration::from_secs(2)), false, false, &deps(&host, &procs, &rec, &clock, &env)).unwrap_err();
        assert!(e.msg.contains("not killed"), "{}", e.msg);
        assert!(o.pid_path().exists(), "a hung service keeps its pid file");
        assert_eq!(*procs.signals.borrow(), vec![Signal::Int]);
        assert!(procs.kill_at.get().is_none());
        let n = rec.0.borrow();
        assert!(n.iter().any(|(crit, s, b)| *crit && s.contains("hung") && b.contains("NOT killed") && b.contains("hang-1.txt")), "{n:?}");
        let latch = gate::read_latch(&base.join("run/omarchyvrd/gpu-gate"), "boot-x").unwrap();
        assert_eq!(latch.0, gate::Level::NeedsReboot);
        assert!(latch.1.contains("hung"), "{}", latch.1);
    }

    #[test]
    fn stop_pid_without_kill_reports_hung() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(100)));
        let mut warnings = vec![];
        let out = stop_pid_with(4242, &procs, &clock, Duration::from_secs(3), false, &mut |_| {}, &mut |m| warnings.push(m.to_string()));
        assert_eq!(out, StopOutcome::Hung);
        assert_eq!(*procs.signals.borrow(), vec![Signal::Int]);
        assert_eq!(warnings.len(), 1);
        assert!(clock.now() >= Duration::from_secs(3));
    }

    #[test]
    fn a_forced_kill_is_an_error_and_gets_no_clean_stop_popup() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, Some(Duration::from_millis(100)));
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-killed");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        let e = stop(
            &o,
            Some(Duration::from_secs(1)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap_err();
        assert!(e.msg.contains("had to be killed"), "{}", e.msg);
        let n = rec.0.borrow();
        assert!(n
            .iter()
            .any(|(crit, s, b)| *crit && s.contains("forced kill") && b.contains("hang-1.txt")));
        assert!(!n.iter().any(|(_, s, _)| s == "Pimax VR stopped"), "{n:?}");
    }

    #[test]
    fn stop_waits_a_moment_before_it_reads_the_kernel_log() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, Some(Duration::from_millis(300)), None);
        let host = Mock::healthy();
        let rec = Recorder::default();
        let env = |_: &str| None;
        let mut o = opts();
        o.session_dir = scratch("stop-delay");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        stop(
            &o,
            Some(Duration::from_secs(5)),
            false, true,
            &deps(&host, &procs, &rec, &clock, &env),
        )
        .unwrap();
        // 0.3 s until it exited (rounded up to the 0.1 s polls) plus 1.5 s for late driver messages
        assert!(
            clock.now() >= Duration::from_millis(1800),
            "{:?}",
            clock.now()
        );
    }

    #[test]
    fn stop_lock_serialises_concurrent_stops() {
        let d = scratch("stop-lock");
        let first = StopLock::acquire(&d).unwrap();
        let d2 = d.clone();
        let t0 = std::time::Instant::now();
        let h = std::thread::spawn(move || {
            let _second = StopLock::acquire(&d2).unwrap(); // blocks until the first is dropped
            t0.elapsed()
        });
        std::thread::sleep(Duration::from_millis(300));
        drop(first);
        let waited = h.join().unwrap();
        assert!(waited >= Duration::from_millis(250), "{waited:?}");
    }

    #[test]
    fn the_session_dir_must_be_a_real_directory_of_ours() {
        let d = scratch("private-dir");
        assert!(ensure_private_dir(&d.join("new/nested")).is_ok());
        let target = d.join("elsewhere");
        std::fs::create_dir_all(&target).unwrap();
        let link = d.join("link");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let e = ensure_private_dir(&link).unwrap_err();
        assert!(e.msg.contains("not a directory owned by you"), "{}", e.msg);
        let file = d.join("file");
        std::fs::write(&file, "x").unwrap();
        assert!(ensure_private_dir(&file).is_err());
        match StartLock::acquire(&link) {
            Err(LockError::Failed(m)) => assert!(m.contains("not a directory owned by you"), "{m}"),
            Err(LockError::Busy) => panic!("busy"),
            Ok(_) => panic!("the symlinked session dir was accepted"),
        }
    }

    #[test]
    fn age_formatting() {
        assert_eq!(fmt_age(0.4), "0s");
        assert_eq!(fmt_age(59.9), "59s");
        assert_eq!(fmt_age(125.0), "2m 05s");
        assert_eq!(fmt_age(3723.0), "1h 02m 03s");
        assert_eq!(fmt_age(-5.0), "0s");
    }

    #[test]
    fn log_tail_reads_only_new_complete_lines_and_survives_truncation() {
        let d = scratch("tail");
        let p = d.join("monado.log");
        let mut t = LogTail::new(&p);
        assert!(t.poll().is_empty(), "file does not exist yet");
        std::fs::write(&p, "one\ntwo\npart").unwrap();
        assert_eq!(t.poll(), vec!["one".to_string(), "two".to_string()]);
        assert!(t.poll().is_empty());
        let mut f = std::fs::OpenOptions::new().append(true).open(&p).unwrap();
        std::io::Write::write_all(&mut f, b"ial\nthree\n").unwrap();
        assert_eq!(t.poll(), vec!["partial".to_string(), "three".to_string()]);
        std::fs::write(&p, "fresh\n").unwrap();
        assert_eq!(t.poll(), vec!["fresh".to_string()]);
    }

    #[test]
    fn session_files_roundtrip() {
        let mut o = opts();
        o.session_dir = scratch("files");
        o.fps = crate::vr::config::Fps::Hz90;
        o.dump_dir = Some(PathBuf::from("/tmp/dd"));
        write_session_files(&o, 777);
        assert_eq!(read_pid(&o.pid_path()), Some(777));
        let m: HashMap<_, _> = read_session_file(&o.summary_path()).into_iter().collect();
        assert_eq!(m["pid"], "777");
        assert_eq!(m["fps"], "90");
        assert_eq!(m["mode"], "3");
        assert_eq!(m["dump_dir"], "/tmp/dd");
        assert_eq!(read_pid(&o.session_dir.join("nope")), None);
    }

    #[test]
    fn service_pids_merges_the_pid_file_and_the_scan_without_duplicates() {
        let clock = FakeClock::new();
        let procs = FakeProcs::new(&clock, None, None);
        let mut o = opts();
        o.session_dir = scratch("pids");
        std::fs::write(o.pid_path(), "4242\n").unwrap();
        assert_eq!(service_pids(&o, &procs), vec![4242]);
        std::fs::write(o.pid_path(), "99\n").unwrap();
        assert_eq!(service_pids(&o, &procs), vec![99, 4242]);
    }
}
