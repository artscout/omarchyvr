//! Decoder and planner against byte-exact slices of the real Windows captures (see examples/make_fixtures.rs).
mod common;

use common::*;
use pimax_eye::gaze::{
    encode_payload, parse_payload, parse_payload_lenient, EyeState, GazeStats, NOMINAL_PERIOD_US,
};
use pimax_eye::pcap::{PcapReader, PcapWriter};
use pimax_eye::replay::{build_plan, find_eye_devices, StepKind};
use pimax_eye::tlv::{Items, Value};
use pimax_eye::ttp::{Direction, Kind, StreamDecoder, STREAM_GAZE};

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn full_size_notifications_match_the_independent_decoder_and_reencode_byte_exact() {
    let pk = packets("eye_gaze_full.usbpcap.gz");
    let gold = golden("eye_gaze_full.golden.gz");
    let g = gaze_packets(&pk);
    assert_eq!(g.len(), 364);
    assert_eq!(gold.len(), 364);
    let mut stats = GazeStats::default();
    for (i, p) in g.iter().enumerate() {
        assert_eq!(
            p.data().len(),
            1242,
            "full snap length keeps the whole frame"
        );
        let payload = &p.data()[32..];
        let s = parse_payload(payload).unwrap_or_else(|e| panic!("sample {i}: {e}"));
        assert!(s.is_complete() && !s.truncated && s.eyes[0].complete && s.eyes[1].complete);
        assert_eq!(
            s.frame_counter,
            Some(5412 + i as u32),
            "counter +1 per packet"
        );
        assert_eq!(golden_line(&s), gold[i], "sample {i}");
        assert_eq!(
            encode_payload(&s),
            payload,
            "sample {i} must re-encode to the same bytes"
        );
        // nobody wore the headset: everything invalid, sentinel values
        for e in &s.eyes {
            assert_eq!(e.state(), EyeState::Lost);
            assert_eq!(e.pupil_diameter_mm, -1.0);
            assert_eq!(e.openness, -1.0);
            assert_eq!(e.pupil_pos, [-1.0, -1.0]);
            assert_eq!(e.aux_pair, [-1.0, -1.0]);
            assert_eq!(e.flags, [1, 0]);
        }
        assert_eq!(s.frame.field_14, 0xfffff);
        assert_eq!(s.frame.field_35, 1);
        assert_eq!(s.frame.field_29, -1.0);
        assert!((s.frame.field_34 - 57.353_592).abs() < 1e-5);
        assert_eq!(s.unknown_fields, 0);
        stats.add(&s);
    }
    assert_eq!(stats.counter_gaps, 0);
    assert_eq!(stats.timestamp_gaps, 0);
    assert_eq!(stats.timestamp_regressions, 0);
    let hz = stats.device_rate_hz().unwrap();
    assert!((120.40..120.60).contains(&hz), "{hz}");
    assert_eq!(stats.first_ts, Some(50_376_465));
}

