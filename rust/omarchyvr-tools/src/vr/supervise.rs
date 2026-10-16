//! `omarchyvr desk|vr restart` and the session watchdog.
//!
//! Every non-dry `desk start` / `vr start` records its argv in `<session dir>/start-argv.txt` (line 1: `desk` or `vr`,
//! then one argument per line). `restart` stops with grace 30 and starts again with exactly those arguments, so
//! nothing the user chose is lost. The start (with auto camera recovery, see `reset::auto_recover`) does the rest.
//!
//! The watchdog is a detached `pimax <kind> watchdog` process that `start` spawns (opt out: `--no-watchdog`; not with
//! `--wait` or `--dry-run`). Every 2 s it looks at monado-service (pid file), the headset HID 34a4:0044 and, when the
//! session needs it, the camera 34a4:0014 (resilience plan P0):
//! - monado-service dead for 6 s: stop (`--no-kill`) -> start with the recorded arguments, at most 3 crash restarts per
//!   10 minutes, then it gives up with a critical notification;
//! - headset unplugged: it waits and spends no try ("Headset disconnected", once; nothing is stopped). Back and steady
//!   for 4 s: a desk session is restarted (no try spent); a `vr` session (maybe a game) only gets a notification.
//!   After 30 minutes away it stops waiting;
//! - camera gone with the headset present: 20 s to come back by itself. Back after 6 s or more: a desk restart (the
//!   broker exits on the first V4L2 error). Not back: one automatic reset-headset per watchdog (desk, through the
//!   start's auto recovery), then the user is asked; `vr` sessions are only notified;
//! - before every restart the GPU gate (`gate`, latched per boot) is read: a needs-reboot verdict (an Xid other than
//!   WayVR's Xid 31, Idling display engine, Stuck layer channel, a display acquire failure, a hung or killed Monado)
//!   ends the watchdog with a critical notification instead; WayVR's Xid 31 is only a caution;
//! - the restart's stop never SIGKILLs Monado (`--no-kill`; also implied by `PIMAX_WATCHDOG=1`).
//!
//! It never fights the user: a user `stop` leaves the marker file `user-stop` (removed by the
//! next start), a held `stop.lock` means a stop is running right now, and a newer start replaces the watchdog
//! (`watchdog.pid` names the current one). Children it runs have `PIMAX_WATCHDOG=1`: they leave no marker and spawn
//! no second watchdog (which would reset the try counter).
use super::gate::{self, Level};
use super::host::{Clock, Notifier, Procs, RealClock, RealHost};
use super::preflight::Host;
use super::reset::{usb_present_in, PID_CAM, PID_HID};
use super::session::{flock_file, read_pid, LockError};
use crate::args::Args;
use crate::{CliError, CliResult};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const ARGV_FILE: &str = "start-argv.txt";
pub const MARKER: &str = "user-stop";
pub const WD_PID: &str = "watchdog.pid";
pub const WD_LOG: &str = "watchdog.log";
/// Stop grace of a restart.
pub const RESTART_GRACE: u64 = 30;
pub const MAX_TRIES: usize = 3;
pub const WINDOW: Duration = Duration::from_secs(600);
/// A problem must last this long before the watchdog acts (USB glitches, a start in progress).
pub const DEBOUNCE: Duration = Duration::from_secs(6);
pub const POLL: Duration = Duration::from_secs(2);

/// True in processes the watchdog started.
pub fn in_watchdog() -> bool {
    std::env::var("PIMAX_WATCHDOG").is_ok_and(|v| v == "1")
}

/// Remember how this session was started; also clears the user-stop marker.
pub fn record_argv(dir: &Path, kind: &str, args: &[String]) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let mut s = String::from(kind);
    for a in args {
        s.push('\n');
        s.push_str(a);
    }
    s.push('\n');
    std::fs::write(dir.join(ARGV_FILE), s)?;
    let _ = std::fs::remove_file(dir.join(MARKER));
    Ok(())
}

/// Replace the recorded arguments in place (a live setting changed, e.g. the number of screens): atomic, and the
/// user-stop marker is left alone. The newer mtime makes the app take the new switches over.
pub fn rewrite_argv(dir: &Path, kind: &str, args: &[String]) -> std::io::Result<()> {
    let mut s = String::from(kind);
    for a in args {
        s.push('\n');
        s.push_str(a);
    }
    s.push('\n');
    let tmp = dir.join(format!("{ARGV_FILE}.tmp"));
    std::fs::write(&tmp, s)?;
    std::fs::rename(tmp, dir.join(ARGV_FILE))
}

/// `(kind, args)` of the last recorded start.
pub fn read_argv(dir: &Path) -> Option<(String, Vec<String>)> {
    let s = std::fs::read_to_string(dir.join(ARGV_FILE)).ok()?;
    let mut l = s.lines();
    let kind = l.next()?.to_string();
    Some((kind, l.map(str::to_string).collect()))
}

/// `stop` calls this: the watchdog must not restart what the user stopped.
pub fn mark_user_stop(dir: &Path) {
    if in_watchdog() {
        return;
    }
    let _ = std::fs::create_dir_all(dir);
    let _ = std::fs::write(dir.join(MARKER), "1\n");
}

