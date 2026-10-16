//! Live checks of `pimax vr` against the real machine. They are `#[ignore]`d: the second one starts the real
//! monado-service and lights the panels. Run them under the headset lock when other agents are active:
//!     cargo test -p pimax-tools --test vr_live -- --ignored --test-threads=1 --nocapture
//!     flock -w 3600 /tmp/pimax-headset.lock cargo test -p pimax-tools --test vr_live -- --ignored --test-threads=1 --nocapture
//! `PIMAX_LIVE_FPS=90` selects the refresh rate of the cycle test (default 72); `PIMAX_MONADO_BUILD` the Monado build.
use std::process::Command;

fn pimax(args: &[&str]) -> (Option<i32>, String) {
    let o = Command::new(env!("CARGO_BIN_EXE_pimax"))
        .arg("vr")
        .args(args)
        .env("PIMAX_NO_NOTIFY", "1")
        .output()
        .unwrap();
    (
        o.status.code(),
        format!(
            "{}{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        ),
    )
}

#[test]
#[ignore]
fn live_dry_run_checks_the_real_machine_without_starting_anything() {
    let (rc, out) = pimax(&["start", "--dry-run"]);
    println!("{out}");
    for name in [
        "headset",
        "keepalive",
        "display",
        "kernel",
        "service",
        "wayland",
    ] {
        assert!(out.contains(name), "{name} missing in the report");
    }
    // on a healthy idle machine the preflight passes; a running service or an Xid makes it fail on purpose
    if out.contains("[FAIL]") {
        assert_eq!(rc, Some(1));
    } else {
        assert_eq!(rc, Some(0));
        assert!(
            out.contains("XRT_COMPOSITOR_DESIRED_MODE=2"),
            "default is 72 Hz"
        );
    }
}

#[test]
#[ignore]
fn live_start_status_stop_cycle() {
    let fps = std::env::var("PIMAX_LIVE_FPS").unwrap_or_else(|_| "72".into());
    let (rc, out) = pimax(&["start", "--fps", &fps]);
    println!("{out}");
    assert_eq!(rc, Some(0), "start failed:\n{out}");
    assert!(out.contains("gyro calibration done"), "{out}");
    assert!(out.contains(&format!("{fps} Hz announced (rc 0)")), "{out}");

    let (rc, out) = pimax(&["status"]);
    println!("{out}");
    assert_eq!(rc, Some(0), "service should be running:\n{out}");
    assert!(
        out.contains(&format!("rate: {fps} Hz (PIMAX_FPS)")),
        "{out}"
    );

    std::thread::sleep(std::time::Duration::from_secs(3));
    let (rc, out) = pimax(&["stop"]);
    println!("{out}");
    assert_eq!(rc, Some(0), "stop failed:\n{out}");
    assert!(
        out.contains("exited cleanly"),
        "SIGINT must be enough:\n{out}"
    );
    assert!(out.contains("no Xid / display engine errors"), "{out}");

    let (rc, _) = pimax(&["status"]);
    assert_eq!(rc, Some(3), "service must be gone");
}
