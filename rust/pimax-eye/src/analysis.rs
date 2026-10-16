//! Offline analysis of USBPcap captures: which devices speak TTP, what was said, how the gaze stream looked, and
//! whether the session start can be replayed.

use crate::gaze::{
    format_line, parse_payload, parse_payload_lenient, EyeState, GazeSample, GazeStats,
};
use crate::pcap::{PcapError, PcapReader, Transfer, UsbPacket};
use crate::replay::{build_plan, DeviceFinder, Plan, EP_BULK_IN, EP_BULK_OUT};
use crate::ttp::{captured_start, opcode_name, Direction, Kind, Wrapper, STREAM_GAZE};
use std::fmt::Write as _;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::path::Path;

/// One kind of TTP message seen in a capture.
#[derive(Debug, Clone, PartialEq)]
pub struct CatalogEntry {
    /// Direction of the bulk transfer.
    pub dir: Direction,
    /// Message class.
    pub kind: Kind,
    /// Opcode or stream id.
    pub id: u32,
    /// Messages of this kind.
    pub count: u64,
    /// Capture time of the first one.
    pub first_time: f64,
    /// Status word of the first one.
    pub status: u32,
    /// Smallest and largest announced payload length.
    pub payload_len: (u32, u32),
    /// Messages whose payload is cut by the capture snap length.
    pub cut: u64,
}

/// Gaze stream statistics of one device.
#[derive(Debug, Clone, Default)]
pub struct GazeSummary {
    /// Notifications seen.
    pub frames: u64,
    /// Notifications cut by the capture snap length (decoded leniently).
    pub cut_frames: u64,
    /// Notifications that failed to decode.
    pub parse_errors: u64,
    /// Completions whose USB length differs from the frame length in the wrapper (the Windows stack's 16 KiB read
    /// buffers in the `poweron` capture); they repeat stale data and are left out of the statistics.
    pub oversized: u64,
    /// First decode error.
    pub first_error: Option<String>,
    /// Statistics over the decoded samples.
    pub stats: GazeStats,
    /// Capture time of the first and last notification.
    pub span: Option<(f64, f64)>,
}

impl GazeSummary {
    /// Rate by the capture clock.
    pub fn capture_rate_hz(&self) -> Option<f64> {
        let (a, b) = self.span?;
        (self.frames > 1 && b > a).then(|| (self.frames - 1) as f64 / (b - a))
    }
}

/// Everything known about one device of a capture.
#[derive(Debug)]
pub struct DeviceReport {
    /// Device address.
    pub device: u16,
    /// Packets of this device kept for the analysis.
    pub packets: usize,
    /// TTP message kinds in order of first appearance.
    pub catalog: Vec<CatalogEntry>,
    /// Gaze stream summary.
    pub gaze: GazeSummary,
    /// Session-start replay plan.
    pub plan: Plan,
}

/// Result of [`analyze_file`].
#[derive(Debug)]
pub struct CaptureReport {
    /// Snap length from the pcap header.
    pub snaplen: u32,
    /// Packets in the file.
    pub packets: u64,
    /// Seconds between the first and the last packet.
    pub duration: f64,
    /// One report per TTP device.
    pub devices: Vec<DeviceReport>,
}

/// One gaze sample with its capture coordinates.
#[derive(Debug, Clone)]
pub struct CapturedSample {
    /// Record number of the completion.
    pub record: u64,
    /// Capture time.
    pub time: f64,
    /// Decoded sample.
    pub sample: GazeSample,
}

fn open(path: &Path) -> Result<PcapReader<BufReader<File>>, PcapError> {
    PcapReader::new(BufReader::with_capacity(1 << 20, File::open(path)?))
}

/// Analyse `path`. With `device` set only that address is examined, otherwise every address that carries TTP traffic.
pub fn analyze_file(path: &Path, device: Option<u16>) -> Result<CaptureReport, PcapError> {
    let mut reader = open(path)?;
    let snaplen = reader.snaplen();
    let mut finder = DeviceFinder::default();
    let mut total = 0u64;
    let mut last = 0.0;
    while let Some(p) = reader.next_packet()? {
        total += 1;
        last = p.time;
        if device.is_none() {
            finder.observe(&p);
        }
    }
    let wanted = device.map_or_else(|| finder.devices(), |d| vec![d]);
    let mut kept: Vec<Vec<UsbPacket>> = wanted.iter().map(|_| Vec::new()).collect();
    let mut reader = open(path)?;
    while let Some(p) = reader.next_packet()? {
        if let Some(i) = wanted.iter().position(|d| *d == p.device) {
            kept[i].push(p);
        }
    }
    let devices = wanted
        .iter()
        .zip(kept)
        .map(|(d, pk)| analyze_device(*d, &pk, |_| {}))
        .collect();
    Ok(CaptureReport {
        snaplen,
        packets: total,
        duration: last,
        devices,
    })
}