/// Options `stop` needs to find the same session (the rest of the start argv would be rejected by `vr stop`).
pub fn stop_args(args: &[String]) -> Vec<String> {
    let mut v = Vec::new();
    let mut i = 0;
    while i < args.len() {
        let a = &args[i];
        let name = a.split('=').next().unwrap_or(a);
        if ["--session-dir", "--service-bin", "--monado-build", "--connector"].contains(&name) {
            v.push(a.clone());
            if !a.contains('=') && i + 1 < args.len() {
                v.push(args[i + 1].clone());
                i += 1;
            }
        }
        i += 1;
    }
    v
}

/// Runs `pimax <args>` and fails on a non-zero exit.
pub trait Runner {
    fn run(&mut self, args: &[String]) -> Result<(), String>;
}

/// stop (grace 30) then start with the recorded arguments. `auto` (the watchdog): the stop gets `--no-kill`, so a
/// hung Monado is never SIGKILLed by an automatic restart.
pub fn restart(kind: &str, dir: &Path, auto: bool, r: &mut dyn Runner, say: &mut dyn FnMut(&str)) -> Result<(), String> {
    let (k, args) = read_argv(dir).ok_or_else(|| {
        format!("no recorded start in {}: start a session once (omarchyvr {kind} start ...) first", dir.join(ARGV_FILE).display())
    })?;
    if k != kind {
        return Err(format!("the last start was `omarchyvr {k} start`: use `omarchyvr {k} restart`"));
    }
    say(&format!("stopping (grace {RESTART_GRACE} s)"));
    let mut stop = vec![kind.to_string(), "stop".into(), "--grace".into(), RESTART_GRACE.to_string()];
    if auto {
        stop.push("--no-kill".into());
    }
    stop.extend(stop_args(&args));
    r.run(&stop).map_err(|e| format!("stop failed: {e}"))?;
    say(&format!("starting again: omarchyvr {kind} start {}", args.join(" ")));
    let mut start = vec![kind.to_string(), "start".into()];
    start.extend(args);
    r.run(&start).map_err(|e| format!("start failed: {e}"))
}

/// Real runner: this very binary.
pub struct SelfRunner {
    pub watchdog: bool,
}

impl Runner for SelfRunner {
    fn run(&mut self, args: &[String]) -> Result<(), String> {
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut c = std::process::Command::new(exe);
        c.args(args).stdin(std::process::Stdio::null());
        if self.watchdog {
            c.env("PIMAX_WATCHDOG", "1");
        }
        let st = c.status().map_err(|e| e.to_string())?;
        if st.success() {
            Ok(())
        } else {
            Err(format!("omarchyvr {} exited with {st}", args.iter().take(2).cloned().collect::<Vec<_>>().join(" ")))
        }
    }
}

fn session_dir_of(a: &mut Args) -> Result<PathBuf, CliError> {
    Ok(a.opt("--session-dir")?
        .or_else(|| std::env::var("PIMAX_VR_SESSION_DIR").ok().filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(super::config::SESSION_DIR)))
}

/// `omarchyvr desk|vr restart [--session-dir DIR]`.
pub fn restart_cli(kind: &str, mut a: Args) -> CliResult {
    let dir = session_dir_of(&mut a)?;
    if let Some(x) = a.0.first() {
        return Err(CliError::usage(format!("unexpected argument {x}\nusage: omarchyvr {kind} restart [--session-dir DIR]")));
    }
    restart(kind, &dir, false, &mut SelfRunner { watchdog: false }, &mut |m| vout!("omarchyvr {kind}: {m}")).map_err(CliError::new)
}

// ---------------------------------------------------------------------------------------------------------------
// watchdog

/// What the watchdog looks at and does.
pub trait WatchEnv {
    /// The user stopped (marker) or a stop is running right now (stop.lock held).
    fn user_stop(&self) -> bool;
    /// A newer start replaced this watchdog.
    fn superseded(&self) -> bool;
    /// `None`: no session pid file; `Some(alive)`.
    fn session(&self) -> Option<bool>;
    fn hid(&self) -> bool;
    fn cam(&self) -> bool;
    /// The GPU safety gate (latched per boot; see `gate`): checked before every automatic restart.
    fn gate(&mut self) -> (Level, String);
    fn notify(&self, critical: bool, title: &str, body: &str);
    /// stop (never SIGKILL) then start with the recorded arguments.
    fn restart(&mut self) -> Result<(), String>;
    fn say(&mut self, line: &str);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    UserStop,
    Superseded,
    /// The restart tries were used up.
    GaveUp,
    /// The headset stayed unplugged for [`ABSENT_GIVE_UP`].
    DeviceGone,
    /// The GPU gate says reboot: nothing is restarted automatically.
    NeedsReboot,
}

/// What kind of session is watched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Policy {
    /// The session uses the camera (passthrough or 6DoF).
    pub needs_camera: bool,
    /// Desk mode: restored automatically on replug and after a camera drop. A `vr` session may carry a game, which
    /// an automatic restart would kill: it only gets notifications (user decision 4).
    pub desk: bool,
}

/// The headset must be back and steady this long before a replug counts.
pub const PLUG_STEADY: Duration = Duration::from_secs(4);
/// The camera may come back by itself within this (a 07:10 drop returned in 4 s); then the headset is reset.
pub const CAM_GRACE: Duration = Duration::from_secs(20);
/// Waiting for an unplugged headset ends after this.
pub const ABSENT_GIVE_UP: Duration = Duration::from_secs(30 * 60);

