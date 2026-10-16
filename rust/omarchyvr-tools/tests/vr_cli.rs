//! Black-box tests of `omarchyvr vr ...` against a fake machine: fake sysfs (USB device, DRM connector), fake
//! `systemctl`/`journalctl`/`notify-send` in front of PATH, and a fake monado-service script. No headset, no GPU and
//! no real Monado are touched; every test uses its own session directory and a unique process name, so tests can run
//! in parallel and next to a real monado-service.
use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

static COUNTER: AtomicU32 = AtomicU32::new(0);

/// Well-behaved service: logs like the real one, calibrates after 0.3 s, shuts down on SIGINT.
const POLITE: &str = r#"#!/bin/bash
echo " INFO [pimax_hmd_create] pimax_video_prepare(${PIMAX_FPS:-72}) -> 0" >&2
echo "FAKE-ENV XRT_NO_STDIN=$XRT_NO_STDIN MODE=$XRT_COMPOSITOR_DESIRED_MODE FPS=$PIMAX_FPS DUMP=$XRT_COMPOSITOR_DUMP_DIR CONN=$XRT_COMPOSITOR_WAYLAND_CONNECTOR PLOG=$PIMAX_LOG SID=$(ps -o sid= -p $$ | tr -d ' ') PID=$$" >&2
echo "FAKE-FDS $(ls /proc/$$/fd | tr '\n' ' ')" >&2
sleep 0.3
echo " INFO [bias_finish] gyro bias done: 0.1 0.2 0.3 dps over 2000 samples (0 restarts); accel 0 1 0 g" >&2
trap 'echo INT >> "${FAKE_SIGLOG:-/dev/null}"; echo " INFO [ipc_server_main_common] Server exiting: 0" >&2; exit 0' INT TERM
[ -n "$FAKE_READY" ] && echo ok > "$FAKE_READY"
while true; do sleep 0.05; done
"#;

/// Like POLITE, but its teardown takes a second (a game is attached), and every SIGINT is counted.
const SLOW_STOP: &str = r#"#!/bin/bash
echo " INFO [bias_finish] gyro bias done: 0.1 0.2 0.3 dps over 2000 samples (0 restarts); accel 0 1 0 g" >&2
trap 'echo INT >> "${FAKE_SIGLOG:-/dev/null}"; sleep 1; echo " INFO [ipc_server_main_common] Server exiting: 0" >&2; exit 0' INT TERM
while true; do sleep 0.05; done
"#;

/// Ignores SIGINT and SIGTERM: only SIGKILL stops it.
const STUBBORN: &str = r#"#!/bin/bash
trap '' INT TERM
echo " INFO [bias_finish] gyro bias done: 0 0 0 dps over 2000 samples (0 restarts); accel 0 1 0 g" >&2
while true; do sleep 0.05; done
"#;

/// Dies during start-up.
const CRASHING: &str = r#"#!/bin/bash
echo "ERROR [comp_window_direct_wayland_init] lease denied by the compositor" >&2
exit 3
"#;

/// Stays up but never finishes the calibration.
const NEVER_READY: &str = r#"#!/bin/bash
echo " INFO [pimax_hmd_create] pimax_video_prepare(72) -> 0" >&2
trap 'exit 0' INT TERM
while true; do sleep 0.05; done
"#;

struct Fixture {
    root: PathBuf,
    comm: String,
}

