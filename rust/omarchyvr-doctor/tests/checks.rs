use omarchyvr_doctor::checks::{connectors, parse_edid_id, usb_devices};
use omarchyvr_doctor::fake::FakeHost;
use omarchyvr_doctor::{run_checks, Level, Opts, Report};
use std::path::PathBuf;

fn opts() -> Opts {
    Opts {
        monado_build: "/home/u/src/monado/build".into(),
        linux_dir: "/repo/linux".into(),
        config_root: PathBuf::from("/nonexistent-pimax-config"),
        cache_root: PathBuf::from("/nonexistent-pimax-cache"),
        nvidia_src_root: "/usr/src".into(),
        connector: None,
    }
}

fn level(r: &Report, id: &str) -> Level {
    r.get(id).unwrap_or_else(|| panic!("no check {id}")).level
}

#[test]
fn healthy_machine_has_no_failures() {
    let r = run_checks(&FakeHost::healthy(), &opts());
    assert!(!r.has_fail(), "{}", r.render());
    for id in [
        "headset",
        "tobii",
        "udev",
        "keepalive",
        "gpu",
        "patches",
        "merged-head-sync",
        "connector",
        "lease",
        "setcap",
        "kernel-log",
        "stale-service",
        "monado-libs",
    ] {
        assert_eq!(level(&r, id), Level::Ok, "{id}\n{}", r.render());
    }
    assert!(r.fix_plan().contains("Nothing to fix"));
}

#[test]
fn edid_identity_decodes_pvr_1019() {
    let mut e = vec![0u8, 255, 255, 255, 255, 255, 255, 0, 0x42, 0xd2, 0x19, 0x10];
    e.resize(128, 0);
    assert_eq!(parse_edid_id(&e), (Some("PVR".into()), Some(0x1019)));
    assert_eq!(parse_edid_id(&[1, 2, 3]), (None, None));
}

#[test]
fn usb_and_connector_scan() {
    let h = FakeHost::healthy();
    let d = usb_devices(&h);
    assert_eq!(d.len(), 2);
    assert_eq!(d[0].serial.as_deref(), Some("SER1"));
    let c = connectors(&h);
    let dp = c.iter().find(|c| c.name == "DP-1").unwrap();
    assert!(dp.is_pimax() && dp.status == "connected");
    assert!(!c.iter().find(|c| c.name == "HDMI-A-1").unwrap().is_pimax());
}

#[test]
fn missing_headset_fails() {
    let mut h = FakeHost::healthy();
    h.files
        .retain(|p, _| !p.starts_with("/sys/bus/usb/devices/1-5.2"));
    let r = run_checks(&h, &opts());
    assert_eq!(level(&r, "headset"), Level::Fail);
    assert_eq!(level(&r, "tobii"), Level::Ok);
}

#[test]
fn other_pimax_model_is_called_out() {
    let mut h = FakeHost::healthy();
    h.file("/sys/bus/usb/devices/1-5.2/idProduct", "0042\n");
    let r = run_checks(&h, &opts());
    assert!(r.get("headset").unwrap().detail.contains("34a4:0042"));
}

#[test]
fn no_device_access_fails_with_udev_fix() {
    let mut h = FakeHost::healthy();
    h.rw.remove(&PathBuf::from("/dev/hidraw3"));
    let r = run_checks(&h, &opts());
    let c = r.get("udev").unwrap();
    assert_eq!(c.level, Level::Fail);
    assert!(c.detail.contains("/dev/hidraw3"));
    assert!(c.fix.as_ref().unwrap().commands[0].contains("70-pimax.rules"));
}

#[test]
fn keepalive_inactive_fails() {
    let mut h = FakeHost::healthy();
    h.cmd(
        "systemctl --user is-active omarchyvr-keepalived",
        3,
        "inactive\n",
    );
    assert_eq!(level(&run_checks(&h, &opts()), "keepalive"), Level::Fail);
}

#[test]
fn unpatched_tree_and_missing_param_fail() {
    let mut h = FakeHost::healthy();
    h.cmd(
        "patch -R -p1 --dry-run --fuzz=0 -s -d /usr/src/nvidia-610.57.04 -i /repo/linux/nvidia-2h1or-flip-fix.patch",
        1,
        "",
    );
    h.files.remove(&PathBuf::from(
        "/sys/module/nvidia_modeset/parameters/merged_head_sync",
    ));
    let r = run_checks(&h, &opts());
    let p = r.get("patches").unwrap();
    assert_eq!(p.level, Level::Fail);
    assert!(
        p.detail.contains("flip-fix") && !p.detail.contains("viewport"),
        "{}",
        p.detail
    );
    assert!(p.fix.as_ref().unwrap().reboot);
    assert_eq!(level(&r, "merged-head-sync"), Level::Fail);
}

#[test]
fn proprietary_driver_fails() {
    let mut h = FakeHost::healthy();
    h.file(
        "/proc/driver/nvidia/version",
        "NVRM version: NVIDIA UNIX x86_64 Kernel Module  610.57.04\n",
    );
    assert_eq!(level(&run_checks(&h, &opts()), "gpu"), Level::Fail);
}

