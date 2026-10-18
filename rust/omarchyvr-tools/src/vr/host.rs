//! The real machine behind `preflight::Host`, process control and notifications.
//!
//! All paths can be redirected for tests: `PIMAX_SYSFS_ROOT` (replaces `/sys`, as in `pimax::discovery`) and
//! `PIMAX_PROC_ROOT` (replaces `/proc`). The external tools (`systemctl`, `journalctl`, `notify-send`) are looked up
//! through `PATH`.
use super::edid::{connector_of_entry, is_headset};
use super::preflight::{caps_grant_sys_nice, is_gpu_error_line, Host};
use pimax::discovery;
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

// Clock, Signal and the process table moved to `omarchyvr-components` (P3): the launcher and the components share them.
pub use omarchyvr_components::components::proc::{Clock, Procs, RealClock, RealProcs, Signal};

/// Desktop notifications.
pub trait Notifier {
    /// Show a notification. `critical` ones stay on screen.
    fn notify(&self, critical: bool, summary: &str, body: &str);
}

/// Uses `notify-send`; failures are ignored (no notification daemon is not an error).
pub struct NotifySend {
    enabled: bool,
    program: String,
}

impl NotifySend {
    /// `enabled = false` turns every call into a no-op.
    pub fn new(enabled: bool) -> Self {
        Self::with_program(enabled, "notify-send")
    }

    /// Use another program than `notify-send` (tests).
    pub fn with_program(enabled: bool, program: &str) -> Self {
        Self {
            enabled,
            program: program.to_string(),
        }
    }
}

