mod common;
use common::*;
use pimax_proto::*;

#[test]
fn imu_hand_vector() {
    let mut r = [0u8; 64];
    r[..4].copy_from_slice(&[0x53, 0x31, 0x08, 0x00]);
    r[4..8].copy_from_slice(&1_000_000u32.to_le_bytes());
    // sample 0: temp, acc (2048, -1024, 0), gyro (164, -164, 16)
    r[14..16].copy_from_slice(&2048i16.to_be_bytes());
    r[16..18].copy_from_slice(&(-1024i16).to_be_bytes());
    r[20..22].copy_from_slice(&164i16.to_be_bytes());
    r[22..24].copy_from_slice(&(-164i16).to_be_bytes());
    r[24..26].copy_from_slice(&16i16.to_be_bytes());
    r[45..47].copy_from_slice(&6590u16.to_le_bytes());
    let s = decode_imu(&r).unwrap();
    assert_eq!(s[0].ts_us, 1_000_000);
    assert_eq!(s[1].ts_us, 1_001_000);
    assert_eq!(s[0].accel_g, [1.0, -0.5, 0.0]);
    assert_eq!(s[0].gyro_dps[0], 10.0);
    assert_eq!(s[0].gyro_dps[1], -10.0);
    assert!((s[0].ipd_mm - 65.9).abs() < 1e-4);
    assert_eq!((s[0].sample_in_report, s[1].sample_in_report), (0, 1));
    r[1] = 0x32;
    assert!(decode_imu(&r).is_none());
}

