//! Background jobs: a list of steps run in order on a thread, output streamed back line by line.
//!
//! Ordering guarantees live here: [`Step::WaitNoMonado`] (LESSONS row 1: before any start, wait until no
//! monado-service is left, with a timeout and a clear message) and [`Step::Guard`] (re-check the camera / eye chip right
//! before the launcher is called, so a check that passed a minute ago cannot be stale). Every [`Step::Run`] command is
//! validated by [`crate::safety::check`] first. The app runs one job at a time.
use crate::cmd::Cmd;
use crate::machine::Machine;
use crate::procs;
use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// A check run inside the job; gets a logger, returns an error to stop the job.
pub type GuardFn = Arc<dyn Fn(&dyn Machine, &mut dyn FnMut(String)) -> Result<(), String> + Send + Sync>;

/// One step.
#[derive(Clone)]
pub enum Step {
    /// Run a command; a non-zero exit ends the job with an error.
    Run(Cmd),
    /// Run a command; its exit code is only reported.
    RunAllowFail(Cmd),
    /// Wait until no monado-service runs (LESSONS row 1).
    WaitNoMonado(Duration),
    /// A check right before the next step.
    Guard(&'static str, GuardFn),
    /// A line for the log.
    Note(String),
}

impl std::fmt::Debug for Step {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Step::Run(c) => write!(f, "Run({})", c.display()),
            Step::RunAllowFail(c) => write!(f, "RunAllowFail({})", c.display()),
            Step::WaitNoMonado(d) => write!(f, "WaitNoMonado({d:?})"),
            Step::Guard(n, _) => write!(f, "Guard({n})"),
            Step::Note(s) => write!(f, "Note({s})"),
        }
    }
}

/// What a job is for (the UI derives its state from it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobKind {
    /// `pimax desk start`.
    DeskStart,
    /// `pimax desk stop` / `pimax vr stop`.
    Stop,
    /// A tool session started with `pimax vr start` (lens test, eye calibration).
    ToolSession(String),
    /// A setup fix.
    Fix(String),
    /// Anything else (hands test, runtime switch, ...).
    Other(String),
}

/// A job description.
#[derive(Debug, Clone)]
pub struct JobSpec {
    /// Kind.
    pub kind: JobKind,
    /// Title for the log.
    pub label: String,
    /// Steps in order.
    pub steps: Vec<Step>,
}

/// Event from a running job.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JobEvent {
    /// Output or progress line.
    Line(String),
    /// Finished.
    Done(Result<(), String>),
}

/// A running job.
pub struct Job {
    /// Kind.
    pub kind: JobKind,
    /// Title.
    pub label: String,
    /// When it started.
    pub started: Instant,
    /// Where its output goes (see [`set_log_dir`]).
    pub log_file: Option<PathBuf>,
    rx: Receiver<JobEvent>,
}

impl Job {
    /// Drain queued events.
    pub fn poll(&self) -> Vec<JobEvent> {
        self.rx.try_iter().collect()
    }
}

/// Time source of the waiting steps (faked in tests).
pub trait Clock: Send + Sync {
    /// Monotonic time.
    fn now(&self) -> Duration;
    /// Sleep (tests advance their fake time).
    fn sleep(&self, d: Duration);
}

/// Wall clock.
pub struct RealClock(Instant);

impl Default for RealClock {
    fn default() -> Self {
        RealClock(Instant::now())
    }
}

impl Clock for RealClock {
    fn now(&self) -> Duration {
        self.0.elapsed()
    }
    fn sleep(&self, d: Duration) {
        std::thread::sleep(d);
    }
}

