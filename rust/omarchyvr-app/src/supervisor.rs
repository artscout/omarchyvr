//! The supervisor: what `omarchyvr-app` runs by default. A small process without any graphics that
//! * holds the single-instance lock (a second start brings the existing window to the front),
//! * starts the window process (`omarchyvr-app --window`) with a renderer: wgpu first, glow when wgpu cannot start,
//!   X11 as a last resort,
//! * receives minidumps of native crashes (it is the window's parent, so it may read its memory),
//! * and when the window process dies, writes what happened (crash report with the window's stderr, its last
//!   breadcrumbs, the minidump) and starts it again: at most 3 times in 2 minutes, then it gives up with a desktop
//!   notification. A normal close, logout (SIGTERM/SIGHUP) or Ctrl+C ends both.
//!
//! The supervisor never touches Monado, the headset or any VR process; it only signals its own child, by pid.
use crate::window::{self, code, Renderer};
use omarchyvr_gui_core::crash::{self, Kind};
use omarchyvr_gui_core::{applog, instance};
use std::os::unix::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

static TERM: AtomicI32 = AtomicI32::new(0);
static FOCUS: AtomicBool = AtomicBool::new(false);

extern "C" fn on_term(sig: libc::c_int) {
    TERM.store(sig, Ordering::SeqCst);
}

extern "C" fn on_usr1(_: libc::c_int) {
    FOCUS.store(true, Ordering::SeqCst);
}

fn install_signals() {
    // SAFETY: sigaction(2) with handlers that only store to atomics (async-signal-safe).
    unsafe {
        for (sig, f) in [
            (libc::SIGTERM, on_term as *const () as usize),
            (libc::SIGINT, on_term as *const () as usize),
            (libc::SIGHUP, on_term as *const () as usize),
            (libc::SIGUSR1, on_usr1 as *const () as usize),
        ] {
            let mut sa: libc::sigaction = std::mem::zeroed();
            sa.sa_sigaction = f;
            sa.sa_flags = libc::SA_RESTART;
            libc::sigemptyset(&mut sa.sa_mask);
            libc::sigaction(sig, &sa, std::ptr::null_mut());
        }
    }
}

/// Send `sig` to our own child.
fn signal_child(c: &Child, sig: libc::c_int) {
    let pid = c.id();
    if pid > 1 && pid <= i32::MAX as u32 {
        // SAFETY: kill(2) with our own child's positive pid (never a group).
        unsafe {
            libc::kill(pid as libc::pid_t, sig);
        }
    }
}

/// A desktop notification (fire and forget).
pub fn notify(title: &str, body: &str) {
    if omarchyvr_gui_core::cmd::dry_run() || std::env::var("PIMAX_NO_NOTIFY").is_ok_and(|v| v == "1") {
        log::info!("(no notification) {title}: {body}");
        return;
    }
    if let Ok(mut c) = Command::new("notify-send")
        .args(["-a", "omarchyvr", "-i", "omarchyvr", title, body])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    {
        std::thread::spawn(move || {
            let _ = c.wait();
        });
    }
}

/// Minidumps from the crash server: the newest one, and when it came.
type LastDump = Arc<Mutex<Option<(PathBuf, Instant)>>>;

struct DumpHandler {
    dir: PathBuf,
    last: LastDump,
}

impl minidumper::ServerHandler for DumpHandler {
    fn create_minidump_file(&self) -> Result<(std::fs::File, PathBuf), std::io::Error> {
        std::fs::create_dir_all(&self.dir)?;
        let p = self.dir.join(format!("{}.dmp", applog::now_utc().replace([':', '-', '.'], "")));
        Ok((std::fs::File::create(&p)?, p))
    }

    fn on_minidump_created(&self, r: Result<minidumper::MinidumpBinary, minidumper::Error>) -> minidumper::LoopAction {
        match r {
            Ok(b) => {
                log::warn!("minidump of the window process written: {}", b.path.display());
                if let Ok(mut l) = self.last.lock() {
                    *l = Some((b.path, Instant::now()));
                }
            }
            Err(e) => log::error!("minidump failed: {e}"),
        }
        minidumper::LoopAction::Continue
    }

    fn on_message(&self, _kind: u32, _buffer: Vec<u8>) {}
}