impl Notifier for NotifySend {
    fn notify(&self, critical: bool, summary: &str, body: &str) {
        if !self.enabled {
            return;
        }
        let child = Command::new(&self.program)
            .args([
                "-a",
                "Pimax VR",
                "-u",
                if critical { "critical" } else { "normal" },
            ])
            // mako/dunst replace the previous notification with the same key instead of stacking them
            .args(["-h", "string:x-canonical-private-synchronous:pimax-vr"])
            .args(["-t", if critical { "0" } else { "6000" }])
            .arg(summary)
            .arg(body)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
        let Ok(mut child) = child else { return };
        // a notification daemon that does not answer must not hold up the VR start: give it two seconds
        let t0 = Instant::now();
        while t0.elapsed() < NOTIFY_TIMEOUT {
            if matches!(child.try_wait(), Ok(Some(_))) {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = child.kill();
        let _ = child.wait();
    }
}

/// How long `notify-send` may take before it is abandoned.
const NOTIFY_TIMEOUT: Duration = Duration::from_secs(2);

/// The machine this process runs on.
pub struct RealHost {
    /// Replaces `/sys`.
    pub sysfs: PathBuf,
    /// Replaces `/proc`.
    pub proc_root: PathBuf,
    /// Uid whose processes count as "ours".
    pub uid: u32,
    /// `comm` of the service the preflight looks for.
    pub service_comm: String,
    /// Program that prints the kernel log (tests use a fake).
    pub journalctl: String,
    /// Where hang evidence is written (the session directory of `vr start`).
    pub session_dir: PathBuf,
}

impl RealHost {
    /// Honour `PIMAX_SYSFS_ROOT` and `PIMAX_PROC_ROOT`.
    pub fn from_env() -> Self {
        // SAFETY: getuid has no arguments and cannot fail.
        let uid = unsafe { libc::getuid() };
        Self {
            sysfs: std::env::var_os("PIMAX_SYSFS_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/sys".into()),
            proc_root: std::env::var_os("PIMAX_PROC_ROOT")
                .map(PathBuf::from)
                .unwrap_or_else(|| "/proc".into()),
            uid,
            service_comm: super::config::SERVICE_COMM.to_string(),
            journalctl: "journalctl".to_string(),
            session_dir: PathBuf::from(super::config::SESSION_DIR),
        }
    }

    /// Read a short sysfs/procfs file, trimmed.
    fn read_trim(p: &Path) -> Option<String> {
        std::fs::read_to_string(p)
            .ok()
            .map(|s| s.trim().to_string())
    }
}

impl Host for RealHost {
    fn usb_headset_present(&self) -> bool {
        let Ok(rd) = std::fs::read_dir(self.sysfs.join("bus/usb/devices")) else {
            return false;
        };
        rd.flatten().any(|e| {
            let p = e.path();
            Self::read_trim(&p.join("idVendor")).as_deref() == Some("34a4")
                && Self::read_trim(&p.join("idProduct"))
                    .and_then(|s| u16::from_str_radix(&s, 16).ok())
                    .is_some_and(|pid| pimax_proto::model::by_pid(pid).is_some())
        })
    }

    fn headset_label(&self) -> String {
        match pimax::discovery::find_headset(&pimax::discovery::Roots::from_env()) {
            Some((m, _)) if m.id != pimax_proto::DREAM_AIR.id => format!("34a4:{:04x} ({})", m.pid, m.name),
            _ => "34a4:0044".into(),
        }
    }

    fn camera_ok(&self) -> Option<bool> {
        super::reset::camera_ok_in(&self.sysfs)
    }

    fn hidraw_interfaces(&self) -> usize {
        let roots = discovery::Roots::from_env();
        discovery::find_interfaces(&roots)
            .iter()
            .filter(|p| p.is_some())
            .count()
    }

    fn keepalive_state(&self) -> String {
        match Command::new("systemctl")
            .args(["--user", "is-active", "omarchyvr-keepalived"])
            .stdin(Stdio::null())
            .stderr(Stdio::null())
            .output()
        {
            Ok(o) => {
                let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                if s.is_empty() {
                    "unknown".into()
                } else {
                    s
                }
            }
            Err(_) => "unknown".into(),
        }
    }

    fn connector_status(&self, connector: &str) -> Option<String> {
        // entries are named card<N>-<connector>, for example card1-DP-1
        let mut names: Vec<String> = std::fs::read_dir(self.sysfs.join("class/drm"))
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| {
                n.strip_prefix("card")
                    .and_then(|r| r.split_once('-'))
                    .is_some_and(|(idx, name)| {
                        !idx.is_empty()
                            && idx.bytes().all(|b| b.is_ascii_digit())
                            && name == connector
                    })
            })
            .collect();
        names.sort();
        // two GPUs can both have a DP-1: the headset is on whichever reports "connected"
        let statuses: Vec<String> = names
            .iter()
            .filter_map(|n| Self::read_trim(&self.sysfs.join("class/drm").join(n).join("status")))
            .collect();
        statuses
            .iter()
            .find(|s| s.as_str() == "connected")
            .or_else(|| statuses.first())
            .cloned()
    }

    fn kernel_errors(&self) -> Result<Vec<String>, String> {
        // coreutils `timeout` keeps a stuck journalctl from hanging the launcher (a desktop entry has no terminal)
        let timeout = ["/usr/bin/timeout", "/bin/timeout"]
            .into_iter()
            .find(|p| Path::new(p).exists());
        let args = ["-k", "-b", "--no-pager", "-q"];
        let mut cmd = match timeout {
            Some(t) => {
                let mut c = Command::new(t);
                c.arg("20").arg(&self.journalctl).args(args);
                c
            }
            None => {
                let mut c = Command::new(&self.journalctl);
                c.args(args);
                c
            }
        };
        let o = cmd
            .stdin(Stdio::null())
            .stderr(Stdio::piped())
            .output()
            .map_err(|e| format!("cannot run journalctl: {e}"))?;
        if o.status.code() == Some(124) && timeout.is_some() {
            return Err("journalctl did not answer within 20 s".into());
        }
        if !o.status.success() {
            return Err(format!(
                "journalctl exited with {}: {}",
                o.status,
                String::from_utf8_lossy(&o.stderr).trim()
            ));
        }
        // a real boot always has kernel lines; journalctl -q prints nothing and exits 0 when the journal is not readable
        if o.stdout.iter().all(u8::is_ascii_whitespace) {
            return Err(
                "journalctl returned no kernel lines (is your user in the systemd-journal or adm group?)"
                    .into(),
            );
        }
        Ok(String::from_utf8_lossy(&o.stdout)
            .lines()
            .filter(|l| is_gpu_error_line(l))
            .map(str::to_string)
            .collect())
    }

    fn service_pids(&self) -> Vec<u32> {
        Procs::find_by_comm(self, &self.service_comm)
    }

    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn service_has_sys_nice(&self, service: &Path) -> Option<bool> {
        if !service.exists() {
            return None;
        }
        Some(read_xattr(service, "security.capability").is_some_and(|v| caps_grant_sys_nice(&v)))
    }

    fn headset_connector(&self) -> Option<String> {
        // read-only: /sys/class/drm/card<N>-<connector>/edid (empty while nothing is connected)
        let drm = self.sysfs.join("class/drm");
        let mut names: Vec<String> = std::fs::read_dir(&drm)
            .ok()?
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .filter(|n| connector_of_entry(n).is_some())
            .collect();
        names.sort();
        names.into_iter().find_map(|n| {
            let edid = std::fs::read(drm.join(&n).join("edid")).ok()?;
            is_headset(&edid).then(|| connector_of_entry(&n).unwrap_or_default().to_string())
        })
    }
}

/// Value of an extended attribute (follows symlinks), `None` if absent or unreadable.
fn read_xattr(path: &Path, name: &str) -> Option<Vec<u8>> {
    use std::os::unix::ffi::OsStrExt;
    let p = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
    let n = std::ffi::CString::new(name).ok()?;
    let mut buf = vec![0u8; 256];
    // SAFETY: both strings are NUL-terminated and live through the call; buf is writable for buf.len() bytes.
    let r = unsafe { libc::getxattr(p.as_ptr(), n.as_ptr(), buf.as_mut_ptr().cast(), buf.len()) };
    if r < 0 {
        return None;
    }
    buf.truncate(r as usize);
    Some(buf)
}

impl RealHost {
    /// The shared process-table reader on the same `/proc` root and uid.
    fn procs(&self) -> RealProcs {
        RealProcs { proc_root: self.proc_root.clone(), uid: self.uid }
    }
}

impl Procs for RealHost {
    fn is_alive(&self, pid: u32) -> bool {
        self.procs().is_alive(pid)
    }

