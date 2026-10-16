//! Live tests: they run the real client against a RUNNING Monado (OpenXR runtime) and are ignored by default.
//! They touch the headset's display path, so run them under the shared lock and with a monado-service you started:
//!
//! ```text
//! # null compositor: no display at all (use analysis/tools/run_lens_test.sh for direct mode). `flock -o`: the sleep
//! # that feeds monado-service must not inherit the lock descriptor, and it is killed by PID at the end.
//! flock -o -w 3600 /tmp/omarchyvr-headset.lock bash -c '
//!   export XR_RUNTIME_JSON=$HOME/src/monado/build/openxr_monado-dev.json IPC_IGNORE_VERSION=1
//!   export XRT_COMPOSITOR_NULL=1 PIMAX_NO_DEVICE=1
//!   sleep 120 | $HOME/src/monado/build/src/xrt/targets/service/monado-service >/tmp/m.log 2>&1 &
//!   SVC=$!; SLEEP=$(jobs -p | tail -1); sleep 4
//!   (cd ~/omarchyvr/rust && cargo test -p pimax-lenstest --test live -- --ignored --test-threads=1 --nocapture)
//!   kill -INT $SVC; sleep 3; kill $SLEEP 2>/dev/null'
//! ```
use std::process::{Command, Stdio};

fn client() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pimax-lenstest"));
    c.stdin(Stdio::null());
    c
}

#[test]
#[ignore = "needs a running monado-service"]
fn client_steps_through_every_pattern_and_exits_cleanly() {
    let o = client()
        .args([
            "--auto",
            "0.5",
            "--seconds",
            "14",
            "--res-div",
            "4",
            "--no-hud",
        ])
        .output()
        .expect("run pimax-lenstest");
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "exit {:?}\nstdout:\n{out}\nstderr:\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stderr)
    );
    for want in [
        "=== pimax-lenstest: active configuration ===",
        "views: 2",
        "PATTERN [1/7] STRAIGHT-LINE GRID",
        "PATTERN [2/7] ANGLE RINGS",
        "step 4/4 BLUE ONLY",
        "PATTERN [7/7] CORNER FRAMES",
        "session state: EXITING",
    ] {
        assert!(out.contains(want), "missing {want:?} in\n{out}");
    }
}

#[test]
#[ignore = "needs a running monado-service"]
fn eye_select_and_square_direction_options_are_accepted() {
    for args in [
        ["--eye", "left", "--pattern", "square25"],
        ["--eye", "right", "--pattern", "corners"],
    ] {
        let o = client()
            .args(args)
            .args(["--seconds", "4", "--res-div", "8", "--square-dir", "up"])
            .output()
            .expect("run");
        assert!(
            o.status.success(),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
        let out = String::from_utf8_lossy(&o.stdout);
        assert!(out.contains(&format!("| eye {} |", args[1])), "{out}");
    }
}

#[test]
#[ignore = "needs a running monado-service"]
fn without_a_runtime_the_error_is_clear() {
    let o = client()
        .env("XR_RUNTIME_JSON", "/nonexistent/runtime.json")
        .args(["--seconds", "1"])
        .output()
        .expect("run");
    assert!(!o.status.success());
    let err = String::from_utf8_lossy(&o.stderr);
    assert!(err.contains("pimax-lenstest: error:"), "{err}");
}
