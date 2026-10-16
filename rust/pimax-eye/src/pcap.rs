//! Reader and writer for USBPcap captures (classic pcap, link type 249), as recorded by `capture.ps1` on Windows.
//!
//! USBPcap packet header (little endian, 27 bytes, 28 for control transfers which add the stage byte):
//! `u16 header_len | u64 irp_id | u32 status | u16 function | u8 info | u16 bus | u16 device | u8 endpoint | u8 transfer | u32 data_len [| u8 stage]`.
//! `info` bit 0 set = completion (device to host direction of the IRP). `data_len` is the real transfer length;
//! the captured bytes after the header may be shorter (snap length).

use std::fmt;
use std::io::{self, Read, Write};

/// pcap link type of USBPcap.
pub const LINKTYPE_USBPCAP: u32 = 249;
/// Size of the fixed USBPcap header part.
pub const USBPCAP_HEADER_LEN: usize = 27;

const MAGIC_US: [u8; 4] = [0xd4, 0xc3, 0xb2, 0xa1];
const MAGIC_NS: [u8; 4] = [0x4d, 0x3c, 0xb2, 0xa1];
const MAX_RECORD: usize = 1 << 26;

/// Pcap reading errors.
#[derive(Debug)]
pub enum PcapError {
    /// I/O error.
    Io(io::Error),
    /// Not a little-endian classic pcap file.
    BadMagic([u8; 4]),
    /// Link type other than USBPcap.
    BadLinkType(u32),
    /// The file ends inside a record.
    Truncated,
    /// A record is malformed.
    BadRecord {
        /// 1-based record number.
        index: u64,
        /// What is wrong.
        reason: &'static str,
    },
}

impl fmt::Display for PcapError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PcapError::Io(e) => write!(f, "I/O error: {e}"),
            PcapError::BadMagic(m) => write!(f, "not a little-endian pcap file (magic {m:02x?})"),
            PcapError::BadLinkType(l) => write!(f, "pcap link type {l}, expected 249 (USBPcap)"),
            PcapError::Truncated => write!(f, "file ends inside a record"),
            PcapError::BadRecord { index, reason } => write!(f, "record {index}: {reason}"),
        }
    }
}

impl std::error::Error for PcapError {}

impl From<io::Error> for PcapError {
    fn from(e: io::Error) -> PcapError {
        PcapError::Io(e)
    }
}

/// USB transfer type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Transfer {
    /// 0.
    Isochronous,
    /// 1.
    Interrupt,
    /// 2.
    Control,
    /// 3.
    Bulk,
    /// Anything else.
    Other(u8),
}

impl Transfer {
    fn from_wire(b: u8) -> Transfer {
        match b {
            0 => Transfer::Isochronous,
            1 => Transfer::Interrupt,
            2 => Transfer::Control,
            3 => Transfer::Bulk,
            o => Transfer::Other(o),
        }
    }

    fn wire(self) -> u8 {
        match self {
            Transfer::Isochronous => 0,
            Transfer::Interrupt => 1,
            Transfer::Control => 2,
            Transfer::Bulk => 3,
            Transfer::Other(o) => o,
        }
    }
}

/// One captured USB packet (an IRP submission or completion).
#[derive(Debug, Clone, PartialEq)]
pub struct UsbPacket {
    /// 1-based record number in the file.
    pub index: u64,
    /// Seconds since the first packet of the file.
    pub time: f64,
    /// pcap record timestamp, seconds part.
    pub ts_sec: u32,
    /// pcap record timestamp, microsecond or nanosecond part.
    pub ts_frac: u32,
    /// pcap `orig_len`.
    pub orig_len: u32,
    /// IRP id.
    pub irp_id: u64,
    /// USBD status of the completion.
    pub status: u32,
    /// URB function.
    pub function: u16,
    /// Info byte; bit 0 = completion.
    pub info: u8,
    /// Bus number.
    pub bus: u16,
    /// Device address on that bus (re-assigned on every enumeration).
    pub device: u16,
    /// Endpoint address including the direction bit (0x80 = IN).
    pub endpoint: u8,
    /// Transfer type.
    pub transfer: Transfer,
    /// Real length of the transfer data (may exceed the captured bytes).
    pub data_len: u32,
    /// Control stage (0 setup, 1 data, 2 status, 3 complete); control transfers only.
    pub stage: Option<u8>,
    header_len: usize,
    raw: Vec<u8>,
}

impl UsbPacket {
    /// Bytes after the USBPcap header, as captured.
    pub fn data(&self) -> &[u8] {
        &self.raw[self.header_len..]
    }

    /// The whole captured packet, header included (what a pcap record contains).
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Completion (device to host direction of the IRP).
    pub fn is_completion(&self) -> bool {
        self.info & 1 != 0
    }