/// The crash server on `$XDG_RUNTIME_DIR/omarchyvr-app-<pid>.crash.sock`, on its own thread.
fn start_crash_server(state: &Path, last: LastDump, stop: Arc<AtomicBool>) -> Option<PathBuf> {
    let base = std::env::var("XDG_RUNTIME_DIR").ok().filter(|d| !d.is_empty()).map(PathBuf::from).unwrap_or_else(std::env::temp_dir);
    let sock = base.join(format!("omarchyvr-app-{}.crash.sock", std::process::id()));
    let _ = std::fs::remove_file(&sock);
    let name = sock.to_str()?.to_string();
    let mut server = match minidumper::Server::with_name(minidumper::SocketName::path(&name)) {
        Ok(s) => s,
        Err(e) => {
            log::warn!("no crash dumps: cannot start the crash server ({e})");
            return None;
        }
    };
    let handler = DumpHandler { dir: crash::crashes_dir(state).join("dumps"), last };
    let _ = std::thread::Builder::new().name("pimax-crash-server".into()).spawn(move || {
        if let Err(e) = server.run(Box::new(handler), &stop, None) {
            log::warn!("crash server ended: {e}");
        }
    });
    Some(sock)
}

/// What the supervisor knows when the window process ended.
struct Ended<'a> {
    state: &'a Path,
    pid: u32,
    renderer: Renderer,
    x11: bool,
    last_dump: &'a LastDump,
}

impl Ended<'_> {
    /// Write a report from the outside: the window's breadcrumbs, the log, its stderr, its minidump.
    fn report(&self, kind: Kind, summary: String, details: String) -> Option<PathBuf> {
        let mut r = crash::Report::new(kind, summary, details);
        let ctx_file = crash::crashes_dir(self.state).join(format!(".context-{}", self.pid));
        let mut ctx: Vec<(String, String)> = std::fs::read_to_string(&ctx_file)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| l.split_once(": ").map(|(k, v)| (k.to_string(), v.to_string())))
            .collect();
        // the window's own breadcrumbs first (page, job, action); what the supervisor knows only when missing
        ctx.retain(|(k, _)| k != "renderer");
        if !ctx.iter().any(|(k, _)| k == "pid") {
            ctx.push(("pid".into(), self.pid.to_string()));
        }
        ctx.push(("renderer".into(), format!("{}{}", self.renderer.name(), if self.x11 { " (X11)" } else { "" })));
        ctx.push(("written by".into(), "supervisor".into()));
        r.context = ctx;
        r.log_tail = applog::tail_file(&applog::log_file(self.state), 150);
        r.stderr_tail = applog::tail_file(&window::stderr_log(self.state), 80);
        let dump = self
            .last_dump
            .lock()
            .ok()
            .and_then(|mut l| l.take())
            .filter(|(_, t)| t.elapsed() < Duration::from_secs(60))
            .map(|(p, _)| p);
        if dump.is_some() {
            r.files.push("minidump.dmp".into());
        }
        let dir = crash::write(self.state, &r).ok()?;
        if let Some(d) = dump {
            if std::fs::rename(&d, dir.join("minidump.dmp")).is_err() {
                let _ = std::fs::copy(&d, dir.join("minidump.dmp"));
            }
        }
        log::error!("{} (report {})", r.summary, dir.display());
        Some(dir)
    }

    /// Move the window's latest minidump (if one came in the last minute) into a report directory.
    fn attach_dump(&self, dir: &Path) {
        let dump = self.last_dump.lock().ok().and_then(|mut l| l.take()).filter(|(_, t)| t.elapsed() < Duration::from_secs(60)).map(|(p, _)| p);
        if let Some(d) = dump {
            if std::fs::rename(&d, dir.join("minidump.dmp")).is_err() {
                let _ = std::fs::copy(&d, dir.join("minidump.dmp"));
            }
            log::info!("minidump attached to {}", dir.display());
        }
    }

    /// The report the window process wrote itself (panic hook, renderer error), if any.
    fn own_report(&self) -> Option<PathBuf> {
        crash::list(self.state)
            .into_iter()
            .take(5)
            .find(|(_, r, _)| r.ctx("pid") == Some(&self.pid.to_string()) && r.kind.fatal())
            .map(|(d, _, _)| d)
    }

    fn cleanup(&self) {
        let _ = std::fs::remove_file(crash::crashes_dir(self.state).join(format!(".context-{}", self.pid)));
        let _ = std::fs::remove_file(crash::hang_marker(self.state, self.pid));
    }
}

