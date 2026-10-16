//! The HID allowlist: approved Dream Air commands pass, everything else is refused before a byte is written.

use pimax_device::devices::dream_air::{Ae, DreamAir};
use pimax_device::hid::{Gate, Refused, SendError};
use pimax_device::{all, by_id, detect, Detected, Hand, HeadsetModule, Status, Support};

fn dream_air() -> &'static dyn HeadsetModule {
    by_id("dream-air").expect("dream-air module")
}

fn pad(b: &[u8]) -> Vec<u8> {
    let mut r = b.to_vec();
    r.resize(64, 0);
    r
}

fn refused(m: &dyn HeadsetModule, r: &[u8]) -> Refused {
    let al = &m.descriptor().allowlist;
    let mut g = Gate::new(al, 0, Vec::<u8>::new());
    let e = match g.send(r) {
        Err(SendError::Refused(e)) => e,
        other => panic!("{:02x?} was not refused: {other:?}", &r[..8.min(r.len())]),
    };
    assert!(g.into_inner().is_empty(), "bytes written for a refused report");
    e
}

#[test]
fn registry_has_one_real_module_and_four_mocks() {
    let ids: Vec<_> = all().iter().map(|m| m.id().to_string()).collect();
    assert_eq!(ids[0], "dream-air");
    assert_eq!(all().iter().filter(|m| !m.is_mock()).count(), 1);
    for id in ["dream-air-se", "crystal-light", "crystal-super", "8kx"] {
        let m = by_id(id).unwrap();
        assert_eq!(m.descriptor().status, Status::Mock);
        assert!(m.descriptor().allowlist.commands.is_empty());
        let c = &m.descriptor().caps;
        for s in [c.display, c.tracking, c.controllers, c.eye_tracking, c.ipd, c.cameras, c.audio] {
            assert!(matches!(s, Support::Unknown | Support::None), "{id} promises a capability");
        }
    }
}

#[test]
fn usb_detection() {
    assert!(matches!(detect(0x34a4, 0x0044), Detected::Module(m) if m.id() == "dream-air"));
    assert!(matches!(detect(0x34a4, 0x0042), Detected::Possibly(m) if m.id() == "dream-air-se"));
    assert!(matches!(detect(0x34a4, 0x0043), Detected::None)); // bridge MCU is not the headset
    assert!(matches!(detect(0x1234, 0x0044), Detected::None));
}

#[test]
fn ipd_motor_matches_pimax_eye_and_is_clamped() {
    let m = dream_air();
    // pimax-eye/src/ipd.rs tests: 60.00 mm = 70 17, 65.90 mm = be 19
    assert_eq!(m.ipd_report(60.0).unwrap(), pad(&[0x58, 0x06, 0x5c, 0x05, 0x01, 0x70, 0x17]));
    assert_eq!(m.ipd_report(65.9).unwrap(), pad(&[0x58, 0x06, 0x5c, 0x05, 0x01, 0xbe, 0x19]));
    // clamped 58..72
    assert_eq!(m.ipd_report(40.0).unwrap()[5..7], 5800u16.to_le_bytes());
    assert_eq!(m.ipd_report(99.0).unwrap()[5..7], 7200u16.to_le_bytes());
    assert!(m.ipd_report(f32::NAN).is_err());
    // a hand-built out-of-range report is refused at the send layer
    let raw = pad(&[0x58, 0x06, 0x5c, 0x05, 0x01, 0x88, 0x13]); // 50.00 mm
    assert!(matches!(refused(m, &raw), Refused::OutOfRange { ref param, value: 5000, .. } if param == "ipd"));
}

#[test]
fn start_pair_only_for_slots_2_and_3() {
    let m = dream_air();
    assert_eq!(m.pair_report(Hand::Left).unwrap(), pad(&[0x58, 0x04, 0x04, 0x00, 0x02]));
    assert_eq!(m.pair_report(Hand::Right).unwrap(), pad(&[0x58, 0x04, 0x04, 0x00, 0x03]));
    assert!(matches!(refused(m, &pad(&[0x58, 0x04, 0x04, 0x00, 0x04])), Refused::OutOfRange { .. }));
    assert!(matches!(refused(m, &pad(&[0x58, 0x04, 0x04, 0x01, 0x02])), Refused::NotListed));
}

#[test]
fn camera_ae_windows_frame_passes_and_bad_gain_does_not() {
    let da = DreamAir(dream_air().descriptor().clone());
    // camera-frames.md / pimax-cam/src/ae.rs test: 58 19 51 00 01 32 47 0c 47 0c 57 08 57 08 0d 00 x4 28 00 3e 00
    let s = Ae { exposure: [0x0c47, 0x0c47, 0x0857, 0x0857], gain: [13; 4], led_exposure: 40, led_gain: 62 };
    let want = pad(&[
        0x58, 0x19, 0x51, 0x00, 0x01, 0x32, 0x47, 0x0c, 0x47, 0x0c, 0x57, 0x08, 0x57, 0x08, 0x0d, 0x00, 0x0d, 0x00, 0x0d, 0x00, 0x0d,
        0x00, 0x28, 0x00, 0x3e, 0x00,
    ]);
    assert_eq!(da.ae_report(&s).unwrap(), want);
    assert!(da.ae_report(&Ae { gain: [0, 13, 13, 13], ..s }).is_err());
    assert!(da.ae_report(&Ae { exposure: [10410, 3000, 3000, 3000], ..s }).is_err(), "power-on exposure is not a Windows value");
}