/// Why a restart is attempted; only crashes spend a try (waiting for or the return of a device does not).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Cause {
    Crash,
    Replug,
    CameraBack,
    CameraReset,
}

struct Watch<'a> {
    env: &'a mut dyn WatchEnv,
    tries: Vec<Duration>,
    /// a restart failed and left the session down
    down: bool,
    caution_told: bool,
}

impl Watch<'_> {
    /// Gate first, then the restart. `Err(end)` ends the watchdog.
    fn restart(&mut self, now: Duration, cause: Cause, why: &str) -> Result<(), End> {
        let (lv, gwhy) = self.env.gate();
        match lv {
            Level::NeedsReboot => {
                self.env.say(&format!("{why}, but the GPU gate says reboot ({gwhy}): no automatic restart"));
                self.env.notify(
                    true,
                    "VR session not restarted: reboot needed",
                    &format!("{why}. GPU/display state: {gwhy}. Restarting now would fail or wedge the display: reboot first (known NVIDIA issue)."),
                );
                return Err(End::NeedsReboot);
            }
            Level::Caution if !self.caution_told => {
                self.caution_told = true;
                self.env.say(&format!("GPU caution ({gwhy}): restarting anyway"));
                self.env.notify(false, "GPU caution", &format!("{gwhy}. The session is restarted anyway; reboot if the headset stays black."));
            }
            _ => {}
        }
        if cause == Cause::Crash {
            self.tries.retain(|t| now - *t < WINDOW);
            if self.tries.len() >= MAX_TRIES {
                self.env.say(&format!("giving up: {why}; {MAX_TRIES} restarts in the last 10 minutes did not hold"));
                self.env.notify(true, "Headset reconnection failed", &format!("{why}. Gave up after {MAX_TRIES} tries in 10 minutes. Try `omarchyvr desk restart` or check the cable and power."));
                return Err(End::GaveUp);
            }
            self.tries.push(now);
            self.env.say(&format!("{why}: restarting the session (try {}/{MAX_TRIES})", self.tries.len()));
        } else {
            self.env.say(&format!("{why}: restarting the session (no try spent)"));
        }
        self.env.notify(false, "Headset reconnecting...", why);
        match self.env.restart() {
            Ok(()) => {
                self.down = false;
                self.env.say("session restarted");
                self.env.notify(false, "Headset reconnected", "The session is running again.");
                Ok(())
            }
            Err(e) => {
                self.down = true;
                self.env.say(&format!("restart failed: {e}"));
                // a failed acquire or a hung Monado shows up in the gate right away
                if self.env.gate().0 == Level::NeedsReboot {
                    let (_, gwhy) = self.env.gate();
                    self.env.say(&format!("the GPU gate says reboot after the failed restart ({gwhy})"));
                    self.env.notify(true, "VR session down: reboot needed", &format!("The restart failed ({e}). {gwhy}. Reboot before the next session."));
                    return Err(End::NeedsReboot);
                }
                Ok(())
            }
        }
    }
}