/// What to do after the window process ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Next {
    /// Done: exit with this code.
    Exit(i32),
    /// Start it again (a crash: counted against the restart budget).
    Restart,
    /// Start it with the next renderer of the chain (not counted).
    NextRenderer,
    /// The supervisor ended a window process that had no window (see [`Watch`]): start a new one after a short wait.
    Respawn,
}

/// Why the supervisor ended a window process itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Why {
    /// No window appeared within the start limit (login autostart before the display was ready, a stuck start).
    NoWindow,
    /// A second start asked to show the window and it did not answer.
    NoAnswer,
}

/// Seconds the window process gets to draw its first frame.
const START_SECS: f64 = 25.0;
/// Seconds the window gets to answer "show yourself".
const RAISE_SECS: f64 = 3.0;

fn env_secs(name: &str, default: f64) -> f64 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Watches one window process through its ack file (`<pid> <counter>`, see `window::write_ack`). Pure: times and the
/// file's text come in as arguments, so the tests need no process.
#[derive(Debug, Clone)]
pub struct Watch {
    pid: u32,
    start_limit: f64,
    raise_limit: f64,
    seen: bool,
    raise: Option<(f64, String)>,
}

impl Watch {
    /// A watch of the window process `pid` with the limits in seconds.
    pub fn new(pid: u32, start_limit: f64, raise_limit: f64) -> Watch {
        Watch { pid, start_limit, raise_limit, seen: false, raise: None }
    }

    /// The window drew its first frame.
    pub fn has_window(&self) -> bool {
        self.seen
    }

    fn mine(&self, ack: &str) -> bool {
        ack.split_whitespace().next().is_some_and(|p| p == self.pid.to_string())
    }

    /// A second start asked to show the window (`ack` = the file's text now).
    pub fn raise_asked(&mut self, now: f64, ack: &str) {
        if self.raise.is_none() {
            self.raise = Some((now, ack.to_string()));
        }
    }

    /// One look. `since_spawn`: seconds since the process started; `now`: seconds on the same clock as
    /// [`Watch::raise_asked`]; `ack`: the ack file's text. Returns why the process must be ended, if so.
    pub fn check(&mut self, since_spawn: f64, now: f64, ack: &str) -> Option<Why> {
        if !self.seen && self.mine(ack) {
            self.seen = true;
        }
        if !self.seen {
            return (since_spawn > self.start_limit).then_some(Why::NoWindow);
        }
        if let Some((t, base)) = &self.raise {
            if self.mine(ack) && ack != base {
                self.raise = None;
            } else if now - t > self.raise_limit {
                return Some(Why::NoAnswer);
            }
        }
        None
    }
}

/// Does Hyprland's client list (`hyprctl -j clients`) have a window of `pid`? Pure: the JSON comes in as text.
pub fn hypr_lists_pid(json: &str, pid: u32) -> bool {
    json.match_indices("\"pid\":").any(|(i, k)| {
        let rest = json[i + k.len()..].trim_start();
        let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
        digits.parse::<u32>().is_ok_and(|p| p == pid)
    })
}

/// Has `pid` a window on the desktop? Asks the compositor; `None` when it cannot be asked (not Hyprland, no answer
/// within 2 s). The ack file cannot tell: a window process whose surface the compositor never mapped (or dropped)
/// still draws and answers "show yourself", and winit cannot map it again (`Visible(true)` does nothing on Wayland).
fn desktop_has_window(pid: u32) -> Option<bool> {
    std::env::var("HYPRLAND_INSTANCE_SIGNATURE").ok().filter(|s| !s.is_empty())?;
    let mut c = Command::new("hyprctl").args(["-j", "clients"]).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::null()).spawn().ok()?;
    let end = Instant::now() + Duration::from_secs(2);
    loop {
        match c.try_wait() {
            Ok(Some(st)) if st.success() => break,
            Ok(None) if Instant::now() < end => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                let _ = c.kill();
                let _ = c.wait();
                return None;
            }
            _ => return None,
        }
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut c.stdout.take()?, &mut out).ok()?;
    // an answer without any client list (an error text) proves nothing
    out.trim_start().starts_with('[').then(|| hypr_lists_pid(&out, pid))
}

/// Seconds after the first frame, and after a second start, before the desktop is asked whether the window is there.
const DESKTOP_CHECK_SECS: f64 = 5.0;