    /// Endpoint direction is IN.
    pub fn is_in(&self) -> bool {
        self.endpoint & 0x80 != 0
    }

    /// The capture holds fewer data bytes than the transfer length.
    pub fn is_truncated(&self) -> bool {
        (self.data().len() as u64) < u64::from(self.data_len)
    }

    /// Parse the USBPcap header of a record body.
    pub fn parse(
        raw: Vec<u8>,
        index: u64,
        ts_sec: u32,
        ts_frac: u32,
        orig_len: u32,
        time: f64,
    ) -> Result<UsbPacket, PcapError> {
        let bad = |reason| PcapError::BadRecord { index, reason };
        if raw.len() < USBPCAP_HEADER_LEN {
            return Err(bad("shorter than the USBPcap header"));
        }
        let le16 = |i: usize| u16::from_le_bytes([raw[i], raw[i + 1]]);
        let le32 = |i: usize| u32::from_le_bytes([raw[i], raw[i + 1], raw[i + 2], raw[i + 3]]);
        // Isochronous packets carry one descriptor per frame in the header, which a small snap length cuts: such a
        // record has no data bytes at all.
        let header_len = usize::from(le16(0));
        if header_len < USBPCAP_HEADER_LEN {
            return Err(bad("header length out of range"));
        }
        let header_len = header_len.min(raw.len());
        let transfer = Transfer::from_wire(raw[22]);
        let stage = (transfer == Transfer::Control && header_len > USBPCAP_HEADER_LEN)
            .then(|| raw[USBPCAP_HEADER_LEN]);
        let mut id = [0u8; 8];
        id.copy_from_slice(&raw[2..10]);
        Ok(UsbPacket {
            index,
            time,
            ts_sec,
            ts_frac,
            orig_len,
            irp_id: u64::from_le_bytes(id),
            status: le32(10),
            function: le16(14),
            info: raw[16],
            bus: le16(17),
            device: le16(19),
            endpoint: raw[21],
            transfer,
            data_len: le32(23),
            stage,
            header_len,
            raw,
        })
    }

    /// Build a packet from its parts (tests, fake devices, fixture slicing).
    #[allow(clippy::too_many_arguments)]
    pub fn build(
        time: f64,
        bus: u16,
        device: u16,
        endpoint: u8,
        transfer: Transfer,
        completion: bool,
        stage: Option<u8>,
        data: &[u8],
        data_len: u32,
    ) -> UsbPacket {
        let header_len = if stage.is_some() {
            USBPCAP_HEADER_LEN + 1
        } else {
            USBPCAP_HEADER_LEN
        };
        let mut raw = Vec::with_capacity(header_len + data.len());
        raw.extend_from_slice(&(header_len as u16).to_le_bytes());
        raw.extend_from_slice(&0u64.to_le_bytes());
        raw.extend_from_slice(&0u32.to_le_bytes());
        raw.extend_from_slice(&0u16.to_le_bytes());
        raw.push(u8::from(completion));
        raw.extend_from_slice(&bus.to_le_bytes());
        raw.extend_from_slice(&device.to_le_bytes());
        raw.push(endpoint);
        raw.push(transfer.wire());
        raw.extend_from_slice(&data_len.to_le_bytes());
        if let Some(s) = stage {
            raw.push(s);
        }
        raw.extend_from_slice(data);
        let ts_sec = time.trunc() as u32;
        let ts_frac = ((time - time.trunc()) * 1e6).round() as u32;
        let orig_len = raw.len() as u32;
        UsbPacket {
            index: 0,
            time,
            ts_sec,
            ts_frac,
            orig_len,
            irp_id: 0,
            status: 0,
            function: 0,
            info: u8::from(completion),
            bus,
            device,
            endpoint,
            transfer,
            data_len,
            stage,
            header_len,
            raw,
        }
    }
}

/// Streaming reader over a USBPcap file.
pub struct PcapReader<R: Read> {
    r: R,
    nanos: bool,
    snaplen: u32,
    index: u64,
    t0: Option<f64>,
}

impl<R: Read> PcapReader<R> {
    /// Read and check the global header.
    pub fn new(mut r: R) -> Result<Self, PcapError> {
        let mut h = [0u8; 24];
        r.read_exact(&mut h)?;
        let magic = [h[0], h[1], h[2], h[3]];
        let nanos = match magic {
            MAGIC_US => false,
            MAGIC_NS => true,
            m => return Err(PcapError::BadMagic(m)),
        };
        let snaplen = u32::from_le_bytes([h[16], h[17], h[18], h[19]]);
        let link = u32::from_le_bytes([h[20], h[21], h[22], h[23]]);
        if link != LINKTYPE_USBPCAP {
            return Err(PcapError::BadLinkType(link));
        }
        Ok(PcapReader {
            r,
            nanos,
            snaplen,
            index: 0,
            t0: None,
        })
    }