/// Collect the gaze samples of one device of a capture (for CSV export).
pub fn collect_samples(path: &Path, device: u16) -> Result<Vec<CapturedSample>, PcapError> {
    let mut reader = open(path)?;
    let mut out = Vec::new();
    while let Some(p) = reader.next_packet()? {
        if p.device != device {
            continue;
        }
        if let Some(Ok(sample)) = gaze_of(&p) {
            out.push(CapturedSample {
                record: p.index,
                time: p.time,
                sample,
            });
        }
    }
    Ok(out)
}

/// Why a gaze notification was not turned into a sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GazeSkip {
    /// USB completion longer than the frame (stale 16 KiB read buffer).
    Oversized,
    /// Payload failed to decode.
    Parse(String),
}

/// Decode the gaze notification in a packet, if it holds one. Cut payloads are decoded leniently.
fn gaze_of(p: &UsbPacket) -> Option<Result<GazeSample, GazeSkip>> {
    if p.transfer != Transfer::Bulk || p.endpoint != EP_BULK_IN || !p.is_completion() {
        return None;
    }
    let start = captured_start(Direction::In, p.data())?;
    if start.header.kind != Kind::Notification || start.header.id != STREAM_GAZE {
        return None;
    }
    let frame_len = Wrapper::parse(p.data()).ok()?.frame_len();
    if p.data_len as usize != frame_len {
        return Some(Err(GazeSkip::Oversized));
    }
    let r = if start.complete {
        parse_payload(&start.payload)
    } else {
        parse_payload_lenient(&start.payload)
    };
    Some(r.map_err(|e| GazeSkip::Parse(e.to_string())))
}

/// Analyse the packets of one device.
pub fn analyze_device(
    device: u16,
    packets: &[UsbPacket],
    mut on_sample: impl FnMut(&CapturedSample),
) -> DeviceReport {
    let mut catalog: Vec<CatalogEntry> = Vec::new();
    let mut gaze = GazeSummary::default();
    for p in packets {
        if p.transfer != Transfer::Bulk || p.data_len == 0 {
            continue;
        }
        let dir = if p.endpoint == EP_BULK_OUT && !p.is_completion() {
            Direction::Out
        } else if p.endpoint == EP_BULK_IN && p.is_completion() {
            Direction::In
        } else {
            continue;
        };
        let Some(start) = captured_start(dir, p.data()) else {
            continue;
        };
        if dir == Direction::In
            && start.header.kind == Kind::Notification
            && start.header.id == STREAM_GAZE
        {
            match gaze_of(p) {
                Some(Ok(sample)) => {
                    gaze.frames += 1;
                    gaze.cut_frames += u64::from(!start.complete);
                    gaze.span = Some(gaze.span.map_or((p.time, p.time), |(a, _)| (a, p.time)));
                    gaze.stats.add(&sample);
                    on_sample(&CapturedSample {
                        record: p.index,
                        time: p.time,
                        sample,
                    });
                }
                Some(Err(GazeSkip::Oversized)) => gaze.oversized += 1,
                Some(Err(GazeSkip::Parse(e))) => {
                    gaze.frames += 1;
                    gaze.parse_errors += 1;
                    gaze.first_error.get_or_insert(e);
                }
                None => {}
            }
            continue;
        }
        let h = start.header;
        if let Some(e) = catalog
            .iter_mut()
            .find(|e| e.dir == dir && e.kind == h.kind && e.id == h.id)
        {
            e.count += 1;
            e.payload_len = (
                e.payload_len.0.min(h.payload_len),
                e.payload_len.1.max(h.payload_len),
            );
            e.cut += u64::from(!start.complete);
        } else {
            catalog.push(CatalogEntry {
                dir,
                kind: h.kind,
                id: h.id,
                count: 1,
                first_time: p.time,
                status: h.status,
                payload_len: (h.payload_len, h.payload_len),
                cut: u64::from(!start.complete),
            });
        }
    }
    DeviceReport {
        device,
        packets: packets.len(),
        catalog,
        gaze,
        plan: build_plan(device, packets),
    }
}

