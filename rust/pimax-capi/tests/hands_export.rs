//! `libpimax_capi` also carries the controller hand-tracking C ABI of `pimax-hands` (include/pimax_hands.h):
//! Monado links exactly one Rust staticlib (two would clash on the bundled std), so the driver gets `pimax_hands_*`
//! from `libpimax_capi.a`. Every function of the hands header must be exported by the cdylib and the staticlib.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn profile_dir() -> PathBuf {
    // target/<profile>/deps/hands_export-xxxx -> target/<profile>
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

/// `cargo test` only builds the rlib; make sure the cdylib and the staticlib exist.
fn build_libs() -> PathBuf {
    static ONCE: Once = Once::new();
    let dir = profile_dir();
    ONCE.call_once(|| {
        let mut c = Command::new(env!("CARGO"));
        // the target dir this test runs from, absolute: a relative CARGO_TARGET_DIR in the caller's environment would
        // otherwise resolve against this crate's directory (a stray <crate>/target-clean and a stale library)
        c.env("CARGO_TARGET_DIR", std::env::current_exe().unwrap().ancestors().nth(3).unwrap());
        c.args(["build", "-p", "pimax-capi", "--manifest-path"])
            .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"));
        if dir.file_name().unwrap() == "release" {
            c.arg("--release");
        }
        assert!(c.status().unwrap().success(), "cargo build pimax-capi");
    });
    dir
}

fn nm(args: &[&str], lib: &Path) -> String {
    let o = Command::new("nm").args(args).arg(lib).output().expect("nm");
    String::from_utf8(o.stdout).unwrap()
}

/// Function names declared at column 0 of the header (`type name(...)`).
fn header_functions() -> Vec<String> {
    let h = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pimax-hands/include/pimax_hands.h"),
    )
    .unwrap();
    let mut names = vec![];
    for line in h.lines() {
        if line.starts_with(|c: char| {
            c.is_whitespace() || c == '#' || c == '/' || c == '*' || c == '}'
        }) || line.starts_with("typedef")
            || !line.contains('(')
        {
            continue;
        }
        let name = line
            .split('(')
            .next()
            .unwrap()
            .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
            .next()
            .unwrap();
        if name.starts_with("pimax_hands_") {
            names.push(name.to_string());
        }
    }
    names
}

#[test]
fn every_hands_header_function_is_exported_by_cdylib_and_staticlib() {
    let names = header_functions();
    assert_eq!(names.len(), 13, "{names:?}");
    let dir = build_libs();
    let so = nm(&["-D", "--defined-only"], &dir.join("libpimax_capi.so"));
    let a = nm(&["-g", "--defined-only"], &dir.join("libpimax_capi.a"));
    for n in &names {
        assert!(
            so.lines().any(|l| l.ends_with(&format!(" T {n}"))),
            "{n} missing from libpimax_capi.so"
        );
        assert!(
            a.lines().any(|l| l.ends_with(&format!(" T {n}"))),
            "{n} missing from libpimax_capi.a"
        );
    }
}

#[test]
fn the_hands_abi_version_is_reachable_through_the_re_export() {
    // Same constant on both sides: a stale libpimax_capi.a would make the Monado driver switch hand tracking off.
    assert_eq!(
        pimax_capi::pimax_hands::ffi::pimax_hands_abi_version(),
        pimax_capi::pimax_hands::ffi::PIMAX_HANDS_ABI_VERSION
    );
}
