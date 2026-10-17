//! C-ABI compatibility: the REAL `libpimax/include/pimax.h` is compiled against this library.
//!  * struct layout / constants identical to what a C compiler computes from the header
//!  * every symbol of the header is exported
//!  * `libpimax/tools/pimax-cli.c` builds unmodified against the cdylib and the staticlib
//!  * a C program drives a full session (callbacks, requests, errors) against a fake headset
//!  * the decoders produce byte-identical output to the original C `decode.c`
use pimax_capi::*;
use std::io::{Read, Write};
use std::mem::{align_of, offset_of, size_of};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Once;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}
fn libpimax() -> PathBuf {
    root().join("libpimax")
}
fn profile_dir() -> PathBuf {
    // target/<profile>/deps/abi-xxxx -> target/<profile>
    std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .to_path_buf()
}
fn tmp() -> PathBuf {
    let d = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("capi");
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Make sure the cdylib and the staticlib exist (cargo test only builds the rlib).
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

/// Directory with `libpimax.so` / `libpimax.so.0` pointing at the Rust cdylib: exactly how it would be installed.
fn shared_dir() -> PathBuf {
    static D: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    D.get_or_init(make_shared_dir).clone()
}

fn make_shared_dir() -> PathBuf {
    let dir = build_libs();
    let d = tmp().join("lib");
    std::fs::create_dir_all(&d).unwrap();
    for n in ["libpimax.so", "libpimax.so.0"] {
        let l = d.join(n);
        let _ = std::fs::remove_file(&l);
        std::os::unix::fs::symlink(dir.join("libpimax_capi.so"), &l).unwrap();
    }
    d
}

fn cc(args: &[&str]) {
    let o = Command::new("cc").args(args).output().expect("cc");
    assert!(
        o.status.success(),
        "cc {args:?}\n{}",
        String::from_utf8_lossy(&o.stderr)
    );
}

#[test]
fn struct_layout_matches_the_c_header() {
    let out = tmp().join("layout");
    let inc = libpimax().join("include");
    cc(&[
        "-std=c11",
        "-Wall",
        "-o",
        out.to_str().unwrap(),
        root()
            .join("rust/pimax-capi/tests/c/layout.c")
            .to_str()
            .unwrap(),
        "-I",
        inc.to_str().unwrap(),
    ]);
    let c = String::from_utf8(Command::new(&out).output().unwrap().stdout).unwrap();

    macro_rules! o { ($s:ty; $($f:ident),*) => {{ let mut v = String::new(); $( v += &format!(" {}={}", stringify!($f), offset_of!($s, $f)); )* v }}; }
    let mut r = String::new();
    r += &format!(
        "imu size={} align={}{}\n",
        size_of::<pimax_imu_sample>(),
        align_of::<pimax_imu_sample>(),
        o!(pimax_imu_sample; ts_us, raw_accel, raw_gyro, accel_g, gyro_dps, ipd_mm, sample_in_report)
    );
    r += &format!(
        "ctrl size={} align={}{}\n",
        size_of::<pimax_controller_report>(),
        align_of::<pimax_controller_report>(),
        o!(pimax_controller_report; slot, ts_us, stick_x, stick_y, trigger, grip, battery, buttons, touch, raw_accel, raw_gyro, raw)
    );
    r += &format!(
        "status size={} align={}{}\n",
        size_of::<pimax_controller_status>(),
        align_of::<pimax_controller_status>(),
        o!(pimax_controller_status; slot, battery, fw, raw)
    );
    r += &format!(
        "callbacks size={} align={}{}\n",
        size_of::<pimax_callbacks>(),
        align_of::<pimax_callbacks>(),
        o!(pimax_callbacks; user, on_state, on_imu, on_controller, on_controller_status, on_log, on_reply)
    );
    r += &format!(
        "state size={} DISCONNECTED={} CONNECTED={}\n",
        size_of::<std::os::raw::c_int>(),
        PIMAX_DISCONNECTED,
        PIMAX_CONNECTED
    );
    r += "consts VID=0x34a4 PID=0x44 NUM_IF=4 PATH_MAX=64 REPORT=64 MAXDATA=60 L=2 R=3 B=0x1 A=0x2 STICK=0x8 MENU=0x10 TSTICK=0x8\n";
    assert_eq!(c, r);
    assert_eq!(PIMAX_NUM_IF, 4);
}

#[test]
fn every_header_function_is_exported() {
    let dir = build_libs();
    let h = std::fs::read_to_string(libpimax().join("include/pimax.h")).unwrap();
    let nm = Command::new("nm")
        .args(["-D", "--defined-only"])
        .arg(dir.join("libpimax_capi.so"))
        .output()
        .unwrap();
    let syms = String::from_utf8(nm.stdout).unwrap();
    let mut n = 0;
    for line in h.lines() {
        let l = line.trim_start();
        // prototypes at column 0 look like: `type pimax_name(`
        if line.starts_with(' ')
            || !l.contains("pimax_")
            || !l.contains('(')
            || l.starts_with("typedef")
            || l.starts_with("/*")
            || l.starts_with('#')
        {
            continue;
        }
        let name = l
            .split('(')
            .next()
            .unwrap()
            .rsplit(|c: char| !(c.is_alphanumeric() || c == '_'))
            .next()
            .unwrap();
        if name.starts_with("pimax_") {
            assert!(
                syms.lines().any(|s| s.ends_with(&format!(" T {name}"))),
                "missing export {name}"
            );
            n += 1;
        }
    }
    assert_eq!(
        n, 17,
        "header declares 17 functions (14 of the C library + 3 haptics)"
    );
    // soname like the C library
    let ro = Command::new("readelf")
        .args(["-d"])
        .arg(dir.join("libpimax_capi.so"))
        .output()
        .unwrap();
    assert!(String::from_utf8_lossy(&ro.stdout).contains("libpimax.so.0"));
}

fn build_cli(dir: &Path, kind: &str) -> PathBuf {
    let out = tmp().join(format!("pimax-cli-{kind}"));
    let inc = libpimax().join("include");
    let cli = libpimax().join("tools/pimax-cli.c");
    let mut a: Vec<String> = vec![
        "-std=c11".into(),
        "-Wall".into(),
        "-o".into(),
        out.display().to_string(),
        cli.display().to_string(),
        "-I".into(),
        inc.display().to_string(),
    ];
    if kind == "shared" {
        let sd = shared_dir();
        a.extend([
            format!("-L{}", sd.display()),
            "-lpimax".into(),
            format!("-Wl,-rpath,{}", sd.display()),
            "-lm".into(),
        ]);
    } else {
        a.extend([
            dir.join("libpimax_capi.a").display().to_string(),
            "-lpthread".into(),
            "-ldl".into(),
            "-lm".into(),
            "-lgcc_s".into(),
        ]);
    }
    let refs: Vec<&str> = a.iter().map(String::as_str).collect();
    cc(&refs);
    out
}

#[test]
fn unmodified_pimax_cli_non_hardware_paths() {
    let dir = build_libs();
    let empty = tmp().join("empty-sys");
    std::fs::create_dir_all(empty.join("class/hidraw")).unwrap();
    for kind in ["shared", "static"] {
        let cli = build_cli(&dir, kind);
        // usage errors
        let o = Command::new(&cli).output().unwrap();
        assert_eq!(o.status.code(), Some(2), "{kind}: no args");
        assert!(String::from_utf8_lossy(&o.stderr).contains("usage:"));
        let o = Command::new(&cli).arg("bogus").output().unwrap();
        assert_eq!(o.status.code(), Some(2));
        assert!(String::from_utf8_lossy(&o.stderr).contains("unknown subcommand bogus"));
        if kind == "shared" {
            // no headset (hermetic: empty sysfs root): waits 10 s, then "headset not found", exit 1
            let o = Command::new(&cli)
                .arg("info")
                .env("PIMAX_SYSFS_ROOT", &empty)
                .output()
                .unwrap();
            assert_eq!(o.status.code(), Some(1));
            assert!(String::from_utf8_lossy(&o.stderr).contains("headset not found"));
        }
    }
}

#[test]
fn c_program_drives_a_full_session_against_a_fake_headset() {
    let sd = shared_dir();
    let out = tmp().join("session-test");
    cc(&[
        "-std=gnu11",
        "-Wall",
        "-o",
        out.to_str().unwrap(),
        root()
            .join("rust/pimax-capi/tests/c/session.c")
            .to_str()
            .unwrap(),
        "-I",
        libpimax().join("include").to_str().unwrap(),
        &format!("-L{}", sd.display()),
        "-lpimax",
        &format!("-Wl,-rpath,{}", sd.display()),
        "-lpthread",
    ]);
    let o = Command::new(&out).output().unwrap();
    assert!(
        o.status.success(),
        "C session test failed:\n{}\n{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(String::from_utf8_lossy(&o.stdout).contains("OK"));
}

fn records(name: &str) -> Vec<u8> {
    let f = std::fs::File::open(root().join("rust/fixtures").join(name)).unwrap();
    let mut d = Vec::new();
    flate2::read::GzDecoder::new(f).read_to_end(&mut d).unwrap();
    d.as_chunks::<72>()
        .0
        .iter()
        .flat_map(|c| c[8..].to_vec())
        .collect()
}

fn run_dump(exe: &Path, input: &[u8]) -> Vec<u8> {
    let mut ch = Command::new(exe)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut si = ch.stdin.take().unwrap();
    let inp = input.to_vec();
    let t = std::thread::spawn(move || {
        let _ = si.write_all(&inp);
    });
    let mut out = Vec::new();
    ch.stdout.take().unwrap().read_to_end(&mut out).unwrap();
    t.join().unwrap();
    assert!(ch.wait().unwrap().success());
    out
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn decoders_via_c_abi_are_identical_to_the_original_c_library() {
    let sd = shared_dir();
    let inc = libpimax().join("include");
    let src = root().join("rust/pimax-capi/tests/c/decode_dump.c");
    let orig = tmp().join("dump-c");
    let rust = tmp().join("dump-rust");
    cc(&[
        "-std=c11",
        "-O1",
        "-o",
        orig.to_str().unwrap(),
        src.to_str().unwrap(),
        libpimax().join("src/decode.c").to_str().unwrap(),
        "-I",
        inc.to_str().unwrap(),
    ]);
    cc(&[
        "-std=c11",
        "-O1",
        "-o",
        rust.to_str().unwrap(),
        src.to_str().unwrap(),
        "-I",
        inc.to_str().unwrap(),
        &format!("-L{}", sd.display()),
        "-lpimax",
        &format!("-Wl,-rpath,{}", sd.display()),
    ]);
    let mut input = Vec::new();
    for f in [
        "p83.rec.gz",
        "p82.rec.gz",
        "ctrl_buttons_hub2.rec.gz",
        "if0_replies_coldstart.rec.gz",
        "if3_log_coldstart.rec.gz",
    ] {
        input.extend(records(f));
    }
    // plus deterministic pseudo-random noise, half of it with valid magics
    let mut x = 0x2545F4914F6CDD1Du64;
    for i in 0..200_000 {
        let mut r = [0u8; 64];
        for b in r.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            *b = x as u8;
        }
        match i % 4 {
            0 => r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]),
            1 => {
                r[..3].copy_from_slice(&[0x53, 0x32, 0x08]);
                r[3] = 2 + (r[3] & 1);
            }
            _ => {}
        }
        input.extend_from_slice(&r);
    }
    let n = input.len() / 64;
    let a = run_dump(&orig, &input);
    let b = run_dump(&rust, &input);
    assert!(n > 300_000);
    assert!(a.len() > 5_000_000, "dump has content ({} bytes)", a.len());
    assert!(
        a == b,
        "decoder output differs between C and Rust (first diff at line {:?})",
        a.split(|&c| c == b'\n')
            .zip(b.split(|&c| c == b'\n'))
            .position(|(x, y)| x != y)
    );
}

