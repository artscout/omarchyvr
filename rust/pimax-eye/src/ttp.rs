//! TTP framing of the EyeChip bulk pipe (OUT EP 0x05, IN EP 0x83).
//!
//! ```text
//! frame   = wrapper(8) | body
//! wrapper = u32 LE dir (0 = host->device, 1 = device->host) | u32 LE len
//!           OUT: len = body length          (0x47 for a 79-byte transfer)
//!           IN : len = whole frame length   (0x4da for a 1242-byte transfer)
//! message = header(24, big endian) | payload(payload_len)
//! header  = magic | seq | status | id | aux | payload_len
//!           magic 0x51 request, 0x52 response, 0x53 notification
//!           id    opcode (request/response) or stream id (notification, 0x0505 = gaze)
//!           status 0 in requests, 1 in the responses seen so far
//! payload = 00 00 | TLV items          (empty, without the 2 zero bytes, when payload_len is 0)
//! ```
//! A message normally fits one frame. Larger ones are split into several frames, each with its own wrapper
//! (OUT: 4087-byte bodies for the 34 612-byte profile upload; IN: the 0x44c reply is 35 + 34 437 + 164 bytes of
//! body). Only the first frame carries the 24-byte header; the receiver counts payload bytes until `payload_len`.
//! The wrapper `len` semantics differ between the directions; this was checked on every bulk transfer of the eight
//! captures that contain the EyeChip (2 960 OUT and 44 000+ IN frames, no exception except the 16 KiB read buffers
//! of the Windows stack in `poweron`, where the USB transfer is larger than the frame).

use crate::tlv::Items;

/// Names of the request opcodes seen in the captures. Derived from payload shapes and timing, not from documentation:
/// treat them as labels, not as protocol truth. A trailing `?` marks a weaker guess.
pub fn opcode_name(op: u32) -> &'static str {
    match op {
        0x3e8 => "hello / capability list?",
        0x640 => "get (three zero ints)?",
        0x76c => "authentication step 1 (reply carries a 256-byte blob)",
        0x777 => "authentication step 2 (16-byte answer + 4-byte tag)",
        0x77b => "authentication finish?",
        0x852 => "configuration upload (2771-byte payload)",
        0x532 => "get platform properties",
        0x58c => "get ids (serial, platform, firmware)",
        0x546 | 0x547 | 0x5d2 => "get settings dump?",
        0x5cd => "get (one int)?",
        0x4b0 => "get feature list",
        0x596 | 0x578 => "get table (zeros)?",
        0x686 => "get supported gaze rates",
        0x67c => "set gaze rate",
        0x672 => "get gaze rate",
        0x5be => "set eye offsets (+IPD/2,0,0,-IPD/2,0,0 mm)",
        0x4c4 => "subscribe stream",
        0x4ce => "unsubscribe stream",
        0x456 => "upload user profile 'human' (34612-byte payload)",
        0x44c => "download user profile",
        0x3f2 => "calibration: status/start?",
        0x407 => "calibration: add target point?",
        0x42e => "calibration: compute?",
        0x3fc => "calibration: finish?",
        _ => "unknown",
    }
}

/// Wrapper size.
pub const WRAPPER_LEN: usize = 8;
/// Header size.
pub const HEADER_LEN: usize = 24;
/// Largest message payload accepted by the stream decoder (the biggest seen is 34 612 bytes).
pub const MAX_PAYLOAD_LEN: usize = 1 << 20;
/// Stream id of the 120 Hz gaze notifications.
pub const STREAM_GAZE: u32 = 0x0505;

/// Magic of requests.
pub const MAGIC_REQUEST: u32 = 0x51;
/// Magic of responses.
pub const MAGIC_RESPONSE: u32 = 0x52;
/// Magic of notifications.
pub const MAGIC_NOTIFICATION: u32 = 0x53;

