//! The supervisor with the real binary, without any display (WAYLAND_DISPLAY and DISPLAY unset, so no window and no
//! GPU is ever opened), in temporary state and runtime directories, notifications off. The window process fails on
//! purpose through its test hook (`PIMAX_TEST_CRASH`): the supervisor must classify it, write the report (with a
//! minidump for a native crash), restart at most 3 times in 2 minutes, honour SIGTERM, and keep one instance.
use omarchyvr_gui_core::crash::{self, Kind};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

fn dirs(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("pimax-sup-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(d.join("state")).unwrap();
    std::fs::create_dir_all(d.join("run")).unwrap();
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(d.join("run"), std::fs::Permissions::from_mode(0o700)).unwrap();
    d
}

fn start(d: &Path, crash: Option<&str>, extra: &[(&str, &str)]) -> Child {
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr-app"));
    c.env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env("XDG_STATE_HOME", d.join("state"))
        .env("XDG_RUNTIME_DIR", d.join("run"))
        .env("PIMAX_NO_NOTIFY", "1")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(x) = crash {
        c.env("PIMAX_TEST_CRASH", x);
    }
    for (k, v) in extra {
        c.env(k, v);
    }
    c.spawn().expect("start omarchyvr-app")
}

fn wait(c: &mut Child, timeout: Duration) -> ExitStatus {
    let t = Instant::now();
    loop {
        if let Some(st) = c.try_wait().unwrap() {
            return st;
        }
        if t.elapsed() > timeout {
            // SAFETY: kill(2) of our own child by pid.
            unsafe {
                libc::kill(c.id() as libc::pid_t, libc::SIGKILL);
            }
            panic!("omarchyvr-app did not end within {timeout:?}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn state(d: &Path) -> PathBuf {
    d.join("state/omarchyvr")
}

fn log(d: &Path) -> String {
    std::fs::read_to_string(state(d).join("logs/omarchyvr-app.log")).unwrap_or_default()
}

fn reports(d: &Path, k: Kind) -> Vec<(PathBuf, crash::Report)> {
    crash::list(&state(d)).into_iter().filter(|(_, r, _)| r.kind == k).map(|(p, r, _)| (p, r)).collect()
}

fn signal(pid: u32, sig: i32) {
    // SAFETY: kill(2) with the positive pid of a process this test started (or its child).
    unsafe {
        libc::kill(pid as libc::pid_t, sig);
    }
}

#[test]
fn no_display_tries_both_renderers_then_gives_up() {
    let d = dirs("nodisplay");
    let mut c = start(&d, None, &[]);
    let st = wait(&mut c, Duration::from_secs(60));
    assert_eq!(st.code(), Some(1), "gave up");
    let l = log(&d);
    assert!(l.contains("renderer wgpu") || l.contains("window process pid"), "{l}");
    assert!(l.contains("(no notification) omarchyvr could not open its window"), "{l}");
    let r = reports(&d, Kind::Renderer);
    assert!(!r.is_empty(), "a renderer report");
    assert!(!crash::unseen(&state(&d)).is_empty(), "shown at the next start");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_panic_is_reported_and_restarted_three_times() {
    let d = dirs("panic");
    let mut c = start(&d, Some("panic"), &[]);
    let st = wait(&mut c, Duration::from_secs(60));
    assert_eq!(st.code(), Some(1), "gave up after the restart budget");
    let l = log(&d);
    assert_eq!(l.matches("window process pid").count(), 4, "first start + 3 restarts:\n{l}");
    assert!(l.contains("giving up: 3 restarts within 2 minutes"), "{l}");
    let p = reports(&d, Kind::Panic);
    assert_eq!(p.len(), 4, "one report per crash (written by the window's panic hook)");
    assert!(p[0].1.summary.contains("test panic (PIMAX_TEST_CRASH)"), "{}", p[0].1.summary);
    assert!(p[0].1.details.contains("backtrace"), "{}", p[0].1.details);
    // stripped binaries: the panic also gets a minidump, and the report names the build id
    assert!(p.iter().any(|(d, _)| d.join("minidump.dmp").is_file()), "no minidump next to a panic report:\n{}", log(&d));
    assert!(p[0].1.system.iter().any(|(k, _)| k == "build id") || p[0].1.system.iter().any(|(k, _)| k == "app"), "{:?}", p[0].1.system);
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_native_crash_gets_a_minidump() {
    let d = dirs("segv");
    let mut c = start(&d, Some("segv"), &[]);
    let st = wait(&mut c, Duration::from_secs(90));
    assert_eq!(st.code(), Some(1));
    let n = reports(&d, Kind::Native);
    assert_eq!(n.len(), 4, "{}", log(&d));
    assert!(n[0].1.summary.contains("SIGSEGV"), "{}", n[0].1.summary);
    let dumps: Vec<u64> = n.iter().filter_map(|(p, _)| std::fs::metadata(p.join("minidump.dmp")).ok()).map(|m| m.len()).collect();
    assert!(!dumps.is_empty() && dumps.iter().all(|s| *s > 1024), "minidumps: {dumps:?}\n{}", log(&d));
    assert!(n[0].1.ctx("written by") == Some("supervisor"));
    assert!(n[0].1.log_tail.iter().any(|l| l.contains("PIMAX_TEST_CRASH=segv")), "the window's last log lines are in the report");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_hang_is_detected_and_a_kill_from_outside_restarts_it() {
    let d = dirs("hang");
    let mut c = start(&d, Some("hang"), &[("PIMAX_HANG_SECS", "1")]);
    let crashes = state(&d).join("crashes");
    let marker_pid = |skip: Option<u32>| -> Option<u32> {
        std::fs::read_dir(&crashes).ok()?.flatten().find_map(|e| {
            let n = e.file_name().to_string_lossy().into_owned();
            n.strip_prefix(".hang-").and_then(|p| p.parse().ok()).filter(|p| Some(*p) != skip)
        })
    };
    let t = Instant::now();
    let mut pid = None;
    while t.elapsed() < Duration::from_secs(30) && pid.is_none() {
        pid = marker_pid(None);
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = pid.expect("the watchdog marked the hang");
    assert!(!reports(&d, Kind::Hang).is_empty(), "the watchdog wrote a hang report");
    // the supervisor looks at the marker once a second; a user takes longer to give up on a window
    std::thread::sleep(Duration::from_millis(1500));
    // "terminate" in the desktop's not-responding dialog
    signal(pid, libc::SIGKILL);
    let t = Instant::now();
    let mut second = None;
    while t.elapsed() < Duration::from_secs(30) && second.is_none() {
        second = marker_pid(Some(pid));
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(second.is_some(), "restarted after the kill:\n{}", log(&d));
    // logout: SIGTERM to the supervisor ends both, no restart
    signal(c.id(), libc::SIGTERM);
    let st = wait(&mut c, Duration::from_secs(30));
    assert_eq!(st.code(), Some(0), "{}", log(&d));
    let l = log(&d);
    assert!(l.contains("does not respond"), "{l}");
    assert!(l.contains("closing the window"), "{l}");
    let killed_hang = crash::list(&state(&d)).into_iter().any(|(_, r, _)| r.kind == Kind::Hang && r.ctx("written by") == Some("supervisor"));
    assert!(killed_hang, "the supervisor wrote the kill of the hung window");
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn one_instance_and_unexpected_exits() {
    let d = dirs("instance");
    let mut first = start(&d, Some("hang"), &[("PIMAX_HANG_SECS", "60")]);
    std::thread::sleep(Duration::from_millis(1500));
    let out = Command::new(env!("CARGO_BIN_EXE_omarchyvr-app"))
        .env_remove("WAYLAND_DISPLAY")
        .env_remove("DISPLAY")
        .env("XDG_STATE_HOME", d.join("state"))
        .env("XDG_RUNTIME_DIR", d.join("run"))
        .env("PIMAX_NO_NOTIFY", "1")
        .output()
        .unwrap();
    assert!(out.status.success());
    assert!(String::from_utf8_lossy(&out.stdout).contains("already running"), "{:?}", out);
    signal(first.id(), libc::SIGTERM);
    assert_eq!(wait(&mut first, Duration::from_secs(30)).code(), Some(0));
    // an exit code nobody expects: reported and restarted
    let d2 = dirs("exit3");
    let mut c = start(&d2, Some("exit3"), &[]);
    assert_eq!(wait(&mut c, Duration::from_secs(60)).code(), Some(1));
    assert_eq!(reports(&d2, Kind::Exit).len(), 4, "{}", log(&d2));
    let _ = std::fs::remove_dir_all(d);
    let _ = std::fs::remove_dir_all(d2);
}
