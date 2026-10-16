//! Replay planning: turn a USBPcap capture of the Windows session into the exact list of host-to-EyeChip transfers
//! that preceded the first gaze notification, and say whether every byte of that list is present in the capture.
//!
//! The plan contains
//! * every **vendor control** transfer (`bmRequestType` type bits 0x40: `0x41/0x30` session info, `0xc1/0x30`, `0xc1/0x46`
//!   module clock, `0x41/0x41` TTP session start) in capture order, and
//! * every **bulk OUT** frame on EP 0x05 (the TTP requests), grouped into messages,
//!
//! up to the first stream-0x0505 notification. Standard requests (descriptors, `SET_CONFIGURATION`) are not part of it:
//! the Linux kernel did its own enumeration. The data comes straight from the capture, so nothing is ever invented;
//! a step is **incomplete** when the capture snap length cut its data (`captured < real length`).

use crate::pcap::{Transfer, UsbPacket};
use crate::ttp::{captured_start, opcode_name, Direction, Kind, Wrapper, HEADER_LEN, STREAM_GAZE};

/// Bulk OUT endpoint used for TTP requests.
pub const EP_BULK_OUT: u8 = 0x05;
/// Bulk IN endpoint carrying responses and notifications.
pub const EP_BULK_IN: u8 = 0x83;
/// USB vendor id of the EyeChip.
pub const EYE_VID: u16 = 0x2104;
/// USB product id of the EyeChip.
pub const EYE_PID: u16 = 0x0561;

/// What the captured response to a request looked like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Expect {
    /// Status word of the response header (1 = ok in every capture).
    pub status: u32,
    /// Payload length announced by the response.
    pub payload_len: u32,
}

/// One transfer to send.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepKind {
    /// Vendor control OUT: 8-byte setup packet plus data stage.
    ControlOut {
        /// Setup packet.
        setup: [u8; 8],
        /// Data stage as captured.
        data: Vec<u8>,
    },
    /// Vendor control IN: only the setup packet is sent; the reply is read and ignored.
    ControlIn {
        /// Setup packet.
        setup: [u8; 8],
    },
    /// One TTP request: one or more bulk OUT frames (each is its own USB transfer).
    Request {
        /// Sequence number.
        seq: u32,
        /// Opcode.
        opcode: u32,
        /// Payload length announced by the header.
        payload_len: u32,
        /// Frames exactly as captured (wrapper included).
        frames: Vec<Vec<u8>>,
        /// Captured response, when seen.
        expect: Option<Expect>,
    },
}

/// A planned transfer plus where it came from.
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    /// Record number of the (first) submission in the capture.
    pub packet_index: u64,
    /// Capture time in seconds.
    pub time: f64,
    /// What to send.
    pub kind: StepKind,
    /// Bytes of real transfer data that the capture does not contain.
    pub missing_bytes: u64,
    /// Real transfer data bytes of this step.
    pub total_bytes: u64,
}

impl Step {
    /// All bytes of the step are present.
    pub fn is_complete(&self) -> bool {
        self.missing_bytes == 0
    }

    /// Short description for listings.
    pub fn label(&self) -> String {
        match &self.kind {
            StepKind::ControlOut { setup, .. } => format!(
                "control OUT  bm={:#04x} req={:#04x} wValue={:#06x} wLength={}",
                setup[0],
                setup[1],
                u16::from_le_bytes([setup[2], setup[3]]),
                u16::from_le_bytes([setup[6], setup[7]])
            ),
            StepKind::ControlIn { setup } => format!(
                "control IN   bm={:#04x} req={:#04x} wValue={:#06x} wLength={}",
                setup[0],
                setup[1],
                u16::from_le_bytes([setup[2], setup[3]]),
                u16::from_le_bytes([setup[6], setup[7]])
            ),
            StepKind::Request {
                seq,
                opcode,
                payload_len,
                frames,
                ..
            } => format!(
                "TTP request  seq={seq:<3} op={opcode:#05x} payload={payload_len:<6} frames={} {}",
                frames.len(),
                opcode_name(*opcode)
            ),
        }
    }
}

/// The list of transfers of one session start.
#[derive(Debug, Clone)]
pub struct Plan {
    /// Device address the plan was extracted for (per-capture, re-assigned on every enumeration).
    pub device: u16,
    /// Transfers in capture order.
    pub steps: Vec<Step>,
    /// Capture time of the first gaze notification, when the capture reaches it.
    pub stream_start: Option<f64>,
    /// OUT frames that continue no message (should be 0).
    pub orphan_frames: u32,
}

impl Plan {
    /// Every byte of every step is in the capture, and there is at least one step.
    pub fn is_complete(&self) -> bool {
        !self.steps.is_empty() && self.steps.iter().all(Step::is_complete)
    }

