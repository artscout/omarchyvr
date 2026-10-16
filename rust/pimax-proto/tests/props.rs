use pimax_proto::*;
use proptest::prelude::*;

fn report() -> impl Strategy<Value = Report> {
    proptest::collection::vec(any::<u8>(), 64).prop_map(|v| {
        let mut r = [0u8; 64];
        r.copy_from_slice(&v);
        r
    })
}

/// Reports with a valid magic so the decoders actually run.
fn structured() -> impl Strategy<Value = Report> {
    (report(), 0usize..6).prop_map(|(mut r, k)| {
        match k {
            0 => r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]),
            1 => r[..3].copy_from_slice(&[0x53, 0x32, 0x08]),
            2 => r[..3].copy_from_slice(&[0x4c, 0x0f, 0x37]),
            3 => r[0] = 0x4c,
            _ => {}
        }
        if k == 1 {
            r[3] = 2 + (r[3] & 1);
        }
        r
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(20000))]

    #[test]
    fn no_decoder_panics(r in prop_oneof![report(), structured()]) {
        let _ = decode_imu(&r);
        let _ = decode_controller(&r);
        let _ = parse_status(&r);
        let _ = parse_reply(&r);
        let _ = calib::parse_block_reply(&r);
        let mut a = CalibAssembler::new();
        let _ = a.feed(&r);
        let _ = parse_serial(&r);
    }

    #[test]
    fn decoders_only_accept_their_magic(r in structured()) {
        prop_assert_eq!(decode_imu(&r).is_some(), r[0] == 0x53 && r[1] == 0x31);
        prop_assert_eq!(decode_controller(&r).is_some(), r[..3] == [0x53, 0x32, 0x08] && (r[3] == 2 || r[3] == 3));
        prop_assert_eq!(parse_reply(&r).is_some(), r[0] == 0x4c);
    }

    #[test]
    fn controller_invariants(r in structured()) {
        if let Some(c) = decode_controller(&r) {
            prop_assert!(c.battery <= 127);
            prop_assert_eq!(c.battery | if c.charging { 0x80 } else { 0 }, r[46]);
            prop_assert!((-128..=127).contains(&c.stick_x) && (-128..=127).contains(&c.stick_y));
            prop_assert_eq!(c.buttons & !(BTN_A | BTN_B | BTN_STICK | BTN_MENU), 0);
            prop_assert_eq!(c.raw, r);
        }
    }

    #[test]
    fn imu_invariants(r in structured()) {
        if let Some(s) = decode_imu(&r) {
            prop_assert_eq!(s[1].ts_us, s[0].ts_us.wrapping_add(1000));
            for k in 0..3 {
                prop_assert_eq!(s[0].accel_g[k], f32::from(s[0].raw_accel[k]) / 2048.0);
            }
        }
    }

    #[test]
    fn command_round_trip(cmd in any::<u8>(), opt in any::<u8>(), data in proptest::collection::vec(any::<u8>(), 0..=60)) {
        let mut r = build_command(cmd, opt, &data).unwrap();
        prop_assert_eq!(r[0], 0x58);
        // a reply with the same layout parses back (LL = len + 3 + check byte accounting is the device's)
        r[0] = 0x4c;
        let p = parse_reply(&r).unwrap();
        prop_assert_eq!((p.cmd, p.opt), (cmd, opt));
        prop_assert_eq!(p.data, &data[..]);
    }

    #[test]
    fn line_splitter_never_panics_and_bounds(chunks in proptest::collection::vec(proptest::collection::vec(any::<u8>(), 0..64), 0..40)) {
        let mut sp = LineSplitter::new();
        for c in &chunks {
            sp.push(c, |l| { assert!(!l.is_empty() && l.len() < 512); assert!(!l.contains(&b'\n') && !l.contains(&0)); });
        }
    }

    #[test]
    fn inflate_never_panics(blob in proptest::collection::vec(any::<u8>(), 0..2000)) {
        let _ = calib::inflate_blob(&blob);
    }
}
