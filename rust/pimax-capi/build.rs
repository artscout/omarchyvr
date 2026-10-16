include!("../lab_data_build.rs");

fn main() {
    lab_data_cfg();
    // Same soname as the C library so the shared object can be installed as libpimax.so.0
    println!("cargo:rustc-cdylib-link-arg=-Wl,-soname,libpimax.so.0");
}
