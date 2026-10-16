//! The window process (`omarchyvr-app --window`, started by the [`crate::supervisor`]; or alone with
//! `--no-supervisor`): the app's log, the panic hook, the native crash handler (a minidump written by the
//! supervisor), the hang watchdog, and the eframe window with the chosen renderer.
//!
//! Rendering never waits for the compositor: no vsync (no blocking buffer swap while the window is on a hidden
//! workspace), frames are limited by the app itself (60 per second at most, about one per second when idle).
use crate::app::PimaxApp;
use eframe::egui;
use omarchyvr_config::Store;
use omarchyvr_gui_core::app::AppCore;
use omarchyvr_gui_core::crash::{self, Kind};
use omarchyvr_gui_core::machine::RealMachine;
use omarchyvr_gui_core::paths::Paths;
use omarchyvr_gui_core::{applog, job};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// Exit codes of the window process (the supervisor acts on them).
pub mod code {
    /// Closed normally.
    pub const OK: i32 = 0;
    /// The renderer could not start (no window was shown): the supervisor tries the other one.
    pub const RENDERER_INIT: i32 = 70;
    /// The renderer failed while the window was shown (a buffer swap, a lost surface).
    pub const RENDERER_RUNTIME: i32 = 71;
    /// No display connection (Wayland/X11).
    pub const NO_DISPLAY: i32 = 72;
    /// A Rust panic on the main thread (the standard exit code).
    pub const PANIC: i32 = 101;
}

/// The renderer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Renderer {
    /// wgpu (Vulkan; recovers from lost or outdated surfaces). Only in builds with the `wgpu` feature.
    Wgpu,
    /// glow (OpenGL through EGL): the default build's only renderer.
    Glow,
}

impl Renderer {
    /// Name (`--renderer`, `PIMAX_RENDERER`).
    pub fn name(self) -> &'static str {
        match self {
            Renderer::Wgpu => "wgpu",
            Renderer::Glow => "glow",
        }
    }

    /// From [`Renderer::name`] (also `vulkan` / `opengl` / `gl`). `wgpu` in a build without it is glow.
    pub fn parse(s: &str) -> Option<Renderer> {
        match s.trim().to_ascii_lowercase().as_str() {
            "wgpu" | "vulkan" if cfg!(feature = "wgpu") => Some(Renderer::Wgpu),
            "wgpu" | "vulkan" => {
                log::warn!("this build has no wgpu renderer (cargo feature `wgpu`): using glow");
                Some(Renderer::Glow)
            }
            "glow" | "opengl" | "gl" => Some(Renderer::Glow),
            _ => None,
        }
    }

    /// The renderers of this build, in the order the supervisor tries them.
    pub fn chain() -> Vec<Renderer> {
        if cfg!(feature = "wgpu") {
            vec![Renderer::Wgpu, Renderer::Glow]
        } else {
            vec![Renderer::Glow]
        }
    }

    /// The first renderer of [`Renderer::chain`].
    pub fn default_for_build() -> Renderer {
        Renderer::chain()[0]
    }
}

/// How the window process runs.
#[derive(Debug, Clone)]
pub struct Options {
    /// Renderer.
    pub renderer: Renderer,
    /// Demo data, nothing is started (`--demo`).
    pub demo: bool,
    /// Started by the supervisor (its stderr goes to a file; the supervisor rotates the log).
    pub supervised: bool,
}

/// The native crash handler (kept for the life of the process; also asked for a minidump by a fatal panic).
static HANDLER: std::sync::OnceLock<crash_handler::CrashHandler> = std::sync::OnceLock::new();

/// A crash on purpose (the supervisor tests): once its minidump is written, the process leaves no system core dump.
static TEST_CRASH: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Set once the first frame was drawn (an error before it is a renderer start failure).
pub static FIRST_FRAME: AtomicBool = AtomicBool::new(false);
/// Set by SIGUSR1 (another `omarchyvr-app` was started): bring the window to the front.
pub static FOCUS: AtomicBool = AtomicBool::new(false);

extern "C" fn on_usr1(_: libc::c_int) {
    FOCUS.store(true, Ordering::SeqCst);
}

/// SIGUSR1 = "show yourself".
pub fn install_focus_signal() {
    // SAFETY: sigaction(2) with a handler that only stores to an atomic (async-signal-safe).
    unsafe {
        let mut sa: libc::sigaction = std::mem::zeroed();
        sa.sa_sigaction = on_usr1 as *const () as usize;
        sa.sa_flags = libc::SA_RESTART;
        libc::sigemptyset(&mut sa.sa_mask);
        libc::sigaction(libc::SIGUSR1, &sa, std::ptr::null_mut());
    }
}