/// The exact CMake structure proposed for Monado (static driver library -> executable), built once against the C sources
/// and once against `libpimax_capi.a`; both must link; the Rust one is also run without a headset.
#[test]
fn cmake_consumer_links_c_sources_and_rust_staticlib_alike() {
    if Command::new("cmake").arg("--version").output().is_err() {
        eprintln!("cmake missing: skipped");
        return;
    }
    let dir = build_libs();
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/cmake");
    let empty = tmp().join("empty-sys2");
    std::fs::create_dir_all(empty.join("class/hidraw")).unwrap();
    for (name, rust) in [("c", "OFF"), ("rust", "ON")] {
        let b = tmp().join(format!("cmake-{name}"));
        let _ = std::fs::remove_dir_all(&b);
        let st = Command::new("cmake")
            .arg("-S")
            .arg(&src)
            .arg("-B")
            .arg(&b)
            .arg(format!("-DPIMAX_USE_RUST_LIBPIMAX={rust}"))
            .arg(format!("-DPIMAX_LIBPIMAX_DIR={}", libpimax().display()))
            .arg(format!("-DPIMAX_RUST_TARGET_DIR={}", dir.display()))
            .output()
            .unwrap();
        assert!(
            st.status.success(),
            "{}",
            String::from_utf8_lossy(&st.stderr)
        );
        let bt = Command::new("cmake")
            .arg("--build")
            .arg(&b)
            .output()
            .unwrap();
        assert!(
            bt.status.success(),
            "{}{}",
            String::from_utf8_lossy(&bt.stdout),
            String::from_utf8_lossy(&bt.stderr)
        );
        // The C build is only linked, never run: it would open the real headset (and send wake + keepalive).
        // The Rust build honours PIMAX_SYSFS_ROOT, so it runs hermetically on an empty sysfs.
        if rust == "ON" {
            let o = Command::new(b.join("service"))
                .env("PIMAX_SYSFS_ROOT", &empty)
                .output()
                .unwrap();
            assert!(
                o.status.success(),
                "rust consumer: {}",
                String::from_utf8_lossy(&o.stdout)
            );
            assert!(String::from_utf8_lossy(&o.stdout).contains("mask=0 wait=-110 send=-107"));
        } else {
            assert!(b.join("service").exists());
        }
    }
}

