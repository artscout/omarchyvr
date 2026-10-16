//! Hardware tests against the real EyeChip. All `#[ignore]`d. Run them under the headset lock:
//!
//! ```text
//! flock -w 3600 /tmp/omarchyvr-headset.lock \
//!   cargo test -p pimax-eye --test live -- --ignored --test-threads=1 --nocapture
//! # full replay (needs a capture whose session start is complete, see analysis/overnight/eye.md):
//! PIMAX_EYE_CAPTURE=/path/to/full-snaplen.pcap flock ... cargo test ... replay_from_capture
//! ```
//! The first two tests send nothing to the device (claim + bulk IN reads only).
use pimax_eye::replay::{EYE_PID, EYE_VID};
use pimax_eye::session::{in_decoder, read_stream, run_plan, ReplayOptions};
use pimax_eye::usbfs::{find_eye_chips, UsbFs};
use std::time::Duration;

#[test]
#[ignore = "needs the headset; run under /tmp/omarchyvr-headset.lock"]
fn eye_chip_is_enumerated_and_the_node_can_be_opened() {
    let chips = find_eye_chips();
    assert_eq!(
        chips.len(),
        1,
        "expected exactly one EyeChip, found {chips:?}"
    );
    let c = &chips[0];
    assert_eq!((c.vid, c.pid), (EYE_VID, EYE_PID));
    assert_eq!(c.product.as_deref(), Some("EyeChip"));
    assert_eq!(
        c.interface_driver(),
        None,
        "no kernel driver may own the interface"
    );
    UsbFs::open(&c.devnode).unwrap_or_else(|e| {
        panic!(
            "open {}: {e} (is the udev rule installed?)",
            c.devnode.display()
        )
    });
    println!(
        "{} serial {:?} speed {:?}",
        c.devnode.display(),
        c.serial,
        c.speed
    );
}

#[test]
#[ignore = "needs the headset; run under /tmp/omarchyvr-headset.lock"]
fn passive_read_sends_nothing_and_decodes_whatever_arrives() {
    let chip = find_eye_chips().remove(0);
    let mut dev = UsbFs::open(&chip.devnode).expect("open");
    dev.claim_interface(0)
        .expect("claim interface 0 (nothing is detached)");
    let mut dec = in_decoder();
    let s = read_stream(&mut dev, &mut dec, Duration::from_secs(3), |_, _| {})
        .expect("bulk IN must not error");
    println!("{s:#?}");
    assert_eq!(s.framing_errors, 0);
    assert_eq!(s.parse_errors, 0, "{:?}", s.first_parse_error);
    if s.samples > 0 {
        let hz = s.host_rate_hz().unwrap();
        assert!((100.0..130.0).contains(&hz), "rate {hz}");
    } else {
        println!("no gaze notification: the chip does not stream without a TTP session (expected)");
        assert!(s.timeouts > 0);
    }
}

#[test]
#[ignore = "needs the headset and PIMAX_EYE_CAPTURE pointing at a capture with a complete session start"]
fn replay_from_capture() {
    let Ok(path) = std::env::var("PIMAX_EYE_CAPTURE") else {
        panic!("set PIMAX_EYE_CAPTURE");
    };
    let report =
        pimax_eye::analysis::analyze_file(std::path::Path::new(&path), None).expect("capture");
    let d = report
        .devices
        .iter()
        .find(|d| d.plan.stream_start.is_some())
        .expect("a device whose session start reaches the first gaze notification");
    assert!(d.plan.is_complete(), "{}", d.plan.describe());
    let chip = find_eye_chips().remove(0);
    let mut dev = UsbFs::open(&chip.devnode).expect("open");
    dev.claim_interface(0).expect("claim");
    let mut dec = in_decoder();
    let rep = run_plan(
        &mut dev,
        &d.plan,
        &ReplayOptions::default(),
        &mut dec,
        |l| println!("{l}"),
    )
    .expect("replay");
    println!("{rep:?}");
    let s = read_stream(&mut dev, &mut dec, Duration::from_secs(10), |_, _| {}).expect("read");
    println!("{s:#?}");
    assert!(s.samples > 1000, "{} samples in 10 s", s.samples);
    assert_eq!(s.parse_errors, 0);
    assert!((110.0..130.0).contains(&s.host_rate_hz().unwrap()));
}
