//! Fixture machines: an idle desk PC like the one this was built on (healthy, one WayVR Xid 31 in the kernel log, the
//! slam build without `cap_sys_nice`) and the same PC with a desk session running (eye + 6DoF). Used by the tests and
//! by `omarchyvr-app --screenshots` (no real machine state ends up in the pictures).
use crate::machine::{FakeMachine, FileMeta};
use crate::paths::Paths;
use omarchyvr_doctor::fake::FakeHost;
use omarchyvr_doctor::Opts;
use std::path::Path;

/// The kernel log of the demo (two WayVR MMU faults: yellow, LESSONS row 9).
pub const KERNEL_LOG: &str = "Oct 14 09:12:01 mau kernel: Linux version 7.2.5-3-omarchy\n\
Oct 14 14:20:00 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel 0x00000016, intr 00000000. MMU Fault: ENGINE GR0_PBDMA0 HUBCLIENT_ESC faulted @ 0x0_0e8e0000. Fault is of type FAULT_PTE ACCESS_TYPE_VIRT_WRITE\n\
Oct 14 15:52:01 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=3401907, name=wayvr, channel 0x00000016, intr 00000000. MMU Fault: ENGINE GR0_PBDMA0 HUBCLIENT_ESC faulted @ 0x0_0e8e0000. Fault is of type FAULT_PTE ACCESS_TYPE_VIRT_WRITE\n";

/// The monado.log of the running demo session.
pub const MONADO_LOG: &str = include_str!("../tests/fixtures/monado-desk-6dof.log");

/// The `omarchyvr desk help` text of the current launcher.
pub const DESK_HELP: &str = include_str!("../tests/fixtures/pimax-desk-help.txt");

/// Paths of the demo user.
pub fn paths() -> Paths {
    Paths::new(
        Path::new("/home/u"),
        Path::new("/home/u/pimax-re"),
        Path::new("/tmp/omarchyvr-session"),
        Path::new("/run/user/1000"),
    )
}

/// Doctor options matching [`paths`] (the doctor's fake patch commands use `/repo/linux`).
pub fn opts(p: &Paths) -> Opts {
    Opts {
        monado_build: p.build("pimax").map(|b| b.dir.clone()).unwrap_or_default(),
        linux_dir: "/repo/linux".into(),
        config_root: "/nonexistent-pimax-demo/config".into(),
        cache_root: "/nonexistent-pimax-demo/cache".into(),
        nvidia_src_root: "/usr/src".into(),
        connector: None,
    }
}

fn service_binary(slam: bool, eye: bool) -> Vec<u8> {
    let mut b = vec![0x7fu8, b'E', b'L', b'F'];
    b.resize(4096, 0);
    if slam {
        b.extend_from_slice(b"PIMAX_SLAM\0slam: %lu frame sets, %lu IMU samples (%lu held from the tracker in %lu camera gaps)\0");
    }
    if eye {
        // the full build (ai/CLAUDE.md section 3): eye, controllers, anchor, the IMU gate
        b.extend_from_slice(b"PIMAX_EYE\0PIMAX_CTRL_TRACK\0origin_gate_mm\0PIMAX_ANCHOR\0PIMAX_CTRL_INHAND\0");
    }
    b
}

