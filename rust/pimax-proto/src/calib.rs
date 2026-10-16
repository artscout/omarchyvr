//! On-device calibration document: 84 reads (`58 07 0a 00 01 AL AH 10`, addresses 0x0000..0x0530 step 0x10),
//! each answered by `4c 17 0a 00 01 AL AH 10 <16 B> CK`. The blob is `u32 LE zlib_len | 12 B | zlib stream | padding`.
use crate::{cmd, frame::build_command, Report};
#[cfg(feature = "alloc")]
use alloc::vec::Vec;

/// Bytes per read.
pub const BLOCK: usize = 16;
/// Total bytes read (84 blocks).
pub const TOTAL: usize = 0x540;
/// Number of blocks.
pub const BLOCKS: usize = TOTAL / BLOCK;
/// Offset of the zlib stream in the blob.
pub const HEADER: usize = 16;

/// Calibration errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalibError {
    /// Blob shorter than its header says.
    Truncated,
    /// zlib stream is invalid.
    Inflate,
    /// Not all blocks were received.
    Incomplete,
}

impl core::fmt::Display for CalibError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(match self {
            CalibError::Truncated => "calibration blob truncated",
            CalibError::Inflate => "calibration zlib stream invalid",
            CalibError::Incomplete => "calibration blocks missing",
        })
    }
}

/// Request for the block at `addr`.
pub fn read_block_request(addr: u16) -> Report {
    build_command(
        cmd::READ_DOC,
        0x00,
        &[0x01, addr as u8, (addr >> 8) as u8, 0x10],
    )
    .unwrap_or([0; 64])
}

/// If `r` is a block reply, return `(address, 16 data bytes)`.
pub fn parse_block_reply(r: &Report) -> Option<(u16, [u8; BLOCK])> {
    if r[0] != 0x4c || r[2] != cmd::READ_DOC || r[4] != 0x01 {
        return None;
    }
    let addr = u16::from(r[5]) | (u16::from(r[6]) << 8);
    let mut d = [0u8; BLOCK];
    d.copy_from_slice(&r[8..8 + BLOCK]);
    Some((addr, d))
}

/// Collects block replies in any order (and duplicates) into the blob.
#[derive(Clone)]
pub struct CalibAssembler {
    buf: [u8; TOTAL],
    got: [bool; BLOCKS],
}

impl Default for CalibAssembler {
    fn default() -> Self {
        Self::new()
    }
}

impl CalibAssembler {
    /// Empty assembler.
    pub const fn new() -> Self {
        Self {
            buf: [0; TOTAL],
            got: [false; BLOCKS],
        }
    }

    /// Feed any IF0 report; returns true if it was a (valid-address) block.
    pub fn feed(&mut self, r: &Report) -> bool {
        match parse_block_reply(r) {
            Some((addr, d)) if (addr as usize) < TOTAL && (addr as usize).is_multiple_of(BLOCK) => {
                let i = addr as usize / BLOCK;
                self.buf[i * BLOCK..(i + 1) * BLOCK].copy_from_slice(&d);
                self.got[i] = true;
                true
            }
            _ => false,
        }
    }

    /// True when all 84 blocks arrived.
    pub fn complete(&self) -> bool {
        self.got.iter().all(|&g| g)
    }

    /// First block address not yet received.
    pub fn next_missing(&self) -> Option<u16> {
        self.got
            .iter()
            .position(|&g| !g)
            .map(|i| (i * BLOCK) as u16)
    }

    /// The assembled blob (only meaningful when complete).
    pub fn blob(&self) -> &[u8; TOTAL] {
        &self.buf
    }

    /// Inflate the document (the XML).
    #[cfg(feature = "alloc")]
    pub fn document(&self) -> Result<Vec<u8>, CalibError> {
        if !self.complete() {
            return Err(CalibError::Incomplete);
        }
        inflate_blob(&self.buf)
    }
}

/// `blob[16 .. 16 + u32le(blob[0..4])]` inflated as zlib.
#[cfg(feature = "alloc")]
pub fn inflate_blob(blob: &[u8]) -> Result<Vec<u8>, CalibError> {
    if blob.len() < HEADER {
        return Err(CalibError::Truncated);
    }
    let n = u32::from_le_bytes([blob[0], blob[1], blob[2], blob[3]]) as usize;
    let end = HEADER.checked_add(n).ok_or(CalibError::Truncated)?;
    if end > blob.len() {
        return Err(CalibError::Truncated);
    }
    miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&blob[HEADER..end], 16 << 20)
        .map_err(|_| CalibError::Inflate)
}

/// Build a device-style blob from an XML document (for tests and tools).
#[cfg(feature = "alloc")]
pub fn build_blob(xml: &[u8]) -> Vec<u8> {
    let z = miniz_oxide::deflate::compress_to_vec_zlib(xml, 6);
    let mut b = Vec::with_capacity(TOTAL.max(HEADER + z.len()));
    b.extend_from_slice(&(z.len() as u32).to_le_bytes());
    b.extend_from_slice(&[0u8; HEADER - 4]);
    b.extend_from_slice(&z);
    if b.len() < TOTAL {
        b.resize(TOTAL, 0);
    }
    b
}

/// Build the 4c reply the device would send for block `addr` of `blob` (for fakes and tests).
pub fn make_block_reply(blob: &[u8], addr: u16) -> Report {
    let mut r = [0u8; 64];
    r[0] = 0x4c;
    r[1] = 0x17;
    r[2] = cmd::READ_DOC;
    r[4] = 0x01;
    r[5] = addr as u8;
    r[6] = (addr >> 8) as u8;
    r[7] = 0x10;
    let a = addr as usize;
    for i in 0..BLOCK {
        r[8 + i] = blob.get(a + i).copied().unwrap_or(0);
    }
    r
}