/// LESSONS row 1: wait until `pgrep -x monado-service` is empty, or give up with a message that says what to do.
pub fn wait_no_monado(
    m: &dyn Machine,
    clock: &dyn Clock,
    timeout: Duration,
    log: &mut dyn FnMut(String),
) -> Result<(), String> {
    let t0 = clock.now();
    let mut last_note = Duration::ZERO;
    loop {
        let pids = procs::monado_pids(m);
        if pids.is_empty() {
            return Ok(());
        }
        let waited = clock.now().saturating_sub(t0);
        if waited >= timeout {
            let list: Vec<String> = pids.iter().map(u32::to_string).collect();
            return Err(format!(
                "monado-service (pid {}) is still running after {:.0} s. A second Monado cannot start while it exits. \
                 Stop it with `pimax vr stop --grace 30 --all` (Desk page) and start again; if it does not exit, \
                 reboot (a stuck NVIDIA flip only clears with a reboot)",
                list.join(", "),
                timeout.as_secs_f64()
            ));
        }
        if waited == Duration::ZERO || waited.saturating_sub(last_note) >= Duration::from_secs(5) {
            last_note = waited;
            let list: Vec<String> = pids.iter().map(u32::to_string).collect();
            log(format!(
                "waiting for the previous monado-service (pid {}) to exit ({:.0} s of {:.0} s)",
                list.join(", "),
                waited.as_secs_f64(),
                timeout.as_secs_f64()
            ));
        }
        clock.sleep(Duration::from_millis(250));
    }
}

static LOG_DIR: OnceLock<PathBuf> = OnceLock::new();
static SEQ: AtomicU64 = AtomicU64::new(0);

/// Job output files go to `dir` (`~/.local/state/pimax/jobs`; the newest 40 are kept). A started command writes into
/// its file, not into a pipe of the app: when the app crashes or is closed during a start, the launcher goes on
/// (no SIGPIPE) and its output stays readable. Without a directory (tests) output is read through pipes.
pub fn set_log_dir(dir: PathBuf) {
    let _ = std::fs::create_dir_all(&dir);
    let mut v: Vec<PathBuf> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().is_some_and(|x| x == "log")).collect())
        .unwrap_or_default();
    v.sort();
    let n = v.len().saturating_sub(40);
    for p in v.into_iter().take(n) {
        let _ = std::fs::remove_file(p);
    }
    let _ = LOG_DIR.set(dir);
}

fn log_file_for(label: &str) -> Option<PathBuf> {
    let dir = LOG_DIR.get()?;
    let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let slug: String = label
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '-' })
        .collect::<String>()
        .split('-')
        .filter(|w| !w.is_empty())
        .take(6)
        .collect::<Vec<_>>()
        .join("-");
    let n = SEQ.fetch_add(1, Ordering::Relaxed);
    Some(dir.join(format!("{}-{n}-{slug}.log", pimax_config::store::stamp(secs))))
}

/// Start a job on a thread. `wake` is called whenever an event is queued (repaint). A panic in the job ends it with
/// an error instead of taking the app down.
pub fn spawn(spec: JobSpec, m: Arc<dyn Machine>, wake: Arc<dyn Fn() + Send + Sync>) -> Job {
    let (tx, rx) = channel();
    let kind = spec.kind.clone();
    let label = spec.label.clone();
    let log_file = log_file_for(&spec.label);
    let file = log_file.clone();
    let body = move || {
        let clock = RealClock::default();
        if let Some(f) = &file {
            let _ = std::fs::write(f, format!("== {}\n", spec.label));
        }
        let r = match crate::crash::catching(|| run_steps_to(&spec.steps, m.as_ref(), &clock, &tx, &*wake, file.as_deref())) {
            Ok(r) => r,
            Err(p) => Err(format!("internal error: {}", crate::worker::panic_text(p.as_ref()))),
        };
        if let Some(f) = &file {
            if let Ok(mut h) = OpenOptions::new().append(true).open(f) {
                let _ = writeln!(h, "== {}", match &r {
                    Ok(()) => "done".to_string(),
                    Err(e) => format!("failed: {e}"),
                });
            }
        }
        let _ = tx.send(JobEvent::Done(r));
        wake();
    };
    if let Err(e) = std::thread::Builder::new().name("pimax-job".into()).spawn(body) {
        log::error!("cannot start the job thread: {e}");
    }
    Job {
        kind,
        label,
        started: Instant::now(),
        log_file,
        rx,
    }
}