/// Run until the user stops, a newer start replaces us, the tries are used up, the GPU gate says reboot, or the
/// headset stays unplugged for 30 minutes.
///
/// Rules (resilience plan P0): waiting for an absent device spends no tries; an unplugged headset is waited for
/// (notify once, never stop anything) and a desk session is restored once it is back and steady; a camera that does
/// not come back within [`CAM_GRACE`] gets one automatic reset-headset (desk; through the start's auto recovery),
/// then the user is asked; the restart's stop never SIGKILLs Monado; nothing restarts while the gate says reboot.
pub fn watch(env: &mut dyn WatchEnv, clock: &dyn Clock, p: Policy) -> End {
    let mut w = Watch { env, tries: Vec::new(), down: false, caution_told: false };
    let mut bad_since: Option<Duration> = None;
    // headset: first absent time of this outage, back-and-steady start, notified
    let mut hid_gone: Option<Duration> = None;
    let mut hid_back: Option<Duration> = None;
    let mut hid_told = false;
    // camera
    let mut cam_gone: Option<Duration> = None;
    let mut cam_told = false;
    let mut reset_used = false;
    loop {
        clock.sleep(POLL);
        if w.env.superseded() {
            return End::Superseded;
        }
        if w.env.user_stop() {
            return End::UserStop;
        }
        let now = clock.now();
        let session = w.env.session();

        // 1. the headset: wait while it is away, spend nothing
        if !w.env.hid() {
            hid_back = None;
            let since = *hid_gone.get_or_insert(now);
            bad_since = None;
            if now - since >= DEBOUNCE && !hid_told {
                hid_told = true;
                w.env.say("headset HID 34a4:0044 disappeared: waiting for it (no restart while it is away)");
                let body = if p.desk {
                    "Waiting for it to come back; the desk session is restored when it is plugged in again."
                } else {
                    "Waiting for it to come back; the VR session and any game keep running."
                };
                w.env.notify(false, "Headset disconnected", body);
            }
            if now - since >= ABSENT_GIVE_UP {
                w.env.say("the headset stayed away for 30 minutes: the watchdog stops waiting");
                w.env.notify(true, "Headset still disconnected", "Stopped waiting after 30 minutes. Plug it in and start the session again.");
                return End::DeviceGone;
            }
            continue;
        }
        if let Some(gone) = hid_gone {
            let back = *hid_back.get_or_insert(now);
            if now - back < PLUG_STEADY {
                continue;
            }
            hid_gone = None;
            hid_back = None;
            let was_long = back - gone >= DEBOUNCE;
            hid_told = false;
            if was_long {
                cam_gone = None; // the camera re-enumerates with the headset
                if p.desk || session == Some(false) || w.down {
                    if let Err(e) = w.restart(now, Cause::Replug, "headset HID 34a4:0044 is back") {
                        return e;
                    }
                } else {
                    w.env.say("headset is back; the VR session is left running (a restart would end a game)");
                    w.env.notify(false, "Headset reconnected", "If the view stays black, restart the session when your game allows: omarchyvr vr restart");
                }
                continue;
            }
        }

        // 2. the camera (passthrough, 6DoF): it may come back by itself; one automatic reset, then ask
        if p.needs_camera {
            if !w.env.cam() {
                let since = *cam_gone.get_or_insert(now);
                bad_since = None;
                if now - since < CAM_GRACE {
                    continue;
                }
                if p.desk && !reset_used {
                    reset_used = true;
                    w.env.say(&format!("camera 34a4:0014 gone for {:.0} s with the headset present: one automatic reset-headset", (now - since).as_secs_f64()));
                    if let Err(e) = w.restart(now, Cause::CameraReset, "camera 34a4:0014 disappeared") {
                        return e;
                    }
                    cam_gone = None;
                } else if !cam_told {
                    cam_told = true;
                    w.env.say("camera 34a4:0014 still gone: asking the user (no automatic reset)");
                    let body = if p.desk {
                        "The camera did not come back after one automatic headset restart. Try Restart headset (omarchyvr vr reset-headset after omarchyvr desk stop) once more, or reboot."
                    } else {
                        "6DoF and passthrough are paused. When your game allows: omarchyvr vr restart (it restarts the headset to recover the camera)."
                    };
                    w.env.notify(true, "Camera dropped", body);
                }
                continue;
            }
            if let Some(since) = cam_gone.take() {
                cam_told = false;
                if now - since >= DEBOUNCE {
                    // the broker exits on the first V4L2 error, so the camera clients are gone: restart (desk)
                    if p.desk || session == Some(false) || w.down {
                        if let Err(e) = w.restart(now, Cause::CameraBack, "camera 34a4:0014 is back") {
                            return e;
                        }
                    } else {
                        w.env.say("camera is back; the VR session is left running (a restart would end a game)");
                        w.env.notify(false, "Camera back", "6DoF stays paused until the session restarts: omarchyvr vr restart when your game allows.");
                    }
                    continue;
                }
            }
        }

        // 3. Monado: a crash spends a try
        let problem = match session {
            Some(false) => Some("monado-service died"),
            None if w.down => Some("the session is down after a failed restart"),
            _ => None,
        };
        let Some(why) = problem else {
            bad_since = None;
            continue;
        };
        let since = *bad_since.get_or_insert(now);
        if now - since < DEBOUNCE {
            continue;
        }
        bad_since = None;
        match w.restart(now, Cause::Crash, why) {
            Ok(()) => {}
            Err(e) => return e,
        }
    }
}

/// The gate from the latch of this boot, this boot's kernel GPU lines and the Monado log; a new finding is latched.
pub fn gate_verdict(env: &dyn Fn(&str) -> Option<String>, kernel: &[String], monado_log: &str) -> (Level, String) {
    let path = gate::gate_path(env);
    let boot = gate::boot_id(env);
    let mut best = match (&path, &boot) {
        (Some(p), Some(b)) => gate::read_latch(p, b).unwrap_or((Level::Ok, String::new())),
        _ => (Level::Ok, String::new()),
    };
    let (kl, kline) = gate::worst(kernel.iter().map(String::as_str), gate::classify_kernel_line);
    let (ml, mline) = gate::worst(monado_log.lines(), gate::classify_monado_line);
    for (lv, line) in [(kl, kline), (ml, mline)] {
        if lv > best.0 {
            let why = line.unwrap_or_default();
            if let (Some(p), Some(b)) = (&path, &boot) {
                gate::latch(p, b, lv, &why);
            }
            best = (lv, why);
        }
    }
    best
}

struct RealWatch {
    dir: PathBuf,
    host: RealHost,
    notifier: super::host::NotifySend,
    runner: SelfRunner,
    kind: String,
    me: u32,
    log: Option<std::fs::File>,
}