    fn comm(&self, pid: u32) -> Option<String> {
        self.procs().comm(pid)
    }

    fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
        self.procs().signal(pid, sig)
    }

    fn find_by_comm(&self, comm: &str) -> Vec<u32> {
        self.procs().find_by_comm(comm)
    }

    fn environ(&self, pid: u32) -> Option<Vec<(String, String)>> {
        self.procs().environ(pid)
    }

    fn exe(&self, pid: u32) -> Option<String> {
        self.procs().exe(pid)
    }

    fn catches_sigint(&self, pid: u32) -> Option<bool> {
        self.procs().catches_sigint(pid)
    }

    fn capture_hang(&self, pid: u32) -> Option<String> {
        let c = super::hang::HangCapture {
            session_dir: self.session_dir.clone(),
            proc_root: self.proc_root.clone(),
            eu_stack: PathBuf::from("/usr/bin/eu-stack"),
            gdb: "gdb".into(),
        };
        let paths = c.capture(pid, super::hang::unix_now(), &super::hang::run_with_timeout);
        super::hang::describe(&paths)
    }

    fn age_secs(&self, pid: u32) -> Option<f64> {
        self.procs().age_secs(pid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn tmp(name: &str) -> PathBuf {
        crate::vr::test_dir(&format!("host-{name}"))
    }

    fn host(root: &Path) -> RealHost {
        RealHost {
            sysfs: root.join("sys"),
            proc_root: root.join("proc"),
            // SAFETY: getuid cannot fail.
            uid: unsafe { libc::getuid() },
            service_comm: "monado-service".into(),
            journalctl: "journalctl".into(),
            session_dir: root.join("sess"),
        }
    }

    #[test]
    fn usb_headset_is_found_by_vendor_and_product() {
        let d = tmp("usb");
        let h = host(&d);
        assert!(!h.usb_headset_present(), "no sysfs at all");
        for (dev, vid, pid) in [("1-2", "2104", "0561"), ("1-3", "34a4", "0043")] {
            let p = d.join("sys/bus/usb/devices").join(dev);
            fs::create_dir_all(&p).unwrap();
            fs::write(p.join("idVendor"), format!("{vid}\n")).unwrap();
            fs::write(p.join("idProduct"), format!("{pid}\n")).unwrap();
        }
        assert!(
            !h.usb_headset_present(),
            "bridge 34a4:0043 is not the headset"
        );
        let p = d.join("sys/bus/usb/devices/1-4");
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("idVendor"), "34a4\n").unwrap();
        fs::write(p.join("idProduct"), "0044\n").unwrap();
        assert!(h.usb_headset_present());
    }

    #[test]
    fn headset_connector_is_found_by_edid_not_by_name() {
        let d = tmp("edid");
        let h = host(&d);
        assert_eq!(h.headset_connector(), None);
        let drm = d.join("sys/class/drm");
        let mut dell = crate::vr::edid::tests::headset_edid();
        dell[8] = 0x10;
        dell[9] = 0xac;
        for (n, edid) in [
            ("card1-DP-1", dell.clone()),
            ("card1-DP-2", vec![]), // nothing connected: empty file
            ("card1-DP-3", crate::vr::edid::tests::headset_edid()),
        ] {
            fs::create_dir_all(drm.join(n)).unwrap();
            fs::write(drm.join(n).join("edid"), edid).unwrap();
        }
        fs::create_dir_all(drm.join("card1")).unwrap();
        assert_eq!(h.headset_connector().as_deref(), Some("DP-3"));
        fs::remove_dir_all(drm.join("card1-DP-3")).unwrap();
        assert_eq!(h.headset_connector(), None);
    }

    #[test]
    fn xattr_of_a_plain_file_is_absent() {
        let d = tmp("xattr");
        let f = d.join("svc");
        fs::write(&f, "x").unwrap();
        let h = host(&d);
        assert_eq!(h.service_has_sys_nice(&f), Some(false));
        assert_eq!(h.service_has_sys_nice(&d.join("nope")), None);
    }

    #[test]
    fn connector_status_matches_any_card_index() {
        let d = tmp("drm");
        let h = host(&d);
        assert_eq!(h.connector_status("DP-1"), None);
        let drm = d.join("sys/class/drm");
        for (n, s) in [
            ("card0-HDMI-A-1", "connected"),
            ("card1-DP-1", "connected\n"),
            ("card1-DP-2", "disconnected\n"),
        ] {
            fs::create_dir_all(drm.join(n)).unwrap();
            fs::write(drm.join(n).join("status"), s).unwrap();
        }
        fs::create_dir_all(drm.join("card1")).unwrap();
        assert_eq!(h.connector_status("DP-1").as_deref(), Some("connected"));
        assert_eq!(h.connector_status("DP-2").as_deref(), Some("disconnected"));
        assert_eq!(h.connector_status("DP-3"), None);
        // the name after card<N>- must match exactly: "HDMI-A-1" is not "A-1", "eDP-1" is not "DP-1"
        assert_eq!(h.connector_status("A-1"), None);
        fs::create_dir_all(drm.join("card1-eDP-1")).unwrap();
        fs::write(drm.join("card1-eDP-1/status"), "disconnected\n").unwrap();
        assert_eq!(h.connector_status("DP-1").as_deref(), Some("connected"));
        assert_eq!(h.connector_status("eDP-1").as_deref(), Some("disconnected"));
    }

    fn fake_proc(root: &Path, pid: u32, comm: &str, state: char, start_ticks: u64) {
        let p = root.join("proc").join(pid.to_string());
        fs::create_dir_all(&p).unwrap();
        fs::write(p.join("comm"), format!("{comm}\n")).unwrap();
        // pid (comm) state ppid pgrp session tty tpgid flags minflt cminflt majflt cmajflt utime stime cutime cstime
        // priority nice threads itrealvalue starttime ...
        fs::write(
            p.join("stat"),
            format!(
                "{pid} ({comm}) {state} 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 0 0\n"
            ),
        )
        .unwrap();
        fs::write(
            p.join("environ"),
            b"A=1\0PIMAX_FPS=90\0XRT_NO_STDIN=1\0junk\0",
        )
        .unwrap();
    }

    #[test]
    fn process_scan_by_comm_skips_zombies_and_other_names() {
        let d = tmp("proc");
        let h = host(&d);
        fake_proc(&d, 100, "monado-service", 'S', 1000);
        fake_proc(&d, 101, "monado-service", 'Z', 1000);
        fake_proc(&d, 102, "omarchyvr", 'S', 1000);
        fake_proc(&d, 103, "monado-service", 'R', 1000);
        fs::create_dir_all(d.join("proc/self")).unwrap(); // non numeric entries are ignored
        assert_eq!(Procs::find_by_comm(&h, "monado-service"), vec![100, 103]);
        assert_eq!(Host::service_pids(&h), vec![100, 103]);
        assert!(h.is_alive(100) && !h.is_alive(101) && !h.is_alive(999));
        assert_eq!(h.comm(102).as_deref(), Some("omarchyvr"));
        assert_eq!(h.comm(999), None);
    }

    #[test]
    fn comm_with_parentheses_and_spaces_does_not_confuse_stat_parsing() {
        let d = tmp("proc-odd");
        let h = host(&d);
        let p = d.join("proc/55");
        fs::create_dir_all(&p).unwrap();
        // comm = "we ) ird" (spaces and a closing parenthesis), state S
        fs::write(
            p.join("stat"),
            "55 (we ) ird) S 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 500 0\n",
        )
        .unwrap();
        assert!(h.is_alive(55));
        // comm = "a (b", state Z
        fs::write(p.join("stat"), "55 (a (b) Z 1 1 1\n").unwrap();
        assert!(!h.is_alive(55));
    }

    #[test]
    fn environ_and_age_are_read_from_proc() {
        let d = tmp("proc-env");
        let h = host(&d);
        fake_proc(&d, 7, "monado-service", 'S', 15_000);
        let env = h.environ(7).unwrap();
        assert!(env.contains(&("PIMAX_FPS".to_string(), "90".to_string())));
        assert!(env.iter().all(|(k, _)| k != "junk"));
        assert!(h.environ(8).is_none());
        // uptime 200 s, process started at tick 15000 (CLK_TCK is 100 on Linux): 150 s after boot -> 50 s old
        fs::write(d.join("proc/uptime"), "200.00 1234.5\n").unwrap();
        let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
        let want = 200.0 - 15_000.0 / hz;
        let got = h.age_secs(7).unwrap();
        assert!((got - want.max(0.0)).abs() < 1e-6, "{got} vs {want}");
        assert!(h.age_secs(8).is_none());
    }

    #[test]
    fn signals_reach_a_real_child_and_fail_for_missing_pids() {
        let h = RealHost::from_env();
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        let pid = c.id();
        assert!(h.is_alive(pid));
        assert_eq!(h.comm(pid).as_deref(), Some("sleep"));
        assert!(Procs::find_by_comm(&h, "sleep").contains(&pid));
        assert!(h.age_secs(pid).unwrap() < 5.0);
        h.signal(pid, Signal::Int).unwrap();
        let st = c.wait().unwrap();
        assert!(!st.success());
        assert!(!h.is_alive(pid));
        assert!(h.signal(pid, Signal::Kill).is_err());
    }

    #[test]
    fn notify_send_that_hangs_is_abandoned_after_the_timeout() {
        let d = tmp("notify");
        let bin = d.join("slow-notify-send");
        fs::write(&bin, "#!/bin/sh\nsleep 60\n").unwrap();
        fs::set_permissions(&bin, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        // Spawning a script that was written a moment ago can fail with ETXTBSY ("text file busy") when another
        // test thread forks while our write descriptor is still open; that returns at once, so try again.
        let (mut t, mut took);
        let mut tries = 0;
        loop {
            t = Instant::now();
            NotifySend::with_program(true, bin.to_str().unwrap()).notify(false, "summary", "body");
            took = t.elapsed();
            tries += 1;
            if took >= NOTIFY_TIMEOUT || tries >= 5 {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(
            took >= NOTIFY_TIMEOUT && took < NOTIFY_TIMEOUT + Duration::from_secs(2),
            "{took:?}"
        );
        // disabled notifier never spawns anything, a missing program is ignored
        let t = Instant::now();
        NotifySend::with_program(false, bin.to_str().unwrap()).notify(true, "x", "y");
        NotifySend::with_program(true, "/nonexistent/notify-send").notify(true, "x", "y");
        assert!(t.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn two_gpus_with_a_dp_1_each_report_the_connected_one() {
        let d = tmp("drm2");
        let h = host(&d);
        let drm = d.join("sys/class/drm");
        for (n, s) in [
            ("card0-DP-1", "disconnected\n"),
            ("card1-DP-1", "connected\n"),
        ] {
            fs::create_dir_all(drm.join(n)).unwrap();
            fs::write(drm.join(n).join("status"), s).unwrap();
        }
        assert_eq!(h.connector_status("DP-1").as_deref(), Some("connected"));
        fs::write(drm.join("card1-DP-1/status"), "disconnected\n").unwrap();
        assert_eq!(h.connector_status("DP-1").as_deref(), Some("disconnected"));
    }

    #[test]
    fn sigint_handler_state_is_read_from_sigcgt() {
        let d = tmp("sigcgt");
        let h = host(&d);
        let p = d.join("proc/9");
        fs::create_dir_all(&p).unwrap();
        // Monado before the first signal: SIGINT (bit 1) and SIGTERM (bit 14) caught
        fs::write(
            p.join("status"),
            "Name:\tx\nSigBlk:\t0000000000000000\nSigCgt:\t0000000000004002\n",
        )
        .unwrap();
        assert_eq!(h.catches_sigint(9), Some(true));
        // after the first SIGINT the SA_RESETHAND handler is gone, SIGTERM is still caught
        fs::write(p.join("status"), "SigCgt:\t0000000000004000\n").unwrap();
        assert_eq!(h.catches_sigint(9), Some(false));
        fs::write(p.join("status"), "Name:\tx\n").unwrap();
        assert_eq!(h.catches_sigint(9), None, "no SigCgt line");
        assert_eq!(h.catches_sigint(10), None, "no such process");
        // a real process that ignores nothing and catches nothing: SIGINT is not caught
        let real = RealHost::from_env();
        let mut c = Command::new("sleep").arg("30").spawn().unwrap();
        assert_eq!(real.catches_sigint(c.id()), Some(false));
        let _ = c.kill();
        let _ = c.wait();
    }

    fn fake_journalctl(d: &Path, body: &str) -> String {
        let p = d.join("journalctl");
        fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&p, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        p.to_string_lossy().into_owned()
    }

    #[test]
    fn kernel_errors_come_from_journalctl_and_an_unreadable_journal_is_not_reported_as_clean() {
        let d = tmp("journal");
        let mut h = host(&d);
        h.journalctl = fake_journalctl(
            &d,
            "echo 'Oct 14 00:00:01 h kernel: nvidia-modeset: Loading NVIDIA UNIX Open Kernel Mode Setting Driver'",
        );
        assert_eq!(Host::kernel_errors(&h), Ok(vec![]));
        h.journalctl = fake_journalctl(
            &d,
            "echo 'Oct 14 00:00:01 h kernel: ok'; echo 'Oct 14 00:00:09 h kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=7'",
        );
        let v = Host::kernel_errors(&h).unwrap();
        assert_eq!(v.len(), 1);
        assert!(v[0].contains("Xid"));
        // journalctl -q prints nothing and exits 0 when the journal cannot be read
        h.journalctl = fake_journalctl(&d, "exit 0");
        let e = Host::kernel_errors(&h).unwrap_err();
        assert!(e.contains("no kernel lines"), "{e}");
        // failure exit code with its message
        h.journalctl = fake_journalctl(&d, "echo 'boom' >&2; exit 3");
        let e = Host::kernel_errors(&h).unwrap_err();
        assert!(e.contains("boom"), "{e}");
        h.journalctl = "/nonexistent/journalctl".into();
        assert!(Host::kernel_errors(&h).is_err());
    }

    #[test]
    fn fake_clock_contract_is_monotonic_for_the_real_one() {
        let c = RealClock::new();
        let a = c.now();
        c.sleep(Duration::from_millis(5));
        assert!(c.now() >= a + Duration::from_millis(5));
    }
}