/// The idle machine.
pub fn idle_machine() -> (FakeMachine, Paths) {
    let p = paths();
    let mut m = FakeMachine {
        host: FakeHost::healthy(),
        ..FakeMachine::new()
    };
    m.set_env("HOME", "/home/u")
        .set_env("XDG_RUNTIME_DIR", "/run/user/1000")
        .set_env("XDG_CURRENT_DESKTOP", "Hyprland");
    m.file("/proc/uptime", "36000.00 200000.00\n");
    m.file("/proc/sys/kernel/osrelease", "7.2.5-3-omarchy\n");
    m.cmd("journalctl -k -b --no-pager -q", 0, KERNEL_LOG);
    m.cmd("pactl get-default-source", 0, "alsa_input.usb-Pimax_Corporation_Pimax_Dream_Air_P511X0X000000000002-04.analog-stereo\n");
    m.cmd("modinfo -F parm nvidia-modeset", 0, "merged_head_sync:Sync merged heads (int)\nconfig_file:path (charp)\n");
    // the headset's camera on USB (34a4:0014) next to the HID chip of the fake host
    m.file("/sys/bus/usb/devices/1-5.4/idVendor", "34a4\n");
    m.file("/sys/bus/usb/devices/1-5.4/idProduct", "0014\n");
    m.file("/sys/bus/usb/devices/1-5.4/busnum", "1\n");
    m.file("/sys/bus/usb/devices/1-5.4/devnum", "8\n");
    m.host.allow("/dev/bus/usb/001/008");
    m.file("/sys/class/video4linux/video0/name", "PimaxSensor: PimaxSensor\n");
    m.file("/sys/class/video4linux/video1/name", "PimaxSensor: PimaxSensor\n");
    m.file("/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook", "[Trigger]\n");
    m.file("/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook", "[Trigger]\n");
    let rules = "SUBSYSTEM==\"hidraw\", ATTRS{idVendor}==\"34a4\", MODE=\"0660\", TAG+=\"uaccess\"\n";
    m.file("/etc/udev/rules.d/70-pimax.rules", rules);
    m.file("/home/u/pimax-re/linux/70-pimax.rules", rules);
    m.file("/home/u/.config/omarchyvr/ipd.env", "OXR_DEBUG_IPD_MM=65.9\n");
    // packages, the keepalive unit with its Rust drop-in, the non-desktop EDID (MANUAL-STEPS 1.1-1.8)
    for f in ["/usr/bin/cargo", "/usr/bin/steam", "/home/u/.local/share/Steam/steamapps/common/Proton - Experimental/proton"] {
        m.file(f, "x");
    }
    m.file("/home/u/.config/systemd/user/omarchyvr-keepalived.service", "[Service]\nExecStart=/usr/bin/python3 %h/pimax-re/linux/pimax_keepalived.py\n");
    m.file("/home/u/.config/systemd/user/omarchyvr-keepalived.service.d/rust.conf", "[Service]\nExecStart=\nExecStart=%h/.local/bin/omarchyvr keepalived\n");
    m.file("/etc/pimax/dp-edid.bin", "EDID-NON-DESKTOP");
    m.file("/home/u/pimax-re/linux/pimax-dp-edid-msvsdb.bin", "EDID-NON-DESKTOP");
    m.file("/home/u/.local/share/omarchyvr/eye-calibration/eyecal-20261014-103635.json", "{}");
    m.file("/home/u/.config/openvr/openvrpaths.vrpath", "{\"runtime\": [\"/home/u/src/xrizer/target/release\"]}\n");
    m.set_meta(
        "/home/u/.config/openvr/openvrpaths.vrpath",
        FileMeta { len: 52, mode: 0o100444, ..Default::default() },
    );
    m.free.insert("/home/u".into(), 412 << 30);
    m.free.insert("/tmp".into(), 30 << 30);
    // builds
    let cache_desk = include_str!("../tests/fixtures/CMakeCache-desk.txt");
    let cache_pimax = include_str!("../tests/fixtures/CMakeCache-pimax.txt");
    let cache_slam = cache_desk.replace("monado-eye", "monado-slam");
    for (dir, cache, slam, eye, cap, branch) in [
        ("/home/u/src/monado-eye", cache_desk.to_string(), true, true, true, "desk"),
        ("/home/u/src/monado-slam", cache_slam, true, false, false, "slam"),
        ("/home/u/src/monado", cache_pimax.to_string(), false, false, true, "pimax"),
    ] {
        let svc = format!("{dir}/build/src/xrt/targets/service/monado-service");
        m.file(&svc, service_binary(slam, eye));
        m.set_meta(&svc, FileMeta { len: service_binary(slam, eye).len() as u64, mtime: 1_791_960_000, ino: 77, mode: 0o100755, is_dir: false });
        m.file(&format!("{dir}/build/CMakeCache.txt"), cache);
        m.file(&format!("{dir}/build/openxr_monado-dev.json"), "{}\n");
        m.file(&format!("{dir}/.git/HEAD"), format!("ref: refs/heads/{branch}\n"));
        m.cmd(&format!("getcap {svc}"), 0, &if cap { format!("{svc} cap_sys_nice=ep\n") } else { String::new() });
    }
    // launcher, app, binaries, models
    for f in ["/home/u/.local/bin/omarchyvr", "/home/u/pimax-re/rust/target/release/omarchyvr"] {
        m.file(f, "omarchyvr launcher v7e9d910");
    }
    m.cmd("/home/u/.local/bin/omarchyvr desk help", 0, DESK_HELP);
    for (b, _) in crate::facts::RELEASE_BINS {
        m.file(&format!("/home/u/pimax-re/rust/target/release/{b}"), "elf");
    }
    for f in [
        "/home/u/src/target-desk/release/wayvr",
        "/home/u/src/wayvr/Cargo.toml",
        "/home/u/.local/lib/libbasalt.so",
        "/home/u/models/parakeet-tdt-0.6b-v3-int8/tokens.txt",
        "/home/u/src/venv-voice/lib/python3.14/site-packages/sherpa_onnx/lib/libsherpa-onnx-c-api.so",
        "/home/u/pimax-re/analysis/tools/run_slam_view.sh",
        "/home/u/pimax-re/analysis/tools/vr_runtime_switch.sh",
        "/home/u/pimax-re/rust/target/release/omarchyvr-app",
    ] {
        m.file(f, "x");
    }
    (m, p)
}

