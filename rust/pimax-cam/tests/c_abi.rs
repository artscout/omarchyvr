//! The C header `include/pimax_cam.h` against the Rust library: struct layouts as computed by a C compiler, and a
//! C program driving calibration + replay through the staticlib.
use pimax_cam::camera::{Camera, FrameInfo};
use pimax_cam::capi::{CCalib, CCamera, CKb4, CStats, OpenParams};
use pimax_cam::clock::ImuClockRef;
use pimax_cam::source::ReplayOptions;
use std::mem::{offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;

fn here() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn profile_dir() -> PathBuf {
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// `cargo test` only builds the rlib: build the staticlib of the same profile.
fn staticlib() -> PathBuf {
    let dir = profile_dir();
    let mut c = Command::new(env!("CARGO"));
    // the target dir this test runs from, absolute: a relative CARGO_TARGET_DIR in the caller's environment would
    // otherwise resolve against this crate's directory (a stray <crate>/target-clean and a stale library)
    c.env("CARGO_TARGET_DIR", std::env::current_exe().unwrap().ancestors().nth(3).unwrap());
    c.args(["build", "-p", "pimax-cam", "--lib", "--manifest-path"])
        .arg(here().join("Cargo.toml"));
    if dir.file_name().unwrap() == "release" {
        c.arg("--release");
    }
    assert!(c.status().unwrap().success(), "cargo build pimax-cam");
    dir.join("libpimax_cam.a")
}

fn compile(out: &Path, lib: Option<&Path>) {
    let mut args: Vec<String> = vec![
        "-std=c11".into(),
        "-Wall".into(),
        "-Werror".into(),
        "-o".into(),
        out.to_string_lossy().into(),
        here().join("tests/c/abi.c").to_string_lossy().into(),
        "-I".into(),
        here().join("include").to_string_lossy().into(),
    ];
    if let Some(l) = lib {
        args.push(l.to_string_lossy().into());
        for x in ["-lpthread", "-ldl", "-lm", "-lrt", "-lgcc_s"] {
            args.push(x.into());
        }
    } else {
        args.push("-DLAYOUT_ONLY".into());
    }
    let o = Command::new("cc").args(&args).output().expect("cc");
    assert!(
        o.status.success(),
        "cc {args:?}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn rust_layout() -> String {
    let mut s = String::new();
    s += &format!(
        "frame_info {} {} {} {} {} {} {} {} {} {}\n",
        size_of::<FrameInfo>(),
        offset_of!(FrameInfo, host_ts_ns),
        offset_of!(FrameInfo, dev_ts_us),
        offset_of!(FrameInfo, mono_ts_ns),
        offset_of!(FrameInfo, counter),
        offset_of!(FrameInfo, kind),
        offset_of!(FrameInfo, exposure),
        offset_of!(FrameInfo, y_offset),
        offset_of!(FrameInfo, tile_width),
        offset_of!(FrameInfo, flags)
    );
    s += &format!(
        "stats {} {} {}\n",
        size_of::<CStats>(),
        offset_of!(CStats, kind_repeats),
        offset_of!(CStats, clock_offset_ns)
    );
    s += &format!(
        "clockref {} {} {}\n",
        size_of::<ImuClockRef>(),
        offset_of!(ImuClockRef, dev_ext_us),
        offset_of!(ImuClockRef, offset_ns)
    );
    s += &format!(
        "kb4 {} {} {} {}\n",
        size_of::<CKb4>(),
        offset_of!(CKb4, fx),
        offset_of!(CKb4, k),
        offset_of!(CKb4, extra)
    );
    s += &format!(
        "camera {} {} {} {} {} {}\n",
        size_of::<CCamera>(),
        offset_of!(CCamera, id),
        offset_of!(CCamera, intr),
        offset_of!(CCamera, rig_rotation),
        offset_of!(CCamera, rig_translation),
        offset_of!(CCamera, time_delta_s)
    );
    s += &format!(
        "calib {} {} {} {} {} {}\n",
        size_of::<CCalib>(),
        offset_of!(CCalib, cameras),
        offset_of!(CCalib, ombc),
        offset_of!(CCalib, tbc),
        offset_of!(CCalib, imu_cam_delta_s),
        offset_of!(CCalib, device_uid)
    );
    s += &format!("open_params {}\n", size_of::<OpenParams>());
    s
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn header_layout_and_c_program() {
    let tmp = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("pimax-cam-c");
    std::fs::create_dir_all(&tmp).unwrap();
    let exe = tmp.join("abi");
    compile(&exe, Some(&staticlib()));
    let calib = here().join("tests/fixtures/headset-calibration.xml");
    let clip = here().join("tests/fixtures/evening2.pxcam");
    let o = Command::new(&exe).arg(&calib).arg(&clip).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout).into_owned();
    assert!(
        o.status.success(),
        "{out}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
    // 1. layouts identical
    let layout: String = out.lines().take(7).map(|l| format!("{l}\n")).collect();
    assert_eq!(layout, rust_layout());
    // 2. calibration through C
    assert!(
        out.contains("calib 4 P511X0X000000000002 trackingB 232.5652236825 body1 0.1053"),
        "{out}"
    );
    // 3. replay: same tiles as the Rust API
    let mut cam = Camera::open(clip.to_str().unwrap(), 0, ReplayOptions::default()).unwrap();
    let mut n = 0;
    while let Some(i) = cam.next_frame(0).unwrap() {
        let sums: Vec<u64> = (0..4)
            .map(|t| cam.tile(t).unwrap().iter().map(|&b| b as u64).sum())
            .collect();
        let line = format!(
            "frame {n} ctr {} kind {} rows {} {} {} {} sums {} {} {} {}",
            i.counter,
            i.kind,
            i.y_offset[0],
            i.y_offset[1],
            i.y_offset[2],
            i.y_offset[3],
            sums[0],
            sums[1],
            sums[2],
            sums[3]
        );
        assert!(out.contains(&line), "missing {line:?} in\n{out}");
        n += 1;
    }
    assert_eq!(n, 2);
    assert!(
        out.contains("end -1: end of replay; frames 2 tracking 1 led 1 mismatch 0"),
        "{out}"
    );
}
