//! Replay of the cold-start frame list `data/replay-frames.txt` (derived from the Windows capture
//! `20261014-043545-cold2-hub2.pcap`, see `analysis/eye-coldstart.md`).
//!
//! Safety model: the list is the only source of bytes. [`Guard`] refuses any control transfer or bulk frame that is
//! not byte-identical to one in the list, refuses the calibration opcodes outright, and refuses a `0x456` message
//! other than the one captured blob (length and FNV-1a hash pinned below).

use crate::transport::Transport;
use crate::ttp::{Direction, Header, Kind, Message, StreamDecoder, WRAPPER_LEN};
use std::collections::HashSet;
use std::io;
use std::time::Duration;

/// The checked-in frame list.
pub const DATA: &str = include_str!("../data/replay-frames.txt");

/// Calibration opcodes that are never sent.
pub const DENIED_OPS: [u32; 5] = [0x3f2, 0x407, 0x42e, 0x3fc, 0x44c];
/// Auth opcodes (`--skip-auth`).
pub const AUTH_OPS: [u32; 2] = [0x76c, 0x777];
/// Calibration upload opcode (`--skip-cal`).
pub const CAL_OP: u32 = 0x456;
/// Payload length of the captured calibration message (header payload size field).
pub const CAL_PAYLOAD_LEN: u32 = 0x8640;
/// FNV-1a 64 of the whole captured 0x456 payload.
pub const CAL_PAYLOAD_FNV: u64 = 7053722925501851203;

/// What a step does.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// Standard enumeration (descriptors, SET_CONFIGURATION): never sent by this tool.
    Enumeration,
    /// Control transfer.
    Control {
        /// 8-byte setup packet.
        setup: [u8; 8],
        /// Data stage of an OUT request.
        data: Vec<u8>,
    },
    /// One TTP request, split into one or more bulk OUT frames.
    Ttp {
        /// Sequence number.
        seq: u32,
        /// Opcode.
        op: u32,
        /// Bulk OUT transfers (each with its 8-byte wrapper).
        frames: Vec<Vec<u8>>,
    },
}

/// One step of the list.
#[derive(Debug, Clone, PartialEq)]
pub struct FStep {
    /// Step number in the file.
    pub n: u32,
    /// Class flag: R, C, AUTH or CAL.
    pub class: String,
    /// Description line.
    pub desc: String,
    /// Capture time of the request in seconds, when given.
    pub t: Option<f64>,
    /// The action.
    pub action: Action,
}

fn unhex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn kv<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let p = line.find(&format!("{key}="))? + key.len() + 1;
    let rest = &line[p..];
    Some(rest.split_whitespace().next().unwrap_or(""))
}

fn num(s: &str) -> Result<u64, String> {
    match s.strip_prefix("0x") {
        Some(h) => u64::from_str_radix(h, 16),
        None => s.parse(),
    }
    .map_err(|e| format!("bad number '{s}': {e}"))
}

/// Parse the opcode/seq of the first frame and check the whole request reassembles into exactly one message.
fn ttp_info(frames: &[Vec<u8>]) -> Result<(Message, Header), String> {
    let mut dec = StreamDecoder::new(Direction::Out);
    let mut msgs = Vec::new();
    for f in frames {
        for r in dec.push(f) {
            msgs.push(r.map_err(|e| format!("framing: {e}"))?);
        }
    }
    if msgs.len() != 1 || dec.buffered() != 0 {
        return Err(format!(
            "frames reassemble into {} messages ({} bytes left over)",
            msgs.len(),
            dec.buffered()
        ));
    }
    let m = msgs.remove(0);
    let h = m.header;
    Ok((m, h))
}