/// Run steps synchronously, output through pipes (tests).
pub fn run_steps(
    steps: &[Step],
    m: &dyn Machine,
    clock: &dyn Clock,
    tx: &Sender<JobEvent>,
    wake: &(dyn Fn() + Send + Sync),
) -> Result<(), String> {
    run_steps_to(steps, m, clock, tx, wake, None)
}

/// Run steps synchronously (the thread body); commands write into `log` when given.
pub fn run_steps_to(
    steps: &[Step],
    m: &dyn Machine,
    clock: &dyn Clock,
    tx: &Sender<JobEvent>,
    wake: &(dyn Fn() + Send + Sync),
    log: Option<&Path>,
) -> Result<(), String> {
    // the last line a command printed: a failed step names it, so the app shows the reason in plain words
    let last = std::cell::RefCell::new(String::new());
    let send = |l: String| {
        if !l.trim().is_empty() && !l.starts_with("$ ") {
            *last.borrow_mut() = l.trim().to_string();
        }
        let _ = tx.send(JobEvent::Line(l));
        wake();
    };
    for s in steps {
        match s {
            Step::Note(t) => send(t.clone()),
            Step::WaitNoMonado(t) => wait_no_monado(m, clock, *t, &mut |l| send(l))?,
            Step::Guard(name, f) => {
                send(format!("check: {name}"));
                f(m, &mut |l| send(l))?;
            }
            Step::Run(c) | Step::RunAllowFail(c) => {
                last.borrow_mut().clear();
                let code = run_cmd(c, &send, log)?;
                let ok = code == Some(0);
                if !ok {
                    let what = match code {
                        Some(n) => format!("exit code {n}"),
                        None => "killed by a signal".into(),
                    };
                    if matches!(s, Step::Run(_)) {
                        let why = last.borrow().clone();
                        return Err(if why.is_empty() {
                            format!("`{}` failed ({what})", short(&c.display()))
                        } else {
                            format!("{why} (`{}` failed, {what})", short(&c.display()))
                        });
                    }
                    send(format!("(`{}` ended with {what})", short(&c.display())));
                }
            }
        }
    }
    Ok(())
}

fn short(s: &str) -> String {
    let first = s.lines().next().unwrap_or("");
    if first.chars().count() > 120 || s.contains('\n') {
        format!("{}...", first.chars().take(117).collect::<String>())
    } else {
        first.to_string()
    }
}

/// Start the process (after the safety check) and stream its output; returns the exit code.
fn run_cmd(c: &Cmd, send: &dyn Fn(String), log: Option<&Path>) -> Result<Option<i32>, String> {
    crate::safety::check(c).map_err(|e| format!("refused `{}`: {e}", short(&c.display())))?;
    crate::cmd::allow_start(&short(&c.display()))?;
    send(format!("$ {}", c.display()));
    let mut cmd = Command::new(&c.program);
    cmd.args(&c.args).stdin(Stdio::null());
    for k in &c.env_remove {
        cmd.env_remove(k);
    }
    cmd.envs(c.env.iter().map(|(k, v)| (k.as_str(), v.as_str())));
    if let Some(d) = &c.cwd {
        cmd.current_dir(d);
    }
    if let Some(path) = log {
        return run_to_file(c, cmd, path, send);
    }
    cmd.stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn().map_err(|e| format!("cannot start {}: {e}", c.program))?;
    let (ltx, lrx) = channel::<String>();
    let mut readers = Vec::new();
    if let Some(o) = child.stdout.take() {
        let t = ltx.clone();
        readers.push(std::thread::spawn(move || pump(o, &|l| {
            let _ = t.send(l);
        })));
    }
    if let Some(e) = child.stderr.take() {
        let t = ltx.clone();
        readers.push(std::thread::spawn(move || pump(e, &|l| {
            let _ = t.send(l);
        })));
    }
    drop(ltx);
    for l in lrx {
        send(l);
    }
    for r in readers {
        let _ = r.join();
    }
    let st = child.wait().map_err(|e| e.to_string())?;
    Ok(st.code())
}