/// Human readable report.
pub fn format_report(r: &CaptureReport, verbose_plan: bool) -> String {
    let mut o = String::new();
    let _ = writeln!(
        o,
        "snap length {} | {} packets | {:.1} s | {} TTP device(s)",
        r.snaplen,
        r.packets,
        r.duration,
        r.devices.len()
    );
    for d in &r.devices {
        let _ = writeln!(
            o,
            "\n== device address {} ({} packets)",
            d.device, d.packets
        );
        if !d.catalog.is_empty() {
            let _ = writeln!(o, "TTP messages (gaze notifications listed separately):");
            for e in &d.catalog {
                let dir = if e.dir == Direction::Out {
                    "OUT"
                } else {
                    "IN "
                };
                let kind = match e.kind {
                    Kind::Request => "req ",
                    Kind::Response => "resp",
                    Kind::Notification => "note",
                };
                let _ = writeln!(
                    o,
                    "  t={:>8.3} {dir} {kind} id={:#06x} x{:<4} status={} payload={}..{}{} {}",
                    e.first_time,
                    e.id,
                    e.count,
                    e.status,
                    e.payload_len.0,
                    e.payload_len.1,
                    if e.cut > 0 {
                        format!(" (cut x{})", e.cut)
                    } else {
                        String::new()
                    },
                    if e.dir == Direction::Out || e.kind == Kind::Response {
                        opcode_name(e.id)
                    } else {
                        ""
                    }
                );
            }
        }
        let g = &d.gaze;
        if g.frames > 0 {
            let _ = writeln!(
                o,
                "gaze stream: {} notifications ({} cut by the snap length, {} undecodable)",
                g.frames, g.cut_frames, g.parse_errors
            );
            if let Some(e) = &g.first_error {
                let _ = writeln!(o, "  first decode error: {e}");
            }
            if g.oversized > 0 {
                let _ = writeln!(
                    o,
                    "  {} oversized completions ignored (USB length differs from the frame length: stale 16 KiB read buffers)",
                    g.oversized
                );
            }
            let _ = writeln!(
                o,
                "  rate: {} by device timestamps, {} by capture clock; counter gaps {}, timestamp gaps {}, regressions {}",
                g.stats.device_rate_hz().map_or("-".into(), |x| format!("{x:.2} Hz")),
                g.capture_rate_hz().map_or("-".into(), |x| format!("{x:.2} Hz")),
                g.stats.counter_gaps,
                g.stats.timestamp_gaps,
                g.stats.timestamp_regressions
            );
            for (i, name) in ["A", "B"].iter().enumerate() {
                let s = g.stats.states[i];
                let _ = writeln!(
                    o,
                    "  eye {name}: lost {} | origin only {} | tracked {}",
                    s[0], s[1], s[2]
                );
            }
        }
        let plan = &d.plan;
        let _ = writeln!(
            o,
            "replay plan: {} steps, {} TTP requests, {} bytes missing -> {}{}",
            plan.steps.len(),
            plan.requests().count(),
            plan.missing_bytes(),
            if plan.is_complete() {
                "COMPLETE"
            } else {
                "NOT replayable verbatim"
            },
            if plan.stream_start.is_none() && !plan.steps.is_empty() {
                " (but no gaze stream starts in this capture, so it is not a session start)"
            } else {
                ""
            }
        );
        if plan.auth_steps().count() > 0 {
            let _ = writeln!(
                o,
                "  contains the authentication exchange (0x76c/0x777/0x77b): the answers are bound to a device challenge and may be refused on replay"
            );
        }
        if verbose_plan {
            o.push_str(&plan.describe());
        } else {
            for s in plan.incomplete_steps() {
                let _ = writeln!(
                    o,
                    "  incomplete: {} ({} of {} bytes missing)",
                    s.label(),
                    s.missing_bytes,
                    s.total_bytes
                );
            }
        }
    }
    o
}

/// CSV header for [`write_csv_row`].
pub const CSV_HEADER: &str = "record,time_s,timestamp_us,counter,cut,\
A_state,A_origin_x,A_origin_y,A_origin_z,A_dir_x,A_dir_y,A_dir_z,A_pupil_mm,A_pupil_pos_x,A_pupil_pos_y,A_flag30,A_flag31,\
B_state,B_origin_x,B_origin_y,B_origin_z,B_dir_x,B_dir_y,B_dir_z,B_pupil_mm,B_pupil_pos_x,B_pupil_pos_y,B_flag32,B_flag33";

