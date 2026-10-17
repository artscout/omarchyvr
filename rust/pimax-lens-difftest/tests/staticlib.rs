//! The staticlib the Monado driver links (`libpimax_lens.a`) exports exactly the ABI of `include/pimax_lens_rs.h`:
//! a C program built only from the header is linked against it and its output is compared with the Rust API.
use pimax_lens::{load_file, mask};
use pimax_lens_difftest::ASPECT;
use std::path::{Path, PathBuf};
use std::process::Command;

fn crate_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../pimax-lens")
}

fn profile_dir() -> PathBuf {
    // target/<profile>/deps/staticlib-xxxx -> target/<profile>
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}

fn tmp() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("lens-staticlib");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// cargo test only builds the rlib; build the staticlib too.
fn build_staticlib() -> PathBuf {
    let dir = profile_dir();
    let mut c = Command::new(env!("CARGO"));
    c.args(["build", "-p", "pimax-lens", "--manifest-path"])
        .arg(crate_dir().join("Cargo.toml"));
    if dir.file_name().unwrap() == "release" {
        c.arg("--release");
    }
    assert!(c.status().unwrap().success(), "cargo build pimax-lens");
    let a = dir.join("libpimax_lens.a");
    assert!(a.exists(), "{}", a.display());
    a
}

fn build_abi_check(lib: &Path) -> PathBuf {
    let out = tmp().join("abi_check");
    let o = Command::new("cc")
        .args(["-std=gnu11", "-Wall", "-Werror", "-o"])
        .arg(&out)
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("csrc/abi_check.c"))
        .arg("-I")
        .arg(crate_dir().join("include"))
        .arg(lib)
        .args(["-lpthread", "-ldl", "-lm", "-lgcc_s"])
        .output()
        .expect("cc");
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    out
}

fn run(prog: &Path, conf: &Path) -> String {
    let o = Command::new(prog)
        .env("PIMAX_LENS_CONF", conf)
        // compare on the code-default baseline (`load_file`), not the shipped Windows fit the driver layers files on
        .env("PIMAX_NO_REFERENCE", "1")
        .env_remove("PIMAX_LENS_OFF")
        .env_remove("PIMAX_LENS_DEBUG_GRID")
        .output()
        .unwrap();
    assert!(
        o.status.success(),
        "exit {:?}\n{}",
        o.status.code(),
        String::from_utf8_lossy(&o.stdout)
    );
    String::from_utf8(o.stdout).unwrap()
}

/// Library and C test program, built once for all tests of this file.
fn built() -> &'static (PathBuf, PathBuf) {
    static B: std::sync::OnceLock<(PathBuf, PathBuf)> = std::sync::OnceLock::new();
    B.get_or_init(|| {
        let lib = build_staticlib();
        let prog = build_abi_check(&lib);
        (lib, prog)
    })
}

fn check(conf_text: &str, name: &str, expect_mask: bool) {
    let prog = &built().1;
    let conf = tmp().join(name);
    std::fs::write(&conf, conf_text).unwrap();
    let out = run(prog, &conf);
    let loaded = load_file(ASPECT, &|_| None, conf.clone());
    let lens = &loaded.lens;
    let mut meshes = 0;
    for line in out.lines() {
        let f: Vec<&str> = line.split_whitespace().collect();
        match f[0] {
            "fov" => {
                let view: usize = f[1].parse().unwrap();
                let got: Vec<f32> = f[2..6].iter().map(|s| s.parse().unwrap()).collect();
                assert_eq!(got, lens.fov_deg(view).to_vec(), "{line}");
            }
            "uv" => {
                let view: usize = f[1].parse().unwrap();
                let (u, v): (f32, f32) = (f[2].parse().unwrap(), f[3].parse().unwrap());
                let got: Vec<u32> = f[4..10]
                    .iter()
                    .map(|s| s.parse::<f32>().unwrap().to_bits())
                    .collect();
                let w = lens.compute(view, u, v);
                let want: Vec<u32> = w
                    .iter()
                    .flat_map(|c| c.iter().map(|x| x.to_bits()))
                    .collect();
                assert_eq!(got, want, "{line}");
            }
            "mask" => assert_eq!(f[2] == "1", expect_mask, "{line}"),
            "meshcounts" => {
                let view: usize = f[1].parse().unwrap();
                let kind = mask::MaskKind::from_i32(f[2].parse().unwrap()).unwrap();
                let m = mask::visibility_mesh(lens, view, kind, 64).unwrap();
                assert_eq!(
                    (
                        f[3].parse::<usize>().unwrap(),
                        f[4].parse::<usize>().unwrap()
                    ),
                    (m.vertices.len(), m.indices.len()),
                    "{line}"
                );
                meshes += 1;
            }
            _ => {}
        }
    }
    assert_eq!(meshes, if expect_mask { 6 } else { 0 });
    let logged: Vec<&str> = out.lines().filter(|l| l.starts_with("log ")).collect();
    assert_eq!(logged.len(), loaded.messages.len(), "{out}");
    assert!(out.contains("done "), "{out}");
}

#[test]
fn staticlib_matches_the_rust_api_for_the_default_config() {
    check("", "empty.conf", false);
}

#[test]
fn staticlib_matches_the_rust_api_with_all_new_features() {
    check(
        "k_r = 0.6 -0.7 0.58 0.05 -0.01\nbase_b = 0.64\nshift_r = 0.004 -0.002\nfit_range = 1.3\nblackout = 0.02 0.03 0.04 0.05\nmask = ellipse 1.05 0.95\nmask_feather = 0.04\n",
        "features.conf",
        true,
    );
}

#[test]
fn every_function_of_the_header_is_exported_by_the_staticlib() {
    let lib = &built().0;
    let header = std::fs::read_to_string(crate_dir().join("include/pimax_lens_rs.h")).unwrap();
    let mut names: Vec<String> = header
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| w.starts_with("pimax_lens_rs_") && !w.ends_with("_fn") && !w.ends_with("_kind"))
        .map(str::to_string)
        .collect();
    names.sort();
    names.dedup();
    // the type names of the header (pimax_lens_rs, ..._mask_kind enumerators in upper case) are not functions
    names.retain(|n| {
        n.chars()
            .all(|c| c.is_ascii_lowercase() || c == '_' || c.is_ascii_digit())
    });
    assert!(names.len() >= 6, "{names:?}");
    let nm = Command::new("nm")
        .arg("-g")
        .arg("--defined-only")
        .arg(lib)
        .output()
        .expect("nm");
    let syms = String::from_utf8_lossy(&nm.stdout);
    for n in names {
        assert!(
            syms.lines().any(|l| l.ends_with(&format!(" T {n}"))),
            "{n} is declared in the header but not exported"
        );
    }
}