/// Parse the frame list.
pub fn parse(text: &str) -> Result<Vec<FStep>, String> {
    let mut steps: Vec<FStep> = Vec::new();
    let mut cur: Option<(u32, String, String, Vec<String>)> = None; // n, class, desc, bulk hex
    let mut finished: Vec<(u32, String, String, Vec<String>)> = Vec::new();
    for line in text.lines() {
        if line.starts_with("STEP ") {
            if let Some(c) = cur.take() {
                finished.push(c);
            }
            let mut p = line.splitn(3, '|');
            let n: u32 = p
                .next()
                .and_then(|s| s.trim().strip_prefix("STEP "))
                .and_then(|s| s.trim().parse().ok())
                .ok_or("bad STEP line")?;
            let class = p.next().ok_or("bad STEP line")?.trim().to_string();
            let desc = p.next().ok_or("bad STEP line")?.trim().to_string();
            cur = Some((n, class, desc, Vec::new()));
        } else if let Some(rest) = line.trim_start().strip_prefix("BULK_OUT ") {
            let hex = rest.split_whitespace().last().ok_or("empty BULK_OUT")?;
            cur.as_mut()
                .ok_or("BULK_OUT outside a step")?
                .3
                .push(hex.to_string());
        }
    }
    if let Some(c) = cur.take() {
        finished.push(c);
    }
    for (n, class, desc, bulk) in finished {
        let t = desc
            .find("(t=")
            .and_then(|p| desc[p + 3..].split(')').next())
            .and_then(|s| s.parse::<f64>().ok());
        let action = if desc.starts_with("standard:") {
            Action::Enumeration
        } else if desc.starts_with("CTRL ") {
            let rt = num(kv(&desc, "bmRT").ok_or("no bmRT")?)? as u8;
            let rq = num(kv(&desc, "bReq").ok_or("no bReq")?)? as u8;
            let wv = num(kv(&desc, "wValue").ok_or("no wValue")?)? as u16;
            let wi = num(kv(&desc, "wIndex").ok_or("no wIndex")?)? as u16;
            let wl = num(kv(&desc, "wLength").ok_or("no wLength")?)? as u16;
            let data = match kv(&desc, "data") {
                Some(h) => unhex(h)?,
                None => Vec::new(),
            };
            if rt & 0x80 == 0 && data.len() != usize::from(wl) {
                return Err(format!("step {n}: data length differs from wLength"));
            }
            let mut setup = [0u8; 8];
            setup[0] = rt;
            setup[1] = rq;
            setup[2..4].copy_from_slice(&wv.to_le_bytes());
            setup[4..6].copy_from_slice(&wi.to_le_bytes());
            setup[6..8].copy_from_slice(&wl.to_le_bytes());
            Action::Control { setup, data }
        } else {
            let frames: Vec<Vec<u8>> = bulk.iter().map(|h| unhex(h)).collect::<Result<_, _>>()?;
            for f in &frames {
                if f.len() < WRAPPER_LEN
                    || u32::from_le_bytes(f[4..8].try_into().unwrap()) as usize != f.len() - 8
                {
                    return Err(format!("step {n}: wrapper length mismatch"));
                }
            }
            let (_, h) = ttp_info(&frames).map_err(|e| format!("step {n}: {e}"))?;
            if h.kind != Kind::Request {
                return Err(format!("step {n}: not a request"));
            }
            if let Some(w) = kv(&desc, "op") {
                if num(w)? != u64::from(h.id) {
                    return Err(format!("step {n}: op in text differs from the frame"));
                }
            }
            Action::Ttp {
                seq: h.seq,
                op: h.id,
                frames,
            }
        };
        steps.push(FStep {
            n,
            class,
            desc,
            t,
            action,
        });
    }
    Ok(steps)
}

/// FNV-1a 64.
pub fn fnv1a(b: &[u8]) -> u64 {
    b.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, &x| {
        (h ^ u64::from(x)).wrapping_mul(0x0100_0000_01b3)
    })
}

/// Which optional parts are left out.
#[derive(Debug, Clone, Copy, Default)]
pub struct Skips {
    /// Omit 0x76c / 0x777.
    pub auth: bool,
    /// Omit 0x456.
    pub cal: bool,
    /// Omit the vendor control write bReq 48.
    pub vendor_write: bool,
}

/// Select the steps to send (enumeration is never sent).
pub fn select(steps: &[FStep], s: Skips) -> Vec<&FStep> {
    steps
        .iter()
        .filter(|st| match &st.action {
            Action::Enumeration => false,
            Action::Control { setup, .. } => {
                !(s.vendor_write && setup[0] == 0x41 && setup[1] == 48)
            }
            Action::Ttp { op, .. } => {
                !(s.auth && AUTH_OPS.contains(op)) && !(s.cal && *op == CAL_OP)
            }
        })
        .collect()
}

