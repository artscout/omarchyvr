// Compiles the ORIGINAL C lens model (csrc/old/pimax_lens.c, byte for byte the file of Monado commit 1da3626, the
// code that ran in the headset before the Rust port) with its three public symbols renamed to c_<name>, against
// tiny stand-ins for the Monado headers (csrc/shim), so it can sit next to the Rust implementation in one binary.
// -ffp-contract=off: the exact float operation sequence of an x86-64 build without FMA, whatever CFLAGS say.
use std::path::PathBuf;

/// The thin C glue of the Monado driver (`pimax_lens.c` with its header): `PIMAX_LENS_GLUE_DIR`, else the pimax driver
/// directory of ~/src/monado-lens or ~/src/monado, as long as it is the new glue that calls `pimax_lens_rs_create`.
fn glue_dir() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(d) = std::env::var("PIMAX_LENS_GLUE_DIR") {
        candidates.push(d.into());
    }
    if let Ok(home) = std::env::var("HOME") {
        for tree in ["monado-lens", "monado"] {
            candidates.push(
                PathBuf::from(&home)
                    .join("src")
                    .join(tree)
                    .join("src/xrt/drivers/pimax"),
            );
        }
    }
    candidates.into_iter().find(|d| {
        std::fs::read_to_string(d.join("pimax_lens.c"))
            .is_ok_and(|s| s.contains("pimax_lens_rs_create"))
    })
}

fn main() {
    println!("cargo:rustc-check-cfg=cfg(have_glue)");
    println!("cargo:rerun-if-env-changed=PIMAX_LENS_GLUE_DIR");
    if let Some(dir) = glue_dir() {
        let mut g = cc::Build::new();
        g.file(dir.join("pimax_lens.c"))
            .include(&dir)
            .include("csrc/shim")
            .include("../pimax-lens/include")
            .opt_level(2)
            .flag("-ffp-contract=off")
            .warnings(false);
        for s in [
            "pimax_lens_load",
            "pimax_lens_destroy",
            "pimax_lens_get_fov",
            "pimax_lens_compute",
            "pimax_lens_visibility_mask",
        ] {
            g.define(s, format!("glue_{s}").as_str());
        }
        g.compile("cglue");
        println!("cargo:rustc-cfg=have_glue");
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("pimax_lens.c").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("pimax_lens.h").display()
        );
        println!(
            "cargo:rustc-env=PIMAX_LENS_GLUE_FILE={}",
            dir.join("pimax_lens.c").display()
        );
    }
    let mut b = cc::Build::new();
    b.file("csrc/old/pimax_lens.c")
        .file("csrc/shim.c")
        .include("csrc/old")
        .include("csrc/shim")
        .opt_level(2)
        .flag("-ffp-contract=off")
        .warnings(false);
    for s in [
        "pimax_lens_load",
        "pimax_lens_get_fov",
        "pimax_lens_compute",
    ] {
        b.define(s, format!("c_{s}").as_str());
    }
    b.compile("clens");
    println!("cargo:rerun-if-changed=csrc");
    println!("cargo:rustc-link-lib=m");
}
