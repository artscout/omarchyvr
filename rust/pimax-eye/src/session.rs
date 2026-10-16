//! Running a replay plan against a [`Transport`] and reading the gaze stream.

use crate::gaze::{parse_payload, GazeSample, GazeStats};
use crate::replay::{Plan, StepKind, EP_BULK_IN, EP_BULK_OUT};
use crate::transport::Transport;
use crate::ttp::{Direction, FrameError, Kind, Message, StreamDecoder, STREAM_GAZE};
use std::fmt;
use std::io;
use std::time::{Duration, Instant};

/// Size of the IN buffer: larger than any frame seen on the stream.
pub const IN_BUFFER: usize = 16384;

/// The unsubscribe request Pimax Play sent when it stopped the gaze stream (`20261013-124248-poweron-hub2.pcap`,
/// device 14, EP 0x05, seq 26, opcode 0x4ce, stream 0x0505). Byte for byte as captured; the device answers with the
/// request's own sequence number, so the stale number is harmless. Only sent on explicit request (`--stop`).
pub const CAPTURED_UNSUBSCRIBE_GAZE: [u8; 43] = [
    0x00, 0x00, 0x00, 0x00, 0x23, 0x00, 0x00, 0x00, // wrapper: OUT, 35 body bytes
    0x00, 0x00, 0x00, 0x51, 0x00, 0x00, 0x00, 0x1a, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x04, 0xce,
    0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
    0x0b, // header: request, seq 26, status 0, op 0x4ce, aux 0, 11 payload bytes
    0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x04, 0x00, 0x00, 0x05, 0x05, // prefix + tag2(0x0505)
];

/// Options for [`run_plan`].
#[derive(Debug, Clone)]
pub struct ReplayOptions {
    /// Skip steps whose data is not fully in the capture instead of refusing the whole plan.
    pub allow_incomplete: bool,
    /// Timeout of every USB transfer and of every response wait.
    pub timeout: Duration,
}

impl Default for ReplayOptions {
    fn default() -> Self {
        ReplayOptions {
            allow_incomplete: false,
            timeout: Duration::from_secs(2),
        }
    }
}

/// Replay failure. A failure stops the replay immediately: nothing is retried and nothing further is sent.
#[derive(Debug)]
pub enum ReplayError {
    /// The plan has no steps.
    EmptyPlan,
    /// Refused before sending anything: these steps are not fully in the capture.
    Incomplete(Vec<IncompleteStep>),
    /// USB error.
    Io {
        /// Step number.
        step: usize,
        /// Step description.
        what: String,
        /// Underlying error.
        source: io::Error,
    },
    /// No response within the timeout.
    NoResponse {
        /// Step number.
        step: usize,
        /// Request sequence number.
        seq: u32,
        /// Request opcode.
        opcode: u32,
    },
    /// The device answered with another status than Windows saw.
    BadStatus {
        /// Step number.
        step: usize,
        /// Sequence number.
        seq: u32,
        /// Opcode.
        opcode: u32,
        /// Status received.
        got: u32,
        /// Status in the capture.
        expected: u32,
    },
    /// The IN stream could not be framed.
    Framing(FrameError),
}

/// A step that cannot be replayed verbatim.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncompleteStep {
    /// Step number.
    pub step: usize,
    /// Description.
    pub what: String,
    /// Bytes missing from the capture.
    pub missing: u64,
    /// Real size of the step.
    pub total: u64,
}

impl fmt::Display for ReplayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ReplayError::EmptyPlan => write!(f, "the capture holds no vendor control or TTP request transfer for this device"),
            ReplayError::Incomplete(v) => {
                write!(f, "refusing to replay: {} step(s) are cut by the capture snap length:", v.len())?;
                for s in v {
                    write!(f, "\n  step {}: {} ({} of {} bytes missing)", s.step, s.what, s.missing, s.total)?;
                }
                Ok(())
            }
            ReplayError::Io { step, what, source } => write!(f, "step {step} ({what}): {source}"),
            ReplayError::NoResponse { step, seq, opcode } => write!(
                f,
                "step {step}: no response to request seq {seq} opcode {opcode:#x} ({})",
                crate::ttp::opcode_name(*opcode)
            ),
            ReplayError::BadStatus { step, seq, opcode, got, expected } => write!(
                f,
                "step {step}: request seq {seq} opcode {opcode:#x} answered with status {got}, Windows saw {expected}"
            ),
            ReplayError::Framing(e) => write!(f, "IN stream framing error: {e}"),
        }
    }
}

impl std::error::Error for ReplayError {}

