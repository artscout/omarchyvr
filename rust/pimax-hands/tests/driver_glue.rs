//! The real Monado driver glue (`pimax_ctrl.c`, compiled object taken from a Monado build tree) driven in-process with
//! synthetic controller reports; see `tests/c/driver_glue.c`. Needs a configured and built Monado tree that contains the
//! hands patch (`MONADO_BUILD`, else `~/src/monado-hands/build-hands`, else `~/src/monado/build`), takes about 7 s of real
//! time (the filters run on the wall clock), touches no hardware. Run with `-- --ignored`.
use std::path::{Path, PathBuf};
use std::process::Command;

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap())
}

/// A Monado build dir whose driver object already contains the hand tracking glue.
fn monado_build() -> Option<PathBuf> {
    let mut cands: Vec<PathBuf> = std::env::var("MONADO_BUILD")
        .map(PathBuf::from)
        .into_iter()
        .collect();
    cands.push(home().join("src/monado-hands/build-hands"));
    cands.push(home().join("src/monado/build"));
    cands.into_iter().find(|b| {
        let o = b.join("src/xrt/drivers/CMakeFiles/drv_pimax.dir/pimax/pimax_ctrl.c.o");
        o.exists()
            && Command::new("nm")
                .arg("-u")
                .arg(&o)
                .output()
                .map(|n| String::from_utf8_lossy(&n.stdout).contains("pimax_hands_update_report"))
                .unwrap_or(false)
    })
}

fn cache_value(build: &Path, key: &str) -> String {
    let c = std::fs::read_to_string(build.join("CMakeCache.txt")).unwrap();
    let prefix = format!("{key}:");
    c.lines()
        .find(|l| l.starts_with(&prefix))
        .and_then(|l| l.split_once('='))
        .map(|(_, v)| v.to_string())
        .unwrap_or_else(|| panic!("{key} not in CMakeCache.txt"))
}

#[test]
#[ignore = "needs a built Monado tree with the hands patch; ~7 s of wall clock"]
fn driver_glue_turns_reports_into_the_right_joints() {
    let Some(build) = monado_build() else {
        eprintln!("no Monado build with the hands glue found (set MONADO_BUILD): skipped");
        return;
    };
    let src = PathBuf::from(cache_value(&build, "CMAKE_HOME_DIRECTORY"));
    let libpimax = PathBuf::from(cache_value(&build, "PIMAX_LIBPIMAX_DIR"));

    // The Rust staticlib the driver links in a real build: libpimax_capi.a (carries pimax_hands_*), built here.
    let profile_dir = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf();
    let mut c = Command::new(env!("CARGO"));
    // the target dir this test runs from, absolute: a relative CARGO_TARGET_DIR in the caller's environment would
    // otherwise resolve against this crate's directory (a stray <crate>/target-clean and a stale library)
    c.env("CARGO_TARGET_DIR", std::env::current_exe().unwrap().ancestors().nth(3).unwrap());
    c.args(["build", "-p", "pimax-capi"]);
    if profile_dir.file_name().unwrap() == "release" {
        c.arg("--release");
    }
    assert!(c.status().unwrap().success(), "cargo build pimax-capi");
    let capi = profile_dir.join("libpimax_capi.a");

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("driver_glue");
    let aux = |lib: &str| build.join("src/xrt/auxiliary").join(lib);
    let status = Command::new("cc")
        .args(["-O1", "-g", "-D_GNU_SOURCE"])
        .arg(format!("-I{}", src.join("src/xrt/auxiliary").display()))
        .arg(format!("-I{}", build.join("src/xrt/auxiliary").display()))
        .arg(format!("-I{}", src.join("src/xrt/include").display()))
        .arg(format!("-I{}", build.join("src/xrt/include").display()))
        .arg(format!("-I{}", src.join("src/xrt/drivers/pimax").display()))
        .arg(format!("-I{}", libpimax.join("include").display()))
        .arg(format!("-I{}", manifest.join("include").display()))
        .arg("-o")
        .arg(&out)
        .arg(manifest.join("tests/c/driver_glue.c"))
        .arg(build.join("src/xrt/drivers/CMakeFiles/drv_pimax.dir/pimax/pimax_ctrl.c.o"))
        .args([
            aux("util/libaux_util.a"),
            aux("math/libaux_math.a"),
            aux("os/libaux_os.a"),
        ])
        .args([
            aux("util/libaux_util.a"),
            aux("os/libaux_os.a"),
            aux("math/libaux_math.a"),
        ])
        .arg(&capi)
        .args([
            "-lstdc++",
            "-lm",
            "-lpthread",
            "-ldl",
            "-lgcc_s",
            "-lrt",
            "-lutil",
        ])
        .status()
        .expect("cc");
    assert!(status.success(), "linking the glue test");

    let o = Command::new(&out).output().unwrap();
    let text = String::from_utf8_lossy(&o.stdout);
    println!("{text}");
    assert!(
        o.status.success() && text.trim_end().ends_with("PASS"),
        "{text}"
    );
}