/// The command writes into the job's file (stdout and stderr); the new part of the file is followed until it exits.
fn run_to_file(c: &Cmd, mut cmd: Command, path: &Path, send: &dyn Fn(String)) -> Result<Option<i32>, String> {
    let e = |e: std::io::Error| format!("{}: {e}", path.display());
    let mut out = OpenOptions::new().create(true).append(true).open(path).map_err(e)?;
    let _ = writeln!(out, "$ {}", c.display());
    let start = out.metadata().map(|m| m.len()).unwrap_or(0);
    let err = out.try_clone().map_err(e)?;
    cmd.stdout(out).stderr(err);
    let mut child = cmd.spawn().map_err(|x| format!("cannot start {}: {x}", c.program))?;
    let mut reader = std::fs::File::open(path).map_err(e)?;
    reader.seek(SeekFrom::Start(start)).map_err(e)?;
    let mut lines = LineSplitter::default();
    let mut buf = [0u8; 8192];
    let mut drain = |reader: &mut std::fs::File, lines: &mut LineSplitter| loop {
        match reader.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => lines.push(&buf[..n], send),
        }
    };
    let status = loop {
        drain(&mut reader, &mut lines);
        match child.try_wait() {
            Ok(Some(st)) => break Some(st),
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => break child.wait().ok(),
        }
    };
    drain(&mut reader, &mut lines);
    lines.finish(send);
    Ok(status.and_then(|s| s.code()))
}

/// Splits a byte stream into lines (invalid UTF-8 replaced, `\r` dropped, very long lines cut into pieces).
#[derive(Default)]
pub struct LineSplitter {
    line: Vec<u8>,
}

impl LineSplitter {
    fn emit(&mut self, out: &dyn Fn(String)) {
        if self.line.last() == Some(&b'\r') {
            self.line.pop();
        }
        out(String::from_utf8_lossy(&self.line).into_owned());
        self.line.clear();
    }

    /// Feed bytes; complete lines go to `out`.
    pub fn push(&mut self, data: &[u8], out: &dyn Fn(String)) {
        for &b in data {
            if b == b'\n' {
                self.emit(out);
            } else {
                self.line.push(b);
                if self.line.len() >= MAX_LINE {
                    self.emit(out);
                }
            }
        }
    }

    /// The rest (a last line without a line break).
    pub fn finish(&mut self, out: &dyn Fn(String)) {
        if !self.line.is_empty() {
            self.emit(out);
        }
    }
}

/// Longest line passed on (longer ones are cut into pieces).
pub const MAX_LINE: usize = 4096;