/// One request/response exchange of a replay.
#[derive(Debug, Clone)]
pub struct Exchange {
    /// Step number.
    pub step: usize,
    /// Sequence number.
    pub seq: u32,
    /// Opcode.
    pub opcode: u32,
    /// Status word of the response.
    pub status: u32,
    /// Payload length of the response.
    pub payload_len: u32,
    /// Time from the last OUT frame to the response.
    pub round_trip: Duration,
}

/// Outcome of a replay.
#[derive(Debug, Default)]
pub struct ReplayReport {
    /// Steps sent.
    pub sent: usize,
    /// Steps skipped because they were incomplete (only with `allow_incomplete`).
    pub skipped: Vec<usize>,
    /// TTP exchanges.
    pub exchanges: Vec<Exchange>,
    /// Bytes returned by control IN steps, per step.
    pub control_in: Vec<(usize, usize)>,
    /// Gaze notifications that arrived while the replay was running.
    pub notifications: u64,
}

/// Send the plan. Every byte sent comes from `plan`, which comes from a capture.
pub fn run_plan<T: Transport>(
    t: &mut T,
    plan: &Plan,
    opts: &ReplayOptions,
    dec: &mut StreamDecoder,
    mut log: impl FnMut(&str),
) -> Result<ReplayReport, ReplayError> {
    if plan.steps.is_empty() {
        return Err(ReplayError::EmptyPlan);
    }
    if !opts.allow_incomplete && !plan.is_complete() {
        return Err(ReplayError::Incomplete(
            plan.steps
                .iter()
                .enumerate()
                .filter(|(_, s)| !s.is_complete())
                .map(|(i, s)| IncompleteStep {
                    step: i,
                    what: s.label(),
                    missing: s.missing_bytes,
                    total: s.total_bytes,
                })
                .collect(),
        ));
    }
    let mut report = ReplayReport::default();
    for (i, step) in plan.steps.iter().enumerate() {
        if !step.is_complete() {
            log(&format!(
                "step {i}: SKIPPED (incomplete in the capture): {}",
                step.label()
            ));
            report.skipped.push(i);
            continue;
        }
        let io_err = |source| ReplayError::Io {
            step: i,
            what: step.label(),
            source,
        };
        match &step.kind {
            StepKind::ControlOut { setup, data } => {
                t.control(*setup, data, opts.timeout).map_err(io_err)?;
            }
            StepKind::ControlIn { setup } => {
                let d = t.control(*setup, &[], opts.timeout).map_err(io_err)?;
                report.control_in.push((i, d.len()));
            }
            StepKind::Request {
                seq,
                opcode,
                frames,
                expect,
                ..
            } => {
                for f in frames {
                    t.bulk_out(EP_BULK_OUT, f, opts.timeout).map_err(io_err)?;
                }
                let sent_at = Instant::now();
                let deadline = sent_at + opts.timeout;
                let resp = loop {
                    let now = Instant::now();
                    if now >= deadline {
                        return Err(ReplayError::NoResponse {
                            step: i,
                            seq: *seq,
                            opcode: *opcode,
                        });
                    }
                    let wait = (deadline - now).min(Duration::from_millis(250));
                    let Some(bytes) = t.bulk_in(EP_BULK_IN, IN_BUFFER, wait).map_err(io_err)?
                    else {
                        continue;
                    };
                    let mut found = None;
                    for r in dec.push(&bytes) {
                        let m = r.map_err(ReplayError::Framing)?;
                        match m.header.kind {
                            Kind::Response if m.header.seq == *seq => found = Some(m),
                            Kind::Notification if m.header.id == STREAM_GAZE => {
                                report.notifications += 1;
                            }
                            _ => {}
                        }
                    }
                    if let Some(m) = found {
                        break m;
                    }
                };
                if let Some(e) = expect {
                    if resp.header.status != e.status {
                        return Err(ReplayError::BadStatus {
                            step: i,
                            seq: *seq,
                            opcode: *opcode,
                            got: resp.header.status,
                            expected: e.status,
                        });
                    }
                }
                log(&format!(
                    "step {i}: seq {seq} op {opcode:#05x} -> status {} payload {} ({} ms)",
                    resp.header.status,
                    resp.header.payload_len,
                    sent_at.elapsed().as_millis()
                ));
                report.exchanges.push(Exchange {
                    step: i,
                    seq: *seq,
                    opcode: *opcode,
                    status: resp.header.status,
                    payload_len: resp.header.payload_len,
                    round_trip: sent_at.elapsed(),
                });
            }
        }
        report.sent += 1;
    }
    Ok(report)
}

