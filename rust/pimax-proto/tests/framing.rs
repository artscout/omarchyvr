use pimax_proto::*;

fn hex(r: &[u8]) -> String {
    r.iter()
        .map(|b| format!("{b:02x}"))
        .collect::<Vec<_>>()
        .join(" ")
}

#[test]
fn wake_and_keepalive_bytes_match_python() {
    // linux/pimax_keepalived.py: WAKE = 58 06 1a 05 01 00 00, keepalive = 58 04 39 00 RR 00
    let w = wake();
    assert_eq!(&w[..8], &[0x58, 0x06, 0x1a, 0x05, 0x01, 0x00, 0x00, 0x00]);
    assert!(w[8..].iter().all(|&b| b == 0));
    for rate in [0u8, 72, 90, 255] {
        let k = keepalive(rate);
        assert_eq!(&k[..6], &[0x58, 0x04, 0x39, 0x00, rate, 0x00]);
        assert!(k[6..].iter().all(|&b| b == 0));
    }
}

#[test]
fn write_buffer_has_report_id_zero() {
    let b = to_write_buf(&wake());
    assert_eq!(b.len(), 65);
    assert_eq!(b[0], 0);
    assert_eq!(b[1], 0x58);
}

#[test]
fn build_command_ll_is_len_plus_three() {
    for n in 0..=MAX_DATA {
        let data = vec![0xAB; n];
        let r = build_command(0x16, 2, &data).unwrap();
        assert_eq!(r[0], 0x58);
        assert_eq!(r[1] as usize, n + 3);
        assert_eq!((r[2], r[3]), (0x16, 2));
        assert_eq!(&r[4..4 + n], &data[..]);
        assert!(r[4 + n..].iter().all(|&b| b == 0));
    }
    assert_eq!(
        build_command(1, 0, &[0; MAX_DATA + 1]),
        Err(FrameError::TooLong)
    );
}

#[test]
fn video_prepare_matches_python_steps() {
    // linux/pimax_video_on.py STEPS and libpimax wire forms
    let c = video_prepare_commands(72);
    assert_eq!(hex(&c[0][..6]), "58 05 5b 05 02 02");
    assert_eq!(hex(&c[1][..5]), "58 04 29 00 01");
    assert_eq!(hex(&c[2][..6]), "58 0d 1a 05 02 48");
    assert_eq!(hex(&c[3][..6]), "58 0d 1a 05 05 00");
    // LL = 0x0d = 10 data bytes + 3
    assert!(c[2][14..].iter().all(|&b| b == 0));
}

#[test]
fn attach_burst_matches_python() {
    // pimax_controllers.py ATTACH_CMDS with the slot as opt
    for slot in [2u8, 3] {
        let c = attach_commands(slot);
        assert_eq!(hex(&c[0][..5]), format!("58 04 16 {slot:02x} 1e"));
        assert_eq!(hex(&c[1][..4]), format!("58 03 50 {slot:02x}"));
        assert_eq!(hex(&c[2][..8]), format!("58 07 0a {slot:02x} 00 00 00 10"));
        assert_eq!(hex(&c[3][..4]), format!("58 03 03 {slot:02x}"));
    }
}

#[test]
fn calib_request_matches_python() {
    // pimax_calib.py: bytes([0x58, 0x07, 0x0A, 0x00, 0x01, addr & 255, addr >> 8, 0x10])
    let r = calib::read_block_request(0x0530);
    assert_eq!(&r[..8], &[0x58, 0x07, 0x0a, 0x00, 0x01, 0x30, 0x05, 0x10]);
}

#[test]
fn reply_parsing() {
    let mut r = [0u8; 64];
    r[..8].copy_from_slice(&[0x4c, 0x09, 0x16, 0x00, 0x04, b'A', b'B', b'C']);
    let p = parse_reply(&r).unwrap();
    assert_eq!((p.cmd, p.opt), (0x16, 0));
    assert_eq!(p.data.len(), 6);
    assert_eq!(parse_serial(&p.data[..4]), b"ABC");
    assert_eq!(parse_serial(&[]), b"");
    assert_eq!(parse_serial(&[9, b'x']), b"x");
    r[0] = 0x53;
    assert!(parse_reply(&r).is_none());
    // LL clamps: 0xff -> 60 data bytes, 0..2 -> empty
    r[0] = 0x4c;
    r[1] = 0xff;
    assert_eq!(parse_reply(&r).unwrap().data.len(), 60);
    r[1] = 1;
    assert_eq!(parse_reply(&r).unwrap().data.len(), 0);
}