/// The eframe options: renderer, no vsync, the window.
pub fn native_options(r: Renderer) -> eframe::NativeOptions {
    let mut o = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("omarchyvr")
            .with_inner_size([1180.0, 820.0])
            .with_min_inner_size([900.0, 560.0])
            .with_app_id("omarchyvr"),
        renderer: match r {
            #[cfg(feature = "wgpu")]
            Renderer::Wgpu => eframe::Renderer::Wgpu,
            _ => eframe::Renderer::Glow,
        },
        ..Default::default()
    };
    // a vsynced swap blocks until the compositor sends a frame callback, which it never does for a window on a
    // hidden workspace: the UI thread would stop and the desktop shows "not responding"
    o.glow_options.vsync = false;
    #[cfg(feature = "wgpu")]
    {
        o.wgpu_options.surface = eframe::SurfaceConfig {
            present_mode: eframe::wgpu::PresentMode::AutoNoVsync,
            desired_maximum_frame_latency: Some(2),
        };
    }
    o
}

/// The native crash handler: on SIGSEGV & co the supervisor writes a minidump of this process (it is our parent,
/// so ptrace is allowed). Kept for the life of the process.
fn attach_crash_handler() -> Option<crash_handler::CrashHandler> {
    let sock = std::env::var("PIMAX_CRASH_SOCKET").ok().filter(|s| !s.is_empty())?;
    let client = match minidumper::Client::with_name(minidumper::SocketName::path(&sock)) {
        Ok(c) => c,
        Err(e) => {
            log::warn!("no crash dumps: cannot reach the supervisor ({e})");
            return None;
        }
    };
    // SAFETY: the closure only calls the minidumper client (a socket write and read), as the crate documents for a
    // crash handler; it does not allocate in a way that can deadlock on the crashed thread's locks.
    let h = crash_handler::CrashHandler::attach(unsafe {
        crash_handler::make_crash_event(move |ctx: &crash_handler::CrashContext| {
            // blocks until the supervisor has written the minidump (it reads this process through ptrace)
            let ok = client.request_dump(ctx).is_ok();
            if TEST_CRASH.load(std::sync::atomic::Ordering::SeqCst) {
                // prctl(2) is a plain system call (async-signal-safe; inside the unsafe block above). Not dumpable:
                // the kernel skips the core dump, so the tests' deliberate crashes do not fill coredumpctl. Only after
                // the minidump above.
                libc::prctl(libc::PR_SET_DUMPABLE, 0, 0, 0, 0);
            }
            crash_handler::CrashEventResult::Handled(ok)
        })
    });
    match h {
        Ok(h) => {
            // SAFETY: getppid(2) cannot fail.
            h.set_ptracer(Some(unsafe { libc::getppid() } as u32));
            Some(h)
        }
        Err(e) => {
            log::warn!("no crash dumps: {e}");
            None
        }
    }
}

/// What a `run_native` error means: (exit code, one line).
pub fn classify(e: &eframe::Error, shown: bool) -> (i32, String) {
    let text = e.to_string();
    let display = matches!(e, eframe::Error::WinitEventLoop(_) | eframe::Error::Winit(_));
    let code = match (display, shown) {
        (true, false) => code::NO_DISPLAY,
        (false, false) => code::RENDERER_INIT,
        (_, true) => code::RENDERER_RUNTIME,
    };
    (code, text)
}

fn real_core(demo: bool) -> AppCore {
    if demo {
        omarchyvr_gui_core::cmd::set_dry_run(true);
        let tmp = std::env::temp_dir().join(format!("pimax-demo-{}", std::process::id()));
        let store = Store::new(tmp.join("cfg"), tmp.join("cache"));
        return AppCore::demo_with(false, store, |m| m, true);
    }
    let env = |k: &str| std::env::var(k).ok();
    let paths = Paths::from_env(&env);
    let opts = omarchyvr_gui_core::paths::doctor_opts(&paths, &env);
    AppCore::new_background(
        Arc::new(RealMachine::from_env()),
        paths,
        opts,
        Arc::new(|k: &str| std::env::var(k).ok()),
        Store::from_env(&env),
    )
}

/// Writes the breadcrumbs every 2 s for the supervisor (it adds them to a report when this process dies from a
/// signal, where nothing runs in here any more).
fn start_context_writer(state: PathBuf) {
    let _ = std::thread::Builder::new().name("pimax-context".into()).spawn(move || {
        let f = crash::crashes_dir(&state).join(format!(".context-{}", std::process::id()));
        let mut last = String::new();
        loop {
            let text: String = crash::context().into_iter().filter(|(k, _)| k != "uptime").map(|(k, v)| format!("{k}: {v}\n")).collect();
            if text != last {
                let _ = std::fs::create_dir_all(crash::crashes_dir(&state));
                let _ = std::fs::write(&f, &text);
                last = text;
            }
            std::thread::sleep(Duration::from_secs(2));
        }
    });
}

