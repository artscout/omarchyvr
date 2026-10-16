//! `libpimax_capi` also carries the eye gaze C ABI (pimax-eye/include/pimax_eye.h). Every function of the header must be
//! exported by the cdylib and the staticlib.
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Once;

fn profile_dir() -> PathBuf {
    // target/<profile>/deps/eye_export-xxxx -> target/<profile>
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
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pimax-eye/include/pimax_eye.h"),
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
        if name.starts_with("pimax_eye_") {
            names.push(name.to_string());
        }
    }
    names
}

#[test]
fn every_eye_header_function_is_exported_by_cdylib_and_staticlib() {
    let names = header_functions();
    assert_eq!(names.len(), 7, "{names:?}");
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
fn the_eye_abi_version_matches_the_header() {
    let h = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../pimax-eye/include/pimax_eye.h"),
    )
    .unwrap();
    let want = format!(
        "#define PIMAX_EYE_ABI_VERSION {}u",
        pimax_capi::eye::PIMAX_EYE_ABI_VERSION
    );
    assert!(
        h.contains(&want),
        "header and crate disagree on the ABI version"
    );
}