#[test]
fn non_nvidia_gpu_is_informational() {
    let mut h = FakeHost::healthy();
    h.file("/sys/class/drm/card1/device/vendor", "0x1002\n");
    let r = run_checks(&h, &opts());
    assert_eq!(level(&r, "gpu"), Level::Info);
    assert!(r.get("patches").is_none());
}

#[test]
fn connector_without_native_mode_fails_with_edid_fix() {
    let mut h = FakeHost::healthy();
    h.file("/sys/class/drm/card1-DP-1/modes", "3840x2160\n");
    let r = run_checks(&h, &opts());
    let c = r.get("connector").unwrap();
    assert_eq!(c.level, Level::Fail);
    assert!(c
        .fix
        .as_ref()
        .unwrap()
        .commands
        .iter()
        .any(|x| x.contains("dp-edid.bin")));
}

#[test]
fn no_pvr_connector_fails() {
    let mut h = FakeHost::healthy();
    h.files
        .remove(&PathBuf::from("/sys/class/drm/card1-DP-1/edid"));
    let r = run_checks(&h, &opts());
    assert_eq!(level(&r, "connector"), Level::Fail);
}

#[test]
fn connector_override_mismatch_warns() {
    let mut o = opts();
    o.connector = Some("DP-2".into());
    let r = run_checks(&FakeHost::healthy(), &o);
    assert_eq!(level(&r, "connector"), Level::Warn);
}

#[test]
fn lease_protocol_missing_fails_and_no_wayland_warns() {
    let mut h = FakeHost::healthy();
    h.cmd("wayland-info", 0, "interface: 'wl_compositor'\n");
    assert_eq!(level(&run_checks(&h, &opts()), "lease"), Level::Fail);
    let mut h = FakeHost::healthy();
    h.env.clear();
    assert_eq!(level(&run_checks(&h, &opts()), "lease"), Level::Warn);
}

#[test]
fn setcap_missing_fails() {
    let mut h = FakeHost::healthy();
    let svc = "/home/u/src/monado/build/src/xrt/targets/service/monado-service";
    h.cmd(&format!("getcap {svc}"), 0, "");
    let r = run_checks(&h, &opts());
    let c = r.get("setcap").unwrap();
    assert_eq!(c.level, Level::Fail);
    assert!(c.fix.as_ref().unwrap().commands[0].starts_with("sudo setcap cap_sys_nice+ep"));
}

#[test]
fn kernel_xid_fails_and_says_reboot() {
    let mut h = FakeHost::healthy();
    h.cmd(
        "journalctl -k -b --no-pager -q",
        0,
        "ok\nNVRM: Xid (PCI:0000:01:00): 109, pid=1\nnvidia-modeset: Idling display engine timed out\n",
    );
    let r = run_checks(&h, &opts());
    let c = r.get("kernel-log").unwrap();
    assert_eq!(c.level, Level::Fail);
    assert!(c.detail.starts_with("2 GPU error lines"));
    assert!(c.fix.as_ref().unwrap().why.contains("Reboot"));
}

#[test]
fn unreadable_kernel_log_warns() {
    let mut h = FakeHost::healthy();
    h.cmds.remove("journalctl -k -b --no-pager -q");
    assert_eq!(level(&run_checks(&h, &opts()), "kernel-log"), Level::Warn);
}

#[test]
fn running_service_warns_with_pid() {
    let mut h = FakeHost::healthy();
    h.file("/proc/777/comm", "monado-service\n");
    let r = run_checks(&h, &opts());
    let c = r.get("stale-service").unwrap();
    assert_eq!(c.level, Level::Warn);
    assert!(c.detail.contains("777"));
}

#[test]
fn json_has_every_check_and_valid_escaping() {
    let mut h = FakeHost::healthy();
    h.cmd(
        "systemctl --user is-active omarchyvr-keepalived",
        3,
        "in\"active\n",
    );
    let j = run_checks(&h, &opts()).to_json().render();
    assert!(j.contains("\"id\": \"keepalive\""));
    assert!(j.contains("in\\\"active"));
    assert!(j.contains("\"worst\": \"fail\""));
}