/// Framing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Not enough bytes for the structure.
    Short {
        /// Bytes needed.
        needed: usize,
        /// Bytes available.
        available: usize,
    },
    /// Wrapper direction word is neither 0 nor 1.
    BadDirection(u32),
    /// Wrapper direction differs from the direction the decoder was created for.
    WrongDirection(Direction),
    /// Wrapper length is impossible (IN frames include the 8 wrapper bytes) or absurdly large.
    BadLength(u32),
    /// Header magic is not 0x51, 0x52 or 0x53.
    BadMagic(u32),
    /// Header announces a payload larger than [`MAX_PAYLOAD_LEN`].
    TooLarge(u32),
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::Short { needed, available } => {
                write!(f, "need {needed} bytes, have {available}")
            }
            FrameError::BadDirection(d) => write!(f, "bad wrapper direction word {d:#x}"),
            FrameError::WrongDirection(d) => write!(f, "unexpected wrapper direction {d:?}"),
            FrameError::BadLength(l) => write!(f, "bad wrapper length {l:#x}"),
            FrameError::BadMagic(m) => write!(f, "bad TTP magic {m:#x}"),
            FrameError::TooLarge(l) => write!(f, "payload length {l} exceeds the limit"),
        }
    }
}

impl std::error::Error for FrameError {}

/// Bulk direction as written in the wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Direction {
    /// Host to device (wire value 0).
    Out,
    /// Device to host (wire value 1).
    In,
}

impl Direction {
    /// Wire value.
    pub fn wire(self) -> u32 {
        match self {
            Direction::Out => 0,
            Direction::In => 1,
        }
    }

    /// From the wire value.
    pub fn from_wire(w: u32) -> Option<Direction> {
        match w {
            0 => Some(Direction::Out),
            1 => Some(Direction::In),
            _ => None,
        }
    }
}

/// The 8-byte wrapper in front of every bulk transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Wrapper {
    /// Direction word.
    pub dir: Direction,
    /// Length word (see the module docs for the direction-dependent meaning).
    pub len: u32,
}

impl Wrapper {
    /// Parse the first 8 bytes.
    pub fn parse(b: &[u8]) -> Result<Wrapper, FrameError> {
        if b.len() < WRAPPER_LEN {
            return Err(FrameError::Short {
                needed: WRAPPER_LEN,
                available: b.len(),
            });
        }
        let d = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let len = u32::from_le_bytes([b[4], b[5], b[6], b[7]]);
        let dir = Direction::from_wire(d).ok_or(FrameError::BadDirection(d))?;
        if dir == Direction::In && (len as usize) < WRAPPER_LEN {
            return Err(FrameError::BadLength(len));
        }
        Ok(Wrapper { dir, len })
    }

    /// Wrapper for a frame carrying `body_len` body bytes.
    pub fn for_body(dir: Direction, body_len: usize) -> Wrapper {
        let len = match dir {
            Direction::Out => body_len,
            Direction::In => body_len + WRAPPER_LEN,
        };
        Wrapper {
            dir,
            len: u32::try_from(len).unwrap_or(u32::MAX),
        }
    }

    /// Bytes after the wrapper.
    pub fn body_len(&self) -> usize {
        match self.dir {
            Direction::Out => self.len as usize,
            Direction::In => (self.len as usize).saturating_sub(WRAPPER_LEN),
        }
    }

    /// Whole frame length on the bus, wrapper included.
    pub fn frame_len(&self) -> usize {
        self.body_len() + WRAPPER_LEN
    }

    /// Wire bytes.
    pub fn encode(&self) -> [u8; WRAPPER_LEN] {
        let mut o = [0u8; WRAPPER_LEN];
        o[..4].copy_from_slice(&self.dir.wire().to_le_bytes());
        o[4..].copy_from_slice(&self.len.to_le_bytes());
        o
    }
}

/// Message class from the header magic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    /// 0x51.
    Request,
    /// 0x52.
    Response,
    /// 0x53.
    Notification,
}