#[test]
fn session_commands_match_pimax_proto() {
    let da = DreamAir(dream_air().descriptor().clone());
    assert_eq!(da.keepalive_report(72).unwrap(), pad(&[0x58, 0x04, 0x39, 0x00, 0x48, 0x00]));
    assert!(da.keepalive_report(60).is_err());
    let al = &dream_air().descriptor().allowlist;
    for (r, id) in [
        (pad(&[0x58, 0x06, 0x1a, 0x05, 0x01, 0x00, 0x00]), "wake"),
        (pad(&[0x58, 0x05, 0x5b, 0x05, 0x02, 0x02]), "app_unlock"),
        (pad(&[0x58, 0x04, 0x29, 0x00, 0x01]), "panel_on"),
        (pad(&[0x58, 0x0d, 0x1a, 0x05, 0x02, 72]), "announce_refresh"),
        (pad(&[0x58, 0x0d, 0x1a, 0x05, 0x05]), "announce_gpu"),
        (pad(&[0x58, 0x04, 0x1a, 0x05, 0x04]), "stop"),
        (pad(&[0x58, 0x04, 0x16, 0x00, 0x1e]), "read_serial"),
        (pad(&[0x58, 0x03, 0x50, 0x02]), "read_info"),
        (pad(&[0x58, 0x03, 0x03, 0x03]), "read_version"),
        (pad(&[0x58, 0x07, 0x0a, 0x02, 0x00, 0x00, 0x00, 0x10]), "read_doc_controller"),
        (pad(&[0x58, 0x07, 0x0a, 0x00, 0x01, 0x10, 0x00, 0x10]), "read_calib_block"),
        (pad(&[0x58, 0x05, 0x3c, 0x05, 0x00, 0x01]), "eye_power_on"),
    ] {
        assert_eq!(al.check(0, &r).map(|c| c.id.as_str()), Ok(id));
        // the hidraw write buffer form (report id 0 prefix) is accepted too
        let mut w = vec![0u8];
        w.extend_from_slice(&r);
        assert_eq!(al.check(0, &w).map(|c| c.id.as_str()), Ok(id));
    }
}

#[test]
fn everything_else_is_refused() {
    let m = dream_air();
    // explicitly denied, with the reason
    assert!(matches!(refused(m, &pad(&[0x58, 0x03, 0x19, 0x00])), Refused::Denied { ref why } if why.contains("DeviceCheckResult")));
    assert!(matches!(refused(m, &pad(&[0x58, 0x05, 0x3c, 0x05, 0x00, 0x00])), Refused::Denied { .. }));
    assert!(matches!(refused(m, &pad(&[0x58, 0x10, 0x55, 0x00])), Refused::Denied { .. }));
    // never approved: visor probe, other 0x5b blobs, unknown opcodes
    assert!(matches!(refused(m, &pad(&[0x58, 0x04, 0x2c, 0x00, 0x00])), Refused::NotListed));
    assert!(matches!(refused(m, &pad(&[0x58, 0x05, 0x5b, 0x05, 0x01, 0x02])), Refused::NotListed));
    assert!(matches!(refused(m, &pad(&[0xff])), Refused::NotListed));
    // trailing garbage after an approved pattern
    let mut r = pad(&[0x58, 0x06, 0x1a, 0x05, 0x01, 0x00, 0x00]);
    r[40] = 1;
    assert!(matches!(refused(m, &r), Refused::Padding { .. }));
    // wrong length
    assert!(matches!(refused(m, &[0x58, 0x06, 0x1a, 0x05, 0x01, 0x00, 0x00]), Refused::Length { got: 7 }));
    // wrong interface
    let al = &m.descriptor().allowlist;
    assert_eq!(al.check(2, &pad(&[0x58, 0x06, 0x1a, 0x05, 0x01, 0x00, 0x00])).err(), Some(Refused::NotListed));
    // unknown ids
    assert!(matches!(al.build("flash_firmware", &[]), Err(Refused::UnknownCommand(_))));
}

#[test]
fn mocks_refuse_every_dream_air_command() {
    let da = &dream_air().descriptor().allowlist;
    for m in all().iter().filter(|m| m.is_mock()) {
        assert!(m.ipd_report(63.0).is_err(), "{}", m.id());
        assert!(m.pair_report(Hand::Left).is_err(), "{}", m.id());
        for c in &da.commands {
            let vals: Vec<(&str, u32)> = c
                .params
                .iter()
                .map(|(n, k)| {
                    let v = match k {
                        pimax_device::hid::Constraint::Range(lo, _) => *lo,
                        pimax_device::hid::Constraint::Set(s) => s[0],
                    };
                    (n.as_str(), v)
                })
                .collect();
            let r = da.build(&c.id, &vals).unwrap();
            assert!(matches!(refused(m.as_ref(), &r), Refused::NotListed), "{} accepted {}", m.id(), c.id);
        }
    }
}