    /// Steps whose data was cut by the capture snap length.
    pub fn incomplete_steps(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| !s.is_complete())
    }

    /// Sum of the missing bytes.
    pub fn missing_bytes(&self) -> u64 {
        self.steps.iter().map(|s| s.missing_bytes).sum()
    }

    /// TTP requests of the plan.
    pub fn requests(&self) -> impl Iterator<Item = &Step> {
        self.steps
            .iter()
            .filter(|s| matches!(s.kind, StepKind::Request { .. }))
    }

    /// Requests that look like the authentication exchange (0x76c, 0x777, 0x77b): their answers depend on a device
    /// challenge, so a verbatim replay may be refused even when the capture is complete.
    pub fn auth_steps(&self) -> impl Iterator<Item = &Step> {
        self.steps.iter().filter(|s| {
            matches!(&s.kind, StepKind::Request { opcode, .. } if matches!(*opcode, 0x76c | 0x777 | 0x77b))
        })
    }

    /// Multi-line listing of the plan.
    pub fn describe(&self) -> String {
        let mut out = format!(
            "device address {} | {} steps ({} TTP requests) | stream starts at {}\n",
            self.device,
            self.steps.len(),
            self.requests().count(),
            self.stream_start
                .map_or_else(|| "never".to_string(), |t| format!("t={t:.4}s"))
        );
        for (i, s) in self.steps.iter().enumerate() {
            let state = if s.is_complete() {
                "complete".to_string()
            } else {
                format!(
                    "INCOMPLETE: {} of {} bytes missing",
                    s.missing_bytes, s.total_bytes
                )
            };
            out.push_str(&format!(
                "{i:>3} t={:>9.4} rec={:<7} {:<95} {state}\n",
                s.time,
                s.packet_index,
                s.label()
            ));
        }
        out
    }
}

/// Collects the device addresses that carry TTP traffic on EP 0x05 / 0x83 while packets are streamed through it.
#[derive(Debug, Default, Clone)]
pub struct DeviceFinder {
    found: std::collections::BTreeSet<u16>,
}

impl DeviceFinder {
    /// Look at one packet.
    pub fn observe(&mut self, p: &UsbPacket) {
        if p.transfer != Transfer::Bulk || p.data_len == 0 {
            return;
        }
        let hit = (p.endpoint == EP_BULK_OUT
            && !p.is_completion()
            && captured_start(Direction::Out, p.data()).is_some())
            || (p.endpoint == EP_BULK_IN
                && p.is_completion()
                && captured_start(Direction::In, p.data()).is_some());
        if hit {
            self.found.insert(p.device);
        }
    }

    /// Device addresses seen, ascending.
    pub fn devices(&self) -> Vec<u16> {
        self.found.iter().copied().collect()
    }
}

/// Device addresses in a capture that carry TTP traffic on EP 0x05 / 0x83.
pub fn find_eye_devices<'a>(packets: impl IntoIterator<Item = &'a UsbPacket>) -> Vec<u16> {
    let mut f = DeviceFinder::default();
    for p in packets {
        f.observe(p);
    }
    f.devices()
}