fn script(path: &Path, body: &str) {
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

impl Fixture {
    fn new(service_body: &str) -> Self {
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        // /proc/PID/comm holds 15 characters at most
        let comm = format!("pvr{}-{n}", std::process::id() % 100_000);
        let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("vr-cli-{comm}"));
        let _ = fs::remove_dir_all(&root);
        for d in ["bin", "svc", "session", "build"] {
            fs::create_dir_all(root.join(d)).unwrap();
        }
        // fake sysfs: the headset on USB and a connected DP-1
        let usb = root.join("sys/bus/usb/devices/1-4");
        fs::create_dir_all(&usb).unwrap();
        fs::write(usb.join("idVendor"), "34a4\n").unwrap();
        fs::write(usb.join("idProduct"), "0044\n").unwrap();
        // ... and its camera 34a4:0014 with a video4linux node, so no start tries to auto-recover on the test machine
        let cam = root.join("sys/bus/usb/devices/1-4.4");
        fs::create_dir_all(cam.join("1-4.4:1.0")).unwrap();
        fs::write(cam.join("idVendor"), "34a4\n").unwrap();
        fs::write(cam.join("idProduct"), "0014\n").unwrap();
        let v4l = root.join("sys/class/video4linux/video0");
        fs::create_dir_all(&v4l).unwrap();
        std::os::unix::fs::symlink(cam.join("1-4.4:1.0"), v4l.join("device")).unwrap();
        let drm = root.join("sys/class/drm/card1-DP-1");
        fs::create_dir_all(&drm).unwrap();
        fs::write(drm.join("status"), "connected\n").unwrap();
        // fake tools
        script(
            &root.join("bin/systemctl"),
            "#!/bin/bash\necho \"${FAKE_KEEPALIVE:-active}\"\n[ \"${FAKE_KEEPALIVE:-active}\" = active ]\n",
        );
        script(
            &root.join("bin/journalctl"),
            "#!/bin/bash\necho 'Oct 13 23:00:00 host kernel: nvidia-modeset: Loading NVIDIA UNIX Open Kernel Mode Setting Driver'\n\
             [ -n \"$FAKE_KERNEL_EXTRA\" ] && echo \"$FAKE_KERNEL_EXTRA\"\nexit 0\n",
        );
        script(
            &root.join("bin/notify-send"),
            "#!/bin/bash\nprintf '%s\\n' \"$*\" >> \"$FAKE_NOTIFY_LOG\"\n",
        );
        // a fake Hyprland: the VR screens' outputs are created and removed in a state file, never in the real one
        script(
            &root.join("bin/hyprctl"),
            "#!/bin/bash\nst=\"$FAKE_HYPR_STATE\"; touch \"$st\"\necho \"hyprctl $*\" >> \"$FAKE_HYPR_LOG\"\n\
             case \"$*\" in\n\
             '-j monitors all') printf '[{\"name\":\"SUNSHINE\",\"width\":1920,\"height\":1080,\"x\":0,\"y\":0,\"focused\":true,\"disabled\":false,\"mirrorOf\":\"none\",\"activeWorkspace\":{\"id\":1}}'\n\
               while read -r n; do printf ',{\"name\":\"%s\",\"width\":1920,\"height\":1080,\"x\":0,\"y\":0,\"disabled\":false,\"mirrorOf\":\"none\",\"activeWorkspace\":{\"id\":9}}' \"$n\"; done < \"$st\"; echo ']';;\n\
             '-j clients') echo '[]';;\n\
             'output create headless '*) echo \"$4\" >> \"$st\"; echo ok;;\n\
             'output remove '*) grep -vx \"$3\" \"$st\" > \"$st.t\"; mv \"$st.t\" \"$st\"; echo ok;;\n\
             *) echo ok;;\n\
             esac\n",
        );
        script(&root.join("svc").join(&comm), service_body);
        fs::write(root.join("build/openxr_monado-dev.json"), "{}").unwrap();
        Fixture { root, comm }
    }

    fn session(&self) -> PathBuf {
        self.root.join("session")
    }

    fn notify_log(&self) -> String {
        fs::read_to_string(self.root.join("notify.log")).unwrap_or_default()
    }

    fn monado_log(&self) -> String {
        fs::read_to_string(self.session().join("monado.log")).unwrap_or_default()
    }

    fn pimax(&self) -> Command {
        let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
        c.arg("vr");
        self.apply_env(&mut c);
        c
    }

    fn apply_env(&self, c: &mut Command) {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        );
        omarchyvr_config::isolate_command(c, &self.root);
        c.env("HOME", self.root.join("home"))
            .env("PATH", path)
            .env("PIMAX_SYSFS_ROOT", self.root.join("sys"))
            .env("PIMAX_NO_WATCHDOG", "1")
            .env("PIMAX_VR_SESSION_DIR", self.session())
            .env("PIMAX_MONADO_BUILD", self.root.join("build"))
            .env("FAKE_NOTIFY_LOG", self.root.join("notify.log"))
            .env("WAYLAND_DISPLAY", "wayland-test")
            .env("XDG_RUNTIME_DIR", &self.root)
            .env("FAKE_SIGLOG", self.root.join("sig.log"))
            .env("FAKE_HYPR_STATE", self.root.join("hypr.state"))
            .env("FAKE_HYPR_LOG", self.root.join("order.log"))
            .env("PIMAX_HYPRCTL", self.root.join("bin/hyprctl"))
            .env_remove("PIMAX_NO_NOTIFY")
            .env_remove("PIMAX_LOG")
            .env_remove("SCALE")
            .env_remove("FAKE_KEEPALIVE")
            .env_remove("FAKE_KERNEL_EXTRA");
    }

    /// Number of SIGINTs the fake service has seen.
    fn sigints(&self) -> usize {
        fs::read_to_string(self.root.join("sig.log"))
            .unwrap_or_default()
            .lines()
            .count()
    }

    /// Path of the fake service executable.
    fn service(&self) -> PathBuf {
        self.root.join("svc").join(&self.comm)
    }

    /// `omarchyvr vr <args> --service-bin <fake>` (the fake service's file name is the process name to look for).
    fn run(&self, args: &[&str]) -> Output {
        let mut c = self.pimax();
        c.args(args)
            .arg("--service-bin")
            .arg(self.root.join("svc").join(&self.comm));
        c.output().unwrap()
    }

    fn pid(&self) -> Option<u32> {
        fs::read_to_string(self.session().join("monado.pid"))
            .ok()?
            .trim()
            .parse()
            .ok()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        if let Some(p) = self.pid() {
            if alive(p) {
                // SAFETY: plain syscall on a pid we started ourselves.
                unsafe { libc::kill(p as i32, libc::SIGKILL) };
            }
        }
    }
}

fn alive(pid: u32) -> bool {
    match fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(s) => !s.rsplit(") ").next().unwrap_or("").starts_with('Z'),
        Err(_) => false,
    }
}

