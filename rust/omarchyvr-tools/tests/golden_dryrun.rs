//! Golden files of `omarchyvr desk|vr start|stop --dry-run` (resilience plan P3: the start/stop code moved into
//! `omarchyvr-components`, and the dry-run output must stay byte-identical). Everything runs in a temp tree: fake HOME,
//! fake sysfs, fake tools on PATH, fake binaries; nothing is started or stopped.
//!
//! `PIMAX_UPDATE_GOLDEN=1 cargo test -p omarchyvr-tools --test golden_dryrun` rewrites the files in tests/golden/.
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

fn script(path: &Path, body: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, body).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A fixed tree under the target tmp dir (the same path every run, so the goldens can name it as `$ROOT`).
fn world(tag: &str) -> PathBuf {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("golden-{tag}"));
    let _ = fs::remove_dir_all(&root);
    let home = root.join("home");
    // fake sysfs: the headset, its camera with a video node, a connected DP-1 with the headset's EDID absent
    let usb = root.join("sys/bus/usb/devices/1-4");
    fs::create_dir_all(&usb).unwrap();
    fs::write(usb.join("idVendor"), "34a4\n").unwrap();
    fs::write(usb.join("idProduct"), "0044\n").unwrap();
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
    script(&root.join("bin/systemctl"), "#!/bin/bash\necho active\n");
    script(
        &root.join("bin/journalctl"),
        "#!/bin/bash\necho 'Oct 13 23:00:00 host kernel: nvidia-modeset: Loading NVIDIA UNIX Open Kernel Mode Setting Driver'\nexit 0\n",
    );
    script(&root.join("bin/notify-send"), "#!/bin/bash\nexit 0\n");
    // never the real compositor: a hyprctl that knows no outputs
    script(&root.join("bin/hyprctl"), "#!/bin/bash\necho \"[]\"\n");
    // a Monado build with the 6DoF code (the binary contains PIMAX_SLAM) and its runtime manifest
    let build = root.join("build");
    script(&build.join("src/xrt/targets/service/monado-service"), "#!/bin/bash\nexit 0\n# PIMAX_SLAM\0\n");
    fs::write(build.join("openxr_monado-dev.json"), "{}").unwrap();
    // the stack's binaries at their default places
    let rel = home.join(".local/bin");
    for b in ["omarchyvr", "omarchyvr-passthrough", "omarchyvr-voice", "omarchyvr-voice-hud", "pimax-ctrltrack", "omarchyvr-perf", "pimax-cam", "omarchyvr-slam"] {
        script(&rel.join(b), "#!/bin/bash\nexit 0\n");
    }
    for b in ["wayvr", "wayvrctl"] {
        script(&home.join("src/target-desk/release").join(b), "#!/bin/bash\nexit 0\n");
    }
    // the camera-hands build (Mercury: the binary has PIMAX_HT_HZ) and its models, for the --hands cases
    let ht = home.join("src/monado-ht/build-ht");
    script(&ht.join("src/xrt/targets/service/monado-service"), "#!/bin/bash\nexit 0\n# PIMAX_SLAM\0PIMAX_HT_HZ\0\n");
    fs::write(ht.join("openxr_monado-dev.json"), "{}").unwrap();
    let models = home.join(".local/share/monado/hand-tracking-models");
    fs::create_dir_all(&models).unwrap();
    for m in ["grayscale_detection_160x160.onnx", "grayscale_keypoint_jan18.onnx"] {
        fs::write(models.join(m), "").unwrap();
    }
    fs::create_dir_all(home.join(".local/lib")).unwrap();
    fs::write(home.join(".local/lib/libbasalt.so"), "").unwrap();
    fs::create_dir_all(root.join("session")).unwrap();
    fs::create_dir_all(root.join("run")).unwrap();
    root
}

fn run(root: &Path, args: &[&str], pid_files: &[&str]) -> String {
    run_env(root, args, pid_files, true)
}