/// Build the plan from the packets of ONE device (any order-preserving filter of the capture).
pub fn build_plan(device: u16, packets: &[UsbPacket]) -> Plan {
    let mut responses = Vec::new();
    for (pos, p) in packets.iter().enumerate() {
        if p.transfer == Transfer::Bulk
            && p.endpoint == EP_BULK_IN
            && p.is_completion()
            && p.data_len > 0
        {
            if let Some(s) = captured_start(Direction::In, p.data()) {
                if s.header.kind == Kind::Response {
                    responses.push((
                        pos,
                        s.header.seq,
                        Expect {
                            status: s.header.status,
                            payload_len: s.header.payload_len,
                        },
                    ));
                }
            }
        }
    }

    let mut steps: Vec<Step> = Vec::new();
    let mut step_pos: Vec<usize> = Vec::new();
    let mut open: Option<(usize, usize)> = None; // (step index, body bytes still expected)
    let mut stream_start = None;
    let mut orphan_frames = 0u32;

    for (pos, p) in packets.iter().enumerate() {
        match p.transfer {
            Transfer::Control if p.stage == Some(0) && !p.is_completion() => {
                let d = p.data();
                if d.len() < 8 || d[0] & 0x60 != 0x40 {
                    continue; // short, or not a vendor request
                }
                let mut setup = [0u8; 8];
                setup.copy_from_slice(&d[..8]);
                let wlen = usize::from(u16::from_le_bytes([setup[6], setup[7]]));
                if d[0] & 0x80 != 0 {
                    steps.push(Step {
                        packet_index: p.index,
                        time: p.time,
                        kind: StepKind::ControlIn { setup },
                        missing_bytes: 0,
                        total_bytes: 8,
                    });
                } else {
                    let data = d[8..].to_vec();
                    let missing = wlen.saturating_sub(data.len()) as u64;
                    steps.push(Step {
                        packet_index: p.index,
                        time: p.time,
                        total_bytes: 8 + wlen as u64,
                        missing_bytes: missing,
                        kind: StepKind::ControlOut { setup, data },
                    });
                }
                step_pos.push(pos);
            }
            Transfer::Bulk if p.endpoint == EP_BULK_OUT && !p.is_completion() && p.data_len > 0 => {
                let d = p.data();
                let real = u64::from(p.data_len);
                let cut = real.saturating_sub(d.len() as u64);
                if let Some((idx, remaining)) = open {
                    let body = Wrapper::parse(d)
                        .map(|w| w.body_len())
                        .unwrap_or((p.data_len as usize).saturating_sub(8));
                    if let StepKind::Request { frames, .. } = &mut steps[idx].kind {
                        frames.push(d.to_vec());
                    }
                    steps[idx].missing_bytes += cut;
                    steps[idx].total_bytes += real;
                    let left = remaining.saturating_sub(body);
                    open = (left > 0).then_some((idx, left));
                } else if let Some(start) = captured_start(Direction::Out, d) {
                    let first_body = Wrapper::parse(d).map_or(0, |w| w.body_len());
                    let total_body = HEADER_LEN + start.header.payload_len as usize;
                    let left = total_body.saturating_sub(first_body);
                    steps.push(Step {
                        packet_index: p.index,
                        time: p.time,
                        kind: StepKind::Request {
                            seq: start.header.seq,
                            opcode: start.header.id,
                            payload_len: start.header.payload_len,
                            frames: vec![d.to_vec()],
                            expect: None,
                        },
                        missing_bytes: cut,
                        total_bytes: real,
                    });
                    step_pos.push(pos);
                    open = (left > 0).then_some((steps.len() - 1, left));
                } else {
                    orphan_frames += 1;
                }
            }
            Transfer::Bulk if p.endpoint == EP_BULK_IN && p.is_completion() && p.data_len > 0 => {
                if let Some(s) = captured_start(Direction::In, p.data()) {
                    if s.header.kind == Kind::Notification && s.header.id == STREAM_GAZE {
                        stream_start = Some(p.time);
                        break;
                    }
                }
            }
            _ => {}
        }
    }

    for (step, &pos) in steps.iter_mut().zip(&step_pos) {
        if let StepKind::Request { seq, expect, .. } = &mut step.kind {
            *expect = responses
                .iter()
                .find(|(rpos, rseq, _)| *rpos > pos && rseq == seq)
                .map(|(_, _, e)| *e);
        }
    }

    Plan {
        device,
        steps,
        stream_start,
        orphan_frames,
    }
}

#[cfg(test)]
pub(crate) mod testutil {
    use super::*;
    use crate::tlv::{push_compound, push_i32};
    use crate::ttp::Message;

    pub fn bulk_out(t: f64, frame: &[u8], real: usize) -> UsbPacket {
        UsbPacket::build(
            t,
            2,
            30,
            EP_BULK_OUT,
            Transfer::Bulk,
            false,
            None,
            frame,
            real as u32,
        )
    }

    pub fn bulk_in(t: f64, frame: &[u8]) -> UsbPacket {
        UsbPacket::build(
            t,
            2,
            30,
            EP_BULK_IN,
            Transfer::Bulk,
            true,
            None,
            frame,
            frame.len() as u32,
        )
    }

    pub fn ctrl(t: f64, setup: [u8; 8], out: &[u8]) -> UsbPacket {
        let mut d = setup.to_vec();
        d.extend_from_slice(out);
        let ep = if setup[0] & 0x80 != 0 { 0x80 } else { 0x00 };
        UsbPacket::build(
            t,
            2,
            30,
            ep,
            Transfer::Control,
            false,
            Some(0),
            &d,
            d.len() as u32,
        )
    }

    pub fn request(seq: u32, op: u32, n_ints: usize) -> Message {
        let mut items = Vec::new();
        for i in 0..n_ints {
            push_i32(&mut items, i as i32);
        }
        Message::new(Kind::Request, seq, 0, op, &items)
    }

    pub fn response(seq: u32, op: u32) -> Message {
        Message::new(Kind::Response, seq, 1, op, &[])
    }

    pub fn notification() -> Message {
        let mut items = Vec::new();
        push_compound(&mut items, 0, crate::gaze::TYPE_GAZE);
        Message::new(Kind::Notification, 0, 0, STREAM_GAZE, &items)
    }
}

#[cfg(test)]
mod tests {
    use super::testutil::*;
    use super::*;