/// Result of [`read_stream`].
#[derive(Debug, Default)]
pub struct StreamSummary {
    /// Wall-clock time spent reading.
    pub elapsed: Duration,
    /// USB bytes read.
    pub bytes: u64,
    /// IN transfers that returned data.
    pub transfers: u64,
    /// Reads that timed out.
    pub timeouts: u64,
    /// Gaze samples decoded.
    pub samples: u64,
    /// Gaze notifications whose payload failed to decode.
    pub parse_errors: u64,
    /// First decode error.
    pub first_parse_error: Option<String>,
    /// Framing errors on the IN stream.
    pub framing_errors: u64,
    /// Messages that were not gaze notifications.
    pub other_messages: u64,
    /// Time of the first sample since the start of the read.
    pub first_sample_after: Option<Duration>,
    /// Host-clock time between the first and the last sample.
    pub sample_span: Duration,
    /// Per-eye statistics.
    pub stats: GazeStats,
}

impl StreamSummary {
    /// Sample rate by the host clock.
    pub fn host_rate_hz(&self) -> Option<f64> {
        (self.samples > 1 && !self.sample_span.is_zero())
            .then(|| (self.samples - 1) as f64 / self.sample_span.as_secs_f64())
    }
}

/// Read and decode the stream for `duration`. `on_sample` gets every sample with the host time since the start.
pub fn read_stream<T: Transport>(
    t: &mut T,
    dec: &mut StreamDecoder,
    duration: Duration,
    mut on_sample: impl FnMut(&GazeSample, Duration),
) -> io::Result<StreamSummary> {
    let start = Instant::now();
    let mut s = StreamSummary::default();
    let mut first_at = None;
    let mut last_at = Duration::ZERO;
    while start.elapsed() < duration {
        let left = duration.saturating_sub(start.elapsed());
        let wait = left
            .min(Duration::from_millis(100))
            .max(Duration::from_millis(1));
        let Some(bytes) = t.bulk_in(EP_BULK_IN, IN_BUFFER, wait)? else {
            s.timeouts += 1;
            continue;
        };
        let at = start.elapsed();
        s.bytes += bytes.len() as u64;
        s.transfers += 1;
        for r in dec.push(&bytes) {
            match r {
                Ok(m) => {
                    handle_message(&m, at, &mut s, &mut on_sample, &mut first_at, &mut last_at)
                }
                Err(_) => s.framing_errors += 1,
            }
        }
    }
    s.elapsed = start.elapsed();
    s.first_sample_after = first_at;
    s.sample_span = last_at.saturating_sub(first_at.unwrap_or(Duration::ZERO));
    Ok(s)
}

fn handle_message(
    m: &Message,
    at: Duration,
    s: &mut StreamSummary,
    on_sample: &mut impl FnMut(&GazeSample, Duration),
    first_at: &mut Option<Duration>,
    last_at: &mut Duration,
) {
    if m.header.kind != Kind::Notification || m.header.id != STREAM_GAZE {
        s.other_messages += 1;
        return;
    }
    match parse_payload(&m.payload) {
        Ok(g) => {
            s.samples += 1;
            s.stats.add(&g);
            first_at.get_or_insert(at);
            *last_at = at;
            on_sample(&g, at);
        }
        Err(e) => {
            s.parse_errors += 1;
            s.first_parse_error.get_or_insert_with(|| e.to_string());
        }
    }
}

/// A fresh decoder for the IN pipe.
pub fn in_decoder() -> StreamDecoder {
    StreamDecoder::new(Direction::In)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlv::tag;
    use crate::ttp::{Header, Wrapper};

    #[test]
    fn captured_unsubscribe_is_byte_exact_with_the_capture() {
        // hex of the OUT transfer at t=10.5906 s of 20261013-124248-poweron-hub2.pcap, taken with an independent script
        let hex = "0000000023000000000000510000001a00000000000004ce000000000000000b0000020000000400000505";
        let want: Vec<u8> = (0..hex.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap())
            .collect();
        assert_eq!(CAPTURED_UNSUBSCRIBE_GAZE.to_vec(), want);
    }

    #[test]
    fn captured_unsubscribe_is_a_valid_request() {
        let w = Wrapper::parse(&CAPTURED_UNSUBSCRIBE_GAZE).unwrap();
        assert_eq!(w.dir, Direction::Out);
        assert_eq!(w.frame_len(), CAPTURED_UNSUBSCRIBE_GAZE.len());
        let m = StreamDecoder::new(Direction::Out)
            .push(&CAPTURED_UNSUBSCRIBE_GAZE)
            .remove(0)
            .unwrap();
        let Header {
            kind,
            seq,
            id,
            payload_len,
            ..
        } = m.header;
        assert_eq!((kind, seq, id, payload_len), (Kind::Request, 26, 0x4ce, 11));
        let items: Vec<_> = m.items().map(|i| i.unwrap()).collect();
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].tag, tag::INT32);
        assert_eq!(items[0].value, &[0, 0, 5, 5]);
    }
}