/// The idle machine with a desk session running (`omarchyvr desk start --eye --6dof`).
pub fn running_machine() -> (FakeMachine, Paths) {
    let (mut m, p) = idle_machine();
    let s = "/tmp/omarchyvr-session";
    let svc = "/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service";
    let env = [
        ("PIMAX_FPS", "72"),
        ("XRT_COMPOSITOR_DESIRED_MODE", "2"),
        ("PIMAX_EYE", "1"),
        ("PIMAX_SLAM", "1"),
        ("PIMAX_CAM_SOURCE", "broker"),
        ("PIMAX_CAM_AE", "1"),
        ("OXR_DEBUG_IPD_MM", "65.9"),
    ];
    // ages: uptime 36000 s; started at 35100 s (15 min ago) etc.
    m.process(372448, "pimax-cam", "/home/u/pimax-re/rust/target/release/pimax-cam broker", &[], 3_509_000, &["/dev/video0"]);
    m.process(372460, "monado-service", svc, &env, 3_510_000, &["/dev/bus/usb/001/009", "/dev/hidraw3"]);
    m.process(372563, "omarchyvr-passt", "/home/u/pimax-re/rust/target/release/omarchyvr-passthrough run --source broker", &[], 3_511_000, &[]);
    m.process(378947, "wayvr", "/home/u/src/target-desk/release/wayvr --openxr --show --no-autostart", &[], 3_512_000, &[]);
    for (n, pid) in [("broker", 372448), ("monado", 372460), ("passthrough", 372563), ("wayvr", 378947)] {
        m.file(&format!("{s}/{n}.pid"), format!("{pid}\n"));
    }
    m.file(&format!("{s}/session.txt"), format!("pid=372460\nfps=72\nmode=2\nstarted_unix=1791968155\nlog={s}/monado.log\ndump_dir=\nservice={svc}\n"));
    m.file(&format!("{s}/monado.log"), MONADO_LOG);
    m.file(&format!("{s}/passthrough.log"), "omarchyvr-passthrough: 25.0 fps, latency 41 ms\n");
    m.file(&format!("{s}/broker.log"), "pimax-cam broker: 60.00 fps, 2 readers, 0 lost\n");
    (m, p)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_machines_build() {
        let (m, p) = idle_machine();
        assert!(crate::procs::monado_pids(&m).is_empty());
        assert_eq!(p.desk_build().dir, Path::new("/home/u/src/monado-eye/build"));
        let (r, _) = running_machine();
        assert_eq!(crate::procs::monado_pids(&r), vec![372460]);
        let age = crate::procs::age_secs(&r, 372460).unwrap();
        assert!((age - 900.0).abs() < 0.5, "{age}");
    }
}