    fn session(truncate_big: bool) -> Vec<UsbPacket> {
        let mut v = Vec::new();
        let mut t = 1.0;
        let mut tick = || {
            t += 0.001;
            t
        };
        v.push(ctrl(tick(), [0x41, 0x30, 0, 0, 0, 0, 4, 0], &[1, 2, 3, 4]));
        v.push(ctrl(tick(), [0xc1, 0x46, 1, 0, 0, 0, 8, 0], &[]));
        v.push(ctrl(tick(), [0x80, 0x06, 0, 3, 9, 4, 0x34, 0], &[])); // standard: ignored
        let r1 = request(1, 0x3e8, 3);
        for f in r1.to_frames(Direction::Out, 4087) {
            v.push(bulk_out(tick(), &f, f.len()));
        }
        v.push(bulk_in(
            tick(),
            &response(1, 0x3e8).to_frames(Direction::In, 4087)[0],
        ));
        // big message: 3 frames of 100 body bytes
        let big = request(2, 0x456, 60);
        let frames = big.to_frames(Direction::Out, 100);
        assert!(frames.len() > 2);
        for f in &frames {
            if truncate_big {
                v.push(bulk_out(tick(), &f[..40.min(f.len())], f.len()));
            } else {
                v.push(bulk_out(tick(), f, f.len()));
            }
        }
        v.push(bulk_in(
            tick(),
            &response(2, 0x456).to_frames(Direction::In, 4087)[0],
        ));
        v.push(bulk_in(
            tick(),
            &notification().to_frames(Direction::In, 4087)[0],
        ));
        // after the stream start: must not be in the plan
        let late = request(3, 0x44c, 0);
        v.push(bulk_out(
            tick(),
            &late.to_frames(Direction::Out, 4087)[0],
            late.body().len() + 8,
        ));
        v
    }

    #[test]
    fn complete_session_gives_complete_plan() {
        let pk = session(false);
        assert_eq!(find_eye_devices(&pk), vec![30]);
        let plan = build_plan(30, &pk);
        assert_eq!(plan.steps.len(), 4, "{}", plan.describe());
        assert!(plan.is_complete());
        assert_eq!(plan.requests().count(), 2);
        assert_eq!(plan.orphan_frames, 0);
        assert!(plan.stream_start.is_some());
        match &plan.steps[3].kind {
            StepKind::Request {
                seq,
                opcode,
                frames,
                expect,
                ..
            } => {
                assert_eq!((*seq, *opcode), (2, 0x456));
                assert!(frames.len() > 2);
                assert_eq!(
                    expect,
                    &Some(Expect {
                        status: 1,
                        payload_len: 0
                    })
                );
            }
            k => panic!("{k:?}"),
        }
        assert!(matches!(plan.steps[0].kind, StepKind::ControlOut { .. }));
        assert!(matches!(plan.steps[1].kind, StepKind::ControlIn { .. }));
        assert_eq!(plan.auth_steps().count(), 0);
    }

    #[test]
    fn truncated_frames_make_the_plan_incomplete() {
        let pk = session(true);
        let plan = build_plan(30, &pk);
        assert_eq!(plan.steps.len(), 4);
        assert!(!plan.is_complete());
        let inc: Vec<_> = plan.incomplete_steps().collect();
        assert_eq!(inc.len(), 1);
        assert!(inc[0].missing_bytes > 0 && inc[0].total_bytes > inc[0].missing_bytes);
        assert_eq!(plan.missing_bytes(), inc[0].missing_bytes);
        assert!(plan.describe().contains("INCOMPLETE"));
        // all frames of the multi-frame message were grouped even though the capture cut them
        match &inc[0].kind {
            StepKind::Request { frames, .. } => assert!(frames.len() > 2),
            k => panic!("{k:?}"),
        }
    }

    #[test]
    fn empty_input_gives_an_empty_incomplete_plan() {
        let plan = build_plan(30, &[]);
        assert!(!plan.is_complete());
        assert!(plan.stream_start.is_none());
    }

    #[test]
    fn auth_steps_are_flagged() {
        let mut pk = Vec::new();
        for (i, op) in [0x76cu32, 0x777, 0x4c4].into_iter().enumerate() {
            let m = request(i as u32 + 1, op, 1);
            let f = m.to_frames(Direction::Out, 4087);
            pk.push(bulk_out(1.0 + i as f64 * 0.001, &f[0], f[0].len()));
        }
        let plan = build_plan(30, &pk);
        assert_eq!(plan.auth_steps().count(), 2);
    }

    #[test]
    fn foreign_devices_are_not_detected() {
        let p = UsbPacket::build(0.0, 2, 9, 0x05, Transfer::Bulk, false, None, &[1; 64], 64);
        assert!(find_eye_devices([&p]).is_empty());
    }
}