fn wait_until(what: &str, secs: u64, mut f: impl FnMut() -> bool) {
    let t = Instant::now();
    while t.elapsed() < Duration::from_secs(secs) {
        if f() {
            return;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    panic!("timed out waiting for {what}");
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

#[test]
fn vr_is_listed_in_the_main_usage_and_has_its_own_help() {
    let f = Fixture::new(POLITE);
    let mut hc = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    omarchyvr_config::isolate_command(&mut hc, Path::new(env!("CARGO_TARGET_TMPDIR")));
    let o = hc
        .arg("--help")
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&o.stdout).contains("vr start|stop|status|steam-options"));
    let o = f.pimax().arg("help").output().unwrap();
    assert!(o.status.success());
    let t = text(&o);
    for want in [
        "start [--fps 72|90]",
        "stop [--grace SECS]",
        "status",
        "steam-options",
    ] {
        assert!(t.contains(want), "{want} in {t}");
    }
    assert_eq!(f.pimax().output().unwrap().status.code(), Some(2));
    assert_eq!(
        f.pimax().arg("bogus").output().unwrap().status.code(),
        Some(2)
    );
}

#[test]
fn steam_options_prints_the_launch_line_on_stdout_only() {
    let f = Fixture::new(POLITE);
    let o = f
        .pimax()
        .args(["steam-options", "--monado-build", "/opt/monado/build"])
        .output()
        .unwrap();
    assert!(o.status.success());
    assert_eq!(
        String::from_utf8_lossy(&o.stdout),
        "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 XR_RUNTIME_JSON=/opt/monado/build/openxr_monado-dev.json %command%\n"
    );
    assert!(String::from_utf8_lossy(&o.stderr).contains("Launch Options"));
}

#[test]
fn steam_options_needs_only_the_container_import_when_active_runtime_points_at_the_build() {
    let f = Fixture::new(POLITE);
    let home = f.root.join("home-rt");
    let build = f.root.join("build-rt");
    std::fs::create_dir_all(home.join(".config/openxr/1")).unwrap();
    std::fs::create_dir_all(&build).unwrap();
    let json = build.join("openxr_monado-dev.json");
    std::fs::write(&json, "{}").unwrap();
    std::os::unix::fs::symlink(&json, home.join(".config/openxr/1/active_runtime.json")).unwrap();
    let o = f
        .pimax()
        .env("HOME", &home)
        .args(["steam-options", "--monado-build"])
        .arg(&build)
        .output()
        .unwrap();
    assert!(o.status.success());
    assert_eq!(String::from_utf8_lossy(&o.stdout), "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%\n");
}

#[test]
fn dry_run_shows_preflight_and_environment_without_starting_anything() {
    let f = Fixture::new(POLITE);
    let dump = f.root.join("dump-dry");
    let o = f.run(&[
        "start",
        "--fps",
        "90",
        "--dry-run",
        "--dump",
        dump.to_str().unwrap(),
    ]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    for want in [
        "headset   34a4:0044 on USB",
        "[ok  ] keepalive",
        "[ok  ] display",
        "[ok  ] kernel",
        "[ok  ] service",
        "XRT_COMPOSITOR_DESIRED_MODE=3",
        "PIMAX_FPS=90",
        &format!("XRT_COMPOSITOR_DUMP_DIR={}", dump.display()),
        "XRT_NO_STDIN=1",
    ] {
        assert!(t.contains(want), "{want} missing in:\n{t}");
    }
    assert!(!f.session().join("monado.log").exists());
    assert!(f.notify_log().is_empty());
    assert!(!dump.exists(), "a dry run creates nothing");
}

#[test]
fn start_status_stop_cycle() {
    let f = Fixture::new(POLITE);
    let dump = f.root.join("dump");
    let o = f.run(&["start", "--fps", "90", "--dump", dump.to_str().unwrap()]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("ready after"), "{t}");
    assert!(t.contains("gyro calibration done"), "{t}");
    assert!(t.contains("90 Hz announced (rc 0)"), "{t}");
    let pid = f.pid().expect("pid file");
    assert!(alive(pid), "the service outlives the launcher");
    assert!(dump.is_dir(), "dump dir created");

    // environment as the real service sees it, and the detach
    let log = f.monado_log();
    assert!(log.contains("XRT_NO_STDIN=1 MODE=3 FPS=90"), "{log}");
    assert!(log.contains(&format!("DUMP={}", dump.display())), "{log}");
    assert!(log.contains("CONN=DP-1"), "{log}");
    let sid: u32 = log
        .split("SID=")
        .nth(1)
        .and_then(|r| r.split_whitespace().next())
        .and_then(|s| s.parse().ok())
        .expect("SID in log");
    assert_eq!(sid, pid, "the service leads its own session (setsid)");

    // notifications
    let n = f.notify_log();
    assert!(n.contains("Pimax VR starting (90 Hz)"), "{n}");
    assert!(n.contains("Pimax VR ready (90 Hz)"), "{n}");

    // a second start is refused by the preflight and must not touch the running service
    let again = f.run(&["start"]);
    assert_eq!(again.status.code(), Some(1));
    assert!(
        text(&again).contains("monado-service already running"),
        "{}",
        text(&again)
    );
    assert!(alive(pid));

    // status
    let st = f.run(&["status"]);
    let s = text(&st);
    assert!(st.status.success(), "{s}");
    assert!(s.contains(&format!("service: running, pid {pid}")), "{s}");
    assert!(
        s.contains("rate: 90 Hz (PIMAX_FPS), display mode index 3"),
        "{s}"
    );
    assert!(s.contains("frame dumps:"), "{s}");

    // stop
    let o = f.run(&["stop"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("exited cleanly"), "{t}");
    assert!(t.contains("no Xid / display engine errors"), "{t}");
    wait_until("service exit", 5, || !alive(pid));
    assert!(f.pid().is_none(), "pid file removed");
    assert!(
        f.monado_log().contains("Server exiting"),
        "SIGINT, not SIGKILL"
    );
    assert!(f.notify_log().contains("Pimax VR stopped"));

    // idempotent
    let o = f.run(&["stop"]);
    assert!(o.status.success());
    assert!(text(&o).contains("not running"));
    let st = f.run(&["status"]);
    assert_eq!(st.status.code(), Some(3));
    assert!(text(&st).contains("service: not running"));
}

#[test]
fn failed_preflight_blocks_the_start_and_notifies() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--service-bin"])
        .arg(f.root.join("svc").join(&f.comm))
        .env("FAKE_KEEPALIVE", "inactive");
    let o = c.output().unwrap();
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("[FAIL] keepalive"), "{t}");
    assert!(t.contains("preflight failed, not starting"), "{t}");
    assert!(
        !f.session().join("monado.log").exists(),
        "nothing was spawned"
    );
    assert!(f.notify_log().contains("Pimax VR: not started"));
    assert!(f.notify_log().contains("-u critical"));
}

#[test]
fn xid_blocks_the_start_and_stop_reports_it_with_exit_code_2() {
    let f = Fixture::new(POLITE);
    let xid = "Oct 13 23:10:00 host kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=1234, name=wine";
    let mut c = f.pimax();
    c.args(["start", "--service-bin"])
        .arg(f.root.join("svc").join(&f.comm))
        .env("FAKE_KERNEL_EXTRA", xid);
    let o = c.output().unwrap();
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("[FAIL] kernel"), "{t}");
    assert!(t.contains("REBOOT"), "{t}");
    let mut c = f.pimax();
    c.args(["stop", "--service-bin"])
        .arg(f.root.join("svc").join(&f.comm))
        .env("FAKE_KERNEL_EXTRA", xid);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(2), "{}", text(&o));
    assert!(text(&o).contains("KERNEL LOG HAS GPU/DISPLAY ERRORS"));
}

