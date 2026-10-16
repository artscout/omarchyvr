//! Property tests: the framing and the decoders are total (never panic), lossless on valid input and consistent under
//! truncation and re-chunking.
use pimax_eye::gaze::{
    encode_payload, parse_payload, parse_payload_lenient, Eye, FrameFields, GazeSample, GazeStats,
    EYE_FIELDS, WIRE_ORDER,
};
use pimax_eye::pcap::{PcapReader, PcapWriter, Transfer, UsbPacket};
use pimax_eye::replay::build_plan;
use pimax_eye::tlv::{fixed16_16_to_f64, fixed22_42_to_f64, push_item, tag, Items, RawItem, Value};
use pimax_eye::ttp::{
    captured_start, Direction, Header, Kind, Message, StreamDecoder, Wrapper, HEADER_LEN,
    MAX_PAYLOAD_LEN, WRAPPER_LEN,
};
use proptest::prelude::*;

// ---------- generators ----------

fn arb_item() -> impl Strategy<Value = (u8, Vec<u8>)> {
    prop_oneof![
        any::<i32>().prop_map(|v| (tag::INT32, v.to_be_bytes().to_vec())),
        any::<i32>().prop_map(|v| (tag::FIXED_16_16, v.to_be_bytes().to_vec())),
        any::<i64>().prop_map(|v| (tag::FIXED_22_42, v.to_be_bytes().to_vec())),
        any::<u32>().prop_map(|v| (tag::COMPOUND, v.to_be_bytes().to_vec())),
        any::<i64>().prop_map(|v| (tag::INT64, v.to_be_bytes().to_vec())),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(|b| {
            let mut v = (b.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&b);
            (tag::STRING, v)
        }),
        prop::collection::vec(any::<u8>(), 0..40).prop_map(|b| {
            let mut v = (b.len() as u32).to_be_bytes().to_vec();
            v.extend_from_slice(&b);
            (tag::BLOB, v)
        }),
        prop::collection::vec(any::<u32>(), 0..6).prop_map(|e| {
            let mut v = (e.len() as u32).to_be_bytes().to_vec();
            for x in e {
                v.extend_from_slice(&x.to_be_bytes());
            }
            (tag::ARRAY, v)
        }),
        any::<u32>().prop_map(|v| (tag::UINT32, v.to_be_bytes().to_vec())),
        prop::collection::vec(any::<i64>(), 0..5).prop_map(|e| {
            let mut v = (e.len() as u32).to_be_bytes().to_vec();
            for x in e {
                v.extend_from_slice(&x.to_be_bytes());
            }
            (tag::ARRAY_FIXED_22_42, v)
        }),
        (
            any::<u8>().prop_filter("unknown tag", |t| !KNOWN_TAGS.contains(t)),
            prop::collection::vec(any::<u8>(), 0..20),
        ),
    ]
}

const KNOWN_TAGS: [u8; 10] = [
    tag::UINT32,
    tag::ARRAY_FIXED_22_42,
    tag::INT32,
    tag::FIXED_16_16,
    tag::FIXED_22_42,
    tag::COMPOUND,
    tag::INT64,
    tag::STRING,
    tag::BLOB,
    tag::ARRAY,
];

fn arb_message() -> impl Strategy<Value = Message> {
    (
        prop_oneof![
            Just(Kind::Request),
            Just(Kind::Response),
            Just(Kind::Notification)
        ],
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        prop::collection::vec(arb_item(), 0..12),
    )
        .prop_map(|(kind, seq, status, id, aux, items)| {
            let mut bytes = Vec::new();
            for (t, v) in items {
                push_item(&mut bytes, t, &v);
            }
            let mut m = Message::new(kind, seq, status, id, &bytes);
            m.header.aux = aux;
            m
        })
}

fn fx16() -> impl Strategy<Value = f64> {
    any::<i32>().prop_map(fixed16_16_to_f64)
}

fn vec3() -> impl Strategy<Value = [f64; 3]> {
    [fx16(), fx16(), fx16()]
}

type EyeRaw = (
    [f64; 3],
    bool,
    [f64; 3],
    bool,
    f64,
    f64,
    [f64; 2],
    bool,
    [f64; 2],
    [i32; 2],
);

fn arb_eye() -> impl Strategy<Value = EyeRaw> {
    (
        vec3(),
        any::<bool>(),
        vec3(),
        any::<bool>(),
        fx16(),
        fx16(),
        [fx16(), fx16()],
        any::<bool>(),
        [
            (-(1i64 << 50)..(1i64 << 50)).prop_map(fixed22_42_to_f64),
            (-(1i64 << 50)..(1i64 << 50)).prop_map(fixed22_42_to_f64),
        ],
        [any::<i32>(), any::<i32>()],
    )
}