/// Is the Wayland socket there? (No `WAYLAND_DISPLAY`: nothing to wait for, true.)
pub fn wayland_ready(env: &dyn Fn(&str) -> Option<String>) -> bool {
    let Some(name) = env("WAYLAND_DISPLAY").filter(|n| !n.is_empty()) else { return true };
    let p = std::path::Path::new(&name).to_path_buf();
    if p.is_absolute() {
        return p.exists();
    }
    env("XDG_RUNTIME_DIR").is_some_and(|d| std::path::Path::new(&d).join(&name).exists())
}

/// Sleep `secs`, in small steps; false when a stop signal came meanwhile.
fn sleep_unless_term(secs: f64) -> bool {
    let end = Instant::now() + Duration::from_secs_f64(secs);
    while Instant::now() < end {
        if TERM.load(Ordering::SeqCst) != 0 {
            return false;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    TERM.load(Ordering::SeqCst) == 0
}

/// The decision for an exit status (pure, tested). `asked` = the supervisor itself asked it to stop; `hung` = the
/// hang marker was there.
pub fn decide(code: Option<i32>, signal: Option<i32>, asked: bool, hung: bool) -> (Next, Option<Kind>) {
    let quit_signal = |s: i32| s == libc::SIGTERM || s == libc::SIGINT || s == libc::SIGHUP;
    if asked {
        // the supervisor asked it to close (logout, Ctrl+C): done, whatever the code
        let kind = match signal {
            Some(s) if s == libc::SIGKILL => hung.then_some(Kind::Hang),
            Some(s) if !quit_signal(s) => Some(Kind::Native),
            _ => None,
        };
        return (Next::Exit(0), kind);
    }
    match (code, signal) {
        (Some(code::OK), _) => (Next::Exit(0), None),
        (Some(code::RENDERER_INIT), _) | (Some(code::NO_DISPLAY), _) => (Next::NextRenderer, None),
        (Some(code::RENDERER_RUNTIME), _) => (Next::Restart, None),
        (Some(code::PANIC), _) => (Next::Restart, Some(Kind::Panic)),
        (Some(_), _) => (Next::Restart, Some(Kind::Exit)),
        (None, Some(s)) if quit_signal(s) => (Next::Exit(0), None),
        (None, Some(s)) if s == libc::SIGKILL => {
            if hung {
                // "terminate" in the desktop's not-responding dialog: start it again
                (Next::Restart, Some(Kind::Hang))
            } else {
                (Next::Exit(0), Some(Kind::Killed))
            }
        }
        (None, Some(_)) => (Next::Restart, Some(Kind::Native)),
        (None, None) => (Next::Restart, Some(Kind::Exit)),
    }
}

fn signal_name(s: i32) -> &'static str {
    match s {
        libc::SIGSEGV => "SIGSEGV (invalid memory access)",
        libc::SIGABRT => "SIGABRT (abort)",
        libc::SIGBUS => "SIGBUS (bus error)",
        libc::SIGILL => "SIGILL (illegal instruction)",
        libc::SIGFPE => "SIGFPE (arithmetic error)",
        libc::SIGTRAP => "SIGTRAP",
        libc::SIGSYS => "SIGSYS (bad system call)",
        libc::SIGKILL => "SIGKILL",
        libc::SIGTERM => "SIGTERM",
        libc::SIGINT => "SIGINT",
        libc::SIGHUP => "SIGHUP",
        libc::SIGPIPE => "SIGPIPE",
        _ => "a signal",
    }
}

/// Run the supervisor (`renderer`: only this one, else wgpu then glow); returns the exit code.
pub fn run(demo: bool, renderer: Option<Renderer>) -> i32 {
    let env = |k: &str| std::env::var(k).ok();
    let state = applog::state_dir(&env);
    if let Err(e) = applog::init(&applog::log_file(&state), "supervisor", true, true) {
        eprintln!("omarchyvr-app: {e}");
    }
    crash::set_build_info(env!("PIMAX_GIT_HASH"), env!("PIMAX_BUILD_DATE"));
    crash::install_panic_hook(state.clone());
    crash::note("process", "supervisor");
    let _lock = match instance::acquire(&instance::lock_path(&env)) {
        Ok(l) => Some(l),
        Err(Some(pid)) => {
            let same = std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "omarchyvr-app");
            if same && pid > 1 && pid != std::process::id() {
                // SAFETY: kill(2) with the positive pid of the other omarchyvr-app (checked by name), SIGUSR1 = show.
                unsafe {
                    libc::kill(pid as libc::pid_t, libc::SIGUSR1);
                }
            }
            println!("omarchyvr is already running (pid {pid}); its window was asked to come to the front.");
            log::info!("second start: omarchyvr-app pid {pid} already runs");
            return 0;
        }
        Err(None) => {
            log::warn!("cannot use the single-instance lock; going on without it");
            None
        }
    };
    install_signals();
    crash::prune(&state, 30);
    let _ = applog::rotate(&window::stderr_log(&state), 1 << 20, 3);
    {
        let _ = std::thread::Builder::new()
            .name("pimax-sysinfo".into())
            .spawn(|| crash::set_system(crash::collect_system(&|k| std::env::var(k).ok(), &[])));
    }
    let last_dump: LastDump = Arc::default();
    let stop = Arc::new(AtomicBool::new(false));
    let sock = start_crash_server(&state, last_dump.clone(), stop.clone());
    log::info!("omarchyvr {} supervisor pid {}", crash::version_line(), std::process::id());

    // renderers to try, in order
    let mut chain: Vec<(Renderer, bool)> = match renderer.or_else(|| env("PIMAX_RENDERER").as_deref().and_then(Renderer::parse)) {
        Some(r) => vec![(r, false)],
        None => Renderer::chain().into_iter().map(|r| (r, false)).collect(),
    };
    if cfg!(feature = "x11") && env("WAYLAND_DISPLAY").is_some() && env("DISPLAY").is_some() {
        chain.push((Renderer::Glow, true));
    }
    let mut idx = 0usize;
    let mut runtime_errors = 0u32;
    let mut restarts: Vec<Instant> = Vec::new();
    let mut respawns: Vec<Instant> = Vec::new();
    let mut display_retries = 0u32;
    let exit = loop {
        let (renderer, x11) = chain[idx];
        // login autostart can run before the compositor's socket exists: wait for it (up to a minute) rather than
        // start a window process that hangs without a display
        if !x11 && !wayland_ready(&env) {
            log::warn!("the Wayland display is not ready yet: waiting for it (up to 60 s)");
            let mut ok = false;
            for _ in 0..120 {
                if !sleep_unless_term(0.5) {
                    break;
                }
                if wayland_ready(&env) {
                    ok = true;
                    break;
                }
            }
            if TERM.load(Ordering::SeqCst) != 0 {
                break 0;
            }
            if !ok {
                log::warn!("the Wayland display never appeared; starting the window anyway");
            }
        }
        let child = spawn_window(&state, renderer, x11, demo, sock.as_deref());
        let mut child = match child {
            Ok(c) => c,
            Err(e) => {
                log::error!("cannot start the window process: {e}");
                notify("omarchyvr could not start", &e);
                break 1;
            }
        };
        let pid = child.id();
        log::info!("window process pid {pid} ({}{})", renderer.name(), if x11 { ", X11" } else { "" });
        let mut asked = false;
        let mut asked_at = Instant::now();
        let mut hung_seen = false;
        let marker = crash::hang_marker(&state, pid);
        let mut tick = 0u32;
        let spawned = Instant::now();
        let mut watch = Watch::new(pid, env_secs("PIMAX_START_SECS", START_SECS), env_secs("PIMAX_RAISE_SECS", RAISE_SECS));
        let mut ended_why: Option<Why> = None;
        let ack_path = window::ack_file(&state);
        // when to ask the desktop whether the window is really there (after the first frame, after a second start)
        let mut desktop_check: Option<Instant> = None;
        let mut had_window = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(st)) => break Some(st),
                Ok(None) => {}
                Err(e) => {
                    log::error!("waiting for the window process failed: {e}");
                    break None;
                }
            }
            let sig = TERM.load(Ordering::SeqCst);
            if sig != 0 && !asked {
                log::info!("supervisor got {}: closing the window", signal_name(sig));
                signal_child(&child, libc::SIGTERM);
                asked = true;
                asked_at = Instant::now();
            }
            if asked && asked_at.elapsed() > Duration::from_secs(10) {
                // our own window process ignored SIGTERM for 10 s (blocked): end it
                log::warn!("window process {pid} did not exit after SIGTERM; SIGKILL");
                signal_child(&child, libc::SIGKILL);
                asked_at = Instant::now() + Duration::from_secs(3600);
            }
            let focus = FOCUS.swap(false, Ordering::SeqCst);
            let ack = if focus || tick.is_multiple_of(3) { std::fs::read_to_string(&ack_path).unwrap_or_default() } else { String::new() };
            if focus {
                signal_child(&child, libc::SIGUSR1);
                watch.raise_asked(spawned.elapsed().as_secs_f64(), &ack);
                if watch.has_window() {
                    desktop_check = Some(Instant::now() + Duration::from_secs_f64(env_secs("PIMAX_DESKTOP_CHECK_SECS", 1.0)));
                }
            }
            if watch.has_window() && !had_window {
                had_window = true;
                desktop_check = Some(Instant::now() + Duration::from_secs_f64(env_secs("PIMAX_DESKTOP_CHECK_SECS", DESKTOP_CHECK_SECS)));
            }
            if desktop_check.is_some_and(|t| Instant::now() >= t) && !asked && ended_why.is_none() {
                desktop_check = None;
                if desktop_has_window(pid) == Some(false) {
                    log::error!("the window process {pid} draws but the desktop has no window of it: ending it and starting a new one");
                    ended_why = Some(Why::NoWindow);
                    signal_child(&child, libc::SIGKILL);
                }
            }
            if tick.is_multiple_of(3) && !asked && ended_why.is_none() {
                let t = spawned.elapsed().as_secs_f64();
                if let Some(why) = watch.check(t, t, &ack) {
                    log::error!(
                        "the window process {pid} {}: ending it and starting a new one",
                        if why == Why::NoWindow { "never showed a window" } else { "did not answer \"show yourself\"" }
                    );
                    ended_why = Some(why);
                    signal_child(&child, libc::SIGKILL);
                }
            }
            tick += 1;
            if tick.is_multiple_of(10) {
                let hung = marker.exists();
                if hung && !hung_seen {
                    log::error!("the window process {pid} does not respond (hang report written)");
                    notify("omarchyvr is not responding", "A hang report was saved. Closing the window restarts it.");
                } else if !hung && hung_seen {
                    log::warn!("the window process {pid} responds again");
                }
                hung_seen = hung;
            }
            std::thread::sleep(Duration::from_millis(100));
        };
        let ended = Ended { state: &state, pid, renderer, x11, last_dump: &last_dump };
        let Some(status) = status else {
            ended.cleanup();
            break 1;
        };
        let (c, s) = (status.code(), status.signal());
        let hung = hung_seen || marker.exists();
        let (mut next, mut kind) = decide(c, s, asked, hung);
        if ended_why.is_some() && !asked {
            next = Next::Respawn;
            kind = None;
        }
        log::info!("window process {pid} ended: {status} -> {next:?}");
        match kind {
            Some(Kind::Native) => {
                let s = s.unwrap_or(0);
                ended.report(Kind::Native, format!("the window process died from {}", signal_name(s)), format!("signal {s}, core dumped: {}", status.core_dumped()));
            }
            Some(Kind::Killed) => {
                ended.report(Kind::Killed, "the window process was killed (SIGKILL: out of memory, or ended from outside)".into(), "signal 9".into());
            }
            Some(Kind::Hang) => {
                ended.report(Kind::Hang, "the window process was killed while it did not respond".into(), format!("{status}"));
            }
            Some(Kind::Panic) => match ended.own_report() {
                // the window wrote the report; its minidump (requested by the panic hook) goes next to it
                Some(dir) => ended.attach_dump(&dir),
                None => {
                    ended.report(Kind::Panic, "the window process panicked (no report of its own)".into(), format!("{status}"));
                }
            },
            Some(Kind::Exit) => {
                ended.report(Kind::Exit, format!("the window process exited with {status}"), format!("{status}"));
            }
            _ => {}
        }
        ended.cleanup();
        match next {
            Next::Exit(code) => break code,
            Next::NextRenderer => {
                let display = c == Some(code::NO_DISPLAY);
                let found = if display { chain.iter().skip(idx + 1).position(|(_, x)| *x).map(|p| idx + 1 + p) } else { (idx + 1 < chain.len()).then_some(idx + 1) };
                match found {
                    Some(n) => {
                        log::warn!("{} {}: trying {}{}", renderer.name(), if display { "has no display" } else { "could not start" }, chain[n].0.name(), if chain[n].1 { " on X11" } else { "" });
                        idx = n;
                        runtime_errors = 0;
                    }
                    None if display && env("WAYLAND_DISPLAY").is_some() && f64::from(display_retries) < env_secs("PIMAX_DISPLAY_RETRIES", 6.0) => {
                        // a session that has a display name but no connection yet: retry with a growing wait
                        let wait = [2.0, 4.0, 8.0, 16.0, 30.0, 30.0][display_retries.min(5) as usize];
                        display_retries += 1;
                        log::warn!("no display connection yet: trying again in {wait} s (try {display_retries})");
                        if !sleep_unless_term(wait) {
                            break 0;
                        }
                        idx = 0;
                    }
                    None => {
                        let msg = if display { "No display connection (Wayland or X11)." } else { "Neither Vulkan (wgpu) nor OpenGL (glow) could open the window." };
                        notify("omarchyvr could not open its window", &format!("{msg} See ~/.local/state/omarchyvr/logs/omarchyvr-app.log"));
                        // the user should see that report next time
                        if let Some((d, _)) = crash::list(&state).into_iter().map(|(d, r, _)| (d, r)).find(|(_, r)| r.kind == Kind::Renderer) {
                            let _ = std::fs::remove_file(d.join(crash::SEEN));
                        }
                        break 1;
                    }
                }
            }
            Next::Respawn => {
                respawns.retain(|t| t.elapsed() < Duration::from_secs(600));
                respawns.push(Instant::now());
                if respawns.len() > 6 {
                    notify("omarchyvr could not open its window", "It started 6 times in 10 minutes without a window. See ~/.local/state/omarchyvr/logs/omarchyvr-app.log");
                    log::error!("giving up: 6 window starts without a window in 10 minutes");
                    break 1;
                }
                let wait = [1.0, 2.0, 4.0, 8.0, 15.0, 30.0][(respawns.len() - 1).min(5)];
                log::warn!("starting a new window process in {wait} s (try {})", respawns.len());
                if !sleep_unless_term(wait) {
                    break 0;
                }
            }
            Next::Restart => {
                if c == Some(code::RENDERER_RUNTIME) {
                    runtime_errors += 1;
                    if runtime_errors >= 2 && idx + 1 < chain.len() {
                        log::warn!("{} failed twice while running: switching to {}", renderer.name(), chain[idx + 1].0.name());
                        idx += 1;
                        runtime_errors = 0;
                    }
                }
                restarts.retain(|t| t.elapsed() < Duration::from_secs(120));
                restarts.push(Instant::now());
                if restarts.len() > 3 {
                    notify("omarchyvr keeps crashing", "It stopped after 3 restarts in 2 minutes. Reports: ~/.local/state/omarchyvr/crashes");
                    log::error!("giving up: 3 restarts within 2 minutes");
                    break 1;
                }
                if TERM.load(Ordering::SeqCst) != 0 {
                    break 0;
                }
                log::warn!("restarting the window (restart {} of 3 in 2 minutes)", restarts.len());
                std::thread::sleep(Duration::from_millis(800));
            }
        }
    };
    stop.store(true, Ordering::SeqCst);
    if let Some(s) = &sock {
        let _ = std::fs::remove_file(s);
    }
    log::info!("supervisor exits ({exit})");
    log::logger().flush();
    exit
}

