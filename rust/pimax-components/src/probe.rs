//! Read-only probes for every component (plan section 2.2). Each one reads sysfs, /proc, a pid file, a log or a
//! file's age; the optional `deep` probes also send a status request to a control socket with a 1 s timeout
//! (passthrough `status`, voice `{"cmd":"ping"}`). Nothing is ever written, started, stopped or signalled.
use crate::model::{Id, Mode};
use crate::procs;
use crate::spec::{Spec, SPECS};
use crate::usb;
use crate::util::{age_s, read_trim, Roots};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

/// One raw observation (before debouncing and dependency logic).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Obs {
    /// Present / running (None: cannot tell, or not applicable).
    pub up: Option<bool>,
    /// Up, but something is off.
    pub degraded: Option<String>,
    pub detail: String,
    pub pid: Option<u32>,
    pub start_ticks: Option<u64>,
    /// Readiness (Monado: the gyro bias line).
    pub ready: Option<bool>,
    /// The process is in uninterruptible sleep (D).
    pub dstate: bool,
}

impl Obs {
    fn up(detail: impl Into<String>) -> Obs {
        Obs { up: Some(true), detail: detail.into(), ..Default::default() }
    }
    fn down(detail: impl Into<String>) -> Obs {
        Obs { up: Some(false), detail: detail.into(), ..Default::default() }
    }
    fn unknown(detail: impl Into<String>) -> Obs {
        Obs { up: None, detail: detail.into(), ..Default::default() }
    }
}

/// Probe options.
#[derive(Debug, Clone)]
pub struct Opts {
    /// Ask the control sockets too (status/ping, 1 s timeout).
    pub deep: bool,
    /// Connector name when no EDID matches (`$PIMAX_CONNECTOR`, default DP-1).
    pub connector: String,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { deep: false, connector: std::env::var("PIMAX_CONNECTOR").ok().filter(|v| !v.is_empty()).unwrap_or_else(|| "DP-1".into()) }
    }
}

/// The launcher's session files.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionFacts {
    /// `start-argv.txt`: (`desk`|`vr`, args).
    pub argv: Option<(String, Vec<String>)>,
    /// Desk if WayVR or passthrough is alive or the argv says desk; Vr if Monado runs alone.
    pub mode: Option<Mode>,
    /// The launcher's `user-stop` marker exists.
    pub user_stop: bool,
    /// A `stop` is running right now (`stop.lock` held).
    pub stopping: bool,
    /// A `start` is running right now (`start.lock` held).
    pub starting: bool,
    /// The launcher's watchdog is alive (`watchdog.pid`).
    pub watchdog: bool,
    /// `session.txt` `service=`: the monado-service binary.
    pub service: Option<String>,
}

/// Is a lock file held by someone (a non-blocking shared lock fails)? Read-only open; released at once.
pub fn lock_held(p: &Path) -> bool {
    use std::os::fd::AsRawFd;
    let Ok(f) = std::fs::File::open(p) else { return false };
    // SAFETY: flock on a valid fd.
    let r = unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) };
    if r == 0 {
        // SAFETY: as above.
        unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_UN) };
        false
    } else {
        std::io::Error::last_os_error().raw_os_error() == Some(libc::EWOULDBLOCK)
    }
}

/// `<session dir>/start-argv.txt`: (`desk`|`vr`, args), as the launcher records it.
pub fn session_facts_argv(session_dir: &Path) -> Option<(String, Vec<String>)> {
    let s = std::fs::read_to_string(session_dir.join("start-argv.txt")).ok()?;
    let mut l = s.lines();
    let kind = l.next()?.trim().to_string();
    (!kind.is_empty()).then(|| (kind, l.map(str::to_string).collect()))
}

pub fn session_facts(r: &Roots) -> SessionFacts {
    let d = &r.session_dir;
    let argv = session_facts_argv(d);
    let alive = |id: &str| crate::spec::spec(id).and_then(|s| s.pid).and_then(|p| procs::from_pid_file(r, &p)).is_some();
    let desk = alive("wayvr") || alive("passthrough");
    let mode = if desk {
        Some(Mode::Desk)
    } else if alive("monado") {
        match argv.as_ref().map(|a| a.0.as_str()) {
            Some("desk") => Some(Mode::Desk),
            _ => Some(Mode::Vr),
        }
    } else {
        argv.as_ref().and_then(|a| Mode::parse(&a.0))
    };
    let watchdog = procs::read_pid(&d.join("watchdog.pid")).and_then(|p| procs::info(&r.proc_, p)).is_some_and(|i| i.alive() && i.comm == "pimax");
    let service = std::fs::read_to_string(d.join("session.txt"))
        .ok()
        .and_then(|t| t.lines().find_map(|l| l.strip_prefix("service=").map(str::to_string)))
        .filter(|s| !s.is_empty());
    SessionFacts {
        argv,
        mode,
        user_stop: d.join("user-stop").exists(),
        stopping: lock_held(&d.join("stop.lock")),
        starting: lock_held(&d.join("start.lock")),
        watchdog,
        service,
    }
}