/// If Monado's pimax driver objects are built on this machine: every libpimax symbol they import is exported by the Rust library.
#[test]
fn monado_driver_objects_resolve_against_the_rust_library() {
    let build = PathBuf::from(std::env::var("HOME").unwrap())
        .join("src/monado/build/src/xrt/drivers/CMakeFiles/drv_pimax.dir/pimax");
    let objs: Vec<_> = ["pimax_hmd.c.o", "pimax_ctrl.c.o", "pimax_prober.c.o"]
        .iter()
        .map(|o| build.join(o))
        .filter(|p| p.exists())
        .collect();
    if objs.is_empty() {
        eprintln!("no Monado build found: skipped");
        return;
    }
    let dir = build_libs();
    let exports = String::from_utf8(
        Command::new("nm")
            .args(["-D", "--defined-only"])
            .arg(dir.join("libpimax_capi.so"))
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap();
    let header = std::fs::read_to_string(libpimax().join("include/pimax.h")).unwrap();
    let mut used = std::collections::BTreeSet::new();
    for o in &objs {
        let nm = String::from_utf8(Command::new("nm").arg("-u").arg(o).output().unwrap().stdout)
            .unwrap();
        for l in nm.lines() {
            let sym = l.trim().trim_start_matches("U ").trim();
            if sym.starts_with("pimax_") && header.contains(&format!("{sym}(")) {
                used.insert(sym.to_string());
            }
        }
    }
    assert!(used.len() >= 4, "{used:?}");
    for s in &used {
        assert!(
            exports.lines().any(|l| l.ends_with(&format!(" T {s}"))),
            "Monado uses {s} but the Rust library does not export it"
        );
    }
    println!("Monado driver imports {used:?}: all provided");
}

/// pimax_close must return promptly even if the application holds a lock its callbacks need (Monado's destroy order):
/// after close starts the library no longer calls back. Here the session is idle (no reports) and the state callback
/// of the disconnect that close itself causes must be suppressed.
#[test]
fn close_returns_promptly_while_caller_holds_callback_lock() {
    use std::os::raw::{c_int, c_void};
    use std::sync::Mutex;
    use std::time::{Duration, Instant};
    static LOCK: Mutex<()> = Mutex::new(());
    unsafe extern "C" fn on_state(_u: *mut c_void, s: c_int) {
        if s == 0 {
            let _g = LOCK.lock().unwrap(); // disconnect during close
        }
    }
    let mut sv = [[0 as c_int; 2]; 4];
    for p in sv.iter_mut() {
        assert_eq!(
            unsafe { libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET, 0, p.as_mut_ptr()) },
            0
        );
    }
    let reads = [sv[0][0], sv[1][0], sv[2][0], sv[3][0]];
    let cb = pimax_callbacks {
        user: std::ptr::null_mut(),
        on_state: Some(on_state),
        on_imu: None,
        on_controller: None,
        on_controller_status: None,
        on_log: None,
        on_reply: None,
    };
    let h = unsafe { pimax_open_with_fds(&cb, reads.as_ptr(), sv[0][1]) };
    assert!(!h.is_null());
    assert_eq!(unsafe { pimax_wait_connected(h, 2000) }, 0);
    let g = LOCK.lock().unwrap();
    let t = Instant::now();
    unsafe { pimax_close(h) };
    drop(g);
    assert!(
        t.elapsed() < Duration::from_millis(500),
        "{:?}",
        t.elapsed()
    );
}
