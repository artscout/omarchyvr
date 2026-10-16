//! Preflight checks before a VR session starts. Everything the checks look at goes through [`Host`], so the logic is
//! tested with a mock and the real machine is only touched by `host::RealHost`.
//!
//! Each failed check names the cause and the fix, because the launcher is meant to be clicked from a desktop entry
//! where the only feedback is a notification.

use std::path::Path;

/// How bad a finding is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// All good.
    Ok,
    /// Worth a look, does not stop the start.
    Warn,
    /// The start must not continue (unless forced).
    Fail,
}

/// One line of the report.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Check {
    /// Short name (`headset`, `keepalive`, ...).
    pub name: &'static str,
    /// Result.
    pub level: Level,
    /// What was found and, for failures, what to do.
    pub detail: String,
}

/// All checks of one run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Report {
    /// In the order they were run.
    pub checks: Vec<Check>,
}

impl Report {
    /// True if no check failed.
    pub fn passed(&self) -> bool {
        !self.checks.iter().any(|c| c.level == Level::Fail)
    }

    /// The failed checks.
    pub fn failures(&self) -> Vec<&Check> {
        self.checks
            .iter()
            .filter(|c| c.level == Level::Fail)
            .collect()
    }

    /// Multi-line text, one check per line.
    pub fn render(&self) -> String {
        self.checks
            .iter()
            .map(|c| {
                let tag = match c.level {
                    Level::Ok => "ok  ",
                    Level::Warn => "warn",
                    Level::Fail => "FAIL",
                };
                format!("  [{tag}] {:<9} {}", c.name, c.detail)
            })
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// One-line summary of the failures for a notification.
    pub fn failure_summary(&self) -> String {
        self.failures()
            .iter()
            .map(|c| format!("{}: {}", c.name, c.detail))
            .collect::<Vec<_>>()
            .join("; ")
    }
}

/// What the checks may ask about the machine.
pub trait Host {
    /// Is 34a4:0044 on the USB bus?
    fn usb_headset_present(&self) -> bool;
    /// How many of the four hidraw interfaces of the headset exist (0..=4).
    fn hidraw_interfaces(&self) -> usize;
    /// `systemctl --user is-active pimax-keepalived` (`active`, `inactive`, `failed`, ...).
    fn keepalive_state(&self) -> String;
    /// Content of `/sys/class/drm/card*-<connector>/status`; `None` if there is no such connector.
    fn connector_status(&self, connector: &str) -> Option<String>;
    /// Lines of this boot's kernel log that mention an NVIDIA Xid or the display engine hang; `Err` if the log
    /// cannot be read.
    fn kernel_errors(&self) -> Result<Vec<String>, String>;
    /// Pids of running monado-service processes of this user.
    fn service_pids(&self) -> Vec<u32>;
    /// Environment variable of the calling process.
    fn env(&self, key: &str) -> Option<String>;
    /// Does the file carry `cap_sys_nice` (permitted and effective in its `security.capability` xattr)? `None` if the
    /// file does not exist or the host cannot tell.
    fn service_has_sys_nice(&self, _service: &Path) -> Option<bool> {
        None
    }
    /// Camera 34a4:0014 on USB with its video4linux node: `Some(true)`; missing while the headset is there:
    /// `Some(false)`; `None` when the host cannot tell (no camera line is shown).
    fn camera_ok(&self) -> Option<bool> {
        None
    }
    /// The DRM connector (`DP-1`) whose EDID says Pimax Dream Air (vendor PVR, product 0x1019), if one is there.
    fn headset_connector(&self) -> Option<String> {
        None
    }
}

/// True for the kernel lines that mean "the GPU or the display engine is in trouble": `grep -E "Xid|Idling"` of the
/// overnight rules, plus "Stuck layer channel" (a stuck flip; the doctor's reboot rule). `gate` grades them.
pub fn is_gpu_error_line(line: &str) -> bool {
    line.contains("Xid") || line.contains("Idling") || line.contains("Stuck layer channel")
}

/// Run all checks (without the capability check, which needs the service path: see [`run_for`]).
pub fn run(host: &dyn Host, connector: &str) -> Report {
    run_for(host, connector, None)
}

/// Run all checks; with `service`, also warn when that monado-service binary lacks `cap_sys_nice`.
pub fn run_for(host: &dyn Host, connector: &str, service: Option<&Path>) -> Report {
    let mut r = Report::default();
    let mut add = |name: &'static str, level: Level, detail: String| {
        r.checks.push(Check {
            name,
            level,
            detail,
        })
    };

