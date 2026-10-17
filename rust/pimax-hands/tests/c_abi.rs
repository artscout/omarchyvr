//! C-ABI compatibility: the REAL `include/pimax_hands.h` is compiled against the Rust staticlib, the way the Monado
//! driver links it.
//!  * struct layouts and constants equal what a C compiler computes from the header
//!  * a C program drives a hand through the exported functions
use pimax_hands::ffi::*;
use pimax_hands::flags;
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn manifest_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}
fn profile_dir() -> PathBuf {
    // target/<profile>/deps/c_abi-xxxx -> target/<profile>
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn tmp() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("hands-c");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// `cargo test` only builds the rlib; make sure the staticlib exists.
fn build_staticlib() -> PathBuf {
    static ONCE: Once = Once::new();
    let dir = profile_dir();
    ONCE.call_once(|| {
        let mut c = Command::new(env!("CARGO"));
        // the target dir this test runs from, absolute: a relative CARGO_TARGET_DIR in the caller's environment would
        // otherwise resolve against this crate's directory (a stray <crate>/target-clean and a stale library)
        c.env("CARGO_TARGET_DIR", std::env::current_exe().unwrap().ancestors().nth(3).unwrap());
        c.args(["build", "-p", "pimax-hands", "--manifest-path"])
            .arg(manifest_dir().join("Cargo.toml"));
        if dir.file_name().unwrap() == "release" {
            c.arg("--release");
        }
        assert!(c.status().unwrap().success(), "cargo build pimax-hands");
    });
    dir.join("libpimax_hands.a")
}

fn cc(args: &[&str]) {
    let o = Command::new("cc").args(args).output().expect("cc");
    assert!(
        o.status.success(),
        "cc {args:?}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

fn inc() -> String {
    format!("-I{}", manifest_dir().join("include").display())
}

#[test]
fn struct_layout_matches_the_c_header() {
    let exe = tmp().join("layout");
    cc(&[
        "-Wall",
        "-Werror",
        &inc(),
        manifest_dir().join("tests/c/layout.c").to_str().unwrap(),
        "-o",
        exe.to_str().unwrap(),
    ]);
    let c = String::from_utf8(Command::new(&exe).output().unwrap().stdout).unwrap();

    macro_rules! o {
        ($t:ty; $($f:ident),+) => {{ let mut s = String::new(); $( s += &format!(" {}={}", stringify!($f), offset_of!($t, $f)); )+ s }};
    }
    let mut r = String::new();
    r += &format!(
        "input size={} align={}",
        size_of::<pimax_hands_input>(),
        align_of::<pimax_hands_input>()
    );
    r += &o!(pimax_hands_input; trigger, grip, stick_x, stick_y, flags);
    r += &format!(
        "\nfinger size={} align={}",
        size_of::<pimax_hands_finger>(),
        align_of::<pimax_hands_finger>()
    );
    r += &o!(pimax_hands_finger; splay, joint_curls);
    r += &format!(
        "\nout size={} align={}",
        size_of::<pimax_hands_out>(),
        align_of::<pimax_hands_out>()
    );
    r += &o!(pimax_hands_out; little, ring, middle, index, thumb, curl, thumb_state);
    r += &format!(
        "\npose size={} align={}",
        size_of::<pimax_hands_pose>(),
        align_of::<pimax_hands_pose>()
    );
    r += &o!(pimax_hands_pose; orientation, position);
    r += &format!(
        "\nconsts abi={} TB1={:#x} TB2={:#x} TS={:#x} CB1={:#x} CB2={:#x} CS={:#x} CM={:#x} th=0,1,2,3,4\n",
        PIMAX_HANDS_ABI_VERSION,
        flags::TOUCH_BTN1,
        flags::TOUCH_BTN2,
        flags::TOUCH_STICK,
        flags::CLICK_BTN1,
        flags::CLICK_BTN2,
        flags::CLICK_STICK,
        flags::CLICK_MENU
    );
    assert_eq!(c, r);
}

#[test]
fn c_program_drives_a_hand_through_the_staticlib() {
    let lib = build_staticlib();
    assert!(lib.exists(), "{lib:?}");
    let exe = tmp().join("session");
    cc(&[
        "-Wall",
        "-Werror",
        &inc(),
        manifest_dir().join("tests/c/session.c").to_str().unwrap(),
        lib.to_str().unwrap(),
        // rustc --print native-static-libs for the staticlib
        "-lgcc_s",
        "-lutil",
        "-lrt",
        "-lpthread",
        "-lm",
        "-ldl",
        "-lc",
        "-o",
        exe.to_str().unwrap(),
    ]);
    let o = Command::new(&exe).output().unwrap();
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(o.status.success() && out.trim() == "PASS", "{out}");
}

#[test]
fn header_declares_every_exported_function() {
    let h = std::fs::read_to_string(manifest_dir().join("include/pimax_hands.h")).unwrap();
    for f in [
        "pimax_hands_abi_version",
        "pimax_hands_create",
        "pimax_hands_destroy",
        "pimax_hands_reset",
        "pimax_hands_update",
        "pimax_hands_update_report",
        "pimax_hands_get",
        "pimax_hands_grip_to_wrist",
        "pimax_hands_demo_input",
    ] {
        assert!(h.contains(&format!("{f}(")), "header lacks {f}");
    }
    // and the staticlib exports exactly those (nm)
    let lib = build_staticlib();
    let nm = Command::new("nm")
        .args(["-g", "--defined-only"])
        .arg(Path::new(&lib))
        .output()
        .unwrap();
    let syms = String::from_utf8_lossy(&nm.stdout);
    for f in [
        "pimax_hands_create",
        "pimax_hands_update",
        "pimax_hands_get",
        "pimax_hands_grip_to_wrist",
    ] {
        assert!(
            syms.lines().any(|l| l.ends_with(&format!(" T {f}"))),
            "{f} not exported"
        );
    }
}