/// The send gate: only list members, never denied opcodes.
pub struct Guard {
    frames: HashSet<Vec<u8>>,
    controls: HashSet<([u8; 8], Vec<u8>)>,
}

impl Guard {
    /// Build from the parsed list; fails if the list itself contains a denied opcode or a foreign 0x456.
    pub fn new(steps: &[FStep]) -> Result<Guard, String> {
        let mut g = Guard {
            frames: HashSet::new(),
            controls: HashSet::new(),
        };
        for st in steps {
            match &st.action {
                Action::Enumeration => {}
                Action::Control { setup, data } => {
                    g.controls.insert((*setup, data.clone()));
                }
                Action::Ttp { op, frames, .. } => {
                    if DENIED_OPS.contains(op) {
                        return Err(format!("step {}: opcode {op:#x} is denied", st.n));
                    }
                    if *op == CAL_OP {
                        let (m, h) = ttp_info(frames)?;
                        if h.payload_len != CAL_PAYLOAD_LEN
                            || (CAL_PAYLOAD_FNV != 0 && fnv1a(&m.payload) != CAL_PAYLOAD_FNV)
                        {
                            return Err("0x456 payload is not the captured blob".into());
                        }
                    }
                    for f in frames {
                        g.frames.insert(f.clone());
                    }
                }
            }
        }
        Ok(g)
    }

    /// Allow one more frame that is not in the list (the captured gaze unsubscribe). Denied opcodes stay refused.
    pub fn allow_frame(&mut self, frame: &[u8]) {
        self.frames.insert(frame.to_vec());
    }

    /// Control transfer through the gate.
    pub fn control<T: Transport>(
        &self,
        t: &mut T,
        setup: [u8; 8],
        data: &[u8],
        timeout: Duration,
    ) -> io::Result<Vec<u8>> {
        let is_in = setup[0] & 0x80 != 0;
        // IN requests carry no host data; they are listed with empty data.
        if !self.controls.contains(&(setup, data.to_vec())) || (!is_in && setup[0] != 0x41) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "control transfer is not in the replay list",
            ));
        }
        t.control(setup, data, timeout)
    }

    /// Bulk OUT through the gate.
    pub fn bulk_out<T: Transport>(
        &self,
        t: &mut T,
        frame: &[u8],
        timeout: Duration,
    ) -> io::Result<()> {
        if !self.frames.contains(frame) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "bulk frame is not in the replay list",
            ));
        }
        if frame.len() >= 8 + 24 {
            let op = u32::from_be_bytes(frame[8 + 12..8 + 16].try_into().unwrap());
            if DENIED_OPS.contains(&op) {
                return Err(io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "calibration opcode refused",
                ));
            }
        }
        t.bulk_out(0x05, frame, timeout)
    }
}