#[test]
fn a_service_that_ignores_sigint_is_killed_after_the_grace_period() {
    let f = Fixture::new(STUBBORN);
    let o = f.run(&["start"]);
    assert!(o.status.success(), "{}", text(&o));
    let pid = f.pid().unwrap();
    let t = Instant::now();
    let o = f.run(&["stop", "--grace", "1"]);
    let s = text(&o);
    assert_eq!(
        o.status.code(),
        Some(1),
        "a forced kill is not a clean stop: {s}"
    );
    assert!(s.contains("WARNING"), "{s}");
    assert!(s.contains("sending SIGKILL"), "{s}");
    assert!(s.contains("killed"), "{s}");
    assert!(s.contains("had to be killed"), "{s}");
    assert!(
        t.elapsed() >= Duration::from_secs(1),
        "waited for the grace period"
    );
    wait_until("service death", 5, || !alive(pid));
    let n = f.notify_log();
    assert!(n.contains("Pimax VR: forced kill"), "{n}");
    assert!(
        !n.contains("Pimax VR stopped"),
        "no 'all fine' popup after a SIGKILL: {n}"
    );
}

#[test]
fn an_automatic_stop_with_no_kill_leaves_a_hung_service_alive_and_latches_the_gate() {
    let f = Fixture::new(STUBBORN);
    let o = f.run(&["start"]);
    assert!(o.status.success(), "{}", text(&o));
    let pid = f.pid().unwrap();
    let o = f.run(&["stop", "--grace", "1", "--no-kill"]);
    let s = text(&o);
    assert_eq!(o.status.code(), Some(1), "{s}");
    assert!(s.contains("NOT killed") && !s.contains("sending SIGKILL"), "{s}");
    std::thread::sleep(Duration::from_millis(300));
    assert!(alive(pid), "an automatic stop never kills monado-service");
    let gate = fs::read_to_string(f.root.join("omarchyvrd/gpu-gate")).unwrap();
    assert!(gate.contains("level=needs-reboot") && gate.contains("hung"), "{gate}");
    assert!(f.notify_log().contains("reboot needed"), "{}", f.notify_log());
}