#[test]
fn imu_p83_real_capture() {
    let rows = load_rec("p83.rec.gz");
    assert!(rows.len() > 60_000);
    let mut prev: Option<u32> = None;
    let (mut n, mut sum_g) = (0usize, 0.0f64);
    for (_, b) in &rows {
        let s = decode_imu(b).expect("every p83 record is an IMU report");
        if let Some(p) = prev {
            let d = s[0].ts_us.wrapping_sub(p);
            assert!(
                (1..5000).contains(&d),
                "timestamps advance by about 2 ms, got {d}"
            );
        }
        prev = Some(s[0].ts_us);
        assert_eq!(s[1].ts_us, s[0].ts_us + 1000);
        let a = s[0].accel_g;
        sum_g += f64::from(a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        n += 1;
    }
    let mean = sum_g / n as f64;
    assert!((0.5..2.0).contains(&mean), "mean |a| = {mean} g");
}

#[test]
fn controller_hand_vector() {
    let mut r = [0u8; 64];
    r[..5].copy_from_slice(&[0x53, 0x32, 0x08, 0x02, 0x43]);
    r[5] = 255;
    r[6] = 1;
    r[7] = 200;
    r[8..12].copy_from_slice(&123456u32.to_le_bytes());
    r[16..18].copy_from_slice(&1024i16.to_be_bytes()); // s0 accel x
    r[44] = 99;
    r[46] = 0xda; // 90 % charging
    r[47] = 0x0b; // B, A, stick
    r[48] = 1; // menu
    r[49] = 0x19; // touch A, stick, hand on
    let c = decode_controller(&r).unwrap();
    assert_eq!((c.stick_x, c.stick_y), (127, -127));
    assert_eq!((c.trigger, c.grip), (200, 99));
    assert_eq!((c.battery, c.charging), (90, true));
    assert_eq!(c.buttons, BTN_B | BTN_A | BTN_STICK | BTN_MENU);
    assert_eq!(c.ts_us, 123456);
    assert_eq!(c.raw_accel, [1024, 0, 0]);
    assert_eq!(c.pressed().collect::<Vec<_>>(), ["B", "A", "stick", "menu"]);
    let t = c.touch_bits();
    assert!(t.a && !t.b && t.stick && t.hand_on && t.thumb(), "{t:?}");
    // the same byte on the right controller decodes the same (bit 4 is "hand on" on both hands)
    r[3] = 3;
    let c = decode_controller(&r).unwrap();
    assert_eq!(c.touch_bits(), t);
    // bits 0 and 1 rise together for either face button: either one is "thumb on the face buttons"
    r[49] = 0x02;
    assert!(decode_controller(&r).unwrap().touch_bits().thumb());
    r[49] = 0x00;
    assert!(!decode_controller(&r).unwrap().touch_bits().hand_on);
    r[49] = 0x19;
    // other slots / magic rejected
    r[3] = 4;
    assert!(decode_controller(&r).is_none());
    r[3] = 2;
    r[2] = 0x09;
    assert!(decode_controller(&r).is_none());
    // second sample used when the first is all zero
    let mut q = [0u8; 64];
    q[..5].copy_from_slice(&[0x53, 0x32, 0x08, 0x03, 0x43]);
    q[30..32].copy_from_slice(&(-5i16).to_be_bytes());
    assert_eq!(decode_controller(&q).unwrap().raw_accel[0], -5);
}

fn parse_golden() -> Vec<Vec<i64>> {
    let g = String::from_utf8(gunzip("ctrl_golden.csv.gz")).unwrap();
    g.lines()
        .map(|l| l.split(',').map(|x| x.parse().unwrap()).collect())
        .collect()
}

/// Rust decode == python `pimax_controllers.decode()` on every controller report we have
/// (right controller round 1 + left/right buttons capture).
#[test]
fn controller_equals_python_on_all_captures() {
    let mut rows = load_rec("p82.rec.gz");
    rows.extend(load_rec("ctrl_buttons_hub2.rec.gz"));
    let gold = parse_golden();
    let reps: Vec<_> = rows
        .iter()
        .filter_map(|(_, b)| decode_controller(b))
        .collect();
    assert_eq!(reps.len(), gold.len(), "same number of decodable reports");
    assert!(reps.len() > 100_000);
    for (i, (c, g)) in reps.iter().zip(&gold).enumerate() {
        // slot,ts,sx,sy,trig,grip,batt,chg,btnmask(B,A,stick,menu),touchmask(A,B,stick,t4),imu*6
        let bm = (c.buttons & BTN_B != 0) as i64
            | ((c.buttons & BTN_A != 0) as i64) << 1
            | ((c.buttons & BTN_STICK != 0) as i64) << 2
            | ((c.buttons & BTN_MENU != 0) as i64) << 3;
        let t = c.touch_bits();
        // the golden was made with the old decoder, where bit 4 rested at 1 on the left and 0 on the right
        let rest4 = c.slot == pimax_proto::SLOT_LEFT;
        let t4_old = t.hand_on != rest4;
        let tm = t.a as i64 | (t.b as i64) << 1 | (t.stick as i64) << 2 | (t4_old as i64) << 3;
        let mine = [
            i64::from(c.slot),
            i64::from(c.ts_us),
            i64::from(c.stick_x),
            i64::from(c.stick_y),
            i64::from(c.trigger),
            i64::from(c.grip),
            i64::from(c.battery),
            c.charging as i64,
            bm,
            tm,
            i64::from(c.raw_accel[0]),
            i64::from(c.raw_accel[1]),
            i64::from(c.raw_accel[2]),
            i64::from(c.raw_gyro[0]),
            i64::from(c.raw_gyro[1]),
            i64::from(c.raw_gyro[2]),
        ];
        assert_eq!(&mine[..], &g[..], "report #{i}");
    }
}

#[test]
fn status_0x37_two_per_packet_and_python_golden() {
    // two announces in one packet (captures2 cold start, left battery 0x49 + right 0x63)
    let mut d = [0u8; 64];
    let h =
        "4c0f370248e8b69102490002010008".to_string() + "2d" + "c70f37034846a791026300020100082e38";
    for (i, b) in (0..h.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&h[i..i + 2], 16).unwrap())
        .enumerate()
    {
        d[i] = b;
    }
    let s = parse_status(&d);
    let a = s[0].unwrap();
    let b = s[1].unwrap();
    assert_eq!((a.slot, a.battery, a.fw), (2, 0x49, [0x08, 0x2d]));
    assert_eq!((b.slot, b.battery, b.fw), (3, 0x63, [0x08, 0x2e]));
    // single message, other replies, wrong magic
    let mut one = [0u8; 64];
    one[..16].copy_from_slice(&[
        0x4c, 0x0f, 0x37, 0x03, 0x48, 4, 0x88, 0x91, 2, 0x63, 0, 2, 1, 0, 8, 0x2e,
    ]);
    let s = parse_status(&one);
    assert!(s[0].is_some() && s[1].is_none());
    let mut ka = [0u8; 64];
    ka[..5].copy_from_slice(&[0x4c, 0x04, 0x39, 0x00, 0x01]);
    assert_eq!(parse_status(&ka), [None, None]);
    one[0] = 0x53;
    assert_eq!(parse_status(&one), [None, None]);

    // every 0x37 packet seen in the cold-start capture agrees with the python parser
    let g = std::fs::read_to_string(fixtures_dir().join("status37_golden.txt")).unwrap();
    let mut n = 0;
    for line in g.lines() {
        let (hexs, exp) = line.split_once(' ').unwrap();
        let mut p = [0u8; 64];
        for i in 0..64 {
            p[i] = u8::from_str_radix(&hexs[2 * i..2 * i + 2], 16).unwrap();
        }
        let got: Vec<String> = parse_status(&p)
            .iter()
            .flatten()
            .map(|s| format!("{}:{}", s.slot, s.battery))
            .collect();
        assert_eq!(got.join(";"), exp);
        n += 1;
    }
    assert!(n >= 20, "{n} packets");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn calibration_reassembly_from_real_windows_capture() {
    let replies = load_rec("if0_replies_coldstart.rec.gz");
    let mut asm = CalibAssembler::new();
    let mut blocks = 0;
    for (_, r) in &replies {
        if asm.feed(r) {
            blocks += 1;
        }
    }
    assert!(blocks >= 84);
    assert!(asm.complete());
    assert_eq!(asm.next_missing(), None);
    let xml = asm.document().unwrap();
    let want = std::fs::read(repo_root().join("analysis/headset-calibration.xml")).unwrap();
    assert_eq!(
        xml, want,
        "inflated blob == analysis/headset-calibration.xml"
    );
    // header: zlib length 1318 at offset 0
    assert_eq!(
        u32::from_le_bytes(asm.blob()[..4].try_into().unwrap()),
        1318
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn calibration_round_trip_and_errors() {
    let xml = std::fs::read(repo_root().join("analysis/headset-calibration.xml")).unwrap();
    let blob = calib::build_blob(&xml);
    assert_eq!(blob.len(), calib::TOTAL);
    // through the wire format, in a shuffled order, with duplicates
    let mut asm = CalibAssembler::new();
    let mut addrs: Vec<u16> = (0..calib::BLOCKS as u16).map(|i| i * 16).collect();
    addrs.reverse();
    addrs.extend([0, 16, 0x530]);
    for a in addrs {
        assert!(!asm.complete() || a < 0x540);
        assert!(asm.feed(&calib::make_block_reply(&blob, a)));
    }
    assert_eq!(asm.document().unwrap(), xml);
    // missing block
    let mut part = CalibAssembler::new();
    for i in 1..calib::BLOCKS as u16 {
        part.feed(&calib::make_block_reply(&blob, i * 16));
    }
    assert_eq!(part.next_missing(), Some(0));
    assert_eq!(part.document(), Err(CalibError::Incomplete));
    // corrupt / truncated
    assert_eq!(calib::inflate_blob(&[1, 2, 3]), Err(CalibError::Truncated));
    let mut bad = blob.clone();
    bad[..4].copy_from_slice(&0xffff_ffffu32.to_le_bytes());
    assert_eq!(calib::inflate_blob(&bad), Err(CalibError::Truncated));
    let mut bad = blob.clone();
    bad[20] ^= 0xff;
    bad[21] ^= 0xff;
    assert_eq!(calib::inflate_blob(&bad), Err(CalibError::Inflate));
    // non-block replies are ignored, addresses outside the range too
    let mut r = calib::make_block_reply(&blob, 0x10);
    r[0] = 0x53;
    assert!(!CalibAssembler::new().feed(&r));
    let r = calib::make_block_reply(&blob, 0x540);
    assert!(!CalibAssembler::new().feed(&r));
    let r = calib::make_block_reply(&blob, 0x11);
    assert!(!CalibAssembler::new().feed(&r));
}

#[test]
fn log_lines_from_real_firmware_capture() {
    let chunks = load_rec("if3_log_coldstart.rec.gz");
    // reference: python pimax_logtail.py (rstrip NULs per chunk, split on \n, strip) plus libpimax's
    // rule of dropping CR inside a line (python would keep "\r\r" in the middle of a line)
    let mut buf = Vec::new();
    let mut want: Vec<String> = Vec::new();
    for (_, c) in &chunks {
        let end = c.iter().rposition(|&b| b != 0).map_or(0, |p| p + 1);
        buf.extend(c[..end].iter().copied().filter(|&b| b != b'\r'));
        while let Some(p) = buf.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buf.drain(..=p).collect();
            let t = String::from_utf8_lossy(&line).trim().to_string();
            if !t.is_empty() {
                want.push(t);
            }
        }
    }
    let mut sp = LineSplitter::new();
    let mut got: Vec<String> = Vec::new();
    for (_, c) in &chunks {
        sp.push(c, |l| {
            let t = String::from_utf8_lossy(l).trim().to_string();
            if !t.is_empty() {
                got.push(t);
            }
        });
    }
    assert!(want.len() > 50, "{} lines", want.len());
    assert_eq!(got, want);
    assert!(got.iter().any(|l| l.contains("request_panel_on")));
}

#[test]
fn log_splitter_edge_cases() {
    let mut sp = LineSplitter::new();
    let mut out: Vec<Vec<u8>> = vec![];
    sp.push(b"ab\r\ncd", |l| out.push(l.to_vec()));
    sp.push(b"ef\n\n\0\0gh\n", |l| out.push(l.to_vec()));
    assert_eq!(out, [b"ab".to_vec(), b"cdef".to_vec(), b"gh".to_vec()]);
    // overlong line is flushed at 511 bytes without losing the next byte
    let mut out2: Vec<usize> = vec![];
    let long = vec![b'x'; 1100];
    sp.push(&long, |l| out2.push(l.len()));
    assert_eq!(out2, [511, 511]);
}