/// Write one CSV row.
pub fn write_csv_row(w: &mut impl Write, s: &CapturedSample) -> io::Result<()> {
    write!(
        w,
        "{},{:.6},{},{},{}",
        s.record,
        s.time,
        s.sample.timestamp_us,
        s.sample.frame_counter.map_or(-1i64, i64::from),
        u8::from(s.sample.truncated)
    )?;
    for e in &s.sample.eyes {
        let st = match e.state() {
            EyeState::Lost => 0,
            EyeState::OriginOnly => 1,
            EyeState::Tracked => 2,
        };
        write!(
            w,
            ",{st},{},{},{},{},{},{},{},{},{},{},{}",
            e.origin_mm[0],
            e.origin_mm[1],
            e.origin_mm[2],
            e.direction[0],
            e.direction[1],
            e.direction[2],
            e.pupil_diameter_mm,
            e.pupil_pos[0],
            e.pupil_pos[1],
            e.flags[0],
            e.flags[1]
        )?;
    }
    writeln!(w)
}

/// Print the first `n` samples of a capture (debugging aid).
pub fn head_lines(samples: &[CapturedSample], n: usize) -> String {
    samples
        .iter()
        .take(n)
        .map(|s| {
            format!(
                "rec {:>7} t={:>9.4}  {}\n",
                s.record,
                s.time,
                format_line(&s.sample)
            )
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeChip;
    use crate::gaze::encode_payload;
    use crate::pcap::PcapWriter;
    use crate::replay::testutil::*;
    use crate::ttp::{Kind, Message};

    fn gaze_frame(n: u32, cut: Option<usize>) -> UsbPacket {
        let s = FakeChip::sample(n, 1_000_000 + i64::from(n) * 8300);
        let mut m = Message::new(Kind::Notification, 0, 0, STREAM_GAZE, &[]);
        m.payload = encode_payload(&s);
        m.header.payload_len = m.payload.len() as u32;
        let f = m.to_frames(Direction::In, 1 << 20).concat();
        let real = f.len();
        let t = 2.0 + f64::from(n) * 0.0083;
        match cut {
            Some(c) => UsbPacket::build(
                t,
                2,
                30,
                EP_BULK_IN,
                Transfer::Bulk,
                true,
                None,
                &f[..c],
                real as u32,
            ),
            None => UsbPacket::build(
                t,
                2,
                30,
                EP_BULK_IN,
                Transfer::Bulk,
                true,
                None,
                &f,
                real as u32,
            ),
        }
    }

    fn write_capture(path: &Path, pk: &[UsbPacket]) {
        let mut w = PcapWriter::new(File::create(path).unwrap(), 65535).unwrap();
        for p in pk {
            w.write_packet(p).unwrap();
        }
    }

    #[test]
    fn analysis_of_a_synthetic_capture() {
        let req = request(1, 0x4c4, 1).to_frames(Direction::Out, 4087)[0].clone();
        let mut pk = vec![bulk_out(1.0, &req, req.len())];
        pk.push(bulk_in(
            1.001,
            &response(1, 0x4c4).to_frames(Direction::In, 4087)[0],
        ));
        for n in 0..50 {
            pk.push(gaze_frame(n, if n % 10 == 9 { Some(485) } else { None }));
        }
        let dir = std::env::temp_dir().join(format!("pimax-eye-an-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("synthetic.usbpcap");
        write_capture(&path, &pk);
        let r = analyze_file(&path, None).unwrap();
        assert_eq!(r.snaplen, 65535);
        assert_eq!(r.devices.len(), 1);
        let d = &r.devices[0];
        assert_eq!(d.device, 30);
        assert_eq!(d.gaze.frames, 50);
        assert_eq!(d.gaze.cut_frames, 5);
        assert_eq!(d.gaze.parse_errors, 0);
        assert_eq!(d.gaze.stats.samples, 50);
        assert_eq!(
            d.gaze.stats.states[0][2], 50,
            "eye A tracked in every synthetic frame"
        );
        assert!((d.gaze.stats.device_rate_hz().unwrap() - 1e6 / 8300.0).abs() < 0.01);
        assert!(d
            .catalog
            .iter()
            .any(|e| e.id == 0x4c4 && e.kind == Kind::Request));
        assert!(d
            .catalog
            .iter()
            .any(|e| e.id == 0x4c4 && e.kind == Kind::Response));
        assert_eq!(d.plan.requests().count(), 1);
        let text = format_report(&r, true);
        assert!(text.contains("gaze stream: 50 notifications"), "{text}");
        // CSV
        let samples = collect_samples(&path, 30).unwrap();
        assert_eq!(samples.len(), 50);
        let mut csv = Vec::new();
        write_csv_row(&mut csv, &samples[0]).unwrap();
        let line = String::from_utf8(csv).unwrap();
        assert_eq!(
            line.trim_end().split(',').count(),
            CSV_HEADER.split(',').count()
        );
        assert!(head_lines(&samples, 2).lines().count() == 2);
        // explicit device filter
        assert!(analyze_file(&path, Some(7)).unwrap().devices[0].gaze.frames == 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