fn arb_sample() -> impl Strategy<Value = GazeSample> {
    (
        any::<i64>(),
        any::<u32>(),
        arb_eye(),
        arb_eye(),
        any::<i32>(),
        [vec3(), vec3()],
        [any::<bool>(), any::<bool>()],
        [any::<i32>(), any::<i32>(), any::<i32>()],
        fx16(),
        fx16(),
        any::<i32>(),
    )
        .prop_map(|(ts, ctr, a, b, f14, aux, auxv, flags, f29, f34, f35)| {
            let eye = |g: EyeRaw| Eye {
                origin_mm: g.0,
                origin_valid: g.1,
                direction: g.2,
                direction_valid: g.3,
                pupil_diameter_mm: g.4,
                openness: g.5,
                pupil_pos: g.6,
                pupil_pos_valid: g.7,
                aux_pair: g.8,
                flags: g.9,
                complete: false,
            };
            GazeSample {
                timestamp_us: ts,
                frame_counter: Some(ctr),
                eyes: [eye(a), eye(b)],
                frame: FrameFields {
                    field_14: f14,
                    aux_points: aux,
                    aux_valid: auxv,
                    flags,
                    field_29: f29,
                    field_34: f34,
                    field_35: f35,
                },
                ..GazeSample::default()
            }
        })
}

/// Re-chunk `data` at pseudo-random boundaries derived from `seed`.
fn chunks(data: &[u8], seed: u32, max: usize) -> Vec<&[u8]> {
    let mut out = Vec::new();
    let mut x = seed | 1;
    let mut i = 0;
    while i < data.len() {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let n = 1 + (x >> 8) as usize % max.max(1);
        let end = (i + n).min(data.len());
        out.push(&data[i..end]);
        i = end;
    }
    out
}

fn cfg(cases: u32) -> ProptestConfig {
    ProptestConfig {
        cases,
        failure_persistence: None,
        ..ProptestConfig::default()
    }
}

// ---------- TLV ----------

proptest! {
    #![proptest_config(cfg(3000))]

    #[test]
    fn tlv_iteration_is_total(bytes in prop::collection::vec(any::<u8>(), 0..300)) {
        let mut it = Items::new(&bytes);
        let mut last = 0;
        let mut ended_with_error = false;
        for r in it.by_ref() {
            match r {
                Ok(item) => {
                    let _ = item.decode();
                    prop_assert!(item.offset >= last);
                    last = item.offset;
                }
                Err(_) => { ended_with_error = true; break; }
            }
        }
        prop_assert!(it.next().is_none());
        if !ended_with_error {
            prop_assert_eq!(it.position(), bytes.len());
        }
    }

    #[test]
    fn tlv_roundtrip(items in prop::collection::vec(arb_item(), 0..20)) {
        let mut bytes = Vec::new();
        for (t, v) in &items { push_item(&mut bytes, *t, v); }
        let got: Vec<RawItem> = Items::new(&bytes).collect::<Result<_, _>>().unwrap();
        prop_assert_eq!(got.len(), items.len());
        for (g, (t, v)) in got.iter().zip(&items) {
            prop_assert_eq!(g.tag, *t);
            prop_assert_eq!(g.value, &v[..]);
            // well-formed encodings never decode to Unknown for known tags
            let known = !matches!(g.decode(), Value::Unknown { .. });
            prop_assert_eq!(known, KNOWN_TAGS.contains(t), "tag {}", t);
        }
    }

    #[test]
    fn fixed_point_is_exact(r16 in any::<i32>(), r22 in -(1i64 << 52)..(1i64 << 52)) {
        prop_assert_eq!((fixed16_16_to_f64(r16) * 65536.0) as i64, i64::from(r16));
        prop_assert_eq!((fixed22_42_to_f64(r22) * 4_398_046_511_104.0) as i64, r22);
    }
}

// ---------- TTP framing ----------