/// Run the window; returns the exit code.
pub fn run(o: Options) -> i32 {
    let env = |k: &str| std::env::var(k).ok();
    let state = applog::state_dir(&env);
    if let Err(e) = applog::init(&applog::log_file(&state), "window", !o.supervised, !o.supervised) {
        eprintln!("omarchyvr-app: {e}");
    }
    crash::set_build_info(env!("PIMAX_GIT_HASH"), env!("PIMAX_BUILD_DATE"));
    crash::install_panic_hook(state.clone());
    crash::note("process", "window");
    crash::note("renderer", o.renderer.name());
    log::info!(
        "omarchyvr {} window starting: renderer {}, pid {}{}{}",
        crash::version_line(),
        o.renderer.name(),
        std::process::id(),
        if o.supervised { ", supervised" } else { "" },
        if o.demo { ", demo data" } else { "" }
    );
    {
        let r = o.renderer.name().to_string();
        let _ = std::thread::Builder::new()
            .name("pimax-sysinfo".into())
            .spawn(move || crash::set_system(crash::collect_system(&|k| std::env::var(k).ok(), &[("renderer", r)])));
    }
    if o.supervised {
        if let Some(h) = attach_crash_handler() {
            if HANDLER.set(h).is_ok() {
                // a fatal panic gets a minidump too: release binaries are stripped, the dump plus the build id is
                // what can be symbolicated later
                crash::set_dump_request(Box::new(|| {
                    if let Some(h) = HANDLER.get() {
                        let _ = h.simulate_signal(libc::SIGABRT as u32);
                    }
                }));
            }
        }
    }
    install_focus_signal();
    let hang_secs = std::env::var("PIMAX_HANG_SECS").ok().and_then(|s| s.parse().ok()).unwrap_or(10);
    crash::start_watchdog(state.clone(), Duration::from_secs(hang_secs));
    if o.supervised {
        start_context_writer(state.clone());
    }
    if !o.demo {
        job::set_log_dir(state.join("jobs"));
    }
    test_crash();
    let core = real_core(o.demo);
    let result = eframe::run_native(
        "omarchyvr",
        native_options(o.renderer),
        Box::new(move |cc| {
            let mut core = core;
            crate::theme::install(&cc.egui_ctx, true);
            let ctx = cc.egui_ctx.clone();
            core.set_wake(Arc::new(move || ctx.request_repaint()));
            core.start_threads();
            core.arm_autostart();
            core.check_crashes();
            Ok(Box::new(PimaxApp::new(core)))
        }),
    );
    match result {
        Ok(()) => {
            log::info!("window closed");
            code::OK
        }
        Err(e) => {
            let shown = FIRST_FRAME.load(Ordering::SeqCst);
            let (exit, text) = classify(&e, shown);
            let summary = format!(
                "{} renderer {}: {text}",
                o.renderer.name(),
                if shown { "failed while the window was shown" } else { "could not start" }
            );
            log::error!("{summary}");
            let mut r = crash::Report::new(Kind::Renderer, summary, format!("eframe error: {e:?}\nexit code {exit}"));
            r.log_tail = applog::recent(200);
            if let Ok(dir) = crash::write(&state, &r) {
                if exit == code::RENDERER_INIT {
                    // the supervisor falls back to the other renderer: nothing to show unless that fails too
                    let _ = crash::mark_seen(&dir);
                }
            }
            exit
        }
    }
}

/// Test hook of the supervisor tests (`tests/supervisor.rs`): `PIMAX_TEST_CRASH=panic|segv|abort|hang|exit3` makes
/// the window process fail that way right after its crash handling is set up.
fn test_crash() {
    let Ok(how) = std::env::var("PIMAX_TEST_CRASH") else { return };
    log::warn!("PIMAX_TEST_CRASH={how}: failing on purpose");
    // no system core dumps from the tests (the app's own report and minidump are what they check)
    let zero = libc::rlimit { rlim_cur: 0, rlim_max: 0 };
    // SAFETY: setrlimit(2) with a valid pointer to a local struct.
    unsafe { libc::setrlimit(libc::RLIMIT_CORE, &zero) };
    TEST_CRASH.store(true, std::sync::atomic::Ordering::SeqCst);
    match how.as_str() {
        "panic" => panic!("test panic (PIMAX_TEST_CRASH)"),
        // SAFETY: deliberately invalid: the test of the native crash path
        "segv" => unsafe { std::ptr::null_mut::<u32>().write_volatile(1) },
        "abort" => std::process::abort(),
        "hang" => {
            crash::beat();
            loop {
                std::thread::sleep(Duration::from_secs(3600));
            }
        }
        "exit3" => std::process::exit(3),
        _ => {}
    }
}

/// Where the window process writes its stderr when supervised.
pub fn stderr_log(state: &Path) -> PathBuf {
    state.join("logs/window-stderr.log")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renderer_names_and_options() {
        assert_eq!(Renderer::parse("gl"), Some(Renderer::Glow));
        assert_eq!(Renderer::parse("x"), None);
        let o = native_options(Renderer::Glow);
        assert!(!o.glow_options.vsync, "no blocking swap");
        assert_eq!(o.renderer, eframe::Renderer::Glow);
        assert_eq!(*Renderer::chain().last().unwrap(), Renderer::Glow, "glow is always there");
        #[cfg(feature = "wgpu")]
        {
            assert_eq!(Renderer::parse("Vulkan"), Some(Renderer::Wgpu));
            let o = native_options(Renderer::Wgpu);
            assert_eq!(o.wgpu_options.surface.present_mode, eframe::wgpu::PresentMode::AutoNoVsync);
        }
        #[cfg(not(feature = "wgpu"))]
        assert_eq!(Renderer::parse("wgpu"), Some(Renderer::Glow), "a build without wgpu uses glow");
    }
}