#[test]
fn a_service_that_dies_during_startup_is_reported_with_its_last_words() {
    let f = Fixture::new(CRASHING);
    let o = f.run(&["start"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(t.contains("ended before it was ready (exit code 3)"), "{t}");
    assert!(t.contains("lease denied by the compositor"), "{t}");
    assert!(f.pid().is_none(), "no stale pid file");
    assert!(f.notify_log().contains("Pimax VR failed to start"));
}

#[test]
fn a_service_that_never_calibrates_times_out_but_keeps_running() {
    let f = Fixture::new(NEVER_READY);
    let o = f.run(&["start", "--timeout", "1"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains("timed out waiting for the gyro calibration"),
        "{t}"
    );
    let pid = f.pid().unwrap();
    assert!(alive(pid), "left running so that the user can look at it");
    let o = f.run(&["stop"]);
    assert!(o.status.success(), "{}", text(&o));
    wait_until("service exit", 5, || !alive(pid));
}

#[test]
fn wait_mode_stays_in_the_foreground_and_stops_the_service_with_the_launcher() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--wait", "--service-bin"])
        .arg(f.root.join("svc").join(&f.comm))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = c.spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut seen = String::new();
    for l in lines.by_ref() {
        let l = l.unwrap();
        seen.push_str(&l);
        seen.push('\n');
        if l.contains("ready after") {
            break;
        }
    }
    assert!(seen.contains("ready after"), "{seen}");
    let pid = f.pid().unwrap();
    assert!(alive(pid));
    std::thread::sleep(Duration::from_millis(300));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the launcher stays while the service runs"
    );
    // SAFETY: plain syscall on our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let st = child.wait().unwrap();
    assert!(st.success(), "{st:?}");
    wait_until("service exit", 5, || !alive(pid));
    assert!(f.pid().is_none());
    assert!(f.monado_log().contains("Server exiting"));
}

#[test]
fn wait_mode_ends_cleanly_when_the_service_is_stopped_with_vr_stop() {
    // the desktop flow: "Pimax VR" runs `start --wait`, the "Stop Pimax VR" action runs `stop`
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--wait", "--service-bin"])
        .arg(f.root.join("svc").join(&f.comm))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    let mut out = String::new();
    for l in lines.by_ref() {
        let l = l.unwrap();
        out.push_str(&l);
        out.push('\n');
        if l.contains("omarchyvr vr: start your game") {
            break;
        }
    }
    assert!(out.contains("ready after"), "{out}");
    let pid = f.pid().unwrap();
    let stop = f.run(&["stop"]);
    assert!(stop.status.success(), "{}", text(&stop));
    let st = child.wait().unwrap();
    assert!(st.success(), "the supervisor ends cleanly: {st:?}");
    let rest: String = lines.map(|l| l.unwrap() + "\n").collect();
    assert!(
        rest.contains("monado-service ended (exit code 0)"),
        "{rest}"
    );
    assert!(rest.contains("no Xid / display engine errors"), "{rest}");
    wait_until("service exit", 5, || !alive(pid));
    assert!(f.pid().is_none(), "pid file removed");
    // the start lock is free again: a new start works
    let again = f.run(&["start", "--dry-run"]);
    assert!(again.status.success(), "{}", text(&again));
    assert!(f.notify_log().contains("Pimax VR stopped"));
}

#[test]
fn the_service_does_not_inherit_the_callers_descriptors() {
    // `flock -w 3600 lock omarchyvr vr start` must not leave the headset lock inside a service that lives for hours
    let f = Fixture::new(POLITE);
    let mut c = Command::new("bash");
    c.arg("-c")
        .arg("exec 7>/dev/null; exec \"$0\" \"$@\"")
        .arg(env!("CARGO_BIN_EXE_omarchyvr"))
        .args(["vr", "start", "--service-bin"])
        .arg(f.service());
    f.apply_env(&mut c);
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let log = f.monado_log();
    let fds = log
        .lines()
        .find_map(|l| l.strip_prefix("FAKE-FDS "))
        .expect("fd list in the log");
    let fds: Vec<&str> = fds.split_whitespace().collect();
    assert!(!fds.contains(&"7"), "fd 7 leaked into the service: {fds:?}");
    assert!(
        fds.contains(&"0") && fds.contains(&"1") && fds.contains(&"2"),
        "{fds:?}"
    );
    let o = f.run(&["stop"]);
    assert!(o.status.success(), "{}", text(&o));
}

#[test]
fn interrupting_the_launcher_while_the_service_starts_stops_the_service() {
    let f = Fixture::new(NEVER_READY);
    let mut c = f.pimax();
    c.args(["start", "--timeout", "60", "--service-bin"])
        .arg(f.service())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let child = c.spawn().unwrap();
    wait_until("service start", 5, || f.pid().is_some_and(alive));
    let pid = f.pid().unwrap();
    std::thread::sleep(Duration::from_millis(300));
    // SAFETY: plain syscall on our own child.
    unsafe { libc::kill(child.id() as i32, libc::SIGTERM) };
    let out = child.wait_with_output().unwrap();
    assert_eq!(
        out.status.code(),
        Some(1),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("interrupted while starting"), "{err}");
    assert!(err.contains("start aborted"), "{err}");
    wait_until("service exit", 5, || !alive(pid));
    assert!(f.pid().is_none(), "pid file removed");
    assert!(f.notify_log().contains("Pimax VR start aborted"));
}

#[test]
fn stop_leaves_services_it_did_not_start_alone_unless_all_is_given() {
    let f = Fixture::new(POLITE);
    // a monado-service of this user that `omarchyvr vr start` knows nothing about (no pid file)
    let mut other_cmd = Command::new(f.service());
    other_cmd
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .env("FAKE_READY", f.root.join("ready"));
    // The harness may run us with SIGINT ignored, and a shell script cannot trap a signal that was ignored on entry
    // (Monado installs its own handler with sigaction, so the real service is not affected).
    // SAFETY: only signal(2) between fork and exec.
    unsafe {
        std::os::unix::process::CommandExt::pre_exec(&mut other_cmd, || {
            libc::signal(libc::SIGINT, libc::SIG_DFL);
            Ok(())
        });
    }
    let mut other = other_cmd.spawn().unwrap();
    let opid = other.id();
    wait_until("fake service", 5, || alive(opid));
    // the script installs its SIGINT trap after a short start-up; bash would swallow an earlier SIGINT
    wait_until("SIGINT trap", 5, || f.root.join("ready").exists());
    let o = f.run(&["stop"]);
    let t = text(&o);
    assert_eq!(o.status.code(), Some(1), "{t}");
    assert!(
        t.contains(&format!("pid {opid}")) && t.contains("--all"),
        "{t}"
    );
    assert!(alive(opid), "the foreign service must still run");
    // status still sees it
    let st = f.run(&["status"]);
    assert!(
        text(&st).contains(&format!("service: running, pid {opid}")),
        "{}",
        text(&st)
    );
    let o = f.run(&["stop", "--all"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(
        other.wait().unwrap().code(),
        Some(0),
        "SIGINT handled, exit code 0"
    );
}

#[test]
fn two_concurrent_stops_deliver_a_single_sigint() {
    // Monado's shutdown handler is single-shot: a second SIGINT during the teardown would kill it half way
    let f = Fixture::new(SLOW_STOP);
    let o = f.run(&["start"]);
    assert!(o.status.success(), "{}", text(&o));
    let pid = f.pid().unwrap();
    let mut a = f.pimax();
    a.args(["stop", "--service-bin"])
        .arg(f.service())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut b = f.pimax();
    b.args(["stop", "--service-bin"])
        .arg(f.service())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let (ca, cb) = (a.spawn().unwrap(), b.spawn().unwrap());
    let (oa, ob) = (
        ca.wait_with_output().unwrap(),
        cb.wait_with_output().unwrap(),
    );
    assert!(oa.status.success(), "{}", text(&oa));
    assert!(ob.status.success(), "{}", text(&ob));
    wait_until("service exit", 5, || !alive(pid));
    assert_eq!(f.sigints(), 1, "exactly one SIGINT reached the service");
    let both = format!("{}{}", text(&oa), text(&ob));
    assert!(both.contains("exited cleanly"), "{both}");
    assert!(
        both.contains("not running"),
        "the second stop finds nothing left: {both}"
    );
}

#[test]
fn a_stop_with_a_client_attached_waits_longer_than_five_seconds() {
    // the service needs 1 s here; the point is that the wait is not cut at the default 5 s when a client is connected:
    // the log says "Client 1 connected" and the printed grace period is 15 s
    let f = Fixture::new(SLOW_STOP);
    let o = f.run(&["start"]);
    assert!(o.status.success(), "{}", text(&o));
    // append what Monado logs when a game connects
    let mut log = fs::OpenOptions::new()
        .append(true)
        .open(f.session().join("monado.log"))
        .unwrap();
    std::io::Write::write_all(&mut log, b" INFO [client_connected] Client 1 connected\n").unwrap();
    let o = f.run(&["stop"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains("a client is still connected: giving monado-service up to 15 s"),
        "{t}"
    );
    assert!(t.contains("SIGKILL after 15 s"), "{t}");
}

#[test]
fn caller_environment_is_announced_and_a_quiet_pimax_log_does_not_break_the_start() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--dry-run", "--service-bin"])
        .arg(f.service())
        .env("XRT_COMPOSITOR_OUTPUT_GAMMA", "0.8");
    let o = c.output().unwrap();
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(
        t.contains("note: passing on from your environment: XRT_COMPOSITOR_OUTPUT_GAMMA=0.8"),
        "{t}"
    );
    // PIMAX_LOG=warn would hide "gyro bias done": the launcher keeps INFO for the service
    let mut c = f.pimax();
    c.args(["start", "--service-bin"])
        .arg(f.service())
        .env("PIMAX_LOG", "warn");
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", text(&o));
    assert!(f.monado_log().contains("PLOG=info"), "{}", f.monado_log());
    assert!(f.run(&["stop"]).status.success());
}

#[test]
fn a_missing_service_binary_is_a_clear_error() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    // a unique file name: a real monado-service that happens to run must not influence this test
    let missing = format!("/nonexistent/{}", f.comm);
    c.args(["start", "--service-bin", &missing]);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o).contains("does not exist (build Monado first)"),
        "{}",
        text(&o)
    );
}