impl WatchEnv for RealWatch {
    fn user_stop(&self) -> bool {
        self.dir.join(MARKER).exists() || matches!(flock_file(&self.dir, "stop.lock", false), Err(LockError::Busy))
    }
    fn superseded(&self) -> bool {
        read_pid(&self.dir.join(WD_PID)) != Some(self.me)
    }
    fn session(&self) -> Option<bool> {
        read_pid(&self.dir.join("monado.pid")).map(|p| self.host.is_alive(p))
    }
    fn hid(&self) -> bool {
        usb_present_in(&self.host.sysfs, PID_HID)
    }
    fn cam(&self) -> bool {
        usb_present_in(&self.host.sysfs, PID_CAM)
    }
    fn gate(&mut self) -> (Level, String) {
        let env = |k: &str| std::env::var(k).ok();
        let kernel = match self.host.kernel_errors() {
            Ok(lines) => lines,
            Err(e) => {
                self.say(&format!("cannot read the kernel log for the GPU gate ({e})"));
                Vec::new()
            }
        };
        let monado = std::fs::read_to_string(self.dir.join("monado.log")).unwrap_or_default();
        gate_verdict(&env, &kernel, &monado)
    }
    fn notify(&self, critical: bool, title: &str, body: &str) {
        self.notifier.notify(critical, title, body);
    }
    fn restart(&mut self) -> Result<(), String> {
        let mut log = Vec::new();
        let kind = self.kind.clone();
        let r = restart(&kind, &self.dir, true, &mut self.runner, &mut |m| log.push(m.to_string()));
        for l in log {
            self.say(&l);
        }
        r
    }
    fn say(&mut self, line: &str) {
        use std::io::Write;
        let l = format!("{} watchdog: {line}", pimax::sys::local_hms());
        if let Some(f) = self.log.as_mut() {
            let _ = writeln!(f, "{l}");
        }
        vout!("{l}");
    }
}

/// `omarchyvr desk|vr watchdog --session-dir DIR [--camera] [--no-notify]` (started by `start`).
pub fn watchdog_cli(kind: &str, mut a: Args) -> CliResult {
    let dir = session_dir_of(&mut a)?;
    let camera = a.flag("--camera");
    let notify = !a.flag("--no-notify");
    if let Some(x) = a.0.first() {
        return Err(CliError::usage(format!("unexpected argument {x}")));
    }
    let me = std::process::id();
    let mut env = RealWatch {
        host: RealHost::from_env(),
        notifier: super::host::NotifySend::new(notify),
        runner: SelfRunner { watchdog: true },
        kind: kind.to_string(),
        me,
        log: std::fs::OpenOptions::new().create(true).append(true).open(dir.join(WD_LOG)).ok(),
        dir,
    };
    env.say(&format!("watching the {kind} session (camera {})", if camera { "needed" } else { "not needed" }));
    let policy = Policy { needs_camera: camera, desk: kind == "desk" };
    let end = watch(&mut env, &RealClock::new(), policy);
    env.say(&format!("exit: {end:?}"));
    if read_pid(&env.dir.join(WD_PID)) == Some(me) {
        let _ = std::fs::remove_file(env.dir.join(WD_PID));
    }
    Ok(())
}

