// SPDX-License-Identifier: MIT OR Apache-2.0
// Shared by the build scripts (`include!("../lab_data_build.rs")`): sets `cfg(lab_data)` when this checkout has the
// private lab data that some tests replay: the factory calibration of the lab headset, the camera recording of the
// room and the eye-chip USB transcript. The public tree (tools/export-public.sh) has synthetic stand-ins instead, and
// the tests that need the real data are ignored there with `#[cfg_attr(not(lab_data), ignore = "...")]`.
fn lab_data_cfg() {
    println!("cargo::rustc-check-cfg=cfg(lab_data)");
    let m = std::path::PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default());
    let root = m.join("../..");
    let marks = [
        root.join("analysis/headset-calibration.xml"),
        root.join("rust/pimax-cam/tests/fixtures/evening2.pxcam"),
        root.join("rust/pimax-eye/fixtures/eye_coldstart_init.usbpcap.gz"),
    ];
    for d in [root.join("analysis"), root.join("rust/pimax-cam/tests/fixtures"), root.join("rust/pimax-eye/fixtures")] {
        if d.is_dir() {
            println!("cargo::rerun-if-changed={}", d.display());
        }
    }
    if marks.iter().all(|p| p.is_file()) {
        println!("cargo::rustc-cfg=lab_data");
    }
}