    // 1. headset on USB
    if host.usb_headset_present() {
        let n = host.hidraw_interfaces();
        if n == 4 {
            add(
                "headset",
                Level::Ok,
                "34a4:0044 on USB, 4/4 hidraw interfaces".into(),
            );
        } else {
            add(
                "headset",
                Level::Warn,
                format!("34a4:0044 on USB but only {n}/4 hidraw interfaces (udev rule linux/70-pimax.rules installed?)"),
            );
        }
    } else {
        add(
            "headset",
            Level::Fail,
            "34a4:0044 not found on USB: check the cable and power; see analysis/usb-stuck-recovery.md".into(),
        );
    }

    // 1b. camera (the USB3 side can drop on its own; start recovers it unless --no-auto-recover)
    match host.camera_ok() {
        Some(true) => add("camera", Level::Ok, "34a4:0014 on USB, video node present".into()),
        Some(false) => add(
            "camera",
            Level::Warn,
            "camera missing: will auto-recover (pimax vr reset-headset)".into(),
        ),
        None => {}
    }

    // 2. keepalive service
    let ka = host.keepalive_state();
    if ka == "active" {
        add("keepalive", Level::Ok, "pimax-keepalived is active".into());
    } else {
        add(
            "keepalive",
            Level::Fail,
            format!("pimax-keepalived is {ka}: run systemctl --user start pimax-keepalived"),
        );
    }

    // 3. display connector
    match host.connector_status(connector).as_deref() {
        Some("connected") => add("display", Level::Ok, format!("{connector} connected")),
        Some(other) => add(
            "display",
            Level::Fail,
            format!("{connector} is {other}: headset off, or the DisplayPort cable/EDID override is not active"),
        ),
        None => add(
            "display",
            Level::Fail,
            format!("no DRM connector {connector} (is the NVIDIA driver loaded?)"),
        ),
    }

    // 4. kernel log of this boot
    match host.kernel_errors() {
        Ok(lines) if lines.is_empty() => add(
            "kernel",
            Level::Ok,
            "no Xid / display engine errors in this boot's kernel log".into(),
        ),
        Ok(lines) => add(
            "kernel",
            Level::Fail,
            format!(
                "{} GPU/display error line(s) this boot (first: {}): REBOOT before the next VR session",
                lines.len(),
                lines[0].trim()
            ),
        ),
        Err(e) => add(
            "kernel",
            Level::Warn,
            format!("could not read the kernel log ({e}); check journalctl -k -b | grep -E 'Xid|Idling' by hand"),
        ),
    }

    // 5. no other service
    let pids = host.service_pids();
    if pids.is_empty() {
        add("service", Level::Ok, "no monado-service running".into());
    } else {
        let list: Vec<String> = pids.iter().map(u32::to_string).collect();
        add(
            "service",
            Level::Fail,
            format!(
                "monado-service already running (pid {}): pimax vr stop first",
                list.join(", ")
            ),
        );
    }

    // 6. Wayland session (the DRM lease is requested from the compositor)
    let have = |k: &str| host.env(k).is_some_and(|v| !v.is_empty());
    if have("WAYLAND_DISPLAY") && have("XDG_RUNTIME_DIR") {
        add(
            "wayland",
            Level::Ok,
            "WAYLAND_DISPLAY and XDG_RUNTIME_DIR set".into(),
        );
    } else {
        add(
            "wayland",
            Level::Warn,
            "WAYLAND_DISPLAY/XDG_RUNTIME_DIR not set: the DRM lease needs the Hyprland session environment".into(),
        );
    }

    // 7. GPU/CPU priority of the compositor (file capability on the binary)
    if let Some(path) = service {
        match host.service_has_sys_nice(path) {
            Some(true) => add(
                "priority",
                Level::Ok,
                "monado-service has cap_sys_nice".into(),
            ),
            Some(false) => add("priority", Level::Warn, sys_nice_advice(path)),
            None => {}
        }
    }
    r
}

/// The warning and the fix for a service binary without `cap_sys_nice`.
pub fn sys_nice_advice(path: &Path) -> String {
    format!(
        "{} lacks cap_sys_nice: the compositor cannot raise its GPU/thread priority and frames can be late under load. Fix (root): sudo setcap cap_sys_nice+ep {}   (every relink of monado-service drops it; the Pimax app fixes it too: Fix on Home, or just Play, one password window)",
        path.display(),
        path.display()
    )
}