/// Hex string.
pub fn hex(b: &[u8]) -> String {
    b.iter().map(|x| format!("{x:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn steps() -> Vec<FStep> {
        parse(DATA).unwrap()
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn list_has_26_steps_and_29_urbs() {
        let s = steps();
        assert_eq!(s.len(), 26);
        let urbs: usize = s
            .iter()
            .map(|x| match &x.action {
                Action::Ttp { frames, .. } => frames.len(),
                _ => 0,
            })
            .sum();
        assert_eq!(urbs, 29);
        let ttp = s
            .iter()
            .filter(|x| matches!(x.action, Action::Ttp { .. }))
            .count();
        assert_eq!(ttp, 21);
        assert_eq!(s[0].action, Action::Enumeration);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn opcodes_in_order_match_the_transcript() {
        let ops: Vec<u32> = steps()
            .iter()
            .filter_map(|x| match &x.action {
                Action::Ttp { op, .. } => Some(*op),
                _ => None,
            })
            .collect();
        assert_eq!(
            ops,
            [
                0x3e8, 0x640, 0x76c, 0x777, 0x852, 0x77b, 0x532, 0x58c, 0x546, 0x547, 0x5d2, 0x5cd,
                0x4b0, 0x596, 0x578, 0x686, 0x67c, 0x5be, 0x672, 0x4c4, 0x456
            ]
        );
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn hello_frame_is_byte_exact() {
        let s = steps();
        let Action::Ttp { frames, seq, op } = &s[5].action else {
            panic!()
        };
        assert_eq!((*seq, *op), (1, 0x3e8));
        assert_eq!(frames[0].len(), 79);
        assert_eq!(&frames[0][..8], &[0, 0, 0, 0, 0x47, 0, 0, 0]);
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn multi_urb_blob_reassembles() {
        let s = steps();
        let Action::Ttp { frames, op, .. } = &s.last().unwrap().action else {
            panic!()
        };
        assert_eq!(*op, CAL_OP);
        assert_eq!(frames.len(), 9);
        let (m, h) = ttp_info(frames).unwrap();
        assert_eq!(h.payload_len, CAL_PAYLOAD_LEN);
        assert_eq!(m.payload.len(), CAL_PAYLOAD_LEN as usize);
        // blob header: name "human" at offset 0x1c in the TLV value, id 2057854115 = 0x7aa85ca3 at blob offset 24
        let pos = m.payload.windows(5).position(|w| w == b"human").unwrap();
        assert!(pos < 128);
        assert_eq!(fnv1a(&m.payload), CAL_PAYLOAD_FNV, "pin CAL_PAYLOAD_FNV");
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn auth_answer_step_has_the_captured_answer() {
        let s = steps();
        let Action::Ttp { frames, op, .. } = &s[8].action else {
            panic!()
        };
        assert_eq!(*op, 0x777);
        let f = &frames[0];
        // 16 B answer d2fc1527...e35 follows the tag-0x15 / len 0x14 / inner len 0x10 prefix
        let h = hex(f);
        assert!(h.contains("10d2fc15272c5147dbd3fea53f91260e35"));
        assert!(h.ends_with("816d4df2"));
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn select_applies_skips() {
        let s = steps();
        let all = select(&s, Skips::default());
        assert_eq!(all.len(), 25);
        let a = select(
            &s,
            Skips {
                auth: true,
                cal: true,
                vendor_write: true,
            },
        );
        assert_eq!(a.len(), 25 - 2 - 1 - 1);
        assert!(a.iter().all(|x| match &x.action {
            Action::Ttp { op, .. } => !AUTH_OPS.contains(op) && *op != CAL_OP,
            Action::Control { setup, .. } => !(setup[0] == 0x41 && setup[1] == 48),
            _ => false,
        }));
    }

    #[test]
    fn guard_refuses_foreign_and_denied() {
        let s = steps();
        let g = Guard::new(&s).unwrap();
        let mut chip = crate::fake::FakeChip::default();
        // a hand-built 0x3f2 start-calibration request
        let m = Message::new(Kind::Request, 99, 0, 0x3f2, &[]);
        let f = m.to_frames(Direction::Out, 4087).remove(0);
        let e = g
            .bulk_out(&mut chip, &f, Duration::from_millis(10))
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
        // a vendor write that is not in the list
        let e = g
            .control(
                &mut chip,
                [0x41, 49, 0, 0, 0, 0, 1, 0],
                &[1],
                Duration::from_millis(10),
            )
            .unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::PermissionDenied);
    }

    #[test]
    fn guard_rejects_list_with_denied_op() {
        let m = Message::new(Kind::Request, 1, 0, 0x407, &[]);
        let f = m.to_frames(Direction::Out, 4087).remove(0);
        let st = FStep {
            n: 1,
            class: "R".into(),
            desc: String::new(),
            t: None,
            action: Action::Ttp {
                seq: 1,
                op: 0x407,
                frames: vec![f],
            },
        };
        assert!(Guard::new(&[st]).is_err());
    }

    #[test]
    fn chunker_handles_split_in_stream() {
        // a 1242 B gaze-like frame split over two URBs, then a response in the second URB's tail
        let m = Message::new(Kind::Response, 3, 1, 0x777, &[]);
        let f = m.to_frames(Direction::In, 4087).remove(0);
        let mut dec = StreamDecoder::new(Direction::In);
        let (a, b) = f.split_at(10);
        assert!(dec.push(a).is_empty());
        let r = dec.push(b);
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].as_ref().unwrap().header.id, 0x777);
    }
}
