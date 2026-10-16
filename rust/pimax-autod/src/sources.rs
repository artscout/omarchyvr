//! Real signal sources. Everything is read-only and needs no root:
//! * USB: `/sys/bus/usb/devices/*/idVendor|idProduct` = 34a4:0044 (same check as `pimax vr` preflight).
//! * Processes: `/proc/*/comm` (+ `cmdline` for Steam's `SteamLaunch AppId=`), the launcher's pid files.
//! * Monado's perf stats file (`$XDG_RUNTIME_DIR/pimax-perf.stats`, written when the launcher sets PIMAX_PERF_FILE).
//! * IMU + controller reports: a short read-only burst on hidraw IF2 (IMU) and IF1 (controllers). Opening hidraw
//!   read-only sends nothing; the kernel gives every open file its own copy of the input reports, so Monado /
//!   pimax-keepalived are not disturbed. Never writes.
use crate::machine::{Kind, Signals};
use crate::motion::{Classifier, Motion, Window};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Paths (overridable for tests).
#[derive(Debug, Clone)]
pub struct Paths {
    pub sys: PathBuf,
    pub proc_: PathBuf,
    pub session_dir: PathBuf,
    pub perf_file: PathBuf,
}

impl Paths {
    pub fn from_env() -> Self {
        let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| "/tmp".into());
        Self {
            sys: std::env::var_os("PIMAX_SYSFS_ROOT").map(PathBuf::from).unwrap_or_else(|| "/sys".into()),
            proc_: "/proc".into(),
            session_dir: std::env::var_os("PIMAX_VR_SESSION_DIR")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| "/tmp/pimax-vr-session".into()),
            perf_file: std::env::var_os("PIMAX_PERF_FILE")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| rt.join("pimax-perf.stats")),
        }
    }
}

/// Headset (34a4:0044) on USB.
pub fn usb_present(sys: &Path) -> bool {
    let Ok(rd) = std::fs::read_dir(sys.join("bus/usb/devices")) else { return false };
    rd.flatten().any(|e| {
        let rd = |f: &str| std::fs::read_to_string(e.path().join(f)).unwrap_or_default();
        rd("idVendor").trim() == "34a4" && rd("idProduct").trim() == "0044"
    })
}

/// One process.
#[derive(Debug, Clone, PartialEq)]
pub struct Proc {
    pub pid: u32,
    pub comm: String,
    pub cmdline: String,
}

/// All processes of this user (comm + cmdline with NULs as spaces).
pub fn list_procs(proc_: &Path) -> Vec<Proc> {
    let Ok(rd) = std::fs::read_dir(proc_) else { return vec![] };
    let mut v = Vec::new();
    for e in rd.flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        let Ok(comm) = std::fs::read_to_string(e.path().join("comm")) else { continue };
        let cmd = std::fs::read(e.path().join("cmdline")).unwrap_or_default();
        let cmdline = String::from_utf8_lossy(&cmd).replace('\0', " ");
        v.push(Proc { pid, comm: comm.trim().to_string(), cmdline });
    }
    v
}

/// pid from `<session_dir>/<name>.pid` if that process is alive with the expected comm.
fn pidfile_alive(procs: &[Proc], session_dir: &Path, name: &str, comm: &str) -> bool {
    let Ok(t) = std::fs::read_to_string(session_dir.join(format!("{name}.pid"))) else { return false };
    let Ok(pid) = t.trim().parse::<u32>() else { return false };
    procs.iter().any(|p| p.pid == pid && p.comm == comm)
}

/// A non-overlay, active main app in the perf stats (fresh only: `age` < 2.5 s).
pub fn perf_main_app(text: &str) -> bool {
    text.lines()
        .filter(|l| l.starts_with("app "))
        .any(|l| l.contains("overlay=0") && l.contains("active=1"))
}

/// Session kind + game flag from the process list.
pub fn classify_session(procs: &[Proc], session_dir: &Path, perf_main: bool) -> (Option<Kind>, bool) {
    let monado = procs.iter().any(|p| p.comm == "monado-service");
    // comm is cut at 15 chars: pimax-passthrough -> pimax-passthrou
    let desk = pidfile_alive(procs, session_dir, "passthrough", "pimax-passthrou")
        || pidfile_alive(procs, session_dir, "wayvr", "wayvr");
    let steam_game = procs.iter().any(|p| p.cmdline.contains("SteamLaunch") && p.cmdline.contains("AppId="));
    let steamvr = procs.iter().any(|p| p.comm == "vrserver" || p.comm == "vrcompositor");
    let game = steam_game || steamvr || (monado && !desk && perf_main);
    let session = match (monado, desk) {
        (true, true) => Some(Kind::Desk),
        (true, false) => Some(Kind::Game),
        _ => None,
    };
    (session, game)
}