fn check_worn(name: &str, expected_states: [u64; 3]) {
    let pk = packets(&format!("{name}.usbpcap.gz"));
    let gold = golden(&format!("{name}.golden.gz"));
    let g = gaze_packets(&pk);
    assert_eq!(g.len(), gold.len());
    let mut states = [0u64; 3];
    let mut stats = GazeStats::default();
    for (i, p) in g.iter().enumerate() {
        assert_eq!(p.data().len(), 485, "snap length 512 keeps 485 data bytes");
        assert_eq!(p.data_len, 1242);
        let payload = &p.data()[32..];
        assert!(
            parse_payload(payload).is_err(),
            "strict decode must reject a cut payload"
        );
        let s = parse_payload_lenient(payload).unwrap_or_else(|e| panic!("sample {i}: {e}"));
        assert!(s.truncated);
        assert!(s.eyes[0].complete, "eye A is fully inside 485 bytes");
        assert!(!s.eyes[1].complete, "eye B is cut off");
        assert!(s.frame_counter.is_none(), "field 15 is beyond the cut");
        assert_eq!(golden_line(&s), gold[i], "sample {i}");
        stats.add(&s);
        let a = &s.eyes[0];
        // validity hierarchy: direction valid implies origin valid; pupil and pupil position follow the direction
        assert!(!a.direction_valid || a.origin_valid, "sample {i}");
        assert_eq!(a.pupil_pos_valid, a.direction_valid, "sample {i}");
        assert_eq!(
            a.pupil_diameter().is_some(),
            a.direction_valid,
            "sample {i}"
        );
        assert_eq!(a.openness, -1.0);
        assert_eq!(a.flags[0], 1);
        match a.state() {
            EyeState::Tracked => {
                states[2] += 1;
                let n = a.direction.iter().map(|c| c * c).sum::<f64>().sqrt();
                assert!(
                    (n - 1.0).abs() < 2e-4,
                    "direction must be a unit vector, got {n} (sample {i})"
                );
                assert!(a.direction[2] > 0.5, "looking forward: +z (sample {i})");
                let d = a.pupil_diameter().unwrap();
                assert!((1.0..8.0).contains(&d), "pupil {d}");
                assert!(
                    a.pupil_pos.iter().all(|c| (0.0..=1.0).contains(c)),
                    "{:?}",
                    a.pupil_pos
                );
                assert!(
                    (20.0..50.0).contains(&a.origin_mm[0]),
                    "eye A origin x is about +31 mm: {:?}",
                    a.origin_mm
                );
                assert!(
                    (-40.0..-10.0).contains(&a.origin_mm[2]),
                    "{:?}",
                    a.origin_mm
                );
                assert_eq!(
                    a.flags[1], 0,
                    "flag 31 is 0 whenever the eye is fully tracked (sample {i})"
                );
                let (yaw, pitch) = a.yaw_pitch_deg().unwrap();
                assert!(yaw.abs() < 60.0 && pitch.abs() < 60.0);
            }
            EyeState::OriginOnly => {
                states[1] += 1;
                assert_eq!(a.direction, [0.0; 3]);
                assert_eq!(
                    a.flags[1], 1,
                    "flag 31 is 1 when only the origin is valid (sample {i})"
                );
            }
            EyeState::Lost => {
                states[0] += 1;
                assert_eq!(a.origin_mm, [0.0; 3]);
            }
        }
    }
    assert_eq!(states, expected_states, "{name}");
    assert_eq!(stats.states[0], states);
    assert_eq!(
        stats.states[1],
        [g.len() as u64, 0, 0],
        "eye B is absent from the cut packets"
    );
    assert_eq!(stats.counter_gaps, 0, "no counter in cut packets");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn worn_idle_window_decodes_like_the_independent_decoder() {
    check_worn("eye_gaze_worn_idle", [41, 20, 339]);
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn worn_controllers_window_decodes_like_the_independent_decoder() {
    check_worn("eye_gaze_worn_controllers", [76, 25, 149]);
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn worn_windows_contain_timestamp_gaps_and_the_expected_rate() {
    for (name, gaps) in [("eye_gaze_worn_idle", 1), ("eye_gaze_worn_controllers", 3)] {
        let pk = packets(&format!("{name}.usbpcap.gz"));
        let mut st = GazeStats::default();
        for p in gaze_packets(&pk) {
            st.add(&parse_payload_lenient(&p.data()[32..]).unwrap());
        }
        assert_eq!(st.timestamp_gaps, gaps, "{name}");
        assert_eq!(st.timestamp_regressions, 0);
        let hz = st.device_rate_hz().unwrap();
        assert!((115.0..121.0).contains(&hz), "{name}: {hz}");
    }
    assert_eq!(NOMINAL_PERIOD_US, 8300);
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn cold_start_session_start_plan() {
    let pk = packets("eye_coldstart_init.usbpcap.gz");
    assert_eq!(find_eye_devices(&pk), vec![30]);
    let plan = build_plan(30, &pk);
    assert_eq!(plan.steps.len(), 39, "{}", plan.describe());
    assert!(plan.stream_start.is_some());
    assert_eq!(plan.orphan_frames, 0);
    assert!(!plan.is_complete());

    // the 21 requests, in order
    let ops: Vec<u32> = plan
        .requests()
        .map(|s| match &s.kind {
            StepKind::Request { opcode, .. } => *opcode,
            _ => unreachable!(),
        })
        .collect();
    assert_eq!(
        ops,
        vec![
            0x3e8, 0x640, 0x76c, 0x777, 0x852, 0x77b, 0x532, 0x58c, 0x546, 0x547, 0x5d2, 0x5cd,
            0x4b0, 0x596, 0x578, 0x686, 0x67c, 0x5be, 0x672, 0x4c4, 0x456
        ]
    );
    // sequence numbers are 1..=21 and every request was answered with status 1
    for (i, s) in plan.requests().enumerate() {
        let StepKind::Request { seq, expect, .. } = &s.kind else {
            unreachable!()
        };
        assert_eq!(*seq, i as u32 + 1);
        assert_eq!(expect.map(|e| e.status), Some(1), "request {seq}");
    }
    // exactly the two big uploads are cut by the snap length
    let cut: Vec<_> = plan
        .incomplete_steps()
        .map(|s| match &s.kind {
            StepKind::Request {
                seq,
                opcode,
                frames,
                ..
            } => (*seq, *opcode, frames.len(), s.missing_bytes),
            _ => panic!("a control step is incomplete"),
        })
        .collect();
    assert_eq!(cut, vec![(5, 0x852, 1, 2318), (21, 0x456, 9, 30343)]);
    assert_eq!(plan.missing_bytes(), 32661);
    assert_eq!(plan.auth_steps().count(), 3);

    // the four leading control transfers (session info, module clock, TTP session start)
    let setups: Vec<[u8; 8]> = plan
        .steps
        .iter()
        .take(4)
        .map(|s| match &s.kind {
            StepKind::ControlOut { setup, .. } | StepKind::ControlIn { setup } => *setup,
            _ => panic!("expected control"),
        })
        .collect();
    assert_eq!(setups[0], [0x41, 0x30, 0, 0, 0, 0, 24, 0]);
    assert_eq!(setups[1], [0xc1, 0x30, 0, 0, 0, 0, 0, 2]);
    assert_eq!(setups[2], [0xc1, 0x46, 1, 0, 0, 0, 8, 0]);
    assert_eq!(setups[3], [0x41, 0x41, 0, 0, 0, 0, 0, 0]);
    let StepKind::ControlOut { data, .. } = &plan.steps[0].kind else {
        unreachable!()
    };
    assert_eq!(
        data,
        &[1, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 4, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn complete_cold_start_requests_decode_into_tlv_items() {
    let pk = packets("eye_coldstart_init.usbpcap.gz");
    let plan = build_plan(30, &pk);
    for s in plan.requests().filter(|s| s.is_complete()) {
        let StepKind::Request {
            frames,
            opcode,
            seq,
            ..
        } = &s.kind
        else {
            unreachable!()
        };
        let mut dec = StreamDecoder::new(Direction::Out);
        let msgs: Vec<_> = frames.iter().flat_map(|f| dec.push(f)).collect();
        assert_eq!(msgs.len(), 1, "request {seq}");
        let m = msgs.into_iter().next().unwrap().unwrap();
        assert_eq!(
            (m.header.kind, m.header.seq, m.header.id),
            (Kind::Request, *seq, *opcode)
        );
        let items: Vec<_> = m.items().collect::<Result<_, _>>().unwrap();
        let vals: Vec<Value> = items.iter().map(|i| i.decode()).collect();
        assert!(
            !vals.iter().any(|v| matches!(v, Value::Unknown { .. })),
            "op {opcode:#x}: {vals:?}"
        );
        match opcode {
            // subscribe stream 0x0505 with an empty option array
            0x4c4 => assert_eq!(
                vals,
                vec![
                    Value::Int32(0x505),
                    Value::Array {
                        count: 0,
                        elements: &[]
                    }
                ]
            ),
            // set gaze rate 120 Hz
            0x67c => assert_eq!(
                vals,
                vec![
                    Value::Compound {
                        count: 2,
                        type_id: 0x2af8
                    },
                    Value::Int32(120),
                    Value::Int32(120)
                ]
            ),
            // eye offsets: +32 mm and -32 mm on x (IPD 64 mm), as two vec3 of S22.42
            0x5be => {
                assert_eq!(vals.len(), 8);
                assert_eq!(
                    vals[0],
                    Value::Compound {
                        count: 3,
                        type_id: 0x1f41
                    }
                );
                assert_eq!(vals[1], Value::Fixed22_42(32 << 42));
                assert_eq!(vals[5], Value::Fixed22_42(-(32 << 42)));
            }
            // authentication step 2: two ints, a 16-byte answer and a 4-byte tag
            0x777 => {
                assert!(matches!(vals[2], Value::Blob(b) if b.len() == 16));
                assert!(matches!(vals[3], Value::Blob(b) if b.len() == 4));
            }
            _ => {}
        }
    }
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn complete_cold_start_responses_decode_and_name_the_device() {
    let pk = packets("eye_coldstart_init.usbpcap.gz");
    let mut found_ids = false;
    let mut responses = 0;
    for p in pk
        .iter()
        .filter(|p| p.endpoint == 0x83 && p.is_completion() && p.data_len > 0)
    {
        if p.is_truncated() {
            continue;
        }
        let msgs = StreamDecoder::new(Direction::In).push(p.data());
        for m in msgs.into_iter().flatten() {
            if m.header.kind != Kind::Response {
                continue;
            }
            responses += 1;
            assert_eq!(m.header.status, 1);
            let vals: Vec<Value> = m.items().map(|i| i.unwrap().decode()).collect();
            if m.header.id == 0x58c {
                let strs: Vec<String> = vals
                    .iter()
                    .filter_map(|v| {
                        if let Value::Str(s) = v {
                            Some(String::from_utf8_lossy(s).into_owned())
                        } else {
                            None
                        }
                    })
                    .collect();
                assert_eq!(strs.len(), 4, "{vals:?}");
                assert!(strs[0].starts_with("XR5DA-"), "{strs:?}");
                assert_eq!(strs[1], "XR5_PIMAX_DA");
                assert_eq!(strs[2], "XR5");
                assert!(strs[3].ends_with("-7.32.0"), "{strs:?}");
                found_ids = true;
            }
        }
    }
    assert!(found_ids);
    assert!(responses >= 15, "{responses}");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn stream_decoder_handles_the_real_notification_stream_with_any_chunking() {
    let pk = packets("eye_gaze_full.usbpcap.gz");
    let wire: Vec<u8> = gaze_packets(&pk)
        .iter()
        .flat_map(|p| p.data().to_vec())
        .collect();
    assert_eq!(wire.len(), 364 * 1242);
    for chunk in [1usize, 7, 512, 1242, 1243, 4096, 100_000] {
        let mut dec = StreamDecoder::new(Direction::In);
        let mut n = 0;
        let mut counter = 5412u32;
        for c in wire.chunks(chunk) {
            for m in dec.push(c) {
                let m = m.unwrap();
                assert_eq!(
                    (m.header.kind, m.header.id, m.header.seq),
                    (Kind::Notification, STREAM_GAZE, 0)
                );
                let s = parse_payload(&m.payload).unwrap();
                assert_eq!(s.frame_counter, Some(counter));
                counter += 1;
                n += 1;
            }
        }
        assert_eq!(n, 364, "chunk size {chunk}");
        assert_eq!(dec.buffered(), 0);
    }
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn pcap_writer_reproduces_the_fixture_bytes() {
    for name in [
        "eye_coldstart_init.usbpcap.gz",
        "eye_gaze_full.usbpcap.gz",
        "eye_gaze_worn_idle.usbpcap.gz",
    ] {
        let original = gunzip(name);
        let mut r = PcapReader::new(&original[..]).unwrap();
        let snap = r.snaplen();
        let mut w = PcapWriter::new(Vec::new(), snap).unwrap();
        for p in r.by_ref() {
            w.write_packet(&p.unwrap()).unwrap();
        }
        assert_eq!(w.into_inner(), original, "{name}");
    }
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn items_iterator_consumes_every_real_payload_exactly() {
    for p in gaze_packets(&packets("eye_gaze_full.usbpcap.gz")) {
        let payload = &p.data()[34..];
        let mut it = Items::new(payload);
        let items: Vec<_> = it.by_ref().collect::<Result<_, _>>().unwrap();
        assert_eq!(items.len(), 132);
        assert_eq!(it.position(), payload.len());
    }
}