impl Kind {
    /// Header magic of this kind.
    pub fn magic(self) -> u32 {
        match self {
            Kind::Request => MAGIC_REQUEST,
            Kind::Response => MAGIC_RESPONSE,
            Kind::Notification => MAGIC_NOTIFICATION,
        }
    }

    /// Kind from a header magic.
    pub fn from_magic(m: u32) -> Option<Kind> {
        match m {
            MAGIC_REQUEST => Some(Kind::Request),
            MAGIC_RESPONSE => Some(Kind::Response),
            MAGIC_NOTIFICATION => Some(Kind::Notification),
            _ => None,
        }
    }
}

/// The 24-byte big-endian message header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    /// Message class.
    pub kind: Kind,
    /// Sequence number: counts requests from 1; responses repeat it; notifications use 0.
    pub seq: u32,
    /// 0 in requests; 1 in every response seen so far; 0 in notifications.
    pub status: u32,
    /// Opcode (request/response) or stream id (notification).
    pub id: u32,
    /// Word at offset 16: 0 everywhere except the responses to the calibration ops 0x407/0x42e, where it holds a handle-like value.
    pub aux: u32,
    /// Payload length announced by the header (includes the 2-byte prefix).
    pub payload_len: u32,
}

impl Header {
    /// Parse the first 24 bytes of a frame body.
    pub fn parse(b: &[u8]) -> Result<Header, FrameError> {
        if b.len() < HEADER_LEN {
            return Err(FrameError::Short {
                needed: HEADER_LEN,
                available: b.len(),
            });
        }
        let w = |i: usize| u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]);
        let kind = Kind::from_magic(w(0)).ok_or(FrameError::BadMagic(w(0)))?;
        Ok(Header {
            kind,
            seq: w(4),
            status: w(8),
            id: w(12),
            aux: w(16),
            payload_len: w(20),
        })
    }

    /// Wire bytes.
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut o = [0u8; HEADER_LEN];
        for (i, v) in [
            self.kind.magic(),
            self.seq,
            self.status,
            self.id,
            self.aux,
            self.payload_len,
        ]
        .into_iter()
        .enumerate()
        {
            o[i * 4..i * 4 + 4].copy_from_slice(&v.to_be_bytes());
        }
        o
    }
}

/// A complete message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Message {
    /// Header.
    pub header: Header,
    /// Payload bytes: the 2-byte prefix and the TLV items. Empty when `payload_len` is 0.
    pub payload: Vec<u8>,
}

impl Message {
    /// Build a message from TLV item bytes (adds the 2-byte prefix when there are items).
    pub fn new(kind: Kind, seq: u32, status: u32, id: u32, items: &[u8]) -> Message {
        let mut payload = Vec::new();
        if !items.is_empty() {
            payload.extend_from_slice(&[0, 0]);
            payload.extend_from_slice(items);
        }
        Message {
            header: Header {
                kind,
                seq,
                status,
                id,
                aux: 0,
                payload_len: payload.len() as u32,
            },
            payload,
        }
    }

    /// Header followed by payload.
    pub fn body(&self) -> Vec<u8> {
        let mut b = Vec::with_capacity(HEADER_LEN + self.payload.len());
        b.extend_from_slice(&self.header.encode());
        b.extend_from_slice(&self.payload);
        b
    }

    /// The TLV items after the 2-byte prefix.
    pub fn items(&self) -> Items<'_> {
        Items::new(self.payload.get(2..).unwrap_or(&[]))
    }

    /// Wire bytes of the message split into frames of at most `max_body` body bytes (at least 1).
    pub fn to_frames(&self, dir: Direction, max_body: usize) -> Vec<Vec<u8>> {
        let body = self.body();
        body.chunks(max_body.max(1))
            .map(|c| {
                let mut f = Wrapper::for_body(dir, c.len()).encode().to_vec();
                f.extend_from_slice(c);
                f
            })
            .collect()
    }
}