/// `explicit_build`: PIMAX_MONADO_BUILD points at the fake desk build (all cases but the default-build --hands ones).
fn run_env(root: &Path, args: &[&str], pid_files: &[&str], explicit_build: bool) -> String {
    for p in pid_files {
        // pid files of processes that do not exist: stop --dry-run must list them and touch nothing
        fs::write(root.join("session").join(format!("{p}.pid")), "999999\n").unwrap();
    }
    let path = format!("{}:/usr/bin:/bin", root.join("bin").display());
    let mut c = Command::new(env!("CARGO_BIN_EXE_omarchyvr"));
    c.args(args)
        .env_clear()
        .env("HOME", root.join("home"))
        .env("PATH", path)
        .env("PIMAX_SYSFS_ROOT", root.join("sys"))
        .env("PIMAX_PROC_ROOT", root.join("proc"))
        .env("PIMAX_HYPRCTL", root.join("bin/hyprctl"))
        .env("PIMAX_NO_WATCHDOG", "1")
        .env("PIMAX_NO_NOTIFY", "1")
        .env("PIMAX_VR_SESSION_DIR", root.join("session"))
        .env("XDG_RUNTIME_DIR", root.join("run"))
        .env("WAYLAND_DISPLAY", "wayland-test");
    if explicit_build {
        c.env("PIMAX_MONADO_BUILD", root.join("build"));
    }
    let o = c.output().unwrap();
    let text = format!(
        "$ omarchyvr {}\nexit {:?}\n--- stdout\n{}--- stderr\n{}",
        args.join(" "),
        o.status.code(),
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    text.replace(&root.display().to_string(), "$ROOT")
}

/// (golden name, argv, pid files present)
const CASES: &[(&str, &[&str], &[&str])] = &[
    ("desk-start-default", &["desk", "start", "--dry-run"], &[]),
    ("desk-start-no-passthrough", &["desk", "start", "--dry-run", "--no-passthrough", "--no-perf"], &[]),
    ("desk-start-6dof-ctrl", &["desk", "start", "--dry-run", "--6dof", "--ctrl-track"], &[]),
    ("desk-start-voice-hud", &["desk", "start", "--dry-run", "--voice", "--voice-hud", "--preset", "work-3"], &[]),
    ("desk-start-assistant-eye", &["desk", "start", "--dry-run", "--preset", "assistant", "--eye", "--fps", "90"], &[]),
    ("desk-start-anchor-6dof", &["desk", "start", "--dry-run", "--6dof", "--anchor", "--no-eye", "--preset", "none"], &[]),
    ("vr-start-default", &["vr", "start", "--dry-run"], &[]),
    ("vr-start-6dof-ctrl-perf", &["vr", "start", "--dry-run", "--6dof", "--ctrl-track", "--perf", "--no-light"], &[]),
    ("vr-start-90-anchor", &["vr", "start", "--dry-run", "--fps", "90", "--anchor", "--no-set-runtime", "--no-light"], &[]),
    // light game mode drops the wrist readout and the room anchor even when asked for
    ("vr-start-light", &["vr", "start", "--dry-run", "--6dof", "--ctrl-track", "--perf", "--anchor", "--light", "--no-set-runtime"], &[]),
    ("vr-start-wait-debug", &["vr", "start", "--dry-run", "--wait", "--debug", "--connector", "DP-1", "--mode", "3"], &[]),
    ("desk-stop", &["desk", "stop", "--dry-run"], &["monado", "wayvr", "passthrough"]),
    (
        "desk-stop-all-clients",
        &["desk", "stop", "--dry-run", "--grace", "12", "--no-kill"],
        &["monado", "wayvr", "passthrough", "voice", "voicecap", "voiceact", "voicehud", "perf", "ctrltrack", "broker"],
    ),
    ("vr-stop", &["vr", "stop", "--dry-run"], &["monado"]),
    // --hands with PIMAX_MONADO_BUILD = a build without Mercury: a warning, the session starts without hands
    ("desk-start-hands-explicit-build", &["desk", "start", "--dry-run", "--6dof", "--hands"], &[]),
    ("vr-stop-6dof-perf", &["vr", "stop", "--dry-run", "--grace", "7", "--no-kill"], &["monado", "perf", "perfwayvr", "ctrltrack", "broker"]),
];

/// --hands cases WITHOUT PIMAX_MONADO_BUILD: one build carries everything, so --hands stays on the default desk
/// build (the old separate hands build is there but not switched to). (golden name, argv, the desk build lacks the
/// hands code)
const HANDS_CASES: &[(&str, &[&str], bool)] = &[
    ("desk-start-hands", &["desk", "start", "--dry-run", "--6dof", "--hands"], false),
    ("vr-start-hands-roles", &["vr", "start", "--dry-run", "--6dof", "--hands", "--hands-roles"], false),
    ("desk-start-hands-no-build", &["desk", "start", "--dry-run", "--6dof", "--ctrl-track", "--hands"], true),
    ("vr-start-hands-no-6dof", &["vr", "start", "--dry-run", "--hands"], false),
];

#[test]
fn hands_dry_run_output_matches_the_golden_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let update = std::env::var("PIMAX_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let mut bad = Vec::new();
    for (name, args, no_build) in HANDS_CASES {
        let root = world(name);
        // the default desk build, so the cases without --hands-build look like the real machine
        let desk = root.join("home/src/monado-eye/build");
        let marker = if *no_build { "# PIMAX_SLAM\0\n" } else { "# PIMAX_SLAM\0PIMAX_HT_HZ\0\n" };
        script(&desk.join("src/xrt/targets/service/monado-service"), &format!("#!/bin/bash\nexit 0\n{marker}"));
        fs::write(desk.join("openxr_monado-dev.json"), "{}").unwrap();
        let got = run_env(&root, args, &[], false);
        let file = dir.join(format!("{name}.txt"));
        if update {
            fs::write(&file, &got).unwrap();
            continue;
        }
        let want = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e} (PIMAX_UPDATE_GOLDEN=1 writes it)", file.display()));
        if got != want {
            let f2 = root.join("got.txt");
            fs::write(&f2, &got).unwrap();
            bad.push(format!("{name}: differs (diff {} {})", file.display(), f2.display()));
        }
        assert!(!root.join("session/start-argv.txt").exists(), "{name}: recorded argv");
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn dry_run_output_matches_the_golden_files() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden");
    let update = std::env::var("PIMAX_UPDATE_GOLDEN").is_ok_and(|v| v == "1");
    let mut bad = Vec::new();
    for (name, args, pids) in CASES {
        let root = world(name);
        let got = run(&root, args, pids);
        let file = dir.join(format!("{name}.txt"));
        if update {
            fs::create_dir_all(&dir).unwrap();
            fs::write(&file, &got).unwrap();
            continue;
        }
        let want = fs::read_to_string(&file).unwrap_or_else(|e| panic!("{}: {e} (PIMAX_UPDATE_GOLDEN=1 writes it)", file.display()));
        if got != want {
            let f2 = root.join("got.txt");
            fs::write(&f2, &got).unwrap();
            bad.push(format!("{name}: differs (diff {} {})", file.display(), f2.display()));
        }
        // a dry run never removes a pid file or creates a session file
        for p in *pids {
            assert!(root.join("session").join(format!("{p}.pid")).exists(), "{name}: {p}.pid removed by a dry run");
        }
        assert!(!root.join("session/start-argv.txt").exists(), "{name}: recorded argv");
        assert!(!root.join("session/user-stop").exists(), "{name}: user-stop marker written by a dry run");
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}