/// `cap_sys_nice` in the permitted set of a file's `security.capability` xattr.
pub fn has_cap_sys_nice(path: &Path) -> Option<bool> {
    use std::os::unix::ffi::OsStrExt;
    let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let name = c"security.capability";
    let mut buf = [0u8; 64];
    // SAFETY: valid C strings and buffer.
    let n = unsafe { libc::getxattr(c.as_ptr(), name.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if n < 0 {
        let e = std::io::Error::last_os_error().raw_os_error();
        return if e == Some(libc::ENODATA) { Some(false) } else { None };
    }
    if n < 8 {
        return Some(false);
    }
    let permitted = u32::from_le_bytes([buf[4], buf[5], buf[6], buf[7]]);
    // CAP_SYS_NICE = 23
    Some(permitted & (1 << 23) != 0)
}

/// Monado logged that head tracking is ready.
pub fn monado_ready(log: &Path) -> bool {
    use std::io::Read;
    let Ok(f) = std::fs::File::open(log) else { return false };
    let mut b = Vec::new();
    let _ = f.take(4 << 20).read_to_end(&mut b);
    let t = String::from_utf8_lossy(&b);
    t.contains("gyro bias done") || t.contains("tracking with the cached gyro bias")
}

/// Camera hands in Monado's log: Some(true) when Mercury came up, Some(false) when it was refused or failed, None
/// when the log says nothing (not asked, or still starting).
pub fn monado_hands(log: &Path) -> Option<bool> {
    use std::io::Read;
    let f = std::fs::File::open(log).ok()?;
    let mut b = Vec::new();
    let _ = f.take(4 << 20).read_to_end(&mut b);
    let t = String::from_utf8_lossy(&b);
    if t.contains("without camera hands") || t.contains("built without Mercury") {
        Some(false)
    } else if t.contains("hands: Mercury up") {
        Some(true)
    } else {
        None
    }
}

/// One request line to a unix socket, one reply line (timeout `t`).
pub fn socket_request(path: &Path, line: &str, t: Duration) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Write};
    let mut s = std::os::unix::net::UnixStream::connect(path).map_err(|e| e.to_string())?;
    s.set_read_timeout(Some(t)).ok();
    s.set_write_timeout(Some(t)).ok();
    writeln!(s, "{line}").map_err(|e| e.to_string())?;
    let mut r = String::new();
    BufReader::new(s).read_line(&mut r).map_err(|e| e.to_string())?;
    Ok(r.trim_end().to_string())
}

fn process(r: &Roots, s: &Spec) -> Obs {
    let Some(ps) = s.pid else { return Obs::unknown("no pid file") };
    match procs::from_pid_file(r, &ps) {
        Some(i) => Obs {
            up: Some(true),
            detail: format!("pid {}", i.pid),
            pid: Some(i.pid),
            start_ticks: Some(i.start_ticks),
            dstate: i.state == 'D',
            degraded: (i.state == 'D').then(|| "in D state (uninterruptible)".to_string()),
            ..Default::default()
        },
        None => {
            let p = procs::pid_path(r, &ps);
            Obs::down(if p.exists() { format!("{} names no live {}", ps.file, ps.comm) } else { "not running".into() })
        }
    }
}

fn keepalive(r: &Roots) -> Obs {
    // `pimax keepalived` (comm "pimax"), normally in pimax-keepalived.service
    for i in procs::by_comm(&r.proc_, "pimax") {
        let argv = procs::cmdline(&r.proc_, i.pid);
        if argv.get(1).map(String::as_str) == Some("keepalived") {
            let cg = std::fs::read_to_string(r.proc_.join(i.pid.to_string()).join("cgroup")).unwrap_or_default();
            let unit = if cg.contains("pimax-keepalived.service") { " (pimax-keepalived.service)" } else { "" };
            return Obs { pid: Some(i.pid), start_ticks: Some(i.start_ticks), ..Obs::up(format!("pid {}{unit}", i.pid)) };
        }
    }
    Obs::down("pimax keepalived is not running")
}