#[test]
fn gate_writes_one_report_and_rate_limits_once_per_request_commands() {
    let m = dream_air();
    let al = &m.descriptor().allowlist;
    let mut g = Gate::new(al, 0, Vec::<u8>::new());
    let r = m.pair_report(Hand::Left).unwrap();
    assert_eq!(g.send(&r).unwrap(), "start_pair");
    assert!(matches!(g.send(&r), Err(SendError::Refused(Refused::TooSoon { .. }))));
    let out = g.into_inner();
    assert_eq!(out.len(), 65);
    assert_eq!(&out[..6], &[0x00, 0x58, 0x04, 0x04, 0x00, 0x02]);
}

/// The approved Windows-capture commands (2026-10-15): exact bytes from analysis/windows-capture-20261015.md, built by
/// the stack's own builders, pass the allowlist; anything outside the captured ranges does not.
#[test]
fn windows_capture_commands_pass_and_match_the_stack_builders() {
    let m = dream_air();
    let al = &m.descriptor().allowlist;
    // vibration: Pimax Play test pulse 58 09 07 03 19 2c 01 (right, 25 %, 300 ms); game stream pulses 12 ms
    let cap = pad(&[0x58, 0x09, 0x07, 0x03, 0x19, 0x2c, 0x01]);
    assert_eq!(&pimax_proto::haptic_command(3, 25, 300)[..], &cap[..]);
    assert_eq!(al.check(0, &cap).map(|c| c.id.as_str()), Ok("haptic"));
    assert_eq!(al.check(0, &pimax_proto::haptic_command(2, 100, 65535)).map(|c| c.id.as_str()), Ok("haptic"));
    assert_eq!(al.check(0, &pimax_proto::haptic_command(2, 1, 12)).map(|c| c.id.as_str()), Ok("haptic"));
    assert!(matches!(refused(m, &pimax_proto::haptic_command(4, 25, 300)), Refused::OutOfRange { .. }));
    assert!(matches!(refused(m, &pad(&[0x58, 0x09, 0x07, 0x03, 101, 0x2c, 0x01])), Refused::OutOfRange { .. }));
    assert!(matches!(refused(m, &pad(&[0x58, 0x09, 0x07, 0x03, 0, 0x2c, 0x01])), Refused::OutOfRange { .. }));
    // unpair: left t=40.871 `58 03 05 02`, right t=52.533 `58 03 05 03`
    for (slot, cap) in [(2u8, pad(&[0x58, 0x03, 0x05, 0x02])), (3, pad(&[0x58, 0x03, 0x05, 0x03]))] {
        assert_eq!(&pimax_proto::unpair(slot)[..], &cap[..]);
        assert_eq!(al.check(0, &cap).map(|c| c.id.as_str()), Ok("unpair"));
    }
    assert!(matches!(refused(m, &pad(&[0x58, 0x03, 0x05, 0x04])), Refused::OutOfRange { .. }));
    assert!(matches!(refused(m, &pad(&[0x58, 0x03, 0x05, 0x00])), Refused::OutOfRange { .. }));
    // restart: take 2 t=8.722 `58 04 28 00 00`; nothing else with opcode 0x28 passes
    let cap = pad(&[0x58, 0x04, 0x28, 0x00, 0x00]);
    assert_eq!(&pimax_proto::sys_reset()[..], &cap[..]);
    assert_eq!(al.check(0, &cap).map(|c| c.id.as_str()), Ok("sys_reset"));
    assert!(matches!(refused(m, &pad(&[0x58, 0x04, 0x28, 0x00, 0x01])), Refused::NotListed));
    assert!(matches!(refused(m, &pad(&[0x58, 0x04, 0x28, 0x01, 0x00])), Refused::NotListed));
    // set IPD: take 6 t=10.397 `58 06 5c 05 01 a8 16` = 58.00 mm
    let cap = pad(&[0x58, 0x06, 0x5c, 0x05, 0x01, 0xa8, 0x16]);
    assert_eq!(m.ipd_report(58.0).unwrap(), cap);
    assert_eq!(al.check(0, &cap).map(|c| c.id.as_str()), Ok("ipd_motor"));
}

#[test]
fn restart_and_unpair_are_once_per_request() {
    let m = dream_air();
    let mut g = Gate::new(&m.descriptor().allowlist, 0, Vec::<u8>::new());
    let r = pimax_proto::sys_reset();
    assert_eq!(g.send(&r).unwrap(), "sys_reset");
    assert!(matches!(g.send(&r), Err(SendError::Refused(Refused::TooSoon { .. }))));
    let u = pimax_proto::unpair(2);
    assert_eq!(g.send(&u).unwrap(), "unpair");
    assert!(matches!(g.send(&u), Err(SendError::Refused(Refused::TooSoon { .. }))));
}