proptest! {
    #![proptest_config(cfg(1500))]

    #[test]
    fn wrapper_roundtrip(out in any::<bool>(), body in 0usize..100_000) {
        let dir = if out { Direction::Out } else { Direction::In };
        let w = Wrapper::for_body(dir, body);
        prop_assert_eq!(w.body_len(), body);
        prop_assert_eq!(w.frame_len(), body + WRAPPER_LEN);
        prop_assert_eq!(Wrapper::parse(&w.encode()).unwrap(), w);
    }

    #[test]
    fn header_roundtrip(m in arb_message()) {
        prop_assert_eq!(Header::parse(&m.header.encode()).unwrap(), m.header);
    }

    #[test]
    fn valid_streams_decode_to_the_original_messages_for_any_framing_and_chunking(
        msgs in prop::collection::vec(arb_message(), 0..6),
        out in any::<bool>(),
        max_body in 1usize..300,
        seed in any::<u32>(),
        chunk_max in 1usize..700,
    ) {
        let dir = if out { Direction::Out } else { Direction::In };
        let wire: Vec<u8> = msgs.iter().flat_map(|m| m.to_frames(dir, max_body).concat()).collect();
        let mut dec = StreamDecoder::new(dir);
        let mut got = Vec::new();
        for c in chunks(&wire, seed, chunk_max) {
            for r in dec.push(c) {
                got.push(r.expect("valid stream must not produce errors"));
            }
        }
        prop_assert_eq!(got, msgs);
        prop_assert_eq!(dec.buffered(), 0);
    }

    #[test]
    fn decoder_is_total_and_bounded_on_garbage(
        bytes in prop::collection::vec(any::<u8>(), 0..2000),
        seed in any::<u32>(),
        out in any::<bool>(),
    ) {
        let dir = if out { Direction::Out } else { Direction::In };
        let mut dec = StreamDecoder::new(dir);
        for c in chunks(&bytes, seed, 97) {
            for r in dec.push(c) { let _ = r; }
            prop_assert!(dec.buffered() <= MAX_PAYLOAD_LEN + HEADER_LEN + WRAPPER_LEN + 97);
        }
    }

    #[test]
    fn decoder_recovers_after_a_reported_error(
        garbage in prop::collection::vec(any::<u8>(), 0..64),
        m in arb_message(),
    ) {
        // The wire has no sync marker, so recovery is only promised once an error was reported (the error drops
        // everything buffered); until then a half-read frame may legitimately swallow what follows.
        let mut dec = StreamDecoder::new(Direction::In);
        let reported = dec.push(&garbage).iter().any(Result::is_err);
        if reported {
            prop_assert_eq!(dec.buffered(), 0);
            let wire = m.to_frames(Direction::In, 1000).concat();
            let got: Vec<_> = dec.push(&wire).into_iter().collect::<Result<_, _>>().unwrap();
            prop_assert_eq!(got, vec![m]);
        }
    }

    #[test]
    fn captured_start_is_total(bytes in prop::collection::vec(any::<u8>(), 0..200), out in any::<bool>()) {
        let dir = if out { Direction::Out } else { Direction::In };
        if let Some(s) = captured_start(dir, &bytes) {
            prop_assert!(s.payload.len() <= s.header.payload_len as usize);
            prop_assert_eq!(s.complete, s.payload.len() == s.header.payload_len as usize);
        }
    }

    #[test]
    fn captured_start_of_a_valid_message_is_exact(m in arb_message(), cut in 0usize..400) {
        let wire = m.to_frames(Direction::Out, 1 << 20).concat();
        let n = cut.min(wire.len());
        match captured_start(Direction::Out, &wire[..n]) {
            None => prop_assert!(n < WRAPPER_LEN + HEADER_LEN),
            Some(s) => {
                prop_assert_eq!(s.header, m.header);
                prop_assert_eq!(&s.payload[..], &m.payload[..s.payload.len()]);
                prop_assert_eq!(s.complete, n == wire.len());
            }
        }
    }
}

// ---------- gaze ----------

proptest! {
    #![proptest_config(cfg(800))]

    #[test]
    fn gaze_encode_parse_roundtrip(s in arb_sample()) {
        let payload = encode_payload(&s);
        let back = parse_payload(&payload).unwrap();
        prop_assert!(back.is_complete() && !back.truncated);
        prop_assert!(back.eyes[0].complete && back.eyes[1].complete);
        prop_assert_eq!(&back.eyes, &s.eyes.clone().map(|mut e| { e.complete = true; e }));
        prop_assert_eq!(back.timestamp_us, s.timestamp_us);
        prop_assert_eq!(back.frame_counter, s.frame_counter);
        prop_assert_eq!(&back.frame, &s.frame);
        prop_assert_eq!(encode_payload(&back), payload);
    }

    #[test]
    fn gaze_truncation_is_prefix_consistent(s in arb_sample(), cut_a in 0.0f64..1.0, cut_b in 0.0f64..1.0) {
        let p = encode_payload(&s);
        let full = parse_payload(&p).unwrap();
        let (a, b) = {
            let x = (cut_a * p.len() as f64) as usize;
            let y = (cut_b * p.len() as f64) as usize;
            (x.min(y), x.max(y))
        };
        let ra = parse_payload_lenient(&p[..a]);
        let rb = parse_payload_lenient(&p[..b]);
        for t in [&ra, &rb].into_iter().flatten() {
            prop_assert!(t.truncated);
            prop_assert_eq!(t.fields & !full.fields, 0);
            if t.has(1) { prop_assert_eq!(t.timestamp_us, full.timestamp_us); }
            for (te, fe) in t.eyes.iter().zip(&full.eyes) {
                if te.complete { prop_assert_eq!(&te.origin_mm, &fe.origin_mm); }
            }
        }
        // a longer prefix never loses fields
        if let (Ok(x), Ok(y)) = (&ra, &rb) { prop_assert_eq!(x.fields & !y.fields, 0); }
        // strict decoding accepts the full payload only
        prop_assert!(parse_payload(&p[..b.min(p.len().saturating_sub(1))]).is_err());
    }

    #[test]
    fn gaze_decoders_are_total_on_mutations(
        s in arb_sample(),
        flips in prop::collection::vec((any::<prop::sample::Index>(), any::<u8>()), 0..8),
        drop_tail in 0usize..64,
    ) {
        let mut p = encode_payload(&s);
        for (i, v) in flips { let n = p.len(); p[i.index(n)] ^= v; }
        let keep = p.len().saturating_sub(drop_tail);
        let mut st = GazeStats::default();
        for t in [parse_payload(&p[..keep]), parse_payload_lenient(&p[..keep])].into_iter().flatten() {
            st.add(&t);
            let _ = pimax_eye::gaze::format_line(&t);
        }
    }

    #[test]
    fn gaze_decoders_are_total_on_garbage(bytes in prop::collection::vec(any::<u8>(), 0..400)) {
        let _ = parse_payload(&bytes);
        let _ = parse_payload_lenient(&bytes);
    }

}

