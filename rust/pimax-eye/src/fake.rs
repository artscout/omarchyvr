//! An in-process EyeChip for tests: it frames and answers TTP requests like the real one and, once the profile upload
//! (or any configured opcode) completed, emits synthetic gaze notifications at the real rate.
//!
//! It also records every byte the client sent, so tests can assert that a replay sent exactly the plan and nothing else.

use crate::gaze::{encode_payload, Eye, FrameFields, GazeSample, NOMINAL_PERIOD_US};
use crate::transport::Transport;
use crate::ttp::{Direction, Kind, Message, StreamDecoder, STREAM_GAZE};
use std::collections::{HashMap, VecDeque};
use std::io;
use std::time::Duration;

/// Everything the client sent.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Sent {
    /// Control transfer: setup and data stage.
    Control {
        /// Setup packet.
        setup: [u8; 8],
        /// OUT data.
        data: Vec<u8>,
    },
    /// Bulk OUT transfer: endpoint and bytes.
    BulkOut {
        /// Endpoint address.
        ep: u8,
        /// Bytes.
        data: Vec<u8>,
    },
}

/// Fake device.
pub struct FakeChip {
    /// Everything received, in order.
    pub sent: Vec<Sent>,
    /// Opcodes whose response gets this status instead of 1.
    pub status_override: HashMap<u32, u32>,
    /// Opcodes that never get a response.
    pub silent: Vec<u32>,
    /// Opcode whose completion starts the gaze stream (`None`: never; the profile upload 0x456 by default).
    pub start_on: Option<u32>,
    /// Stream gaze from the beginning (a session left open by an earlier run).
    pub streaming: bool,
    /// Emitted frame period.
    pub period: Duration,
    /// Number of notifications emitted so far.
    pub emitted: u32,
    inbound: StreamDecoder,
    queue: VecDeque<Vec<u8>>,
    next_ts: i64,
}

impl Default for FakeChip {
    fn default() -> Self {
        FakeChip::new()
    }
}

impl FakeChip {
    /// A chip that starts streaming after opcode 0x456.
    pub fn new() -> FakeChip {
        FakeChip {
            sent: Vec::new(),
            status_override: HashMap::new(),
            silent: Vec::new(),
            start_on: Some(0x456),
            streaming: false,
            period: Duration::from_micros(NOMINAL_PERIOD_US as u64),
            emitted: 0,
            inbound: StreamDecoder::new(Direction::Out),
            queue: VecDeque::new(),
            next_ts: 50_000_000,
        }
    }

    /// The synthetic sample number `n`: eye A tracked on a slow circle, eye B invalid.
    pub fn sample(n: u32, ts: i64) -> GazeSample {
        let ph = f64::from(n) * 0.02;
        let mut dir = [0.2 * ph.sin(), 0.1 * ph.cos(), 1.0];
        let norm = dir.iter().map(|c| c * c).sum::<f64>().sqrt();
        for c in &mut dir {
            *c /= norm;
        }
        let a = Eye {
            origin_mm: [31.0, -2.0, -22.0],
            origin_valid: true,
            direction: dir,
            direction_valid: true,
            pupil_diameter_mm: 3.25,
            pupil_pos: [0.5, 0.5],
            pupil_pos_valid: true,
            flags: [1, 0],
            ..Eye::default()
        };
        let b = Eye {
            flags: [1, 0],
            ..Eye::default()
        };
        GazeSample {
            timestamp_us: ts,
            frame_counter: Some(1000 + n),
            eyes: [a, b],
            frame: FrameFields {
                field_14: 0xfffff,
                field_35: 1,
                ..FrameFields::default()
            },
            ..GazeSample::default()
        }
    }

    fn notification(&mut self) -> Vec<u8> {
        let g = FakeChip::sample(self.emitted, self.next_ts);
        self.emitted += 1;
        self.next_ts += NOMINAL_PERIOD_US;
        let payload = encode_payload(&g);
        let mut m = Message::new(Kind::Notification, 0, 0, STREAM_GAZE, &[]);
        m.payload = payload;
        m.header.payload_len = m.payload.len() as u32;
        m.to_frames(Direction::In, 1 << 20).concat()
    }

    fn answer(&mut self, req: &Message) {
        if self.silent.contains(&req.header.id) {
            return;
        }
        let status = self
            .status_override
            .get(&req.header.id)
            .copied()
            .unwrap_or(1);
        let resp = Message::new(Kind::Response, req.header.seq, status, req.header.id, &[]);
        // large replies are split like the real chip does for 0x44c: header + 0 payload here, one frame
        self.queue
            .push_back(resp.to_frames(Direction::In, 1 << 20).concat());
        if req.header.id == 0x4ce {
            self.streaming = false;
            self.queue.retain(|f| f.len() < 1000); // pending gaze frames go away with the subscription
        }
        if Some(req.header.id) == self.start_on {
            self.streaming = true;
        }
    }
}

impl Transport for FakeChip {
    fn control(&mut self, setup: [u8; 8], out: &[u8], _timeout: Duration) -> io::Result<Vec<u8>> {
        self.sent.push(Sent::Control {
            setup,
            data: out.to_vec(),
        });
        if setup[0] & 0x80 != 0 {
            Ok(vec![
                0;
                usize::from(u16::from_le_bytes([setup[6], setup[7]]))
                    .min(8)
            ])
        } else {
            Ok(Vec::new())
        }
    }

    fn bulk_out(&mut self, ep: u8, data: &[u8], _timeout: Duration) -> io::Result<()> {
        self.sent.push(Sent::BulkOut {
            ep,
            data: data.to_vec(),
        });
        for m in self.inbound.push(data) {
            let m = m.map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e.to_string()))?;
            if m.header.kind == Kind::Request {
                self.answer(&m);
            }
        }
        Ok(())
    }

    fn bulk_in(
        &mut self,
        _ep: u8,
        _max_len: usize,
        timeout: Duration,
    ) -> io::Result<Option<Vec<u8>>> {
        if let Some(f) = self.queue.pop_front() {
            return Ok(Some(f));
        }
        if self.streaming {
            std::thread::sleep(self.period.min(timeout));
            return Ok(Some(self.notification()));
        }
        std::thread::sleep(timeout.min(Duration::from_millis(5)));
        Ok(None)
    }
}
