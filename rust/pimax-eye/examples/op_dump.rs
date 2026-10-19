//! Save the payload of every TTP message with one opcode from a USBPcap capture, one file per message
//! (`PREFIX-<n>-<req|resp>.bin`), reassembled by the stream decoder (long answers span several bulk transfers).
//! Offline analysis only (used for the Crystal Super's profile download 0x44c and upload 0x456, 2026-10-18).
//!
//! `cargo run --release -p pimax-eye --example op_dump -- CAPTURE.pcap DEVICE OPCODE_HEX|all PREFIX`

use pimax_eye::pcap::{PcapReader, Transfer};
use pimax_eye::ttp::{Direction, Kind, StreamDecoder};
use std::fs::File;
use std::io::BufReader;

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    if a.len() < 4 {
        eprintln!("usage: op_dump CAPTURE.pcap DEVICE OPCODE_HEX PREFIX");
        std::process::exit(1);
    }
    let dev: u16 = a[1].parse().expect("device");
    // "all": list every request and response (no files)
    let all = a[2] == "all";
    let op = if all { 0 } else { u32::from_str_radix(a[2].trim_start_matches("0x"), 16).expect("opcode") };
    let mut r = PcapReader::new(BufReader::with_capacity(1 << 20, File::open(&a[0]).expect("open")))
        .expect("pcap");
    let mut dec_in = StreamDecoder::new(Direction::In);
    let mut dec_out = StreamDecoder::new(Direction::Out);
    let mut n = 0usize;
    while let Some(p) = r.next_packet().expect("read") {
        if p.device != dev || p.transfer != Transfer::Bulk {
            continue;
        }
        let (dec, what) = if p.is_in() && p.is_completion() {
            (&mut dec_in, "resp")
        } else if !p.is_in() && !p.is_completion() {
            (&mut dec_out, "req")
        } else {
            continue;
        };
        for m in dec.push(p.data()).into_iter().flatten() {
            if m.header.kind == Kind::Notification {
                continue;
            }
            if all {
                println!("{:9.3} s {what:4} seq {:3} op {:#06x} payload {:6} B", p.time, m.header.seq, m.header.id, m.payload.len());
                continue;
            }
            if m.header.id != op {
                continue;
            }
            let path = format!("{}-{n}-{what}.bin", a[3]);
            std::fs::write(&path, &m.payload).expect("write");
            println!("{:.3} s seq {} {what} payload {} B -> {path}", p.time, m.header.seq, m.payload.len());
            n += 1;
        }
    }
}