/// Incremental decoder for one direction of the bulk pipe. Feed it the bytes of consecutive USB transfers (any chunking).
///
/// On a framing error the buffered bytes and any half-assembled message are dropped and the error is reported once;
/// decoding continues with the next bytes.
#[derive(Debug)]
pub struct StreamDecoder {
    dir: Direction,
    buf: Vec<u8>,
    hdr: Vec<u8>,
    partial: Option<Partial>,
}

#[derive(Debug)]
struct Partial {
    header: Header,
    payload: Vec<u8>,
    want: usize,
}

impl StreamDecoder {
    /// Decoder for frames of `dir`.
    pub fn new(dir: Direction) -> StreamDecoder {
        StreamDecoder {
            dir,
            buf: Vec::new(),
            hdr: Vec::new(),
            partial: None,
        }
    }

    /// Bytes buffered but not yet part of a complete frame.
    pub fn buffered(&self) -> usize {
        self.buf.len()
    }

    fn reset(&mut self) {
        self.buf.clear();
        self.hdr.clear();
        self.partial = None;
    }

    /// Feed bytes; returns the messages completed by them and the errors met.
    pub fn push(&mut self, data: &[u8]) -> Vec<Result<Message, FrameError>> {
        let mut out = Vec::new();
        self.buf.extend_from_slice(data);
        loop {
            if self.buf.len() < WRAPPER_LEN {
                break;
            }
            let w = match Wrapper::parse(&self.buf) {
                Ok(w) if w.dir == self.dir => w,
                Ok(_) => {
                    out.push(Err(FrameError::WrongDirection(self.dir)));
                    self.reset();
                    break;
                }
                Err(e) => {
                    out.push(Err(e));
                    self.reset();
                    break;
                }
            };
            let total = w.frame_len();
            if total > MAX_PAYLOAD_LEN + HEADER_LEN + WRAPPER_LEN {
                out.push(Err(FrameError::BadLength(w.len)));
                self.reset();
                break;
            }
            if self.buf.len() < total {
                break;
            }
            let body: Vec<u8> = self.buf[WRAPPER_LEN..total].to_vec();
            self.buf.drain(..total);
            if let Err(e) = self.feed_body(&body, &mut out) {
                out.push(Err(e));
                self.reset();
                break;
            }
        }
        out
    }

    fn feed_body(
        &mut self,
        mut body: &[u8],
        out: &mut Vec<Result<Message, FrameError>>,
    ) -> Result<(), FrameError> {
        while !body.is_empty() {
            if let Some(mut p) = self.partial.take() {
                let take = (p.want - p.payload.len()).min(body.len());
                p.payload.extend_from_slice(&body[..take]);
                body = &body[take..];
                if p.payload.len() == p.want {
                    out.push(Ok(Message {
                        header: p.header,
                        payload: p.payload,
                    }));
                } else {
                    self.partial = Some(p);
                }
                continue;
            }
            // The header normally sits whole in the first frame, but accept one split over several frames.
            let take = (HEADER_LEN - self.hdr.len()).min(body.len());
            self.hdr.extend_from_slice(&body[..take]);
            body = &body[take..];
            if self.hdr.len() < HEADER_LEN {
                break;
            }
            let header = Header::parse(&self.hdr)?;
            self.hdr.clear();
            let want = header.payload_len as usize;
            if want > MAX_PAYLOAD_LEN {
                return Err(FrameError::TooLarge(header.payload_len));
            }
            if want == 0 {
                out.push(Ok(Message {
                    header,
                    payload: Vec::new(),
                }));
            } else {
                self.partial = Some(Partial {
                    header,
                    payload: Vec::with_capacity(want),
                    want,
                });
            }
        }
        Ok(())
    }
}

/// The start of a message found in one captured USB transfer, possibly cut by the capture snap length.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedStart {
    /// Header.
    pub header: Header,
    /// Payload bytes present in the transfer (a prefix of the real payload).
    pub payload: Vec<u8>,
    /// Whether `payload` is the whole payload announced by the header.
    pub complete: bool,
}