/// Does a `security.capability` xattr value grant `CAP_SYS_NICE` (bit 23) as permitted and effective? The value is
/// `struct vfs_cap_data`: u32 magic_etc (bit 0 = effective flag), then permitted/inheritable u32 pairs.
pub fn caps_grant_sys_nice(xattr: &[u8]) -> bool {
    const CAP_SYS_NICE: u32 = 23;
    if xattr.len() < 12 {
        return false;
    }
    let magic = u32::from_le_bytes([xattr[0], xattr[1], xattr[2], xattr[3]]);
    let permitted = u32::from_le_bytes([xattr[4], xattr[5], xattr[6], xattr[7]]);
    magic & 1 != 0 && permitted & (1 << CAP_SYS_NICE) != 0
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A healthy machine that individual tests spoil.
    pub(crate) struct Mock {
        pub usb: bool,
        pub ifaces: usize,
        pub keepalive: String,
        pub conn: Option<String>,
        pub kernel: Result<Vec<String>, String>,
        pub pids: Vec<u32>,
        pub env: HashMap<String, String>,
        pub nice: Option<bool>,
    }

    impl Mock {
        pub(crate) fn healthy() -> Self {
            Mock {
                usb: true,
                ifaces: 4,
                keepalive: "active".into(),
                conn: Some("connected".into()),
                kernel: Ok(vec![]),
                pids: vec![],
                env: [
                    ("WAYLAND_DISPLAY", "wayland-1"),
                    ("XDG_RUNTIME_DIR", "/run/user/1000"),
                ]
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
                nice: None,
            }
        }
    }

    impl Host for Mock {
        fn usb_headset_present(&self) -> bool {
            self.usb
        }
        fn hidraw_interfaces(&self) -> usize {
            self.ifaces
        }
        fn keepalive_state(&self) -> String {
            self.keepalive.clone()
        }
        fn connector_status(&self, _c: &str) -> Option<String> {
            self.conn.clone()
        }
        fn kernel_errors(&self) -> Result<Vec<String>, String> {
            self.kernel.clone()
        }
        fn service_pids(&self) -> Vec<u32> {
            self.pids.clone()
        }
        fn env(&self, key: &str) -> Option<String> {
            self.env.get(key).cloned()
        }
        fn service_has_sys_nice(&self, _s: &Path) -> Option<bool> {
            self.nice
        }
    }

    #[test]
    fn capability_xattr_decoding() {
        // setcap cap_sys_nice=ep (VFS_CAP_REVISION_2 | EFFECTIVE, permitted bit 23)
        let ep = [
            1, 0, 0, 0x02, 0, 0, 0x80, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert!(caps_grant_sys_nice(&ep));
        let mut p_only = ep;
        p_only[0] = 0; // permitted but not effective: file-capability binaries would not get it
        assert!(!caps_grant_sys_nice(&p_only));
        let mut other = ep;
        other[6] = 0x40; // CAP_SYS_PTRACE-ish bit 22 instead
        assert!(!caps_grant_sys_nice(&other));
        assert!(!caps_grant_sys_nice(&[]));
        assert!(!caps_grant_sys_nice(&ep[..8]));
    }

    #[test]
    fn missing_cap_sys_nice_warns_with_the_setcap_line_for_that_path() {
        let svc = Path::new("/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service");
        let mut m = Mock::healthy();
        m.nice = Some(false);
        let r = run_for(&m, "DP-1", Some(svc));
        assert!(r.passed());
        assert_eq!(level(&r, "priority"), Level::Warn);
        let d = &r.checks.last().unwrap().detail;
        assert!(
            d.contains(&format!("sudo setcap cap_sys_nice+ep {}", svc.display())),
            "{d}"
        );
        m.nice = Some(true);
        assert_eq!(
            level(&run_for(&m, "DP-1", Some(svc)), "priority"),
            Level::Ok
        );
        // binary missing / unknown: no line; and the plain run() never adds it
        m.nice = None;
        assert_eq!(run_for(&m, "DP-1", Some(svc)).checks.len(), 6);
        m.nice = Some(false);
        assert_eq!(run(&m, "DP-1").checks.len(), 6);
    }

    fn level(r: &Report, name: &str) -> Level {
        r.checks.iter().find(|c| c.name == name).unwrap().level
    }

    #[test]
    fn healthy_machine_passes_all_six_checks() {
        let r = run(&Mock::healthy(), "DP-1");
        assert!(r.passed());
        assert_eq!(r.checks.len(), 6);
        assert!(
            r.checks.iter().all(|c| c.level == Level::Ok),
            "{}",
            r.render()
        );
        let names: Vec<&str> = r.checks.iter().map(|c| c.name).collect();
        assert_eq!(
            names,
            [
                "headset",
                "keepalive",
                "display",
                "kernel",
                "service",
                "wayland"
            ]
        );
        assert!(r.render().contains("[ok  ] headset"));
        assert_eq!(r.failure_summary(), "");
    }

    #[test]
    fn missing_headset_fails() {
        let mut m = Mock::healthy();
        m.usb = false;
        let r = run(&m, "DP-1");
        assert!(!r.passed());
        assert_eq!(level(&r, "headset"), Level::Fail);
        assert!(r
            .failure_summary()
            .starts_with("headset: 34a4:0044 not found"));
    }

    #[test]
    fn missing_hidraw_interfaces_only_warn() {
        let mut m = Mock::healthy();
        m.ifaces = 2;
        let r = run(&m, "DP-1");
        assert!(r.passed());
        assert_eq!(level(&r, "headset"), Level::Warn);
        assert!(r.checks[0].detail.contains("2/4"));
    }

    #[test]
    fn keepalive_not_active_fails() {
        for s in ["inactive", "failed", "unknown", "activating"] {
            let mut m = Mock::healthy();
            m.keepalive = s.into();
            let r = run(&m, "DP-1");
            assert_eq!(level(&r, "keepalive"), Level::Fail, "{s}");
            assert!(r.render().contains(&format!("pimax-keepalived is {s}")));
        }
    }

    #[test]
    fn display_must_be_connected() {
        let mut m = Mock::healthy();
        m.conn = Some("disconnected".into());
        let r = run(&m, "DP-1");
        assert_eq!(level(&r, "display"), Level::Fail);
        assert!(r.render().contains("DP-1 is disconnected"));
        m.conn = None;
        let r = run(&m, "DP-2");
        assert_eq!(level(&r, "display"), Level::Fail);
        assert!(r.render().contains("no DRM connector DP-2"));
    }

    #[test]
    fn xid_in_the_kernel_log_fails_and_says_reboot() {
        let mut m = Mock::healthy();
        m.kernel = Ok(vec![
            "Oct 13 23:00:00 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=1".into(),
            "Oct 13 23:00:05 mau kernel: nvidia-modeset: Idling display engine timed out".into(),
        ]);
        let r = run(&m, "DP-1");
        assert!(!r.passed());
        let d = &r.failures()[0].detail;
        assert!(d.contains("2 GPU/display error line(s)"), "{d}");
        assert!(d.contains("REBOOT"), "{d}");
        assert!(d.contains("Xid (PCI"), "{d}");
    }

    #[test]
    fn unreadable_kernel_log_only_warns() {
        let mut m = Mock::healthy();
        m.kernel = Err("journalctl: permission denied".into());
        let r = run(&m, "DP-1");
        assert!(r.passed());
        assert_eq!(level(&r, "kernel"), Level::Warn);
    }

    #[test]
    fn running_service_fails_and_lists_pids() {
        let mut m = Mock::healthy();
        m.pids = vec![4242, 77];
        let r = run(&m, "DP-1");
        assert_eq!(level(&r, "service"), Level::Fail);
        assert!(r.render().contains("pid 4242, 77"));
    }

    #[test]
    fn wayland_environment_is_a_warning() {
        let mut m = Mock::healthy();
        m.env.remove("WAYLAND_DISPLAY");
        let r = run(&m, "DP-1");
        assert!(r.passed());
        assert_eq!(level(&r, "wayland"), Level::Warn);
        m.env.insert("WAYLAND_DISPLAY".into(), String::new());
        assert_eq!(level(&run(&m, "DP-1"), "wayland"), Level::Warn);
    }

    #[test]
    fn several_failures_are_all_reported() {
        let mut m = Mock::healthy();
        m.usb = false;
        m.keepalive = "failed".into();
        m.pids = vec![1];
        let r = run(&m, "DP-1");
        assert_eq!(r.failures().len(), 3);
        let s = r.failure_summary();
        assert!(s.contains("headset:") && s.contains("keepalive:") && s.contains("service:"));
    }

    #[test]
    fn gpu_error_line_matcher_follows_the_overnight_rule() {
        assert!(is_gpu_error_line("NVRM: Xid (PCI:0000:01:00): 31, pid=9"));
        assert!(is_gpu_error_line(
            "nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x0000c67e"
        ));
        assert!(!is_gpu_error_line(
            "nvidia-modeset: Loading NVIDIA UNIX Open Kernel Mode Setting Driver"
        ));
        assert!(!is_gpu_error_line(
            "usb 1-3: new high-speed USB device number 15"
        ));
    }
}