/// Probe every component. `gate` fills `gpu.gate` (the gate is latched by the caller, not probed here).
pub fn probe_all(r: &Roots, o: &Opts, gate: crate::model::GateLevel) -> BTreeMap<Id, Obs> {
    let mut m: BTreeMap<Id, Obs> = BTreeMap::new();
    let hid = usb::present(&r.sys, usb::VID, usb::PID_HID);
    m.insert("usb.hid", if hid { Obs::up("34a4:0044 on USB") } else { Obs::down("34a4:0044 not on USB") });
    let cam = usb::present(&r.sys, usb::VID, usb::PID_CAM);
    let node = usb::camera_node(&r.sys);
    m.insert(
        "usb.cam",
        match (cam, node) {
            (true, true) => Obs::up("34a4:0014 on USB, video node present"),
            (true, false) => Obs { degraded: Some("on USB but no video4linux node yet".into()), ..Obs::up("34a4:0014 on USB") },
            (false, _) => Obs::down(if usb::present(&r.sys, usb::HUB.0, usb::HUB.1) {
                "34a4:0014 not on USB (hub 05e3:0620 present)"
            } else {
                "34a4:0014 and its hub 05e3:0620 not on USB"
            }),
        },
    );
    let eye = usb::present(&r.sys, usb::VID, usb::PID_EYE);
    m.insert("usb.eye", if eye { Obs::up("34a4:0043 on USB") } else { Obs::down("34a4:0043 not on USB") });
    m.insert("keepalive", keepalive(r));
    m.insert(
        "display.dp1",
        match usb::headset_connector(&r.sys, &o.connector) {
            Some((n, true)) => Obs::up(format!("{n} connected")),
            Some((n, false)) => Obs::down(format!("{n} disconnected")),
            None => Obs::unknown(format!("no {} connector in sysfs", o.connector)),
        },
    );
    m.insert(
        "gpu.gate",
        match gate {
            crate::model::GateLevel::Ok => Obs::up("no GPU fault this boot"),
            crate::model::GateLevel::Caution => Obs { degraded: Some("WayVR Xid 31 this boot (warning only)".into()), ..Obs::up("caution") },
            crate::model::GateLevel::NeedsReboot => Obs::down("GPU fault this boot: reboot needed"),
        },
    );
    let facts = session_facts(r);
    let service = facts.service.clone();
    m.insert(
        "host.caps",
        match service.as_deref().map(|s| (s, has_cap_sys_nice(Path::new(s)))) {
            Some((_, Some(true))) => Obs::up("monado-service has cap_sys_nice"),
            Some((s, Some(false))) => Obs {
                degraded: Some(format!("{s} lacks cap_sys_nice (stutter); fix: pkexec setcap cap_sys_nice+ep {s}")),
                ..Obs::up("no cap_sys_nice")
            },
            Some((s, None)) => Obs::unknown(format!("{s}: cannot read capabilities")),
            None => Obs::unknown("no session.txt service= yet"),
        },
    );
    m.insert(
        "host.mhs",
        match read_trim(&r.sys.join("module/nvidia_modeset/parameters/merged_head_sync")).as_deref() {
            Some("1") | Some("Y") => Obs::up("merged_head_sync=1"),
            Some(v) => Obs { degraded: Some(format!("merged_head_sync={v} (needs 1)")), ..Obs::up("set") },
            None => Obs::unknown("merged_head_sync not exposed (nvidia_modeset not loaded or unpatched)"),
        },
    );
    let rt = r.home.join(".config/openxr/1/active_runtime.json");
    m.insert(
        "host.runtime-link",
        match std::fs::read_to_string(&rt) {
            Ok(t) if t.to_lowercase().contains("monado") => Obs::up("active_runtime.json -> Monado"),
            Ok(_) => Obs { degraded: Some("active_runtime.json does not point at Monado (the launcher relinks at start)".into()), ..Obs::up("other runtime") },
            Err(_) => Obs::down("no ~/.config/openxr/1/active_runtime.json"),
        },
    );
    for s in SPECS.iter().filter(|s| s.pid.is_some()) {
        m.insert(s.id, process(r, s));
    }
    // Monado: readiness, frames
    if let Some(mo) = m.get_mut("monado") {
        if mo.up == Some(true) {
            mo.ready = Some(monado_ready(&r.session_dir.join("monado.log")));
            if let Some(age) = age_s(&r.perf_stats()) {
                if age > 5.0 {
                    mo.detail.push_str(&format!(", perf stats {age:.0} s old"));
                }
                // Monado rewrites its stats about every second while it composes: a stale file after the
                // gyro calibration means frames are not advancing (the reconciler acts on it only after a resume)
                if age > 10.0 && mo.ready == Some(true) && (5.0..3600.0).contains(&age) && mo.degraded.is_none() {
                    mo.degraded = Some(format!("frames stalled: perf stats {age:.0} s old"));
                }
            }
            if mo.ready == Some(false) && mo.degraded.is_none() {
                mo.detail.push_str(", gyro calibration not done");
            }
        }
    }
    if let Some(b) = m.get_mut("broker") {
        if b.up == Some(true) && !r.runtime.join("pimax-cam-broker.sock").exists() {
            b.degraded = Some("no pimax-cam-broker.sock".into());
        }
    }
    if let Some(c) = m.get_mut("ctrltrack") {
        if c.up == Some(true) {
            if let Some(a) = age_s(&r.runtime.join("pimax-ctrltrack.pose")) {
                c.detail.push_str(&format!(", pose file {a:.0} s old"));
            }
        }
    }
    if o.deep {
        let t = Duration::from_secs(1);
        if let Some(p) = m.get_mut("passthrough") {
            if p.up == Some(true) {
                match socket_request(&r.runtime.join("pimax-passthrough.sock"), "status", t) {
                    Ok(rep) if rep.starts_with("ok") => p.detail.push_str(&format!(", {}", rep.trim_start_matches("ok").trim())),
                    Ok(rep) => p.degraded = Some(format!("control socket: {rep}")),
                    Err(e) => p.degraded = Some(format!("control socket: {e}")),
                }
            }
        }
        if let Some(v) = m.get_mut("voice.listen") {
            if v.up == Some(true) {
                if let Err(e) = socket_request(&r.runtime.join("pimax-voice.sock"), "{\"cmd\":\"ping\"}", t) {
                    v.degraded = Some(format!("socket: {e}"));
                }
            }
        }
    }
    // features and settings derive from what they need
    let up = |m: &BTreeMap<Id, Obs>, id: &str| m.get(id).and_then(|o| o.up) == Some(true);
    let eye_obs = if up(&m, "monado") && up(&m, "usb.eye") { Obs::up("Monado runs with the eye tracker on USB") } else { Obs::down("needs Monado and 34a4:0043") };
    m.insert("eye", eye_obs);
    let six = if up(&m, "monado") && up(&m, "broker") { Obs::up("Monado with the camera broker") } else { Obs::down("needs Monado and the broker") };
    m.insert("sixdof", six);
    let hands = if up(&m, "monado") && up(&m, "broker") {
        match monado_hands(&r.session_dir.join("monado.log")) {
            Some(true) => Obs::up("Mercury runs in Monado"),
            Some(false) => Obs::down("Monado runs without camera hands (see monado.log: models, build)"),
            None => Obs::down("not in monado.log (needs --hands and the hands build)"),
        }
    } else {
        Obs::down("needs Monado and the broker")
    };
    m.insert("hands", hands);
    let wayvr_up = up(&m, "wayvr");
    m.insert("eye.laser", if wayvr_up && up(&m, "eye") { Obs::up("applies through WayVR") } else { Obs::down("needs WayVR and eye tracking") });
    m.insert("preset", if wayvr_up { Obs::up("applies through WayVR") } else { Obs::down("needs WayVR") });
    m.insert("anchor.relocalize", Obs::unknown("one-shot job"));
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::procs::fake_proc;
    use crate::util::{put, TempDir};

    /// A fake host with a full desk session.
    pub fn fake_desk(r: &Roots) {
        for (port, pid) in [("2-4", "0044"), ("2-4.1", "0014"), ("2-4.2", "0043")] {
            put(&r.sys, &format!("bus/usb/devices/{port}/idVendor"), "34a4\n");
            put(&r.sys, &format!("bus/usb/devices/{port}/idProduct"), &format!("{pid}\n"));
        }
        put(&r.sys, "class/video4linux/video0/idVendor", "34a4\n");
        put(&r.sys, "class/video4linux/video0/idProduct", "0014\n");
        std::fs::create_dir_all(r.sys.join("class/video4linux/video0/device")).unwrap();
        put(&r.sys, "class/drm/card1-DP-1/status", "connected\n");
        put(&r.sys, "module/nvidia_modeset/parameters/merged_head_sync", "1\n");
        let procs = [
            (100, "monado.pid", "monado-service"),
            (101, "broker.pid", "pimax-cam"),
            (102, "wayvr.pid", "wayvr"),
            (103, "passthrough.pid", "pimax-passthrou"),
            (104, "ctrltrack.pid", "pimax-ctrltrack"),
            (105, "perf.pid", "pimax-perf"),
        ];
        for (pid, file, comm) in procs {
            fake_proc(&r.proc_, pid, comm, 'S', &[comm]);
            put(&r.session_dir, file, &format!("{pid}\n"));
        }
        fake_proc(&r.proc_, 90, "pimax", 'S', &["pimax", "keepalived"]);
        put(&r.session_dir, "monado.log", " INFO [bias_finish] gyro bias done: 0 0 0 dps\n");
        put(&r.runtime, "pimax-cam-broker.sock", "");
        put(&r.home, ".config/openxr/1/active_runtime.json", "{\"runtime\":{\"name\":\"Monado\"}}");
    }

    #[test]
    fn a_full_fake_desk_session_probes_up() {
        let t = TempDir::new("probe");
        let r = Roots::under(t.path());
        fake_desk(&r);
        let m = probe_all(&r, &Opts::default(), crate::model::GateLevel::Ok);
        for id in ["usb.hid", "usb.cam", "usb.eye", "keepalive", "display.dp1", "monado", "broker", "wayvr", "passthrough", "ctrltrack", "perf", "eye", "sixdof", "preset", "host.mhs", "host.runtime-link", "gpu.gate"] {
            assert_eq!(m[id].up, Some(true), "{id}: {:?}", m[id]);
            assert_eq!(m[id].degraded, None, "{id}");
        }
        assert_eq!(m["monado"].ready, Some(true));
        assert_eq!(m["monado"].pid, Some(100));
        assert_eq!(m["voice.listen"].up, Some(false));
        assert_eq!(m["host.caps"].up, None, "no session.txt");
        let f = session_facts(&r);
        assert_eq!(f.mode, Some(Mode::Desk));
        assert!(!f.user_stop && !f.stopping && !f.watchdog);
        // unplug: the camera goes, its hub stays
        std::fs::remove_dir_all(r.sys.join("bus/usb/devices/2-4.1")).unwrap();
        let m = probe_all(&r, &Opts::default(), crate::model::GateLevel::Ok);
        assert_eq!(m["usb.cam"].up, Some(false));
        assert!(m["usb.cam"].detail.contains("hub 05e3:0620 not"), "{}", m["usb.cam"].detail);
    }

    #[test]
    fn hands_follow_monados_log() {
        let t = TempDir::new("probe-hands");
        let r = Roots::under(t.path());
        fake_desk(&r);
        let m = probe_all(&r, &Opts::default(), crate::model::GateLevel::Ok);
        assert_eq!(m["hands"].up, Some(false), "no hands line");
        put(&r.session_dir, "monado.log", " INFO [bias_finish] gyro bias done\n INFO [tracker_thread] hands: Mercury up in 75 ms\n");
        let m = probe_all(&r, &Opts::default(), crate::model::GateLevel::Ok);
        assert_eq!(m["hands"].up, Some(true), "{:?}", m["hands"]);
        put(&r.session_dir, "monado.log", "ERROR [pimax_ht_create] hands: Mercury models not usable (x); VR continues without camera hands\n");
        assert_eq!(monado_hands(&r.session_dir.join("monado.log")), Some(false));
    }

    #[test]
    fn lock_detection() {
        use std::os::fd::AsRawFd;
        let t = TempDir::new("lock");
        let p = t.path().join("stop.lock");
        assert!(!lock_held(&p));
        std::fs::write(&p, "").unwrap();
        assert!(!lock_held(&p));
        let f = std::fs::File::open(&p).unwrap();
        // SAFETY: valid fd.
        assert_eq!(unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX) }, 0);
        assert!(lock_held(&p));
        drop(f);
        assert!(!lock_held(&p));
    }
}