/// Look for a message start at the beginning of a captured transfer. Returns `None` for continuation transfers
/// (no valid wrapper of the expected direction or no valid header).
pub fn captured_start(dir: Direction, transfer: &[u8]) -> Option<CapturedStart> {
    let w = Wrapper::parse(transfer).ok()?;
    if w.dir != dir {
        return None;
    }
    let body = &transfer[WRAPPER_LEN..];
    let header = Header::parse(body).ok()?;
    let rest = &body[HEADER_LEN..];
    let want = header.payload_len as usize;
    // The frame may end before the payload does (multi-frame message) or the capture may have cut the transfer.
    let in_frame = w.body_len().saturating_sub(HEADER_LEN);
    let take = want.min(rest.len()).min(in_frame);
    Some(CapturedStart {
        header,
        payload: rest[..take].to_vec(),
        complete: take == want,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tlv::{push_i32, tag};

    fn sample_message(seq: u32, n_items: usize) -> Message {
        let mut items = Vec::new();
        for i in 0..n_items {
            push_i32(&mut items, i as i32);
        }
        Message::new(Kind::Request, seq, 0, 0x4c4, &items)
    }

    #[test]
    fn wrapper_semantics_match_the_captures() {
        // OUT 79-byte transfer: len = 0x47; IN 1242-byte transfer: len = 0x4da
        let out = Wrapper::parse(&[0, 0, 0, 0, 0x47, 0, 0, 0]).unwrap();
        assert_eq!(
            (out.dir, out.body_len(), out.frame_len()),
            (Direction::Out, 71, 79)
        );
        let inn = Wrapper::parse(&[1, 0, 0, 0, 0xda, 0x04, 0, 0]).unwrap();
        assert_eq!(
            (inn.dir, inn.body_len(), inn.frame_len()),
            (Direction::In, 1234, 1242)
        );
        assert_eq!(inn.encode(), [1, 0, 0, 0, 0xda, 4, 0, 0]);
        assert_eq!(Wrapper::for_body(Direction::In, 1234), inn);
        assert_eq!(Wrapper::for_body(Direction::Out, 71), out);
    }

    #[test]
    fn wrapper_rejects_garbage() {
        assert!(matches!(
            Wrapper::parse(&[2, 0, 0, 0, 8, 0, 0, 0]),
            Err(FrameError::BadDirection(2))
        ));
        assert!(matches!(
            Wrapper::parse(&[1, 0, 0, 0, 7, 0, 0, 0]),
            Err(FrameError::BadLength(7))
        ));
        assert!(matches!(
            Wrapper::parse(&[0; 7]),
            Err(FrameError::Short { .. })
        ));
    }

    #[test]
    fn real_request_header_roundtrip() {
        // seq 20, opcode 0x4c4 from the cold-start capture: 0x4c4 (stream 0x0505, empty option array)
        let wire = hex(
            "000000002c000000000000510000001400000000000004c400000000000000140000020000000400000505170000000400000000",
        );
        let w = Wrapper::parse(&wire).unwrap();
        assert_eq!(w.frame_len(), wire.len());
        let h = Header::parse(&wire[WRAPPER_LEN..]).unwrap();
        assert_eq!(
            h,
            Header {
                kind: Kind::Request,
                seq: 20,
                status: 0,
                id: 0x4c4,
                aux: 0,
                payload_len: 20
            }
        );
        assert_eq!(h.encode(), wire[WRAPPER_LEN..WRAPPER_LEN + HEADER_LEN]);
        let mut dec = StreamDecoder::new(Direction::Out);
        let msgs = dec.push(&wire);
        assert_eq!(msgs.len(), 1);
        let m = msgs[0].as_ref().unwrap();
        let items: Vec<_> = m.items().map(|i| i.unwrap()).collect();
        assert_eq!(items.len(), 2);
        assert_eq!((items[0].tag, items[1].tag), (tag::INT32, tag::ARRAY));
    }

    fn hex(s: &str) -> Vec<u8> {
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn multi_frame_message_is_reassembled() {
        let m = sample_message(9, 40);
        for dir in [Direction::Out, Direction::In] {
            let wire: Vec<u8> = m.to_frames(dir, 17).concat(); // header split over two frames
            let mut dec = StreamDecoder::new(dir);
            let got: Vec<_> = dec
                .push(&wire)
                .into_iter()
                .collect::<Result<_, _>>()
                .unwrap();
            assert_eq!(got, vec![m.clone()]);
            assert_eq!(dec.buffered(), 0);
        }
    }

    #[test]
    fn byte_by_byte_feed_gives_the_same_messages() {
        let msgs: Vec<Message> = (0..5).map(|i| sample_message(i, i as usize * 3)).collect();
        let wire: Vec<u8> = msgs
            .iter()
            .flat_map(|m| m.to_frames(Direction::In, 30).concat())
            .collect();
        let mut dec = StreamDecoder::new(Direction::In);
        let mut got = Vec::new();
        for b in &wire {
            for r in dec.push(std::slice::from_ref(b)) {
                got.push(r.unwrap());
            }
        }
        assert_eq!(got, msgs);
    }

    #[test]
    fn two_messages_in_one_frame_are_split() {
        let a = sample_message(1, 2);
        let b = sample_message(2, 3);
        let mut body = a.body();
        body.extend_from_slice(&b.body());
        let mut wire = Wrapper::for_body(Direction::In, body.len())
            .encode()
            .to_vec();
        wire.extend_from_slice(&body);
        let got: Vec<_> = StreamDecoder::new(Direction::In)
            .push(&wire)
            .into_iter()
            .collect::<Result<_, _>>()
            .unwrap();
        assert_eq!(got, vec![a, b]);
    }

    #[test]
    fn errors_reset_and_decoding_continues() {
        let good = sample_message(3, 1);
        let mut dec = StreamDecoder::new(Direction::In);
        // wrong direction wrapper
        let r = dec.push(&[0, 0, 0, 0, 8, 0, 0, 0]);
        assert_eq!(r, vec![Err(FrameError::WrongDirection(Direction::In))]);
        // bad magic
        let mut bad = Wrapper::for_body(Direction::In, 24).encode().to_vec();
        bad.extend_from_slice(&[0u8; 24]);
        let r = dec.push(&bad);
        assert_eq!(r, vec![Err(FrameError::BadMagic(0))]);
        // header larger than the limit
        let mut h = good.header;
        h.payload_len = (MAX_PAYLOAD_LEN + 1) as u32;
        let mut big = Wrapper::for_body(Direction::In, 24).encode().to_vec();
        big.extend_from_slice(&h.encode());
        let r = dec.push(&big);
        assert_eq!(r, vec![Err(FrameError::TooLarge(h.payload_len))]);
        // and a good one afterwards
        let r = dec.push(&good.to_frames(Direction::In, 1000).concat());
        assert_eq!(r, vec![Ok(good)]);
    }

    #[test]
    fn captured_start_handles_truncation_and_continuations() {
        let m = sample_message(4, 200); // payload 1802 bytes
        let wire = m.to_frames(Direction::In, 100_000).concat();
        let cut = &wire[..485];
        let s = captured_start(Direction::In, cut).unwrap();
        assert_eq!(s.header, m.header);
        assert!(!s.complete);
        assert_eq!(s.payload, m.payload[..485 - WRAPPER_LEN - HEADER_LEN]);
        let full = captured_start(Direction::In, &wire).unwrap();
        assert!(full.complete);
        assert_eq!(full.payload, m.payload);
        // multi-frame: the first frame holds the header and 11 payload bytes only
        let frames = m.to_frames(Direction::In, HEADER_LEN + 11);
        let first = captured_start(Direction::In, &frames[0]).unwrap();
        assert!(!first.complete);
        assert_eq!(first.payload.len(), 11);
        // a continuation frame does not start with a header
        assert!(captured_start(Direction::In, &frames[1]).is_none());
        assert!(captured_start(Direction::Out, &wire).is_none());
    }
}