/// Header + vendor PVR (42 d2) + product 0x1019 (19 10) of the Dream Air's EDID.
fn headset_edid() -> Vec<u8> {
    let mut e = vec![0u8; 128];
    e[..8].copy_from_slice(&[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    e[8..12].copy_from_slice(&[0x42, 0xd2, 0x19, 0x10]);
    e
}

fn add_connector(f: &Fixture, name: &str, edid: &[u8]) {
    let d = f.root.join("sys/class/drm").join(name);
    fs::create_dir_all(&d).unwrap();
    fs::write(d.join("status"), "connected\n").unwrap();
    fs::write(d.join("edid"), edid).unwrap();
}

#[test]
fn the_headset_connector_is_found_by_its_edid_and_connector_overrides_it() {
    let f = Fixture::new(POLITE);
    add_connector(&f, "card1-DP-3", &headset_edid());
    let o = f.run(&["start", "--dry-run"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("headset found on connector DP-3"), "{t}");
    assert!(t.contains("XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-3"), "{t}");
    assert!(t.contains("DP-3 connected"), "{t}");
    let o = f.run(&["start", "--dry-run", "--connector", "DP-1"]);
    let t = text(&o);
    assert!(t.contains("XRT_COMPOSITOR_WAYLAND_CONNECTOR=DP-1"), "{t}");
    assert!(!t.contains("headset found on connector"), "{t}");
}

#[test]
fn a_service_without_cap_sys_nice_gets_a_warning_with_the_setcap_line() {
    let f = Fixture::new(POLITE);
    let o = f.run(&["start", "--dry-run"]);
    let t = text(&o);
    assert!(
        o.status.success(),
        "a missing capability must only warn: {t}"
    );
    assert!(t.contains("[warn] priority"), "{t}");
    assert!(
        t.contains(&format!(
            "sudo setcap cap_sys_nice+ep {}",
            f.service().display()
        )),
        "{t}"
    );
}

#[test]
fn desk_start_dry_run_lists_monado_passthrough_wayvr_in_order() {
    let f = Fixture::new(POLITE);
    let pt = f.root.join("bin/fake-passthrough");
    let wv = f.root.join("bin/fake-wayvr");
    script(&pt, "#!/bin/bash\nexit 0\n");
    script(&wv, "#!/bin/bash\nexit 0\n");
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    c.args(["desk", "start", "--dry-run", "--eye", "--service-bin"])
        .arg(f.service())
        .args(["--passthrough-bin"])
        .arg(&pt)
        .args(["--wayvr-bin"])
        .arg(&wv)
        .args(["--wayvr-home"])
        .arg(f.root.join("wv-home"))
        .args(["--perf-bin"])
        .arg(&pt)
        .args(["--pimax-bin"])
        .arg(&pt);
    f.apply_env(&mut c);
    let o = c.output().unwrap();
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    let i = |s: &str| t.find(s).unwrap_or_else(|| panic!("{s} missing in:\n{t}"));
    assert!(
        i("1. monado-service") < i("2. ") && i("2. ") < i("3. "),
        "{t}"
    );
    assert!(t.contains(&format!("{} run", pt.display())), "{t}");
    assert!(t.contains("--openxr --show --no-autostart"), "{t}");
    assert!(t.contains("PIMAX_EYE=1"), "{t}");
    assert!(i("1a. VR screens: 4") < i("1. monado-service"), "{t}");
    assert!(t.contains("screens watch --wayvrctl-bin"), "{t}");
    assert!(!f.root.join("hypr.state").exists(), "a dry run creates no output");
    // dry run starts and creates nothing
    assert!(!f.root.join("wv-home").exists());
    assert!(!f.session().join("passthrough.pid").exists());
    // the same without passthrough has two steps only
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    c.args([
        "desk",
        "start",
        "--dry-run",
        "--no-passthrough",
        "--no-perf",
        "--screens",
        "1",
        "--service-bin",
    ])
    .arg(f.service())
    .args(["--wayvr-bin"])
    .arg(&wv);
    f.apply_env(&mut c);
    let t = text(&c.output().unwrap());
    assert!(!t.contains("3. ") && !t.contains("passthrough"), "{t}");
}

#[test]
fn desk_start_refuses_before_starting_monado_when_a_binary_is_missing() {
    let f = Fixture::new(POLITE);
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    c.args(["desk", "start", "--service-bin"])
        .arg(f.service())
        .args(["--passthrough-bin", "/nonexistent/omarchyvr-passthrough"]);
    f.apply_env(&mut c);
    let o = c.output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(
        text(&o).contains("/nonexistent/omarchyvr-passthrough does not exist"),
        "{}",
        text(&o)
    );
    assert!(f.pid().is_none(), "Monado must not have been started");
}

#[test]
fn mount_shared_prints_the_command_and_reports_the_mount_state() {
    let f = Fixture::new(POLITE);
    let proc_root = f.root.join("proc");
    fs::create_dir_all(&proc_root).unwrap();
    let run = |mounts: &str| {
        fs::write(proc_root.join("mounts"), mounts).unwrap();
        let mut mc = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
        omarchyvr_config::isolate_command(&mut mc, &proc_root);
        let o = mc
            .arg("mount-shared")
            .env("PIMAX_PROC_ROOT", &proc_root)
            .env("PIMAX_SHARED_UUID", "0123456789ABCDEF")
            .output()
            .unwrap();
        assert!(o.status.success(), "{}", text(&o));
        text(&o)
    };
    let t = run("/dev/a / btrfs rw 0 0\n");
    assert!(t.contains("NOT mounted: /mnt/shared"), "{t}");
    assert!(t.contains("UUID=0123456789ABCDEF /mnt/shared"), "{t}");
    let t = run("/dev/nvme1n1p2 /mnt/shared ntfs3 rw 0 0\n");
    assert!(
        t.contains("mounted: /mnt/shared (/dev/nvme1n1p2, ntfs3)"),
        "{t}"
    );
    assert!(!t.contains("NOT mounted"), "{t}");
}

#[test]
fn desk_start_and_stop_run_the_processes_in_order_and_stop_them_in_reverse() {
    let f = Fixture::new(POLITE);
    let order = f.root.join("order.log");
    let client = |name: &str| {
        let p = f.root.join("bin").join(format!("{}-{name}", f.comm));
        script(
            &p,
            &format!(
                "#!/bin/bash\necho \"start {name} $*\" >> '{o}'\ntrap 'echo \"int {name}\" >> \"{o}\"; exit 0' INT TERM\nwhile true; do sleep 0.05; done\n",
                o = order.display()
            ),
        );
        p
    };
    let (pt, wv, sc) = (client("pt"), client("wv"), client("sc"));
    let desk = |sub: &str, extra: &[&str]| {
        let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
        c.args(["desk", sub, "--service-bin"])
            .arg(f.service())
            .arg("--passthrough-bin")
            .arg(&pt)
            .arg("--wayvr-bin")
            .arg(&wv)
            .arg("--wayvr-home")
            .arg(f.root.join("wv-home"))
            .args(["--settle", "0.5", "--no-perf"])
            .arg("--pimax-bin")
            .arg(&sc)
            .args(extra);
        f.apply_env(&mut c);
        c.output().unwrap()
    };
    let o = desk("start", &[]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(f.pid().is_some_and(alive), "{t}");
    let pid = |n: &str| -> u32 {
        fs::read_to_string(f.session().join(format!("{n}.pid")))
            .unwrap()
            .trim()
            .parse()
            .unwrap()
    };
    let (ppt, pwv) = (pid("passthrough"), pid("wayvr"));
    assert!(alive(ppt) && alive(pwv));
    let started = fs::read_to_string(&order).unwrap();
    // the VR screens' outputs exist before anything of the session starts
    let first_start = started.find("start pt").unwrap();
    assert!(started.find("hyprctl output create headless PIMAX-LEFT").unwrap() < first_start, "{started}");
    assert!(started.find("hyprctl eval hl.monitor({ output = \"PIMAX-UP\"").unwrap() < started.find("hyprctl output create headless PIMAX-UP").unwrap());
    assert_eq!(fs::read_to_string(f.root.join("hypr.state")).unwrap().lines().count(), 3);
    assert!(fs::read_to_string(f.session().join("screens.txt")).unwrap().starts_with("screens: 4 (SUNSHINE, left = PIMAX-LEFT"));
    let lines: Vec<&str> = started.lines().filter(|l| l.starts_with("start")).collect();
    assert!(lines[0].starts_with("start pt run"), "{started}");
    assert!(
        lines[1].starts_with("start wv --openxr --show --no-autostart"),
        "{started}"
    );
    assert!(f
        .root
        .join("wv-home/config/wayvr/conf.d/10-am.yaml")
        .exists());
    let st = desk("status", &[]);
    assert!(
        text(&st).contains(&format!("wayvr: running, pid {pwv}")),
        "{}",
        text(&st)
    );

    let o = desk("stop", &["--grace", "10"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    let all = fs::read_to_string(&order).unwrap();
    let ints: Vec<&str> = all.lines().filter(|l| l.starts_with("int")).collect();
    assert_eq!(ints, ["int sc", "int wv", "int pt"], "{all}");
    assert!(lines[2].starts_with("start sc desk screens watch"), "{started}");
    // removed after the session, once Monado is gone
    assert!(all.rfind("hyprctl output remove PIMAX-LEFT").unwrap() > all.find("int pt").unwrap(), "{all}");
    assert_eq!(fs::read_to_string(f.root.join("hypr.state")).unwrap(), "");
    assert!(!alive(ppt) && !alive(pwv));
    assert!(f.pid().is_none(), "monado pid file removed");
    assert_eq!(f.sigints(), 1, "Monado got exactly one SIGINT");
    assert!(t.contains("kernel log of this boot: no Xid"), "{t}");
    assert!(!f.session().join("wayvr.pid").exists());
}

#[test]
fn dry_run_warns_when_active_runtime_points_at_another_build_and_start_repairs_it() {
    let f = Fixture::new(POLITE);
    let home = f.root.join("home");
    let (old, new) = (f.root.join("old-build"), f.root.join("new-build"));
    for d in [&old, &new] {
        std::fs::create_dir_all(d).unwrap();
        std::fs::write(d.join("openxr_monado-dev.json"), "{}").unwrap();
    }
    let link = home.join(".config/openxr/1/active_runtime.json");
    std::fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(old.join("openxr_monado-dev.json"), &link).unwrap();
    let o = f.run(&["start", "--dry-run", "--monado-build", new.to_str().unwrap()]);
    let t = text(&o);
    assert!(t.contains("openxr runtime") && t.contains("old-build"), "{t}");
    assert_eq!(std::fs::read_link(&link).unwrap(), old.join("openxr_monado-dev.json"), "dry run changes nothing");
    let o = f.run(&["start", "--monado-build", new.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(std::fs::read_link(&link).unwrap(), new.join("openxr_monado-dev.json"));
    let o = f.run(&["stop"]);
    assert!(o.status.success(), "{}", text(&o));
    assert_eq!(std::fs::read_link(&link).unwrap(), new.join("openxr_monado-dev.json"), "stop leaves it");
}

#[test]
fn monado_start_keeps_the_openxr_runtime_when_games_run_with_steamvr() {
    // Work (desk start) runs Monado while the app says "Games run with: SteamVR": the global OpenXR runtime keeps
    // pointing at SteamVR (WayVR gets XR_RUNTIME_JSON itself); live finding 2026-10-15
    let f = Fixture::new(POLITE);
    let home = f.root.join("home");
    fs::create_dir_all(home.join(".config/omarchyvr")).unwrap();
    fs::write(home.join(".config/omarchyvr/games.toml"), "runtime = \"steamvr\"\n").unwrap();
    let steam = f.root.join("steamxr_linux64.json");
    fs::write(&steam, "{}").unwrap();
    let link = home.join(".config/openxr/1/active_runtime.json");
    fs::create_dir_all(link.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&steam, &link).unwrap();
    let o = f.run(&["start", "--runtime", "monado"]);
    let t = text(&o);
    assert!(o.status.success(), "{t}");
    assert!(t.contains("OpenXR runtime left as it is (Games run with: SteamVR)"), "{t}");
    assert!(!t.contains("[warn] openxr runtime"), "no relink warning: {t}");
    assert_eq!(fs::read_link(&link).unwrap(), steam, "the games runtime is kept");
    f.run(&["stop"]);
}

#[test]
fn no_set_runtime_leaves_active_runtime_alone() {
    let f = Fixture::new(POLITE);
    let link = f.root.join("home/.config/openxr/1/active_runtime.json");
    let o = f.run(&["start", "--no-set-runtime"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!link.exists() && std::fs::symlink_metadata(&link).is_err());
    f.run(&["stop"]);
}

#[test]
fn preflight_warns_about_a_missing_camera_and_no_auto_recover_skips_the_recovery() {
    let f = Fixture::new(POLITE);
    fs::remove_dir_all(f.root.join("sys/bus/usb/devices/1-4.4")).unwrap();
    fs::remove_dir_all(f.root.join("sys/class/video4linux")).unwrap();
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    c.args(["vr", "start", "--dry-run", "--no-auto-recover", "--service-bin"])
        .arg(f.service());
    f.apply_env(&mut c);
    let t = text(&c.output().unwrap());
    assert!(t.contains("[warn] camera") && t.contains("camera missing: will auto-recover"), "{t}");
    assert!(!t.contains("restarting the headset"), "{t}");
}

#[test]
fn start_records_its_arguments_and_restart_without_a_record_says_so() {
    let f = Fixture::new(POLITE);
    let o = f.pimax().arg("restart").output().unwrap();
    assert_eq!(o.status.code(), Some(1));
    assert!(text(&o).contains("no recorded start"), "{}", text(&o));
    let o = f.run(&["start", "--dry-run"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!f.session().join("start-argv.txt").exists(), "a dry run records nothing");
    let o = f.run(&["start", "--fps", "90"]);
    assert!(o.status.success(), "{}", text(&o));
    let rec = fs::read_to_string(f.session().join("start-argv.txt")).unwrap();
    assert!(rec.starts_with("vr\n--fps\n90\n"), "{rec}");
    // a user stop leaves the marker that keeps a watchdog from fighting it
    let o = f.run(&["stop"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(f.session().join("user-stop").exists());
    // the next start clears it again
    let o = f.run(&["start"]);
    assert!(o.status.success(), "{}", text(&o));
    assert!(!f.session().join("user-stop").exists());
    let _ = f.run(&["stop"]);
}

#[test]
fn start_spawns_the_watchdog_which_leaves_when_the_user_stops() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--service-bin"]).arg(f.service()).env_remove("PIMAX_NO_WATCHDOG");
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", text(&o));
    let wd: u32 = fs::read_to_string(f.session().join("watchdog.pid")).unwrap().trim().parse().unwrap();
    assert!(alive(wd), "watchdog runs");
    assert!(text(&o).contains("session watchdog started"), "{}", text(&o));
    let o = f.run(&["stop"]);
    assert!(o.status.success(), "{}", text(&o));
    for _ in 0..80 {
        if !alive(wd) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    panic!("the watchdog did not leave after a user stop");
}

#[test]
fn no_watchdog_flag_starts_none() {
    let f = Fixture::new(POLITE);
    let mut c = f.pimax();
    c.args(["start", "--no-watchdog", "--service-bin"]).arg(f.service()).env_remove("PIMAX_NO_WATCHDOG");
    let o = c.output().unwrap();
    assert!(o.status.success(), "{}", text(&o));
    assert!(!f.session().join("watchdog.pid").exists());
    let _ = f.run(&["stop"]);
}
