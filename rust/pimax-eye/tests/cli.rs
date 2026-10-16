//! Black-box tests of the command line on the fixture captures.
mod common;

use common::gunzip;
use pimax_eye::cli::run;
use std::path::PathBuf;

fn tmp(name: &str, bytes: &[u8]) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pimax-eye-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, bytes).unwrap();
    p
}

fn exec(args: &[&str]) -> (i32, String, String) {
    let a: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let (mut o, mut e) = (Vec::new(), Vec::new());
    let c = run(&a, &mut o, &mut e);
    (
        c,
        String::from_utf8(o).unwrap(),
        String::from_utf8(e).unwrap(),
    )
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn pcap_command_reports_the_full_size_stream() {
    let f = tmp("full.pcap", &gunzip("eye_gaze_full.usbpcap.gz"));
    let csv = f.with_extension("csv");
    let (c, out, _) = exec(&[
        "pcap",
        f.to_str().unwrap(),
        "--csv",
        csv.to_str().unwrap(),
        "--head",
        "2",
    ]);
    assert_eq!(c, 0);
    assert!(
        out.contains("gaze stream: 364 notifications (0 cut by the snap length, 0 undecodable)"),
        "{out}"
    );
    assert!(out.contains("120.48 Hz"), "{out}");
    assert!(
        out.contains("eye A: lost 364 | origin only 0 | tracked 0"),
        "{out}"
    );
    assert!(out.contains("first 2 samples"), "{out}");
    let text = std::fs::read_to_string(&csv).unwrap();
    assert_eq!(text.lines().count(), 365);
    assert!(text
        .lines()
        .nth(1)
        .unwrap()
        .starts_with("1,0.000000,50376465,5412,0,0,"));
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn pcap_command_reports_the_worn_stream_states() {
    let f = tmp("idle.pcap", &gunzip("eye_gaze_worn_idle.usbpcap.gz"));
    let (c, out, _) = exec(&["pcap", f.to_str().unwrap()]);
    assert_eq!(c, 0);
    assert!(
        out.contains("gaze stream: 400 notifications (400 cut by the snap length, 0 undecodable)"),
        "{out}"
    );
    assert!(
        out.contains("eye A: lost 41 | origin only 20 | tracked 339"),
        "{out}"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn plan_command_lists_the_cut_uploads() {
    let f = tmp("init.pcap", &gunzip("eye_coldstart_init.usbpcap.gz"));
    let (c, out, _) = exec(&["plan", f.to_str().unwrap()]);
    assert_eq!(c, 0);
    assert_eq!(out.matches("INCOMPLETE").count(), 2, "{out}");
    assert!(out.contains("op=0x852") && out.contains("op=0x456"));
    assert!(
        out.contains("=> NOT replayable verbatim (32661 bytes missing)"),
        "{out}"
    );
    let (c, out, _) = exec(&["pcap", f.to_str().unwrap()]);
    assert_eq!(c, 0);
    assert!(out.contains("replay plan: 39 steps, 21 TTP requests, 32661 bytes missing -> NOT replayable verbatim"), "{out}");
    assert!(out.contains("authentication exchange"));
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn stream_refuses_an_incomplete_capture_without_opening_the_device() {
    let f = tmp("init2.pcap", &gunzip("eye_coldstart_init.usbpcap.gz"));
    // a bogus --usb-device proves the device is not even looked at
    let (c, _, err) = exec(&[
        "stream",
        "--capture",
        f.to_str().unwrap(),
        "--usb-device",
        "/nonexistent/node",
    ]);
    assert_eq!(c, 3, "{err}");
    assert!(err.contains("refusing to replay: 2 step(s)"), "{err}");
    assert!(
        err.contains("nothing was sent; the USB device was not opened"),
        "{err}"
    );
    assert!(err.contains("-Snaplen 65535"), "{err}");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn stream_dry_run_with_allow_incomplete_prints_the_plan_only() {
    let f = tmp("init3.pcap", &gunzip("eye_coldstart_init.usbpcap.gz"));
    let (c, out, _) = exec(&[
        "stream",
        "--capture",
        f.to_str().unwrap(),
        "--allow-incomplete",
        "--dry-run",
    ]);
    assert_eq!(c, 0);
    assert!(out.contains("dry run: device not opened"), "{out}");
    assert!(out.contains("39 steps (21 TTP requests)"), "{out}");
}

#[test]
fn stream_without_device_or_capture_fails_cleanly() {
    let (c, _, err) = exec(&[
        "stream",
        "--usb-device",
        "/nonexistent/node",
        "--seconds",
        "1",
    ]);
    assert_eq!(c, 2, "{err}");
    assert!(err.contains("cannot open /nonexistent/node"), "{err}");
}

#[test]
fn stream_rejects_a_capture_without_ttp_traffic() {
    // a pcap with only the camera/IMU-like noise: header only
    let mut w = pimax_eye::pcap::PcapWriter::new(Vec::new(), 512).unwrap();
    w.write_packet(&pimax_eye::pcap::UsbPacket::build(
        0.0,
        2,
        5,
        0x81,
        pimax_eye::pcap::Transfer::Interrupt,
        true,
        None,
        &[1, 2, 3],
        3,
    ))
    .unwrap();
    let f = tmp("noise.pcap", &w.into_inner());
    let (c, _, err) = exec(&["stream", "--capture", f.to_str().unwrap()]);
    assert_eq!(c, 3, "{err}");
    assert!(err.contains("no device with TTP traffic"), "{err}");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn skip_op_leaves_named_requests_out_and_makes_the_plan_complete() {
    let f = tmp("init4.pcap", &gunzip("eye_coldstart_init.usbpcap.gz"));
    let p = f.to_str().unwrap();
    let (c, out, err) = exec(&[
        "stream",
        "--capture",
        p,
        "--skip-op",
        "0x852,1110",
        "--dry-run",
    ]);
    assert_eq!(c, 0, "{err}");
    assert!(
        out.contains("--skip-op: leaving 2 step(s) out of the replay"),
        "{out}"
    );
    assert!(out.contains("37 steps (19 TTP requests)"), "{out}");
    assert!(!out.contains("INCOMPLETE"), "{out}");
    // skipping only one of the two cut uploads still refuses
    let (c, _, err) = exec(&["stream", "--capture", p, "--skip-op", "0x852", "--dry-run"]);
    assert_eq!(c, 3, "{err}");
    assert!(err.contains("refusing to replay: 1 step(s)"), "{err}");
    let (c, _, err) = exec(&["stream", "--capture", p, "--skip-op", "bogus", "--dry-run"]);
    assert_eq!(c, 1);
    assert!(err.contains("bad opcode"), "{err}");
}