#[test]
fn haptic_bytes_match_the_windows_capture() {
    // 20261015-125216-room-setup-floor-hub2.pcap t=9.09 (Pimax Play test pulse, right, 25 %, 300 ms), EP 0x01, 64 bytes:
    let r = haptic_command(SLOT_RIGHT, 25, 300);
    assert_eq!(
        &r[..10],
        &[0x58, 0x09, 0x07, 0x03, 0x19, 0x2c, 0x01, 0, 0, 0]
    );
    assert!(r[7..].iter().all(|&b| b == 0));
    // 20261015-125445-game-doe2-hub2.pcap t=123.79 (game stream, left, 99 %, 11 ms): 58 09 07 02 63 0b 00
    let l = haptic_command(SLOT_LEFT, 99, 11);
    assert_eq!(hex(&l[..10]), "58 09 07 02 63 0b 00 00 00 00");
    assert!(l[7..].iter().all(|&b| b == 0));
    assert_eq!(cmd::HAPTIC, 0x07);
    // clamps: AMP 1..100 (never 0 on the wire), DUR >= 1
    assert_eq!(haptic_command(3, 0, 0)[4..7], [1, 1, 0]);
    assert_eq!(haptic_command(3, 250, 65535)[4..7], [100, 0xff, 0xff]);
}

#[test]
fn haptic_openxr_mapping() {
    assert_eq!(haptic_from_openxr(0.25, 300_000_000), Some((25, 300)));
    assert_eq!(
        haptic_from_openxr(1.0, -1),
        Some((100, 12)),
        "XR_MIN_HAPTIC_DURATION -> 12 ms like games"
    );
    assert_eq!(haptic_from_openxr(0.5, 0), Some((50, 12)));
    assert_eq!(
        haptic_from_openxr(0.001, 1),
        Some((1, 1)),
        "tiny amplitude still buzzes, 1 ns -> 1 ms"
    );
    assert_eq!(haptic_from_openxr(2.0, 11_200_000), Some((100, 12)));
    assert_eq!(
        haptic_from_openxr(0.3, 1_000_000_000_000),
        Some((30, 65535))
    );
    assert_eq!(
        haptic_from_openxr(0.0, 300_000_000),
        None,
        "amplitude 0 = stop: nothing is sent"
    );
    assert_eq!(haptic_from_openxr(f32::NAN, 300_000_000), None);
}

#[test]
fn video_prepare_90_matches_the_windows_capture() {
    // 20261015-123612-settings-bright-ipd-hz-hub2.pcap t=63.53 / 63.61 (switch to 90 Hz) and t=52.03 (72 Hz)
    let c = video_prepare_commands(90);
    assert_eq!(
        hex(&c[2][..17]),
        "58 0d 1a 05 02 5a 00 00 00 00 00 00 00 00 00 00 00"
    );
    assert_eq!(
        hex(&c[3][..17]),
        "58 0d 1a 05 05 00 00 00 00 00 00 00 00 00 00 00 00"
    );
    assert_eq!(
        hex(&video_prepare_commands(72)[2][..6]),
        "58 0d 1a 05 02 48"
    );
    // keepalive at 90 Hz after the switch: 58 04 39 00 5a 00
    assert_eq!(&keepalive(90)[..6], &[0x58, 0x04, 0x39, 0x00, 0x5a, 0x00]);
}

#[test]
fn round_pos_matches_std_round() {
    // every f32 in 0..=100 at a 1e-4 step plus the halves and the values just below them
    let mut x = 0.0f32;
    while x <= 100.0 {
        assert_eq!(pimax_proto::frame::round_pos(x), x.round() as u32, "{x}");
        x += 1e-4;
    }
    for k in 0..=200u32 {
        let h = k as f32 + 0.5;
        for v in [h, f32::from_bits(h.to_bits() - 1), f32::from_bits(h.to_bits() + 1), k as f32] {
            assert_eq!(pimax_proto::frame::round_pos(v), v.round() as u32, "{v}");
        }
    }
}