/// The fix plan is only text, but a user will paste it: it must never contain a command the project forbids.
#[test]
fn fix_plan_never_contains_forbidden_commands() {
    let mut h = FakeHost::healthy();
    h.files.clear();
    h.cmds.clear();
    h.env.clear();
    h.file(
        "/proc/driver/nvidia/version",
        "NVRM version: NVIDIA UNIX x86_64 Kernel Module  610.57.04\n",
    );
    h.file("/sys/class/drm/card1/device/vendor", "0x10de\n");
    h.dirs.insert("/usr/src/nvidia-610.57.04".into());
    h.file("/repo/linux/nvidia-2h1or-viewport.patch", "p");
    h.file("/repo/linux/nvidia-2h1or-flip-fix.patch", "p");
    h.file("/repo/linux/nvidia-evo-makeroom-bounded-wait.patch", "p");
    h.cmd("patch -R -p1 --dry-run --fuzz=0 -s -d /usr/src/nvidia-610.57.04 -i /repo/linux/nvidia-2h1or-viewport.patch", 1, "");
    h.cmd("patch -R -p1 --dry-run --fuzz=0 -s -d /usr/src/nvidia-610.57.04 -i /repo/linux/nvidia-2h1or-flip-fix.patch", 1, "");
    h.cmd("journalctl -k -b --no-pager -q", 0, "NVRM: Xid 79\n");
    h.file("/sys/class/drm/card1-DP-1/status", "connected\n");
    h.file("/sys/class/drm/card1-DP-1/modes", "1920x1080\n");
    let mut e = vec![0u8, 255, 255, 255, 255, 255, 255, 0, 0x42, 0xd2, 0x19, 0x10];
    e.resize(128, 0);
    h.files.insert("/sys/class/drm/card1-DP-1/edid".into(), e);
    h.file(
        "/sys/class/hidraw/hidraw3/device/uevent",
        "HID_ID=0003:000034A4:00000044\n",
    );
    h.file("/sys/bus/usb/devices/1-1/idVendor", "34a4")
        .file("/sys/bus/usb/devices/1-1/idProduct", "0044");
    h.file(
        "/home/u/src/monado/build/src/xrt/targets/service/monado-service",
        "x",
    );
    h.cmd(
        "getcap /home/u/src/monado/build/src/xrt/targets/service/monado-service",
        0,
        "",
    );
    h.file("/proc/9/comm", "monado-service");
    let r = run_checks(&h, &opts());
    assert!(r.count(Level::Fail) >= 5, "{}", r.render());
    let plan = r.fix_plan();
    assert!(
        plan.contains("setcap") && plan.contains("nvidia-patch-reapply.sh"),
        "{plan}"
    );
    // the "## n." headings and "# why:" lines are prose; only command lines are checked strictly
    let forbidden = [
        "/status",
        "echo off",
        "echo detect",
        "kill -9",
        "kill -KILL",
        "pkill -9",
        "SIGKILL",
        "debugfs",
        "/edid_override",
        "flash",
        "dd if=",
    ];
    for line in plan.lines().filter(|l| !l.starts_with('#')) {
        for f in forbidden {
            assert!(!line.contains(f), "forbidden '{f}' in command line: {line}");
        }
    }
    for f in forbidden {
        assert!(!plan.contains(&format!("echo {f}")), "{f}");
    }
}

const SVC: &str = "/home/u/src/monado/build/src/xrt/targets/service/monado-service";

#[test]
fn missing_library_is_parsed_from_the_loader_message() {
    use omarchyvr_doctor::checks::missing_library;
    let e = format!("{SVC}: error while loading shared libraries: libonnxruntime.so.1: cannot open shared object file: No such file or directory\n");
    assert_eq!(missing_library(&e).as_deref(), Some("libonnxruntime.so.1"));
    // the loader's trace mode (what the check runs): a missing library is listed as "not found"
    let t = "\tlibc.so.6 => /usr/lib/libc.so.6 (0x7f)\n\tlibonnxruntime.so.1 => not found\n\tlibm.so.6 => /usr/lib/libm.so.6 (0x7f)\n";
    assert_eq!(missing_library(t).as_deref(), Some("libonnxruntime.so.1"));
    assert_eq!(missing_library("\tlibc.so.6 => /usr/lib/libc.so.6 (0x7f)\n"), None);
    assert_eq!(missing_library("monado-service: usage: ...\n"), None);
    assert_eq!(missing_library("error while loading shared libraries: \n"), None);
}

#[test]
fn monado_that_cannot_load_its_libraries_fails_in_plain_words() {
    let mut h = FakeHost::healthy();
    h.cmds.insert(
        format!("timeout 5 env LD_TRACE_LOADED_OBJECTS=1 {SVC}"),
        omarchyvr_doctor::CmdOut {
            code: Some(0),
            stdout: "\tlibc.so.6 => /usr/lib/libc.so.6 (0x7f)\n\tlibonnxruntime.so.1 => not found\n".into(),
            stderr: String::new(),
        },
    );
    let r = run_checks(&h, &opts());
    let c = r.get("monado-libs").unwrap();
    assert_eq!(c.level, Level::Fail);
    assert!(c.detail.contains("Monado cannot load its libraries: libonnxruntime.so.1") && c.detail.contains("reinstall"), "{}", c.detail);
    assert!(r.has_fail());
}

#[test]
fn monado_is_not_started_while_a_session_runs() {
    let mut h = FakeHost::healthy();
    h.file("/proc/777/comm", "monado-service\n");
    // the answer would fail, but it must not even be asked
    h.cmds.remove(&format!("timeout 5 env LD_TRACE_LOADED_OBJECTS=1 {SVC}"));
    let r = run_checks(&h, &opts());
    let c = r.get("monado-libs").unwrap();
    assert_eq!(c.level, Level::Ok);
    assert!(c.detail.contains("not checked"), "{}", c.detail);
}
