//! Dump every TTP notification of one stream id from a USBPcap capture as one line of decoded TLV items
//! (`time_s stream item item ...`). Offline analysis only (used for the 0x0509 "wearable foveated" stream).
//!
//! `cargo run --release -p pimax-eye --example notes_dump -- CAPTURE.pcap DEVICE STREAM_HEX [MAX]`

use pimax_eye::pcap::{PcapReader, Transfer};
use pimax_eye::tlv::{fixed16_16_to_f64, fixed22_42_to_f64, Items, Value};
use pimax_eye::ttp::{captured_start, Direction, Kind};
use std::fs::File;
use std::io::{BufReader, Write};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 3 {
        eprintln!("usage: notes_dump CAPTURE.pcap DEVICE STREAM_HEX [MAX]");
        std::process::exit(1);
    }
    let dev: u16 = a[1].parse().expect("device");
    let stream = u32::from_str_radix(a[2].trim_start_matches("0x"), 16).expect("stream");
    let max: usize = a.get(3).map_or(usize::MAX, |s| s.parse().expect("max"));
    let mut r = PcapReader::new(BufReader::with_capacity(1 << 20, File::open(&a[0]).expect("open")))
        .expect("pcap");
    let out = std::io::stdout();
    let mut out = out.lock();
    let mut n = 0usize;
    while let Some(p) = r.next_packet().expect("read") {
        if p.device != dev || p.transfer != Transfer::Bulk || !p.is_in() || !p.is_completion() {
            continue;
        }
        let Some(s) = captured_start(Direction::In, p.data()) else {
            continue;
        };
        if s.header.kind != Kind::Notification || s.header.id != stream || s.payload.len() < 2 {
            continue;
        }
        let mut line = format!("{:.6} {:#06x}", p.time, stream);
        for it in Items::new(&s.payload[2..]).flatten() {
            let t = match it.decode() {
                Value::Int32(x) => format!(" i{x}"),
                Value::Uint32(x) => format!(" u{x}"),
                Value::Int64(x) => format!(" l{x}"),
                Value::Fixed16_16(x) => format!(" f{:.5}", fixed16_16_to_f64(x)),
                Value::Fixed22_42(x) => format!(" g{:.4}", fixed22_42_to_f64(x)),
                Value::Compound { count, type_id } => format!(" c{count}:{type_id:#x}"),
                other => format!(" ?{other:?}"),
            };
            line.push_str(&t);
        }
        let _ = writeln!(out, "{line}");
        n += 1;
        if n >= max {
            break;
        }
    }
}
