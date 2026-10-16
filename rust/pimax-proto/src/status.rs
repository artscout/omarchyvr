//! Unsolicited controller heartbeat (cmd 0x37) on IF0, about every 2 s per slot while a controller is awake but not attached.
//! `4c 0f 37 SS 48 <u32> <batt%> 00 02 01 00 08 <fw>`; one 64-byte packet may carry two messages (second at byte 16).
use crate::{frame::is_slot, Report};

/// A parsed 0x37 message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerStatus {
    /// 2 or 3.
    pub slot: u8,
    /// Battery percent.
    pub battery: u8,
    /// Firmware bytes (offset 14, 15 of the message).
    pub fw: [u8; 2],
    /// The packet it came from.
    pub raw: Report,
}

/// Parse up to two 0x37 messages from one IF0 packet.
pub fn parse_status(r: &Report) -> [Option<ControllerStatus>; 2] {
    let mut out = [None, None];
    if r[0] != 0x4c {
        return out;
    }
    for (i, off) in [0usize, 16].into_iter().enumerate() {
        if r[off + 2] == 0x37 && is_slot(r[off + 3]) {
            out[i] = Some(ControllerStatus {
                slot: r[off + 3],
                battery: r[off + 9],
                fw: [r[off + 14], r[off + 15]],
                raw: *r,
            });
        }
    }
    out
}