#[test]
fn gaze_field_tables_agree() {
    // every wire-order field belongs to exactly one eye group or to the 13 frame fields
    let eye: Vec<u8> = EYE_FIELDS.iter().flatten().copied().collect();
    assert_eq!(eye.len(), 22);
    assert_eq!(WIRE_ORDER.iter().filter(|f| eye.contains(f)).count(), 22);
    assert_eq!(WIRE_ORDER.len() - 22, 13);
}

// ---------- pcap and planning ----------

fn arb_packet() -> impl Strategy<Value = UsbPacket> {
    (
        0u16..4,
        prop_oneof![Just(0x05u8), Just(0x83), Just(0x80), Just(0x00), Just(0x81)],
        prop_oneof![
            Just(Transfer::Bulk),
            Just(Transfer::Control),
            Just(Transfer::Interrupt),
            Just(Transfer::Isochronous)
        ],
        any::<bool>(),
        prop::collection::vec(any::<u8>(), 0..120),
        0u32..5000,
        0.0f64..100.0,
    )
        .prop_map(|(dev, ep, tr, comp, data, extra, t)| {
            let stage = (tr == Transfer::Control).then_some(if comp { 3 } else { 0 });
            let len = data.len() as u32 + extra;
            UsbPacket::build(t, 2, dev, ep, tr, comp, stage, &data, len)
        })
}

proptest! {
    #![proptest_config(cfg(300))]

    #[test]
    fn pcap_roundtrip(pk in prop::collection::vec(arb_packet(), 0..30)) {
        let mut w = PcapWriter::new(Vec::new(), 512).unwrap();
        for p in &pk { w.write_packet(p).unwrap(); }
        let bytes = w.into_inner();
        let back: Vec<UsbPacket> = PcapReader::new(&bytes[..]).unwrap().collect::<Result<_, _>>().unwrap();
        prop_assert_eq!(back.len(), pk.len());
        for (a, b) in pk.iter().zip(&back) {
            prop_assert_eq!(a.raw(), b.raw());
            prop_assert_eq!((a.device, a.endpoint, a.transfer, a.stage, a.data_len), (b.device, b.endpoint, b.transfer, b.stage, b.data_len));
        }
    }

    #[test]
    fn plan_builder_is_total_and_consistent(pk in prop::collection::vec(arb_packet(), 0..60)) {
        let mine: Vec<UsbPacket> = pk.into_iter().filter(|p| p.device == 1).collect();
        let plan = build_plan(1, &mine);
        for s in &plan.steps {
            prop_assert!(s.missing_bytes <= s.total_bytes);
            prop_assert_eq!(s.is_complete(), s.missing_bytes == 0);
        }
        prop_assert_eq!(plan.missing_bytes(), plan.steps.iter().map(|s| s.missing_bytes).sum::<u64>());
    }

    #[test]
    fn pcap_reader_is_total_on_garbage(body in prop::collection::vec(any::<u8>(), 0..400)) {
        let mut b = Vec::new();
        b.extend_from_slice(&[0xd4, 0xc3, 0xb2, 0xa1, 2, 0, 4, 0]);
        b.extend_from_slice(&[0; 8]);
        b.extend_from_slice(&512u32.to_le_bytes());
        b.extend_from_slice(&249u32.to_le_bytes());
        b.extend_from_slice(&body);
        if let Ok(r) = PcapReader::new(&b[..]) { for p in r { let _ = p; } }
    }
}