/// Read-only hidraw burst: IMU motion class (IF2) and controller input (IF1).
pub struct HidBurst {
    cls: Classifier,
    last_ctrl: [Option<(u32, u8, bool, bool)>; 2],
    pub last_std: Option<f64>,
}

impl HidBurst {
    pub fn new(still_dps: f64, worn_dps: f64) -> Self {
        Self { cls: Classifier::new(still_dps, worn_dps), last_ctrl: [None, None], last_std: None }
    }

    /// Controller state worth reacting to: buttons, touch a/b/stick (not t4), trigger / grip pressed.
    fn ctrl_key(r: &pimax_proto::ControllerReport) -> (u32, u8, bool, bool) {
        (r.buttons, r.touch & 0x0f, r.trigger > 40, r.grip > 40)
    }

    /// Feed controller reports; true if any controller changed state since the last call.
    pub fn controller_input(&mut self, reports: &[pimax_proto::ControllerReport]) -> bool {
        let mut changed = false;
        for r in reports {
            let i = usize::from(r.slot == pimax_proto::SLOT_RIGHT);
            let k = Self::ctrl_key(r);
            if self.last_ctrl[i].is_some_and(|o| o != k) {
                changed = true;
            }
            self.last_ctrl[i] = Some(k);
        }
        changed
    }

    /// Classify one window's std (exposed for the replay path).
    pub fn classify(&mut self, std: Option<f64>) -> Motion {
        self.last_std = std;
        self.cls.classify(std)
    }

    /// Read IF2 (and IF1) for `dur`. Unknown motion when the headset is not there or silent.
    pub fn sample(&mut self, dur: Duration) -> (Motion, bool) {
        let ifs = pimax::discovery::find_interfaces(&pimax::discovery::Roots::from_env());
        let open = |i: usize| ifs[i].as_ref().and_then(|p| pimax::hid::open(p, false).ok());
        let Some(imu) = open(2) else { return (self.classify(None), false) };
        let ctrl = open(1);
        let mut w = Window::default();
        let mut reps = Vec::new();
        let t0 = Instant::now();
        let mut buf: pimax_proto::Report = [0; 64];
        while t0.elapsed() < dur {
            let files: Vec<&std::fs::File> = std::iter::once(&imu).chain(ctrl.as_ref()).collect();
            match pimax::sys::wait_readable(&files, Some(Duration::from_millis(100))) {
                Ok(r) if r.iter().any(|x| *x) => {}
                Ok(_) => continue,
                Err(_) => break,
            }
            while let Ok(Some(_)) = pimax::hid::read_report(&imu, &mut buf) {
                if let Some(s) = pimax_proto::decode_imu(&buf) {
                    for x in s {
                        w.push(x.gyro_dps.map(f64::from));
                    }
                }
            }
            if let Some(c) = &ctrl {
                while let Ok(Some(_)) = pimax::hid::read_report(c, &mut buf) {
                    if let Some(r) = pimax_proto::decode_controller(&buf) {
                        reps.push(r);
                    }
                }
            }
        }
        let m = self.classify(w.std_dps());
        (m, self.controller_input(&reps))
    }
}

/// The real collector.
pub struct Collector {
    pub paths: Paths,
    pub hid: Option<HidBurst>,
    pub burst: Duration,
}

impl Collector {
    pub fn new(paths: Paths, cfg: &crate::config::Config) -> Self {
        Self {
            paths,
            hid: cfg.imu.then(|| HidBurst::new(cfg.still_dps, cfg.worn_dps)),
            burst: Duration::from_millis(400),
        }
    }