    /// Snap length of the capture (512 for most of ours, 65535 for `full-packets`).
    pub fn snaplen(&self) -> u32 {
        self.snaplen
    }

    /// Next packet, `Ok(None)` at a clean end of file.
    pub fn next_packet(&mut self) -> Result<Option<UsbPacket>, PcapError> {
        let mut rh = [0u8; 16];
        let mut got = 0;
        while got < rh.len() {
            let n = self.r.read(&mut rh[got..])?;
            if n == 0 {
                return if got == 0 {
                    Ok(None)
                } else {
                    Err(PcapError::Truncated)
                };
            }
            got += n;
        }
        let ts_sec = u32::from_le_bytes([rh[0], rh[1], rh[2], rh[3]]);
        let ts_frac = u32::from_le_bytes([rh[4], rh[5], rh[6], rh[7]]);
        let incl = u32::from_le_bytes([rh[8], rh[9], rh[10], rh[11]]) as usize;
        let orig = u32::from_le_bytes([rh[12], rh[13], rh[14], rh[15]]);
        self.index += 1;
        if incl > MAX_RECORD {
            return Err(PcapError::BadRecord {
                index: self.index,
                reason: "record length out of range",
            });
        }
        let mut body = vec![0u8; incl];
        self.r.read_exact(&mut body).map_err(|e| {
            if e.kind() == io::ErrorKind::UnexpectedEof {
                PcapError::Truncated
            } else {
                PcapError::Io(e)
            }
        })?;
        let div = if self.nanos { 1e9 } else { 1e6 };
        let abs = f64::from(ts_sec) + f64::from(ts_frac) / div;
        let t0 = *self.t0.get_or_insert(abs);
        UsbPacket::parse(body, self.index, ts_sec, ts_frac, orig, abs - t0).map(Some)
    }
}

impl<R: Read> Iterator for PcapReader<R> {
    type Item = Result<UsbPacket, PcapError>;

    fn next(&mut self) -> Option<Self::Item> {
        match self.next_packet() {
            Ok(Some(p)) => Some(Ok(p)),
            Ok(None) => None,
            Err(e) => Some(Err(e)),
        }
    }
}

/// Writer producing the same record layout (microsecond timestamps).
pub struct PcapWriter<W: Write> {
    w: W,
}

impl<W: Write> PcapWriter<W> {
    /// Write the global header.
    pub fn new(mut w: W, snaplen: u32) -> io::Result<Self> {
        let mut h = Vec::with_capacity(24);
        h.extend_from_slice(&MAGIC_US);
        h.extend_from_slice(&2u16.to_le_bytes());
        h.extend_from_slice(&4u16.to_le_bytes());
        h.extend_from_slice(&0i32.to_le_bytes());
        h.extend_from_slice(&0u32.to_le_bytes());
        h.extend_from_slice(&snaplen.to_le_bytes());
        h.extend_from_slice(&LINKTYPE_USBPCAP.to_le_bytes());
        w.write_all(&h)?;
        Ok(PcapWriter { w })
    }

    /// Append a packet with its original timestamps (byte-exact for microsecond captures).
    pub fn write_packet(&mut self, p: &UsbPacket) -> io::Result<()> {
        let mut h = Vec::with_capacity(16);
        h.extend_from_slice(&p.ts_sec.to_le_bytes());
        h.extend_from_slice(&p.ts_frac.to_le_bytes());
        h.extend_from_slice(&(p.raw.len() as u32).to_le_bytes());
        h.extend_from_slice(&p.orig_len.to_le_bytes());
        self.w.write_all(&h)?;
        self.w.write_all(&p.raw)
    }