/// Called by `start` after the session came up: spawn the watchdog (unless opted out: --no-watchdog or PIMAX_NO_WATCHDOG=1 / inside the watchdog).
pub fn spawn_watchdog(kind: &str, dir: &Path, needs_camera: bool, notify: bool) {
    use std::os::unix::process::CommandExt;
    if in_watchdog() || std::env::var("PIMAX_NO_WATCHDOG").is_ok_and(|v| !v.is_empty() && v != "0") {
        return;
    }
    let Ok(exe) = std::env::current_exe() else { return };
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join(WD_LOG));
    let mut c = std::process::Command::new(exe);
    c.args([kind, "watchdog", "--session-dir"]).arg(dir).stdin(std::process::Stdio::null());
    if needs_camera {
        c.arg("--camera");
    }
    if !notify {
        c.arg("--no-notify");
    }
    if let Ok(l) = log {
        if let Ok(l2) = l.try_clone() {
            c.stdout(l2);
        }
        c.stderr(l);
    }
    // SAFETY: setsid(2) is async-signal-safe; the watchdog outlives the terminal and the launcher.
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    match c.spawn() {
        Ok(ch) => {
            let _ = std::fs::write(dir.join(WD_PID), format!("{}\n", ch.id()));
            verr!("omarchyvr {kind}: session watchdog started (pid {}, log {}; --no-watchdog to skip)", ch.id(), dir.join(WD_LOG).display());
        }
        Err(e) => verr!("omarchyvr {kind}: WARNING: could not start the watchdog: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::{Cell, RefCell};

    struct Clk(Cell<Duration>);
    impl Clock for Clk {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// Scripted world: times in seconds of the fake clock.
    struct Env<'a> {
        clk: &'a Clk,
        hid_gone: Vec<(f64, f64)>,
        cam_gone: Vec<(f64, f64)>,
        /// a successful restart brings a missing camera back (the reset-headset worked)
        cam_heals: bool,
        monado_dies_at: Vec<f64>,
        user_stop_at: Option<f64>,
        superseded_at: Option<f64>,
        /// gate level from that time on
        gate_at: Vec<(f64, Level)>,
        /// restart results in order (then Ok)
        restarts: Vec<Result<(), String>>,
        restarted: Vec<f64>,
        notes: RefCell<Vec<(bool, String)>>,
        lines: Vec<String>,
        /// a successful restart heals every window that started before it
        last_restart: f64,
    }
    impl<'a> Env<'a> {
        fn new(clk: &'a Clk) -> Self {
            Self {
                clk,
                hid_gone: vec![],
                cam_gone: vec![],
                cam_heals: true,
                monado_dies_at: vec![],
                user_stop_at: None,
                superseded_at: None,
                gate_at: vec![],
                restarts: vec![],
                restarted: vec![],
                notes: RefCell::new(vec![]),
                lines: vec![],
                last_restart: -1.0,
            }
        }
        fn t(&self) -> f64 {
            self.clk.now().as_secs_f64()
        }
        fn in_window(&self, w: &[(f64, f64)], heal: bool) -> bool {
            w.iter().any(|&(a, b)| self.t() >= a && self.t() < b && (!heal || self.last_restart < a))
        }
    }
    impl WatchEnv for Env<'_> {
        fn user_stop(&self) -> bool {
            self.user_stop_at.is_some_and(|u| self.t() >= u)
        }
        fn superseded(&self) -> bool {
            self.superseded_at.is_some_and(|u| self.t() >= u)
        }
        fn session(&self) -> Option<bool> {
            Some(!self.monado_dies_at.iter().any(|&d| self.t() >= d && self.last_restart < d))
        }
        fn hid(&self) -> bool {
            !self.in_window(&self.hid_gone, false)
        }
        fn cam(&self) -> bool {
            !self.in_window(&self.cam_gone, self.cam_heals)
        }
        fn gate(&mut self) -> (Level, String) {
            let t = self.t();
            self.gate_at.iter().filter(|g| t >= g.0).map(|g| (g.1, format!("{:?} line", g.1))).max().unwrap_or((Level::Ok, String::new()))
        }
        fn notify(&self, c: bool, t: &str, _b: &str) {
            self.notes.borrow_mut().push((c, t.to_string()));
        }
        fn restart(&mut self) -> Result<(), String> {
            self.restarted.push(self.t());
            self.clk.sleep(Duration::from_secs(1)); // a restart takes time
            let r = if self.restarts.is_empty() { Ok(()) } else { self.restarts.remove(0) };
            if r.is_ok() {
                self.last_restart = self.t();
            }
            r
        }
        fn say(&mut self, l: &str) {
            self.lines.push(l.to_string());
        }
    }

    fn titles(e: &Env) -> Vec<String> {
        e.notes.borrow().iter().map(|n| n.1.clone()).collect()
    }

    const DESK_CAM: Policy = Policy { needs_camera: true, desk: true };
    const DESK: Policy = Policy { needs_camera: false, desk: true };
    const VR: Policy = Policy { needs_camera: false, desk: false };
    const VR_CAM: Policy = Policy { needs_camera: true, desk: false };

    #[test]
    fn healthy_session_is_left_alone_until_the_user_stops() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.user_stop_at = Some(300.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert!(e.restarted.is_empty() && e.notes.borrow().is_empty());
    }

    #[test]
    fn short_usb_glitches_are_ignored() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 14.0)];
        e.cam_gone = vec![(20.0, 23.0)];
        e.user_stop_at = Some(60.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert!(e.restarted.is_empty() && e.notes.borrow().is_empty(), "{:?}", e.notes);
    }

    #[test]
    fn an_unplug_of_five_minutes_spends_no_tries_and_the_replug_restores_the_desk_once() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 310.0)];
        e.cam_gone = vec![(10.0, 310.0)]; // the camera goes with the headset
        // then three crashes: all three tries are still there
        e.monado_dies_at = vec![400.0, 450.0, 500.0];
        e.user_stop_at = Some(560.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert_eq!(e.restarted.len(), 4, "{:?}", e.restarted);
        assert!(e.restarted[0] >= 314.0 && e.restarted[0] < 318.0, "{:?}", e.restarted);
        assert!(e.lines.iter().any(|l| l.contains("no try spent")), "{:?}", e.lines);
        assert!(e.lines.iter().any(|l| l.contains("try 3/3")), "{:?}", e.lines);
        let t = titles(&e);
        assert_eq!(t.iter().filter(|x| *x == "Headset disconnected").count(), 1, "{t:?}");
        assert!(!t.iter().any(|x| x.contains("failed")), "{t:?}");
    }

    #[test]
    fn a_vr_session_is_never_restarted_for_an_unplug_or_a_camera_drop() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 100.0)];
        e.cam_gone = vec![(200.0, 1e9)];
        e.user_stop_at = Some(400.0);
        assert_eq!(watch(&mut e, &c, VR_CAM), End::UserStop);
        assert!(e.restarted.is_empty());
        let t = titles(&e);
        assert_eq!(t, ["Headset disconnected", "Headset reconnected", "Camera dropped"], "{t:?}");
    }

    #[test]
    fn the_watchdog_stops_waiting_after_thirty_minutes_away() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 1e9)];
        assert_eq!(watch(&mut e, &c, DESK), End::DeviceGone);
        assert!(e.restarted.is_empty());
        assert!(c.now() >= ABSENT_GIVE_UP && c.now() < ABSENT_GIVE_UP + Duration::from_secs(20));
        assert!(e.notes.borrow().last().unwrap().0, "critical");
    }

    #[test]
    fn monado_dying_while_the_headset_is_away_waits_for_the_headset() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 200.0)];
        e.monado_dies_at = vec![12.0];
        e.user_stop_at = Some(300.0);
        assert_eq!(watch(&mut e, &c, VR), End::UserStop);
        assert_eq!(e.restarted.len(), 1);
        assert!(e.restarted[0] >= 204.0, "{:?}", e.restarted);
    }

    #[test]
    fn a_camera_that_does_not_come_back_gets_one_reset_then_the_user_is_asked() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.cam_gone = vec![(10.0, 1e9)];
        e.cam_heals = false;
        e.user_stop_at = Some(600.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert_eq!(e.restarted.len(), 1, "{:?}", e.restarted);
        assert!(e.restarted[0] >= 30.0 && e.restarted[0] < 34.0, "{:?}", e.restarted);
        let t = titles(&e);
        assert_eq!(t, ["Headset reconnecting...", "Headset reconnected", "Camera dropped"], "{t:?}");
        assert!(e.notes.borrow().last().unwrap().0, "the question is critical");
    }

    #[test]
    fn a_reset_that_heals_the_camera_and_a_failed_reset_is_not_repeated() {
        // heals
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.cam_gone = vec![(10.0, 1e9)];
        e.user_stop_at = Some(300.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert_eq!(e.restarted.len(), 1);
        assert_eq!(titles(&e), ["Headset reconnecting...", "Headset reconnected"]);
        // the reset's start fails (camera never back): the session is down, but no second reset and no try loop
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.cam_gone = vec![(10.0, 1e9)];
        e.cam_heals = false;
        e.restarts = vec![Err("camera did not come back".into())];
        e.user_stop_at = Some(900.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert_eq!(e.restarted.len(), 1, "{:?}", e.restarted);
        assert!(titles(&e).contains(&"Camera dropped".to_string()));
    }

    #[test]
    fn a_camera_blip_that_returns_restarts_the_desk_but_a_short_one_does_not() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.cam_gone = vec![(10.0, 22.0)];
        e.cam_heals = false;
        e.user_stop_at = Some(100.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert_eq!(e.restarted.len(), 1);
        assert!(e.restarted[0] >= 22.0 && e.restarted[0] < 25.0, "{:?}", e.restarted);
        assert!(e.lines.iter().any(|l| l.contains("camera 34a4:0014 is back") && l.contains("no try spent")));
    }

    #[test]
    fn camera_loss_is_ignored_when_the_session_does_not_need_it() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.cam_gone = vec![(10.0, 1e9)];
        e.user_stop_at = Some(100.0);
        assert_eq!(watch(&mut e, &c, DESK), End::UserStop);
        assert!(e.restarted.is_empty());
    }

    #[test]
    fn monado_death_restarts() {
        for p in [DESK, VR] {
            let c = Clk(Cell::new(Duration::ZERO));
            let mut e = Env::new(&c);
            e.monado_dies_at = vec![10.0];
            e.user_stop_at = Some(100.0);
            watch(&mut e, &c, p);
            assert_eq!(e.restarted.len(), 1, "{p:?}");
            assert!(e.lines.iter().any(|l| l.contains("try 1/3")));
        }
    }

    #[test]
    fn no_restart_when_the_gpu_gate_says_reboot() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0];
        e.gate_at = vec![(11.0, Level::NeedsReboot)];
        assert_eq!(watch(&mut e, &c, DESK), End::NeedsReboot);
        assert!(e.restarted.is_empty());
        let n = e.notes.borrow();
        assert!(n.len() == 1 && n[0].0 && n[0].1.contains("reboot"), "{n:?}");
        // the replug path is gated too
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.hid_gone = vec![(10.0, 60.0)];
        e.gate_at = vec![(0.0, Level::NeedsReboot)];
        assert_eq!(watch(&mut e, &c, DESK), End::NeedsReboot);
        assert!(e.restarted.is_empty());
    }

    #[test]
    fn a_wayvr_xid31_is_only_a_caution() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0, 100.0];
        e.gate_at = vec![(0.0, Level::Caution)];
        e.user_stop_at = Some(200.0);
        assert_eq!(watch(&mut e, &c, DESK), End::UserStop);
        assert_eq!(e.restarted.len(), 2);
        assert_eq!(titles(&e).iter().filter(|t| *t == "GPU caution").count(), 1);
    }

    #[test]
    fn a_failed_restart_that_trips_the_gate_ends_the_watchdog() {
        // the start failed on the display acquire: the gate latches and nothing is retried
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0];
        e.restarts = vec![Err("monado-service ended during start-up".into())];
        e.gate_at = vec![(16.5, Level::NeedsReboot)];
        assert_eq!(watch(&mut e, &c, DESK), End::NeedsReboot);
        assert_eq!(e.restarted.len(), 1);
        assert_eq!(titles(&e).last().unwrap(), "VR session down: reboot needed");
    }

    #[test]
    fn gives_up_after_three_tries_in_ten_minutes() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0];
        e.restarts = vec![Err("a".into()), Err("b".into()), Err("c".into()), Err("d".into())];
        assert_eq!(watch(&mut e, &c, DESK), End::GaveUp);
        assert_eq!(e.restarted.len(), 3);
        assert!(c.now() < Duration::from_secs(600));
        let t = titles(&e);
        assert_eq!(t.last().unwrap(), "Headset reconnection failed");
        assert!(e.notes.borrow().last().unwrap().0, "critical");
    }

    #[test]
    fn a_runtime_that_keeps_crashing_exhausts_the_tries_but_spaced_crashes_do_not() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = (0..10).map(|k| 10.0 + 150.0 * k as f64).collect();
        assert_eq!(watch(&mut e, &c, DESK), End::GaveUp);
        assert_eq!(e.restarted.len(), 3);

        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = (0..5).map(|k| 10.0 + 700.0 * k as f64).collect();
        e.user_stop_at = Some(3500.0);
        assert_eq!(watch(&mut e, &c, DESK), End::UserStop);
        assert_eq!(e.restarted.len(), 5);
    }

    #[test]
    fn never_fights_a_user_stop_or_a_newer_start() {
        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0];
        e.user_stop_at = Some(10.0); // the stop that killed it
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::UserStop);
        assert!(e.restarted.is_empty());

        let c = Clk(Cell::new(Duration::ZERO));
        let mut e = Env::new(&c);
        e.monado_dies_at = vec![10.0];
        e.superseded_at = Some(12.0);
        assert_eq!(watch(&mut e, &c, DESK_CAM), End::Superseded);
        assert!(e.restarted.is_empty());
    }

    #[test]
    fn gate_verdict_latches_new_findings_per_boot() {
        let d = tmp("gate");
        let proc_root = d.join("proc");
        std::fs::create_dir_all(proc_root.join("sys/kernel/random")).unwrap();
        std::fs::write(proc_root.join("sys/kernel/random/boot_id"), "boot-1\n").unwrap();
        let (r, pr) = (d.join("run").display().to_string(), proc_root.display().to_string());
        let env = move |k: &str| match k {
            "XDG_RUNTIME_DIR" => Some(r.clone()),
            "PIMAX_PROC_ROOT" => Some(pr.clone()),
            _ => None,
        };
        let wayvr = "kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=7, name=wayvr, channel 0x1".to_string();
        assert_eq!(gate_verdict(&env, &[], "gyro bias done\n").0, Level::Ok);
        assert_eq!(gate_verdict(&env, std::slice::from_ref(&wayvr), "").0, Level::Caution);
        // the caution is latched: an empty kernel log later still reads caution
        assert_eq!(gate_verdict(&env, &[], "").0, Level::Caution);
        let (lv, why) = gate_verdict(&env, &[wayvr], "ERROR vkAcquireDrmDisplayEXT failed: VK_ERROR_INITIALIZATION_FAILED\n");
        assert_eq!(lv, Level::NeedsReboot);
        assert!(why.contains("vkAcquireDrmDisplayEXT"), "{why}");
        // latched: the next Monado log (a new start) is clean but the verdict stays until a reboot
        assert_eq!(gate_verdict(&env, &[], "").0, Level::NeedsReboot);
        std::fs::write(proc_root.join("sys/kernel/random/boot_id"), "boot-2\n").unwrap();
        assert_eq!(gate_verdict(&env, &[], "").0, Level::Ok);
    }

    struct Rec(Vec<Vec<String>>, Option<usize>);
    impl Runner for Rec {
        fn run(&mut self, args: &[String]) -> Result<(), String> {
            self.0.push(args.to_vec());
            if self.1 == Some(self.0.len()) {
                return Err("boom".into());
            }
            Ok(())
        }
    }

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("pimax-supervise-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn sv(a: &[&str]) -> Vec<String> {
        a.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn argv_roundtrip_and_marker_cleared_by_start() {
        let d = tmp("argv");
        mark_user_stop(&d);
        assert!(d.join(MARKER).exists());
        record_argv(&d, "desk", &sv(&["--6dof", "--preset", "work-3", "--session-dir", "/x y"])).unwrap();
        assert!(!d.join(MARKER).exists());
        let (k, a) = read_argv(&d).unwrap();
        assert_eq!(k, "desk");
        assert_eq!(a, sv(&["--6dof", "--preset", "work-3", "--session-dir", "/x y"]));
        assert_eq!(stop_args(&a), sv(&["--session-dir", "/x y"]));
        assert_eq!(stop_args(&sv(&["--service-bin=/a", "--6dof", "--connector", "DP-2"])), sv(&["--service-bin=/a", "--connector", "DP-2"]));
    }

    #[test]
    fn restart_stops_with_grace_30_then_starts_with_the_same_flags() {
        let d = tmp("restart");
        let args = sv(&["--6dof", "--ctrl-track", "--no-watchdog"]);
        record_argv(&d, "desk", &args).unwrap();
        let mut r = Rec(vec![], None);
        let mut said = vec![];
        restart("desk", &d, false, &mut r, &mut |m| said.push(m.to_string())).unwrap();
        assert_eq!(r.0[0], sv(&["desk", "stop", "--grace", "30"]));
        assert_eq!(r.0[1], sv(&["desk", "start", "--6dof", "--ctrl-track", "--no-watchdog"]));
        assert_eq!(said.len(), 2);
        // the watchdog's restart never lets the stop SIGKILL Monado
        let mut r = Rec(vec![], None);
        restart("desk", &d, true, &mut r, &mut |_| {}).unwrap();
        assert_eq!(r.0[0], sv(&["desk", "stop", "--grace", "30", "--no-kill"]));
    }

    #[test]
    fn restart_errors_are_clear() {
        let d = tmp("restart-err");
        let mut r = Rec(vec![], None);
        let e = restart("desk", &d, false, &mut r, &mut |_| {}).unwrap_err();
        assert!(e.contains("no recorded start") && r.0.is_empty(), "{e}");
        record_argv(&d, "vr", &sv(&["--6dof"])).unwrap();
        let e = restart("desk", &d, false, &mut r, &mut |_| {}).unwrap_err();
        assert!(e.contains("omarchyvr vr restart") && r.0.is_empty(), "{e}");
        let mut r = Rec(vec![], Some(1));
        let e = restart("vr", &d, false, &mut r, &mut |_| {}).unwrap_err();
        assert!(e.starts_with("stop failed") && r.0.len() == 1, "{e}");
    }
}
