//! In-memory [`Host`] for tests (also usable by the GUI self-test).
use crate::host::{CmdOut, Host};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

/// A fake machine: files, commands, environment, access rights.
#[derive(Default, Clone)]
pub struct FakeHost {
    /// path -> content
    pub files: BTreeMap<PathBuf, Vec<u8>>,
    /// Directories that exist without files.
    pub dirs: BTreeSet<PathBuf>,
    /// "prog arg arg" -> output
    pub cmds: BTreeMap<String, CmdOut>,
    /// Environment.
    pub env: BTreeMap<String, String>,
    /// Paths the user can read and write.
    pub rw: HashSet<PathBuf>,
}

impl FakeHost {
    /// Add a file.
    pub fn file(&mut self, p: &str, c: impl AsRef<[u8]>) -> &mut Self {
        self.files.insert(p.into(), c.as_ref().to_vec());
        self
    }
    /// Add a command result.
    pub fn cmd(&mut self, line: &str, code: i32, stdout: &str) -> &mut Self {
        self.cmds.insert(
            line.into(),
            CmdOut {
                code: Some(code),
                stdout: stdout.into(),
                stderr: String::new(),
            },
        );
        self
    }
    /// Grant read/write access.
    pub fn allow(&mut self, p: &str) -> &mut Self {
        self.rw.insert(p.into());
        self
    }

    /// A host where every check passes (a USB headset with serial `SER1`, hidraw access, NVIDIA open module with all
    /// patches, a PVR connector with 7104x3840, wayland lease protocol, setcap set, clean kernel log).
    pub fn healthy() -> FakeHost {
        let mut h = FakeHost::default();
        let dev = "/sys/bus/usb/devices/1-5.2";
        h.file(&format!("{dev}/idVendor"), "34a4\n")
            .file(&format!("{dev}/idProduct"), "0044\n")
            .file(&format!("{dev}/serial"), "SER1\n")
            .file(&format!("{dev}/bcdDevice"), "0102\n")
            .file(&format!("{dev}/busnum"), "1\n")
            .file(&format!("{dev}/devnum"), "7\n")
            .allow("/dev/bus/usb/001/007");
        let tobii = "/sys/bus/usb/devices/1-5.3";
        h.file(&format!("{tobii}/idVendor"), "2104\n")
            .file(&format!("{tobii}/idProduct"), "0561\n")
            .file(&format!("{tobii}/serial"), "XR5DA-SER1\n")
            .file(&format!("{tobii}/busnum"), "1\n")
            .file(&format!("{tobii}/devnum"), "9\n")
            .allow("/dev/bus/usb/001/009");
        h.file(
            "/sys/class/hidraw/hidraw3/device/uevent",
            "HID_ID=0003:000034A4:00000044\n",
        )
        .allow("/dev/hidraw3");
        h.file(
            "/sys/class/hidraw/hidraw0/device/uevent",
            "HID_ID=0003:0000046D:0000C52B\n",
        );
        h.cmd("systemctl --user is-active omarchyvr-keepalived", 0, "active\n");
        h.file(
            "/proc/driver/nvidia/version",
            "NVRM version: NVIDIA UNIX Open Kernel Module for x86_64  610.57.04  Release Build\n",
        );
        h.file("/sys/class/drm/card1/device/vendor", "0x10de\n");
        h.file(
            "/sys/module/nvidia_modeset/parameters/merged_head_sync",
            "1\n",
        );
        h.dirs.insert("/usr/src/nvidia-610.57.04".into());
        h.file(
            "/usr/src/nvidia-610.57.04/src/nvidia-modeset/src/nvkms-evo.c",
            "x",
        );
        for p in crate::checks::PATCHES {
            h.cmd(
                &format!(
                    "patch -R -p1 --dry-run --fuzz=0 -s -d /usr/src/nvidia-610.57.04 -i /repo/linux/{}",
                    p.file
                ),
                0,
                "",
            );
            h.file(&format!("/repo/linux/{}", p.file), "patch");
        }
        // EDID: manufacturer PVR (0x42d2), product 0x1019
        let mut edid = vec![0u8, 255, 255, 255, 255, 255, 255, 0, 0x42, 0xd2, 0x19, 0x10];
        edid.resize(128, 0);
        h.files
            .insert("/sys/class/drm/card1-DP-1/edid".into(), edid);
        h.file("/sys/class/drm/card1-DP-1/status", "connected\n")
            .file(
                "/sys/class/drm/card1-DP-1/modes",
                "3840x2160\n7104x3840\n7104x3840\n",
            );
        h.file("/sys/class/drm/card1-HDMI-A-1/status", "disconnected\n");
        h.env.insert("WAYLAND_DISPLAY".into(), "wayland-1".into());
        h.cmd(
            "wayland-info",
            0,
            "interface: 'wp_drm_lease_device_v1', version: 1, name: 67\n",
        );
        let svc = "/home/u/src/monado/build/src/xrt/targets/service/monado-service";
        h.file(svc, "elf");
        h.cmd(
            &format!("getcap {svc}"),
            0,
            &format!("{svc} cap_sys_nice=ep\n"),
        );
        h.cmd(&format!("timeout 5 env LD_TRACE_LOADED_OBJECTS=1 {svc}"), 0, "\tlibc.so.6 => /usr/lib/libc.so.6 (0x7f)\n");
        h.cmd("journalctl -k -b --no-pager -q", 0, "kernel: all quiet\n");
        h.file("/proc/1/comm", "systemd\n");
        h.file("/proc/4242/comm", "firefox\n");
        h
    }
}

impl Host for FakeHost {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        self.files.get(p).cloned()
    }
    fn list(&self, p: &Path) -> Vec<String> {
        let mut s = BTreeSet::new();
        for f in self.files.keys().chain(self.dirs.iter()) {
            if let Ok(rest) = f.strip_prefix(p) {
                if let Some(c) = rest.components().next() {
                    s.insert(c.as_os_str().to_string_lossy().into_owned());
                }
            }
        }
        s.into_iter().collect()
    }
    fn exists(&self, p: &Path) -> bool {
        self.files.contains_key(p)
            || self.dirs.contains(p)
            || self.files.keys().any(|f| f.starts_with(p))
    }
    fn can_rw(&self, p: &Path) -> bool {
        self.rw.contains(p)
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        let line = std::iter::once(prog)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.cmds.get(&line).cloned()
    }
    fn env(&self, key: &str) -> Option<String> {
        self.env.get(key).cloned()
    }
}
