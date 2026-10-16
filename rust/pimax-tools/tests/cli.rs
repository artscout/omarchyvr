//! Black-box tests of the `pimax` binary that need no headset (empty fake sysfs).
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

fn bin() -> Command {
    let mut c = Command::new(env!("CARGO_BIN_EXE_pimax"));
    let empty = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("cli-empty-sys");
    std::fs::create_dir_all(empty.join("class/hidraw")).unwrap();
    c.env("PIMAX_SYSFS_ROOT", empty)
        .env("XDG_RUNTIME_DIR", env!("CARGO_TARGET_TMPDIR"));
    c
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn usage_and_unknown_subcommand() {
    let o = bin().output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stdout).contains("keepalived"));
    let o = bin().arg("--help").output().unwrap();
    assert_eq!(o.status.code(), Some(0));
    for sub in [
        "keepalived",
        "probe",
        "imu",
        "controllers",
        "calib",
        "video-on",
        "logtail",
        "info",
    ] {
        assert!(
            String::from_utf8_lossy(&o.stdout).contains(sub),
            "usage lists {sub}"
        );
    }
    let o = bin().arg("frobnicate").output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&o.stderr).contains("unknown subcommand frobnicate"));
    let o = bin().args(["imu", "--bogus"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
    let o = bin().args(["imu", "--rate"]).output().unwrap();
    assert_eq!(o.status.code(), Some(2));
}

#[test]
fn commands_without_headset_fail_cleanly() {
    for (args, msg) in [
        (&["imu"][..], "IF2 hidraw not found"),
        (&["controllers", "1"], "IF0 not found"),
        (&["probe", "1"], "IF0/IF2 not found"),
    ] {
        let o = bin().args(args).output().unwrap();
        assert_eq!(o.status.code(), Some(1), "{args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(msg),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }
}

#[test]
fn logtail_without_devices_runs_for_its_duration() {
    let t = Instant::now();
    let o = bin().args(["logtail", "0.4"]).output().unwrap();
    assert!(o.status.success());
    assert!(t.elapsed() >= Duration::from_millis(350) && t.elapsed() < Duration::from_secs(2));
}

#[test]
fn keepalived_waits_and_stops_on_sigterm() {
    let mut c = bin()
        .arg("keepalived")
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(400));
    let pid = c.id() as i32;
    // CPU while idle-waiting for the headset: practically zero
    let cpu = pimax::sys::proc_cpu_seconds(&format!("/proc/{pid}/stat")).unwrap();
    assert!(cpu < 0.2, "idle cpu {cpu}s");
    assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
    let t = Instant::now();
    let st = c.wait().unwrap();
    assert!(st.success(), "{st:?}");
    assert!(t.elapsed() < Duration::from_secs(1));
    let mut out = String::new();
    c.stdout.take().unwrap().read_to_string(&mut out).unwrap();
    assert!(out.contains("waiting for headset 34a4:0044"), "{out}");
    assert!(out.trim_end().ends_with("stopped"), "{out}");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn calib_offline_decodes_the_real_windows_blob() {
    // assemble the blob from the Windows cold-start capture, then run `pimax calib --blob`
    let mut d = Vec::new();
    let f = std::fs::File::open(root().join("rust/fixtures/if0_replies_coldstart.rec.gz")).unwrap();
    flate2::read::GzDecoder::new(f).read_to_end(&mut d).unwrap();
    let mut asm = pimax_proto::CalibAssembler::new();
    for c in d.as_chunks::<72>().0 {
        asm.feed(c[8..].try_into().unwrap());
    }
    assert!(asm.complete());
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("calib-cli");
    std::fs::create_dir_all(&dir).unwrap();
    let (blob, out) = (dir.join("blob.bin"), dir.join("out.xml"));
    std::fs::write(&blob, asm.blob()).unwrap();
    let o = bin()
        .current_dir(root())
        .args(["calib"])
        .arg(&out)
        .arg("--blob")
        .arg(&blob)
        .output()
        .unwrap();
    let so = String::from_utf8_lossy(&o.stdout);
    assert!(
        o.status.success(),
        "{so}{}",
        String::from_utf8_lossy(&o.stderr)
    );
    assert!(
        so.contains("read 1344 B, zlib 1318 B -> xml 3410 B"),
        "{so}"
    );
    assert!(
        so.contains("IDENTICAL to analysis/headset-calibration.xml"),
        "{so}"
    );
    assert_eq!(
        std::fs::read(&out).unwrap(),
        std::fs::read(root().join("analysis/headset-calibration.xml")).unwrap()
    );
    // corrupt blob -> error exit
    let mut bad = asm.blob().to_vec();
    bad[30] ^= 0xff;
    std::fs::write(&blob, &bad).unwrap();
    let o = bin()
        .args(["calib"])
        .arg(&out)
        .arg("--blob")
        .arg(&blob)
        .output()
        .unwrap();
    assert_eq!(o.status.code(), Some(1));
}
