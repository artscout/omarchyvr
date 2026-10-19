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

/// The frame list file `data/replay-frames.txt`: the synthetic stand-in in the public tree (no steps), the private
/// full cold-start list (with the two auth steps) when `tools/lab-data.sh link` linked it.
pub const DATA: &str = include_str!("../data/replay-frames.txt");

/// The public BASE list `data/base-frames.txt` (user decision 2026-10-17, docs/EYE.md): the cold start without the
/// two auth steps (0x76c, 0x777) and with the developer's calibration blob (0x456) as the built-in test calibration.
pub const BASE: &str = include_str!("../data/base-frames.txt");

/// Which list [`list`] chose.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ListKind {
    /// The public base list.
    Base,
    /// The private full list (linked from the lab data).
    Full,
}

/// The list the eye start uses: the private full list when it is linked in (it has steps), else the base list.
pub fn list() -> (&'static str, ListKind) {
    if DATA.lines().any(|l| l.starts_with("STEP ")) {
        (DATA, ListKind::Full)
    } else {
        (BASE, ListKind::Base)
    }
}

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
    /// Build from the parsed built-in list; fails if the list itself contains a denied opcode or a foreign 0x456.
    pub fn new(steps: &[FStep]) -> Result<Guard, String> {
        Guard::new_with(steps, true)
    }

    /// As [`Guard::new`]; `pin_cal` false accepts the 0x456 the list carries (a list made from the user's own capture
    /// by [`from_plan`], which holds that headset's own profile upload) instead of only the built-in blob.
    pub fn new_with(steps: &[FStep], pin_cal: bool) -> Result<Guard, String> {
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
                    if *op == CAL_OP && pin_cal {
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

/// A frame list (the format of `data/base-frames.txt`) from the start plan of a Windows capture: every vendor control
/// transfer and TTP request up to the first gaze notification, byte for byte as captured, in capture order. The two
/// auth steps are left out unless `keep_auth` (their answer depends on a challenge of that session). Fails when the
/// capture cut any data (a list is never padded or guessed).
pub fn from_plan(plan: &crate::replay::Plan, source: &str, keep_auth: bool) -> Result<String, String> {
    use crate::replay::StepKind;
    if !plan.is_complete() {
        return Err(format!(
            "the capture misses {} bytes of the start (snap length); capture again with a full snap length",
            plan.missing_bytes()
        ));
    }
    let mut o = String::new();
    o.push_str("# Eye chip start list made by `pimax-eye frames` from a Windows capture of this headset.\n");
    o.push_str(&format!("# Source: {source}\n"));
    o.push_str("# Every byte is from the capture (Pimax Play's own start of the eye chip). Private: it holds this\n");
    o.push_str("# headset's eye profile upload (0x456). Do not share or commit it.\n\n");
    let mut n = 0u32;
    for st in &plan.steps {
        let t = st.time;
        match &st.kind {
            StepKind::ControlOut { setup, data } => {
                n += 1;
                let data = if data.is_empty() { String::new() } else { format!(" data={}", hex(data)) };
                o.push_str(&format!("STEP {n} | C | {}{data} (t={t:.4})\n\n", ctrl_desc(setup)));
            }
            StepKind::ControlIn { setup } => {
                n += 1;
                o.push_str(&format!("STEP {n} | R | {} (t={t:.4})\n\n", ctrl_desc(setup)));
            }
            StepKind::Request { seq, opcode, frames, .. } => {
                if AUTH_OPS.contains(opcode) && !keep_auth {
                    continue;
                }
                if DENIED_OPS.contains(opcode) {
                    return Err(format!("the capture's start sends the denied opcode {opcode:#x}"));
                }
                n += 1;
                let class = if AUTH_OPS.contains(opcode) {
                    "AUTH"
                } else if *opcode == CAL_OP {
                    "CAL"
                } else {
                    "C"
                };
                o.push_str(&format!(
                    "STEP {n} | {class} | seq={seq} op={opcode:#05x} {} (t={t:.4})\n",
                    crate::ttp::opcode_name(*opcode)
                ));
                for f in frames {
                    o.push_str(&format!("  BULK_OUT ep=0x05 len={} {}\n", f.len(), hex(f)));
                }
                o.push('\n');
            }
        }
    }
    Ok(o)
}

/// Bytes before the profile body in a 0x456 upload or 0x44c download payload: `00 00 | 15 | u32 BE len+4 | u32 BE len`.
pub const PROFILE_PREFIX_LEN: usize = 11;
/// The Tobii `calibration.setpm` header: `u32 LE 4 | u32 LE 1 | u32 LE len+4 | 4 bytes`, then the profile body.
pub const SETPM_HEADER_LEN: usize = 16;
/// Largest profile body accepted (the captured ones are 30-38 KB).
pub const PROFILE_MAX_LEN: usize = 256 * 1024;
/// Body bytes per bulk OUT frame of a long request (Windows: 4095-byte URBs = wrapper + 4087).
pub const OUT_MAX_BODY: usize = 4087;

/// Check a profile body's own layout: it starts with `u32 LE len - 168` (both captured profiles, 30 875 and 37 401 B).
pub fn check_profile(body: &[u8]) -> Result<(), String> {
    if body.len() < 168 || body.len() > PROFILE_MAX_LEN {
        return Err(format!("profile body of {} bytes is out of range", body.len()));
    }
    let lead = u32::from_le_bytes(body[..4].try_into().unwrap()) as usize;
    if lead != body.len() - 168 {
        return Err(format!("profile body does not start with its length ({lead} for {} bytes)", body.len()));
    }
    Ok(())
}

/// The profile body of a Tobii `calibration.setpm` (what Pimax's calibration saved under Windows), checked.
pub fn profile_from_setpm(file: &[u8]) -> Result<Vec<u8>, String> {
    if file.len() < SETPM_HEADER_LEN {
        return Err("not a calibration.setpm (too short)".into());
    }
    let w = |i: usize| u32::from_le_bytes(file[i..i + 4].try_into().unwrap()) as usize;
    let body = &file[SETPM_HEADER_LEN..];
    if w(0) != 4 || w(4) != 1 || w(8) != body.len() + 4 {
        return Err("not a calibration.setpm (header does not match)".into());
    }
    check_profile(body)?;
    Ok(body.to_vec())
}

/// The profile body inside a 0x456 upload or 0x44c download payload, when the payload has that layout.
pub fn profile_body(payload: &[u8]) -> Option<&[u8]> {
    let p = payload.get(..PROFILE_PREFIX_LEN)?;
    let body = &payload[PROFILE_PREFIX_LEN..];
    let be = |i: usize| u32::from_be_bytes(p[i..i + 4].try_into().unwrap()) as usize;
    (p[..3] == [0, 0, 0x15] && be(3) == body.len() + 4 && be(7) == body.len()).then_some(body)
}

/// The 0x456 payload that uploads `body` (the layout of the captured uploads and downloads).
pub fn profile_payload(body: &[u8]) -> Vec<u8> {
    let mut p = Vec::with_capacity(PROFILE_PREFIX_LEN + body.len());
    p.extend_from_slice(&[0, 0, 0x15]);
    p.extend_from_slice(&(body.len() as u32 + 4).to_be_bytes());
    p.extend_from_slice(&(body.len() as u32).to_be_bytes());
    p.extend_from_slice(body);
    p
}

/// Put the upload of `body` in place of each 0x456 step of the list (the Crystal Super's start uploads the profile
/// twice): same header (seq, status, aux) as that captured upload, the new payload, split into frames as Windows splits
/// it. The list must have a 0x456 step.
pub fn with_profile(steps: &mut [FStep], body: &[u8]) -> Result<(), String> {
    check_profile(body)?;
    let mut done = false;
    for st in steps.iter_mut() {
        if let Action::Ttp { op, frames, .. } = &mut st.action {
            if *op != CAL_OP {
                continue;
            }
            let (_, h) = ttp_info(frames)?;
            let payload = profile_payload(body);
            let m = Message {
                header: Header { payload_len: payload.len() as u32, ..h },
                payload,
            };
            *frames = m.to_frames(Direction::Out, OUT_MAX_BODY);
            done = true;
        }
    }
    if done {
        Ok(())
    } else {
        Err("the list has no 0x456 step".into())
    }
}

fn ctrl_desc(setup: &[u8; 8]) -> String {
    let wv = u16::from_le_bytes([setup[2], setup[3]]);
    let wi = u16::from_le_bytes([setup[4], setup[5]]);
    let wl = u16::from_le_bytes([setup[6], setup[7]]);
    format!(
        "CTRL {} bmRT={:#04x} bReq={} wValue={wv} wIndex={wi} wLength={wl}",
        if setup[0] & 0x80 != 0 { "IN " } else { "OUT" },
        setup[0],
        setup[1]
    )
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

    fn base_steps() -> Vec<FStep> {
        parse(BASE).unwrap()
    }

    #[test]
    fn a_list_made_from_a_plan_parses_back_to_the_same_transfers() {
        use crate::replay::{Plan, Step, StepKind};
        // a plan with the public base list's transfers, as a capture of it would give
        let base = parse(BASE).unwrap();
        let mut plan = Plan { device: 7, steps: Vec::new(), stream_start: Some(9.0), orphan_frames: 0 };
        for (i, st) in base.iter().enumerate() {
            let kind = match &st.action {
                Action::Enumeration => continue,
                Action::Control { setup, data } if setup[0] & 0x80 == 0 => {
                    StepKind::ControlOut { setup: *setup, data: data.clone() }
                }
                Action::Control { setup, .. } => StepKind::ControlIn { setup: *setup },
                Action::Ttp { seq, op, frames } => StepKind::Request {
                    seq: *seq,
                    opcode: *op,
                    payload_len: 0,
                    frames: frames.clone(),
                    expect: None,
                },
            };
            plan.steps.push(Step { packet_index: i as u64, time: i as f64, kind, missing_bytes: 0, total_bytes: 1 });
        }
        let text = from_plan(&plan, "test", false).unwrap();
        let back = parse(&text).unwrap();
        let want: Vec<&Action> = base.iter().map(|s| &s.action).filter(|a| **a != Action::Enumeration).collect();
        let got: Vec<&Action> = back.iter().map(|s| &s.action).collect();
        assert_eq!(got, want);
        // the made list is accepted by the gate with its own profile upload
        assert!(Guard::new_with(&back, false).is_ok());
        // a cut capture is refused
        plan.steps[0].missing_bytes = 3;
        assert!(from_plan(&plan, "test", false).is_err());
    }

    #[test]
    fn base_list_parses_and_has_no_auth_ops() {
        let s = base_steps();
        assert_eq!(s.len(), 24, "26 steps of the cold start minus the 2 auth steps");
        assert!(s.iter().all(|x| x.class != "AUTH"));
        let ops: Vec<u32> = s
            .iter()
            .filter_map(|x| match &x.action {
                Action::Ttp { op, .. } => Some(*op),
                _ => None,
            })
            .collect();
        assert!(ops.iter().all(|o| !AUTH_OPS.contains(o) && !DENIED_OPS.contains(o)), "{ops:x?}");
        assert_eq!(
            ops,
            [
                0x3e8, 0x640, 0x852, 0x77b, 0x532, 0x58c, 0x546, 0x547, 0x5d2, 0x5cd, 0x4b0, 0x596, 0x578,
                0x686, 0x67c, 0x5be, 0x672, 0x4c4, 0x456
            ]
        );
        // skipping auth changes nothing (it is absent); enumeration is never sent
        assert_eq!(select(&s, Skips::default()).len(), 23);
        assert_eq!(select(&s, Skips { auth: true, ..Skips::default() }).len(), 23);
    }

    #[test]
    fn base_calibration_step_matches_the_pins() {
        let s = base_steps();
        let Action::Ttp { frames, op, .. } = &s.last().unwrap().action else {
            panic!()
        };
        assert_eq!(*op, CAL_OP);
        let (m, h) = ttp_info(frames).unwrap();
        assert_eq!(h.payload_len, CAL_PAYLOAD_LEN);
        assert_eq!(m.payload.len(), CAL_PAYLOAD_LEN as usize);
        assert_eq!(fnv1a(&m.payload), CAL_PAYLOAD_FNV);
        // the guard accepts the list (it pins the blob) and rejects a changed blob
        assert!(Guard::new(&s).is_ok());
        let mut bad = s.clone();
        if let Action::Ttp { frames, .. } = &mut bad.last_mut().unwrap().action {
            let n = frames.len() - 1;
            let l = frames[n].len() - 1;
            frames[n][l] ^= 1;
        }
        assert!(Guard::new(&bad).is_err());
    }

    #[test]
    fn list_picks_the_base_list_unless_the_full_one_is_linked() {
        let (text, kind) = list();
        match kind {
            ListKind::Base => assert_eq!(text, BASE),
            ListKind::Full => {
                assert_eq!(text, DATA);
                assert!(parse(text).unwrap().iter().any(|x| x.class == "AUTH"));
            }
        }
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn base_frames_are_byte_identical_to_the_private_list() {
        let full = steps();
        let base = base_steps();
        assert_eq!(base.len() + 2, full.len());
        for b in &base {
            let f = full.iter().find(|x| x.n == b.n).expect("same step number in the full list");
            assert_eq!(b.class, f.class, "step {}", b.n);
            assert_eq!(b.desc, f.desc, "step {}", b.n);
            assert_eq!(b.action, f.action, "step {}", b.n);
        }
        let missing: Vec<u32> = full.iter().filter(|f| !base.iter().any(|b| b.n == f.n)).map(|f| f.n).collect();
        assert_eq!(missing, [8, 9]);
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

    fn cal_frames(steps: &[FStep]) -> Vec<Vec<u8>> {
        steps
            .iter()
            .find_map(|s| match &s.action {
                Action::Ttp { op, frames, .. } if *op == CAL_OP => Some(frames.clone()),
                _ => None,
            })
            .expect("a 0x456 step")
    }

    #[test]
    fn the_captured_profile_upload_is_rebuilt_byte_for_byte_from_its_body() {
        let mut base = base_steps();
        let captured = cal_frames(&base);
        let (m, _) = ttp_info(&captured).unwrap();
        let body = profile_body(&m.payload).expect("the captured upload has the profile layout").to_vec();
        check_profile(&body).unwrap();
        with_profile(&mut base, &body).unwrap();
        assert_eq!(cal_frames(&base), captured);
        // and the list still passes the pinned guard
        Guard::new(&base).unwrap();
    }

    #[test]
    fn another_profile_replaces_the_upload_and_needs_the_unpinned_guard() {
        let mut base = base_steps();
        let mut body = vec![0x5au8; 37401];
        body[..4].copy_from_slice(&(37401u32 - 168).to_le_bytes());
        let mut setpm = Vec::new();
        for w in [4u32, 1, 37405, 0x1244_0846] {
            setpm.extend_from_slice(&w.to_le_bytes());
        }
        setpm.extend_from_slice(&body);
        assert_eq!(profile_from_setpm(&setpm).unwrap(), body);
        with_profile(&mut base, &body).unwrap();
        let frames = cal_frames(&base);
        // 24 + 37 412 bytes in 4087-byte bodies, as Windows splits it
        assert_eq!(frames.len(), 10);
        assert!(frames[..9].iter().all(|f| f.len() == 4095));
        let (m, h) = ttp_info(&frames).unwrap();
        assert_eq!(h.payload_len as usize, 37412);
        assert_eq!(profile_body(&m.payload).unwrap(), &body[..]);
        assert!(Guard::new(&base).is_err());
        Guard::new_with(&base, false).unwrap();
    }

    #[test]
    fn a_broken_setpm_or_profile_is_refused() {
        assert!(profile_from_setpm(&[4, 0, 0, 0]).is_err());
        let mut f = vec![0u8; 16 + 400];
        f[0] = 4;
        f[4] = 1;
        f[8..12].copy_from_slice(&404u32.to_le_bytes());
        // body does not start with its length
        assert!(profile_from_setpm(&f).is_err());
        f[16..20].copy_from_slice(&(400u32 - 168).to_le_bytes());
        assert!(profile_from_setpm(&f).is_ok());
        f[8] = 0;
        assert!(profile_from_setpm(&f).is_err());
        assert!(check_profile(&[0u8; 100]).is_err());
        let mut steps = base_steps();
        steps.retain(|s| !matches!(&s.action, Action::Ttp { op, .. } if *op == CAL_OP));
        assert!(with_profile(&mut steps, &f[16..]).is_err());
    }
}
