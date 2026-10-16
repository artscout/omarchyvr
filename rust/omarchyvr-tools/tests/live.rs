//! Live checks of the `omarchyvr` binary against the real headset (read-only, next to the running service).
//!     cargo test -p omarchyvr-tools --test live -- --ignored --test-threads=1 --nocapture
use std::path::PathBuf;
use std::process::Command;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn pimax(args: &[&str]) -> (bool, String, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_omarchyvr"))
        .args(args)
        .current_dir(root())
        .output()
        .unwrap();
    (
        o.status.success(),
        String::from_utf8_lossy(&o.stdout).into_owned(),
        String::from_utf8_lossy(&o.stderr).into_owned(),
    )
}

#[test]
#[ignore]
fn live_info() {
    let (ok, out, err) = pimax(&["info"]);
    println!("{out}{err}");
    assert!(ok);
    assert!(out.contains("serial: P511"), "{out}");
    assert!(out.contains("IF3: /dev/hidraw"));
}

#[test]
#[ignore]
fn live_calib_identical_to_windows() {
    let out = std::env::temp_dir().join("pimax-live-calib.xml");
    let (ok, so, _) = pimax(&["calib", out.to_str().unwrap()]);
    println!("{so}");
    assert!(ok && so.contains("IDENTICAL to analysis/headset-calibration.xml"));
}

#[test]
#[ignore]
fn live_imu_json_and_text() {
    let (ok, out, _) = pimax(&["imu", "--json", "--rate", "50", "--seconds", "2"]);
    assert!(ok);
    let n = out.lines().filter(|l| l.starts_with("{\"t_us\"")).count();
    println!("json lines in 2 s @50 Hz: {n}");
    assert!((70..=110).contains(&n), "{n}");
    let (ok, out, _) = pimax(&["imu", "--seconds", "1", "--rate", "5", "--axis-mode", "0"]);
    assert!(
        ok && out.contains("yaw=") && out.contains("pitch="),
        "{out}"
    );
    let (ok, out, _) = pimax(&["imu", "--still", "2"]);
    assert!(ok && out.contains("accel mean (g)"), "{out}");
}

#[test]
#[ignore]
fn live_controllers_and_logtail() {
    let (ok, out, _) = pimax(&["controllers", "3"]);
    println!(
        "{}",
        out.lines().rev().take(3).collect::<Vec<_>>().join("\n")
    );
    assert!(ok && out.contains("done: reports L="));
    let (ok, _, _) = pimax(&["logtail", "1"]);
    assert!(ok);
    let (ok, out, _) = pimax(&["probe", "3", "--passive"]);
    assert!(ok && out.contains("done:") && out.contains("IMU"), "{out}");
}
