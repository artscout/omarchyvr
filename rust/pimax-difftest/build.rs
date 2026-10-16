include!("../lab_data_build.rs");

// Compiles the ORIGINAL C library (libpimax/src/*.c, untouched) into this test crate with every public symbol
// renamed to c_<name>, so it can sit next to the Rust implementation in one binary.
const SYMS: &[&str] = &[
    "pimax_find_interfaces",
    "pimax_open",
    "pimax_close",
    "pimax_is_connected",
    "pimax_wait_connected",
    "pimax_set_video_rate",
    "pimax_send",
    "pimax_request",
    "pimax_read_serial",
    "pimax_read_version",
    "pimax_controller_attach",
    "pimax_video_prepare",
    "pimax_decode_imu",
    "pimax_decode_controller",
];

fn main() {
    lab_data_cfg();
    let lib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../libpimax");
    let mut b = cc::Build::new();
    b.file(lib.join("src/pimax.c"))
        .file(lib.join("src/decode.c"))
        .file("csrc/shim.c")
        .include(lib.join("include"))
        .opt_level(2)
        .flag("-pthread")
        .warnings(false);
    for s in SYMS {
        b.define(s, format!("c_{s}").as_str());
    }
    b.compile("cpimax");
    println!("cargo:rerun-if-changed=csrc/shim.c");
    println!("cargo:rerun-if-changed={}", lib.join("src").display());
    println!(
        "cargo:rerun-if-changed={}",
        lib.join("include/pimax.h").display()
    );
    println!("cargo:rustc-link-lib=pthread");
}