    /// Recover the inner writer.
    pub fn into_inner(self) -> W {
        self.w
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pk(
        t: f64,
        dev: u16,
        ep: u8,
        tr: Transfer,
        comp: bool,
        stage: Option<u8>,
        data: &[u8],
    ) -> UsbPacket {
        UsbPacket::build(t, 2, dev, ep, tr, comp, stage, data, data.len() as u32)
    }

    #[test]
    fn write_then_read_roundtrip() {
        let pkts = vec![
            pk(1000.5, 30, 0x05, Transfer::Bulk, false, None, &[1, 2, 3]),
            pk(1000.75, 30, 0x83, Transfer::Bulk, true, None, &[9; 40]),
            pk(
                1001.25,
                30,
                0x80,
                Transfer::Control,
                false,
                Some(0),
                &[0xc1, 0x46, 1, 0, 0, 0, 8, 0],
            ),
        ];
        let mut w = PcapWriter::new(Vec::new(), 512).unwrap();
        for p in &pkts {
            w.write_packet(p).unwrap();
        }
        let bytes = w.into_inner();
        let mut r = PcapReader::new(&bytes[..]).unwrap();
        assert_eq!(r.snaplen(), 512);
        let got: Vec<_> = r.by_ref().collect::<Result<_, _>>().unwrap();
        assert_eq!(got.len(), 3);
        for (a, b) in pkts.iter().zip(&got) {
            assert_eq!(a.raw(), b.raw());
            assert_eq!(a.data(), b.data());
            assert_eq!(
                (a.device, a.endpoint, a.transfer, a.stage),
                (b.device, b.endpoint, b.transfer, b.stage)
            );
        }
        assert!((got[1].time - 0.25).abs() < 1e-6 && got[0].time == 0.0);
        assert_eq!(got[2].stage, Some(0));
        assert!(got[1].is_completion() && got[1].is_in() && !got[0].is_in());
    }

    #[test]
    fn truncation_is_visible() {
        let p = UsbPacket::build(
            0.0,
            2,
            30,
            0x05,
            Transfer::Bulk,
            false,
            None,
            &[0; 485],
            2803,
        );
        assert!(p.is_truncated());
        let q = pk(0.0, 30, 0x05, Transfer::Bulk, false, None, &[0; 20]);
        assert!(!q.is_truncated());
    }

    #[test]
    fn rejects_foreign_files() {
        assert!(matches!(
            PcapReader::new(&[0u8; 24][..]).err().unwrap(),
            PcapError::BadMagic(_)
        ));
        let mut h = Vec::new();
        h.extend_from_slice(&MAGIC_US);
        h.extend_from_slice(&[0; 12]);
        h.extend_from_slice(&65535u32.to_le_bytes());
        h.extend_from_slice(&1u32.to_le_bytes());
        assert!(matches!(
            PcapReader::new(&h[..]).err().unwrap(),
            PcapError::BadLinkType(1)
        ));
        assert!(PcapReader::new(&h[..10]).is_err());
    }

    #[test]
    fn truncated_file_and_bad_records() {
        let mut w = PcapWriter::new(Vec::new(), 512).unwrap();
        w.write_packet(&pk(0.0, 30, 0x83, Transfer::Bulk, true, None, &[7; 100]))
            .unwrap();
        let bytes = w.into_inner();
        for cut in [bytes.len() - 1, bytes.len() - 100, 24 + 5] {
            let mut r = PcapReader::new(&bytes[..cut]).unwrap();
            assert!(
                matches!(r.next_packet(), Err(PcapError::Truncated)),
                "cut {cut}"
            );
        }
        // record shorter than a USBPcap header
        let mut h = bytes[..24].to_vec();
        h.extend_from_slice(&[0; 8]);
        h.extend_from_slice(&4u32.to_le_bytes());
        h.extend_from_slice(&4u32.to_le_bytes());
        h.extend_from_slice(&[0; 4]);
        let mut r = PcapReader::new(&h[..]).unwrap();
        assert!(matches!(
            r.next_packet(),
            Err(PcapError::BadRecord { index: 1, .. })
        ));
    }

    #[test]
    fn isochronous_record_with_a_cut_header_is_accepted() {
        // header_len says 27 + 12 + 12*10 but only 40 bytes were captured
        let mut raw = vec![0u8; 40];
        raw[..2].copy_from_slice(&159u16.to_le_bytes());
        raw[16] = 1;
        raw[22] = 0; // isochronous
        raw[23..27].copy_from_slice(&1_600_000u32.to_le_bytes());
        let p = UsbPacket::parse(raw, 1, 0, 0, 1_600_000, 0.0).unwrap();
        assert_eq!(p.transfer, Transfer::Isochronous);
        assert!(p.data().is_empty());
        assert!(p.is_truncated());
        // but a header shorter than the fixed part is still an error
        let mut raw = vec![0u8; 40];
        raw[..2].copy_from_slice(&20u16.to_le_bytes());
        assert!(UsbPacket::parse(raw, 2, 0, 0, 40, 0.0).is_err());
    }

    #[test]
    fn arbitrary_garbage_never_panics() {
        let mut x = 0x1234_5678u32;
        for len in 0..300usize {
            let mut b = Vec::with_capacity(24 + len);
            b.extend_from_slice(&MAGIC_US);
            b.extend_from_slice(&[0; 12]);
            b.extend_from_slice(&512u32.to_le_bytes());
            b.extend_from_slice(&LINKTYPE_USBPCAP.to_le_bytes());
            for _ in 0..len {
                x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                b.push((x >> 24) as u8);
            }
            if let Ok(r) = PcapReader::new(&b[..]) {
                for p in r {
                    let _ = p;
                }
            }
        }
    }
}