    pub fn collect(&mut self) -> Signals {
        let plugged = usb_present(&self.paths.sys);
        let procs = list_procs(&self.paths.proc_);
        let fresh = std::fs::metadata(&self.paths.perf_file)
            .and_then(|m| m.modified())
            .ok()
            .and_then(|t| t.elapsed().ok())
            .is_some_and(|a| a < Duration::from_millis(2500));
        let perf_main = fresh && std::fs::read_to_string(&self.paths.perf_file).is_ok_and(|t| perf_main_app(&t));
        let (session, game) = classify_session(&procs, &self.paths.session_dir, perf_main);
        let (motion, input) = match (&mut self.hid, plugged) {
            (Some(h), true) => h.sample(self.burst),
            _ => (Motion::Unknown, false),
        };
        Signals { plugged, motion, visor: None, session, game, input }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(pid: u32, comm: &str, cmd: &str) -> Proc {
        Proc { pid, comm: comm.into(), cmdline: cmd.into() }
    }

    #[test]
    fn usb_scan_finds_the_headset_only() {
        let d = std::env::temp_dir().join(format!("autod-usb-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let dev = d.join("bus/usb/devices");
        for (n, v, pr) in [("1-1", "046d", "c52b"), ("3-2", "34a4", "0014")] {
            std::fs::create_dir_all(dev.join(n)).unwrap();
            std::fs::write(dev.join(n).join("idVendor"), format!("{v}\n")).unwrap();
            std::fs::write(dev.join(n).join("idProduct"), format!("{pr}\n")).unwrap();
        }
        assert!(!usb_present(&d), "camera alone is not the headset");
        std::fs::create_dir_all(dev.join("3-1")).unwrap();
        std::fs::write(dev.join("3-1/idVendor"), "34a4\n").unwrap();
        std::fs::write(dev.join("3-1/idProduct"), "0044\n").unwrap();
        assert!(usb_present(&d));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn sessions_and_games() {
        let d = std::env::temp_dir().join(format!("autod-sess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let none: Vec<Proc> = vec![p(1, "systemd", "/sbin/init")];
        assert_eq!(classify_session(&none, &d, false), (None, false));
        let monado = vec![p(10, "monado-service", "monado-service")];
        assert_eq!(classify_session(&monado, &d, false), (Some(Kind::Game), false));
        assert_eq!(classify_session(&monado, &d, true), (Some(Kind::Game), true), "main app = game");
        // desk: the passthrough pid file points at a live passthrough
        std::fs::write(d.join("passthrough.pid"), "11\n").unwrap();
        let mut desk = monado.clone();
        desk.push(p(11, "pimax-passthrou", "pimax-passthrough run"));
        assert_eq!(classify_session(&desk, &d, true), (Some(Kind::Desk), false), "passthrough is the main app");
        // a stale pid file (pid reused by something else) is not desk
        let stale = vec![p(10, "monado-service", ""), p(11, "bash", "bash")];
        assert_eq!(classify_session(&stale, &d, false).0, Some(Kind::Game));
        // a Steam game
        let mut g = desk.clone();
        g.push(p(20, "reaper", "/x/reaper SteamLaunch AppId=3189340 -- game.exe"));
        assert_eq!(classify_session(&g, &d, false), (Some(Kind::Desk), true));
        // SteamVR without Monado
        let svr = vec![p(30, "vrserver", "vrserver")];
        assert_eq!(classify_session(&svr, &d, false), (None, true));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn perf_stats_main_app() {
        let t = "v=1\nt_ms=1\ncomp_fps=72\napp overlay=1 active=1 fps=72 frames=1 cpu_ms=1 gpu_ms=1\n";
        assert!(!perf_main_app(t));
        assert!(perf_main_app(&format!("{t}app overlay=0 active=1 fps=71.9 frames=900 cpu_ms=3.1 gpu_ms=5.2\n")));
        assert!(!perf_main_app("app overlay=0 active=0 fps=0\n"));
    }

    #[test]
    fn controller_changes_are_input() {
        let mut h = HidBurst::new(0.2, 0.3);
        let r = |buttons: u32, trigger: u8| pimax_proto::ControllerReport {
            slot: 2,
            ts_us: 0,
            stick_x: 0,
            stick_y: 0,
            trigger,
            grip: 0,
            battery: 50,
            charging: false,
            buttons,
            touch: 0x10,
            raw_accel: [0; 3],
            raw_gyro: [0; 3],
            raw: [0; 64],
        };
        assert!(!h.controller_input(&[r(0, 0)]), "first report is the baseline");
        assert!(!h.controller_input(&[r(0, 10)]), "trigger noise");
        assert!(h.controller_input(&[r(0, 200)]));
        assert!(h.controller_input(&[r(pimax_proto::BTN_A, 200)]));
        assert!(!h.controller_input(&[]));
    }
}