/// Read a pipe to EOF and hand out its lines: invalid UTF-8 is replaced, very long lines are split, and the pipe is
/// always drained (a reader that stopped early would kill a build with SIGPIPE).
pub fn pump(mut r: impl Read, out: &dyn Fn(String)) {
    let mut buf = [0u8; 8192];
    let mut lines = LineSplitter::default();
    loop {
        let n = match r.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        lines.push(&buf[..n], out);
    }
    lines.finish(out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::sync::Mutex;

    /// Virtual time.
    #[derive(Default)]
    struct FakeClock(AtomicU64);
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            Duration::from_millis(self.0.load(Ordering::SeqCst))
        }
        fn sleep(&self, d: Duration) {
            self.0.fetch_add(d.as_millis() as u64, Ordering::SeqCst);
        }
    }

    /// A machine whose monado-service exits after a while (its stat turns into a zombie).
    struct Exiting {
        m: Mutex<FakeMachine>,
        polls: AtomicU64,
        exit_after: u64,
    }
    impl pimax_doctor::Host for Exiting {
        fn read(&self, p: &std::path::Path) -> Option<Vec<u8>> {
            if p.ends_with("stat") && self.polls.fetch_add(1, Ordering::SeqCst) >= self.exit_after {
                return Some(b"9 (monado-service) Z 1".to_vec());
            }
            self.m.lock().unwrap().read(p)
        }
        fn list(&self, p: &std::path::Path) -> Vec<String> {
            self.m.lock().unwrap().list(p)
        }
        fn exists(&self, p: &std::path::Path) -> bool {
            self.m.lock().unwrap().exists(p)
        }
        fn can_rw(&self, _: &std::path::Path) -> bool {
            false
        }
        fn run(&self, _: &str, _: &[&str]) -> Option<pimax_doctor::CmdOut> {
            None
        }
        fn env(&self, _: &str) -> Option<String> {
            None
        }
    }
    impl Machine for Exiting {
        fn host(&self) -> &dyn pimax_doctor::Host {
            self
        }
        fn read_link(&self, _: &std::path::Path) -> Option<std::path::PathBuf> {
            None
        }
        fn meta(&self, _: &std::path::Path) -> Option<crate::machine::FileMeta> {
            None
        }
        fn free_bytes(&self, _: &std::path::Path) -> Option<u64> {
            None
        }
        fn now_unix(&self) -> u64 {
            0
        }
        fn uid(&self) -> u32 {
            1000
        }
        fn read_range(&self, _: &std::path::Path, _: u64, _: usize) -> Option<Vec<u8>> {
            None
        }
    }

    fn exiting(after: u64) -> Exiting {
        let mut m = FakeMachine::new();
        m.process(9, "monado-service", "/b/monado-service", &[], 1, &[]);
        Exiting {
            m: Mutex::new(m),
            polls: AtomicU64::new(0),
            exit_after: after,
        }
    }

    #[test]
    fn waits_for_the_previous_monado_then_continues() {
        let m = exiting(12);
        let clock = FakeClock::default();
        let mut lines = Vec::new();
        assert_eq!(wait_no_monado(&m, &clock, Duration::from_secs(45), &mut |l| lines.push(l)), Ok(()));
        assert!(clock.now() >= Duration::from_secs(2), "{:?}", clock.now());
        assert!(lines[0].contains("waiting for the previous monado-service (pid 9)"), "{lines:?}");
    }

    #[test]
    fn gives_up_with_a_clear_message() {
        let m = exiting(u64::MAX);
        let clock = FakeClock::default();
        let mut lines = Vec::new();
        let e = wait_no_monado(&m, &clock, Duration::from_secs(45), &mut |l| lines.push(l)).unwrap_err();
        assert!(e.contains("pid 9") && e.contains("pimax vr stop --grace 30 --all"), "{e}");
        assert!(clock.now() >= Duration::from_secs(45));
        assert!(lines.len() >= 9, "a progress line every 5 s: {}", lines.len());
    }

    #[test]
    fn steps_run_in_order_and_stop_at_the_first_failure() {
        let m = FakeMachine::new();
        let (tx, rx) = channel();
        let guard: GuardFn = Arc::new(|_, log| {
            log("camera free".into());
            Ok(())
        });
        let r = run_steps(
            &[
                Step::WaitNoMonado(Duration::from_secs(1)),
                Step::Guard("camera", guard),
                Step::Run(Cmd::shell("echo one; echo two >&2")),
                Step::RunAllowFail(Cmd::shell("exit 3")),
                Step::Run(Cmd::shell("exit 4")),
                Step::Note("never".into()),
            ],
            &m,
            &FakeClock::default(),
            &tx,
            &|| {},
        );
        assert_eq!(r, Err("`/bin/sh -c 'exit 4'` failed (exit code 4)".to_string()));
        let lines: Vec<String> = rx
            .try_iter()
            .filter_map(|e| match e {
                JobEvent::Line(l) => Some(l),
                JobEvent::Done(_) => None,
            })
            .collect();
        assert!(lines.contains(&"camera free".to_string()));
        assert!(lines.contains(&"one".to_string()) && lines.contains(&"two".to_string()));
        assert!(lines.iter().any(|l| l.contains("ended with exit code 3")));
        assert!(!lines.contains(&"never".to_string()));
    }

    #[test]
    fn unsafe_commands_never_start() {
        let m = FakeMachine::new();
        let (tx, _rx) = channel();
        let r = run_steps(&[Step::Run(Cmd::shell("pkill -f monado-service"))], &m, &FakeClock::default(), &tx, &|| {});
        assert!(r.unwrap_err().starts_with("refused"));
        let failing_guard: GuardFn = Arc::new(|_, _| Err("camera busy: pid 7".into()));
        let r = run_steps(
            &[Step::Guard("camera", failing_guard), Step::Run(Cmd::shell("echo started"))],
            &m,
            &FakeClock::default(),
            &tx,
            &|| {},
        );
        assert_eq!(r, Err("camera busy: pid 7".into()));
    }

    #[test]
    fn pump_survives_invalid_utf8_and_long_lines() {
        let data: Vec<u8> = [b"ok\r\n".as_slice(), &[0xff, 0xfe, b'x', b'\n'], &vec![b'a'; MAX_LINE + 10], b"\nlast"].concat();
        let got = std::cell::RefCell::new(Vec::new());
        pump(&data[..], &|l| got.borrow_mut().push(l));
        let got = got.into_inner();
        assert_eq!(got[0], "ok");
        assert_eq!(got[1], "\u{fffd}\u{fffd}x");
        assert_eq!(got[2].len(), MAX_LINE);
        assert_eq!(got[3], "a".repeat(10));
        assert_eq!(got[4], "last");
    }

    #[test]
    fn spawn_reports_done() {
        let j = spawn(
            JobSpec {
                kind: JobKind::Other("t".into()),
                label: "t".into(),
                steps: vec![Step::Run(Cmd::shell("echo hi"))],
            },
            Arc::new(FakeMachine::new()),
            Arc::new(|| {}),
        );
        let mut done = None;
        let mut lines = Vec::new();
        for _ in 0..300 {
            for e in j.poll() {
                match e {
                    JobEvent::Line(l) => lines.push(l),
                    JobEvent::Done(r) => done = Some(r),
                }
            }
            if done.is_some() {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(done, Some(Ok(())));
        assert!(lines.contains(&"hi".to_string()));
    }

    #[test]
    fn output_through_a_file_survives_and_is_streamed() {
        let d = std::env::temp_dir().join(format!("pimax-job-file-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let f = d.join("job.log");
        let (tx, rx) = channel();
        let r = run_steps_to(
            &[
                Step::Run(Cmd::shell("echo out; echo err >&2; printf 'no newline'")),
                Step::RunAllowFail(Cmd::shell("exit 3")),
            ],
            &FakeMachine::new(),
            &FakeClock::default(),
            &tx,
            &|| {},
            Some(&f),
        );
        assert_eq!(r, Ok(()));
        let lines: Vec<String> = rx.try_iter().filter_map(|e| if let JobEvent::Line(l) = e { Some(l) } else { None }).collect();
        for want in ["out", "err", "no newline"] {
            assert!(lines.iter().any(|l| l == want), "{want} in {lines:?}");
        }
        assert!(lines.iter().any(|l| l.contains("ended with exit code 3")), "{lines:?}");
        let text = std::fs::read_to_string(&f).unwrap();
        assert!(text.contains("$ /bin/sh -c") && text.contains("out\n") && text.contains("no newline"), "{text}");
        let _ = std::fs::remove_dir_all(d);
    }
}