fn spawn_window(state: &Path, r: Renderer, x11: bool, demo: bool, sock: Option<&Path>) -> Result<Child, String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find myself: {e}"))?;
    let log = window::stderr_log(state);
    if let Some(d) = log.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    let out = std::fs::OpenOptions::new().create(true).append(true).open(&log).map_err(|e| format!("{}: {e}", log.display()))?;
    let err = out.try_clone().map_err(|e| e.to_string())?;
    let mut c = Command::new(exe);
    c.arg("--window").args(["--renderer", r.name()]);
    if demo {
        c.arg("--demo");
    }
    c.env("PIMAX_SUPERVISED", "1").stdin(Stdio::null()).stdout(out).stderr(err);
    if let Some(s) = sock {
        c.env("PIMAX_CRASH_SOCKET", s);
    }
    if x11 {
        c.env_remove("WAYLAND_DISPLAY");
    }
    c.spawn().map_err(|e| format!("cannot start the window process: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watch_ends_a_window_process_without_a_window_and_one_that_does_not_answer() {
        let mut w = Watch::new(10, 25.0, 3.0);
        assert_eq!(w.check(5.0, 5.0, ""), None);
        assert_eq!(w.check(24.0, 24.0, "99 1\n"), None, "an ack of another pid is not ours");
        assert_eq!(w.check(26.0, 26.0, ""), Some(Why::NoWindow));
        let mut w = Watch::new(10, 25.0, 3.0);
        assert_eq!(w.check(2.0, 2.0, "10 1\n"), None);
        assert!(w.has_window());
        w.raise_asked(40.0, "10 1\n");
        assert_eq!(w.check(42.0, 42.0, "10 1\n"), None, "still within 3 s");
        assert_eq!(w.check(42.5, 42.5, "10 2\n"), None);
        assert_eq!(w.check(60.0, 60.0, "10 2\n"), None, "answered: nothing pending");
        w.raise_asked(70.0, "10 2\n");
        assert_eq!(w.check(72.0, 72.0, "10 2\n"), None);
        assert_eq!(w.check(73.5, 73.5, "10 2\n"), Some(Why::NoAnswer));
        let mut w = Watch::new(10, 25.0, 3.0);
        w.raise_asked(1.0, "");
        assert_eq!(w.check(10.0, 10.0, ""), None, "a window still starting is not killed by a raise");
    }

    #[test]
    fn hyprland_client_list_is_read_by_pid() {
        let json = r#"[{"address": "0x1", "class": "foot", "pid": 73951, "title": "a"},
            {"class": "omarchyvr", "pid":653846, "workspace": {"id": 2}}]"#;
        assert!(hypr_lists_pid(json, 653846));
        assert!(hypr_lists_pid(json, 73951));
        assert!(!hypr_lists_pid(json, 2174), "the window process of the bug: drawing, but no client");
        assert!(!hypr_lists_pid(json, 7395), "a prefix of another pid is not it");
        assert!(!hypr_lists_pid("[]", 1));
    }

    #[test]
    fn the_wayland_socket_is_looked_for() {
        let d = std::env::temp_dir().join(format!("pimax-wl-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let dir = d.to_string_lossy().to_string();
        let env = |k: &str| match k {
            "WAYLAND_DISPLAY" => Some("wayland-9".to_string()),
            "XDG_RUNTIME_DIR" => Some(dir.clone()),
            _ => None,
        };
        assert!(!wayland_ready(&env));
        std::fs::write(d.join("wayland-9"), "").unwrap();
        assert!(wayland_ready(&env));
        assert!(wayland_ready(&|_| None), "no display name: nothing to wait for");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn exit_decisions() {
        assert_eq!(decide(Some(0), None, false, false), (Next::Exit(0), None));
        assert_eq!(decide(Some(70), None, false, false), (Next::NextRenderer, None));
        assert_eq!(decide(Some(72), None, false, false), (Next::NextRenderer, None));
        assert_eq!(decide(Some(71), None, false, false), (Next::Restart, None));
        assert_eq!(decide(Some(101), None, false, false), (Next::Restart, Some(Kind::Panic)));
        assert_eq!(decide(Some(3), None, false, false), (Next::Restart, Some(Kind::Exit)));
        assert_eq!(decide(None, Some(libc::SIGSEGV), false, false), (Next::Restart, Some(Kind::Native)));
        assert_eq!(decide(None, Some(libc::SIGABRT), false, true), (Next::Restart, Some(Kind::Native)));
        // the desktop's "terminate" on a hung window: restart; a plain kill: respect it
        assert_eq!(decide(None, Some(libc::SIGKILL), false, true), (Next::Restart, Some(Kind::Hang)));
        assert_eq!(decide(None, Some(libc::SIGKILL), false, false), (Next::Exit(0), Some(Kind::Killed)));
        // logout, Ctrl+C, or the supervisor asked
        assert_eq!(decide(None, Some(libc::SIGTERM), false, false), (Next::Exit(0), None));
        assert_eq!(decide(None, Some(libc::SIGHUP), false, false), (Next::Exit(0), None));
        assert_eq!(decide(None, Some(libc::SIGTERM), true, false), (Next::Exit(0), None));
        assert_eq!(decide(Some(0), None, true, false), (Next::Exit(0), None));
        assert_eq!(decide(Some(1), None, true, false), (Next::Exit(0), None), "asked to close: no restart");
        assert_eq!(decide(None, Some(libc::SIGKILL), true, true), (Next::Exit(0), Some(Kind::Hang)), "hung at close");
    }
}
