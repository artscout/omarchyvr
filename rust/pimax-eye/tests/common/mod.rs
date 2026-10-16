//! Shared helpers: fixture loading and the golden-line format of `fixtures/make_golden.py`.
#![allow(dead_code)]
use flate2::read::GzDecoder;
use pimax_eye::gaze::{GazeSample, EYE_FIELDS, WIRE_ORDER};
use pimax_eye::pcap::{PcapReader, Transfer, UsbPacket};
use pimax_eye::replay::EP_BULK_IN;
use std::io::Read;
use std::path::PathBuf;

pub fn path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures")
        .join(name)
}

pub fn gunzip(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    GzDecoder::new(std::fs::File::open(path(name)).unwrap_or_else(|e| panic!("{name}: {e}")))
        .read_to_end(&mut out)
        .unwrap();
    out
}

pub fn packets(name: &str) -> Vec<UsbPacket> {
    let bytes = gunzip(name);
    PcapReader::new(&bytes[..])
        .unwrap()
        .collect::<Result<Vec<_>, _>>()
        .unwrap()
}

pub fn golden(name: &str) -> Vec<String> {
    String::from_utf8(gunzip(name))
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

/// Gaze notification completions (IN, bulk, EP 0x83, TTP notification magic).
pub fn gaze_packets(pk: &[UsbPacket]) -> Vec<&UsbPacket> {
    pk.iter()
        .filter(|p| {
            p.transfer == Transfer::Bulk
                && p.endpoint == EP_BULK_IN
                && p.is_completion()
                && p.data().get(8..12) == Some(&[0, 0, 0, 0x53])
        })
        .collect()
}

fn raw16(x: f64) -> i64 {
    (x * 65536.0).round() as i64
}

fn raw22(x: f64) -> i64 {
    (x * 4_398_046_511_104.0).round() as i64
}

/// Raw integer values of field `f` of `s`, as the independent Python decoder prints them.
fn field_raw(s: &GazeSample, f: u8) -> Vec<i64> {
    use pimax_eye::gaze::field as F;
    let slot = EYE_FIELDS
        .iter()
        .enumerate()
        .find_map(|(g, row)| row.iter().position(|x| *x == f).map(|i| (g, i)));
    let v3 = |v: [f64; 3]| v.iter().map(|c| raw16(*c)).collect::<Vec<_>>();
    if let Some((g, i)) = slot {
        let e = &s.eyes[g];
        return match i {
            0 => v3(e.origin_mm),
            1 => vec![i64::from(e.origin_valid)],
            2 => v3(e.direction),
            3 => vec![i64::from(e.direction_valid)],
            4 => vec![raw16(e.pupil_diameter_mm)],
            5 => vec![raw16(e.openness)],
            6 => e.aux_pair.iter().map(|c| raw22(*c)).collect(),
            7 => e.pupil_pos.iter().map(|c| raw16(*c)).collect(),
            8 => vec![i64::from(e.pupil_pos_valid)],
            9 => vec![i64::from(e.flags[0])],
            _ => vec![i64::from(e.flags[1])],
        };
    }
    let fr = &s.frame;
    match f {
        F::TIMESTAMP => vec![s.timestamp_us],
        F::COUNTER => vec![i64::from(s.frame_counter.unwrap() as i32)],
        F::FIELD_14 => vec![i64::from(fr.field_14)],
        F::AUX_POINT_1 => v3(fr.aux_points[0]),
        F::AUX_VALID_1 => vec![i64::from(fr.aux_valid[0])],
        F::AUX_POINT_2 => v3(fr.aux_points[1]),
        F::AUX_VALID_2 => vec![i64::from(fr.aux_valid[1])],
        F::FLAG_26 => vec![i64::from(fr.flags[0])],
        F::FLAG_27 => vec![i64::from(fr.flags[1])],
        F::FLAG_28 => vec![i64::from(fr.flags[2])],
        F::FIELD_29 => vec![raw16(fr.field_29)],
        F::FIELD_34 => vec![raw16(fr.field_34)],
        F::FIELD_35 => vec![i64::from(fr.field_35)],
        _ => unreachable!("field {f}"),
    }
}

pub fn golden_line(s: &GazeSample) -> String {
    WIRE_ORDER
        .iter()
        .filter(|f| s.has(**f))
        .map(|f| {
            let v: Vec<String> = field_raw(s, *f).iter().map(i64::to_string).collect();
            format!("{f}={}", v.join(","))
        })
        .collect::<Vec<_>>()
        .join(" ")
}

use pimax_eye::replay::{build_plan, Plan, Step, StepKind};
use pimax_eye::ttp::{Direction, Message};

/// The real cold-start session plan (39 steps), cut at the two uploads the capture truncates.
pub fn cold_start_plan() -> Plan {
    build_plan(30, &packets("eye_coldstart_init.usbpcap.gz"))
}

/// The same plan with the missing upload bytes FABRICATED (zeros), so the replay engine can be exercised end to end
/// against the fake chip. Never used with real hardware.
pub fn fabricated_complete_plan() -> Plan {
    let mut plan = cold_start_plan();
    for step in &mut plan.steps {
        if step.is_complete() {
            continue;
        }
        let StepKind::Request {
            seq,
            opcode,
            payload_len,
            frames,
            ..
        } = &mut step.kind
        else {
            panic!("only requests are cut")
        };
        // build exact-size frames: header + payload_len zero-ish bytes, split like Windows (4087-byte bodies)
        let mut m = Message::new(Kind::Request, *seq, 0, *opcode, &[]);
        m.payload = vec![0u8; *payload_len as usize];
        m.header.payload_len = *payload_len;
        *frames = m.to_frames(Direction::Out, 4087);
        step.missing_bytes = 0;
    }
    plan
}

pub fn step_frames(s: &Step) -> Vec<Vec<u8>> {
    match &s.kind {
        StepKind::Request { frames, .. } => frames.clone(),
        _ => Vec::new(),
    }
}

use pimax_eye::ttp::Kind;
