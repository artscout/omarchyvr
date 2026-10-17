//! A clean install (2026-10-17 parity audit): no Basalt source tree, no calibration of the user's own, no gyro bias
//! measured yet. `omarchyvr-slam export` (the first step of every 6DoF start) still writes the Basalt files: the
//! shipped Dream Air reference calibration, Basalt's EuRoC base config built in, the Dream Air default gyro bias.
//! Before, it failed with "no calibration file found" and then "no Basalt base VIO config found".
use std::path::PathBuf;
use std::process::Command;

#[test]
fn export_works_in_a_clean_home_with_the_shipped_reference() {
    let root = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join(format!("slam-clean-home-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let home = root.join("home");
    std::fs::create_dir_all(&home).unwrap();
    let out = root.join("basalt");
    let o = Command::new(env!("CARGO_BIN_EXE_omarchyvr-slam"))
        .args(["export", "--out"])
        .arg(&out)
        .env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("OMARCHYVR_NO_MIGRATE", "1")
        .env_remove("PIMAX_CAM_CALIB")
        .env_remove("PIMAX_CALIB")
        .env_remove("PIMAX_REPO")
        .env_remove("PIMAX_SRC_DIR")
        .env_remove("PIMAX_SLAM_IMU")
        .env_remove("PIMAX_NO_REFERENCE")
        .output()
        .unwrap();
    let t = format!("{}{}", String::from_utf8_lossy(&o.stdout), String::from_utf8_lossy(&o.stderr));
    assert!(o.status.success(), "{t}");
    assert!(t.contains("built-in reference") && t.contains("(reference)"), "the shipped reference calibration: {t}");
    assert!(t.contains("built in: Basalt"), "the built-in EuRoC base: {t}");
    for f in ["pimax-basalt.toml", "pimax-basalt-calib.json", "pimax-basalt-config.json"] {
        assert!(out.join(f).is_file(), "{f}: {t}");
    }
    // the reference switched off: a plain error, as before (an own calibration is then required)
    let o = Command::new(env!("CARGO_BIN_EXE_omarchyvr-slam"))
        .args(["export", "--out"])
        .arg(root.join("basalt-off"))
        .env("HOME", &home)
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("OMARCHYVR_NO_MIGRATE", "1")
        .env_remove("PIMAX_CAM_CALIB")
        .env_remove("PIMAX_CALIB")
        .env_remove("PIMAX_REPO")
        .env("PIMAX_NO_REFERENCE", "1")
        .output()
        .unwrap();
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).contains("no calibration file found"));
    let _ = std::fs::remove_dir_all(&root);
}
