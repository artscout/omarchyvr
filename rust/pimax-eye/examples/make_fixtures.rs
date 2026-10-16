//! Regenerates `fixtures/*.usbpcap.gz` from the Windows captures, which are not in git.
//!
//!     cargo run -p pimax-eye --example make_fixtures -- ~/pimax-re
//!
//! The fixtures are byte-exact slices of the original USBPcap records (same timestamps, same snap-length truncation),
//! so the tests exercise the real wire data. Slices:
//! * `eye_coldstart_init`: every packet of the EyeChip (device 30) from its enumeration to 0.14 s after the first gaze
//!   notification of `20261013-130457-cold-start`: standard requests, the vendor control transfers and the 23 TTP requests;
//! * `eye_gaze_full`: all 364 full-size (1242-byte) gaze notifications of `20261013-130650-full-packets` (nobody wearing);
//! * `eye_gaze_worn_idle` / `eye_gaze_worn_controllers`: windows of 485-byte (snap length) notifications recorded
//!   with the headset worn, chosen to hold all three eye-A states and timestamp gaps.
use flate2::{write::GzEncoder, Compression};
use pimax_eye::analysis::analyze_device;
use pimax_eye::gaze::{EyeState, NOMINAL_PERIOD_US};
use pimax_eye::pcap::{PcapReader, PcapWriter, Transfer, UsbPacket};
use pimax_eye::replay::{EP_BULK_IN, EYE_PID, EYE_VID};
use std::fs::File;
use std::io::BufReader;
use std::path::{Path, PathBuf};

fn read_all(path: &Path, device: u16) -> Vec<UsbPacket> {
    let r = PcapReader::new(BufReader::new(File::open(path).expect("open capture")))
        .expect("pcap header");
    r.map(|p| p.expect("record"))
        .filter(|p| p.device == device)
        .collect()
}

fn write(out: &Path, snaplen: u32, packets: &[&UsbPacket]) {
    let gz = GzEncoder::new(File::create(out).expect("create"), Compression::best());
    let mut w = PcapWriter::new(gz, snaplen).expect("header");
    for p in packets {
        w.write_packet(p).expect("write");
    }
    w.into_inner().finish().expect("finish gzip");
    let size = std::fs::metadata(out).map(|m| m.len()).unwrap_or(0);
    println!(
        "{}: {} packets, {} bytes",
        out.display(),
        packets.len(),
        size
    );
}

fn is_gaze_completion(p: &UsbPacket) -> bool {
    p.transfer == Transfer::Bulk
        && p.endpoint == EP_BULK_IN
        && p.is_completion()
        && p.data_len == 1242
        && p.data().get(8..12) == Some(&[0, 0, 0, 0x53])
}

/// Pick `n` consecutive gaze completions covering all three eye-A states and at least `min_gaps` timestamp gaps.
fn pick_window(pk: &[UsbPacket], n: usize, min_gaps: u64) -> Vec<&UsbPacket> {
    let gaze: Vec<&UsbPacket> = pk.iter().filter(|p| is_gaze_completion(p)).collect();
    let mut best: Option<(u64, usize)> = None;
    for start in (0..gaze.len().saturating_sub(n)).step_by(10) {
        let win = &gaze[start..start + n];
        let mut states = [0u64; 3];
        let mut gaps = 0u64;
        let mut prev: Option<i64> = None;
        for p in win {
            let Ok(s) = pimax_eye::gaze::parse_payload_lenient(&p.data()[32..]) else {
                continue;
            };
            states[match s.eyes[0].state() {
                EyeState::Lost => 0,
                EyeState::OriginOnly => 1,
                EyeState::Tracked => 2,
            }] += 1;
            if let Some(q) = prev {
                if s.timestamp_us - q > NOMINAL_PERIOD_US * 3 / 2 {
                    gaps += 1;
                }
            }
            prev = Some(s.timestamp_us);
        }
        let score =
            states.iter().map(|x| (*x).min(40)).sum::<u64>() * 10 + gaps.min(min_gaps) * 100;
        if states.iter().all(|x| *x >= 20) && gaps >= min_gaps {
            println!("  window start {start}: states {states:?}, {gaps} timestamp gaps");
            return win.to_vec();
        }
        if best.is_none_or(|(b, _)| score > b) {
            best = Some((score, start));
        }
    }
    let start = best.map_or(0, |b| b.1);
    println!("  no ideal window, best start {start}");
    gaze[start..(start + n).min(gaze.len())].to_vec()
}

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| ".".into()));
    let out = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures");
    std::fs::create_dir_all(&out).unwrap();
    println!("EyeChip is {EYE_VID:04x}:{EYE_PID:04x}");

    // cold start init
    let cold = read_all(
        &root.join("captures2/20261013-130457-cold-start-hub2.pcap"),
        30,
    );
    let first_gaze = cold
        .iter()
        .find(|p| is_gaze_completion(p))
        .expect("gaze")
        .time;
    let init: Vec<&UsbPacket> = cold
        .iter()
        .filter(|p| p.time <= first_gaze + 0.14)
        .collect();
    write(&out.join("eye_coldstart_init.usbpcap.gz"), 512, &init);

    // full-size gaze
    let full = read_all(
        &root.join("captures2/20261013-130650-full-packets-hub2.pcap"),
        30,
    );
    let g: Vec<&UsbPacket> = full.iter().filter(|p| is_gaze_completion(p)).collect();
    write(&out.join("eye_gaze_full.usbpcap.gz"), 65535, &g);

    // worn, cut notifications
    let idle = read_all(
        &root.join("captures/20261013-124423-idle-tracking-hub2.pcap"),
        22,
    );
    println!("idle-tracking:");
    let w = pick_window(&idle, 400, 1);
    write(&out.join("eye_gaze_worn_idle.usbpcap.gz"), 512, &w);
    let ctl = read_all(
        &root.join("captures/20261013-124629-controllers-hub2.pcap"),
        22,
    );
    println!("controllers:");
    let w = pick_window(&ctl, 250, 2);
    write(&out.join("eye_gaze_worn_controllers.usbpcap.gz"), 512, &w);

    // sanity: the analysis sees what the tests will assume
    let d = analyze_device(
        30,
        &cold
            .iter()
            .filter(|p| p.time <= first_gaze + 0.14)
            .cloned()
            .collect::<Vec<_>>(),
        |_| {},
    );
    println!(
        "init slice: {} catalog entries, plan {} steps, missing {} bytes",
        d.catalog.len(),
        d.plan.steps.len(),
        d.plan.missing_bytes()
    );
}
