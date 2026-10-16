//! Wire protocol of the Pimax Dream Air (USB 34a4:0044) over HID reports.
//!
//! Pure functions on 64-byte reports: no I/O, no allocation in the hot decoders, `#![forbid(unsafe_code)]`.
//! Protocol references: `analysis/init-sequence.md`, `analysis/controllers-linux.md`.
//!
//! Host to device: `58 LL CMD OPT DATA..` (LL = data length + 3), zero padded to 64 bytes.
//! Device to host on IF0: `4c LL CMD OPT DATA.. CK`; IF2 carries IMU reports `53 31 08 00 ..`,
//! IF1 (and sometimes others) controller reports `53 32 08 SS 43 ..`, IF3 an ASCII firmware log.
#![no_std]
#![forbid(unsafe_code)]

#[cfg(feature = "alloc")]
extern crate alloc;
#[cfg(feature = "std")]
extern crate std;

pub mod calib;
pub mod controller;
pub mod frame;
pub mod imu;
pub mod log;
pub mod status;

pub use calib::{CalibAssembler, CalibError};
pub use controller::{decode_controller, ControllerReport, TouchBits};
pub use frame::{
    attach_commands, build_command, haptic_command, haptic_from_openxr, keepalive, parse_reply,
    parse_serial, sys_reset, to_write_buf, unpair, video_prepare_commands, wake, FrameError, Reply,
};
pub use imu::{decode_imu, ImuSample};
pub use log::LineSplitter;
pub use status::{parse_status, ControllerStatus};

/// USB vendor id of the headset.
pub const VID: u16 = 0x34a4;
/// USB product id of the headset (IF0..IF3).
pub const PID: u16 = 0x0044;
/// Product id of the display bridge MCU (ASCII log on EP 0x81).
pub const PID_BRIDGE: u16 = 0x0043;
/// Number of USB interfaces (0..3).
pub const NUM_IF: usize = 4;
/// Size of every HID report.
pub const REPORT_SIZE: usize = 64;
/// Payload bytes of one command (64 - 4 header).
pub const MAX_DATA: usize = 60;
/// Keepalive period of the host, in milliseconds.
pub const KEEPALIVE_MS: u64 = 213;
/// Controller slot.
pub const SLOT_LEFT: u8 = 2;
/// Controller slot.
pub const SLOT_RIGHT: u8 = 3;

/// One 64-byte HID report.
pub type Report = [u8; REPORT_SIZE];

/// Controller button bit (`ControllerReport::buttons`).
pub const BTN_B: u32 = 0x01;
/// Controller button bit.
pub const BTN_A: u32 = 0x02;
/// Controller button bit.
pub const BTN_STICK: u32 = 0x08;
/// Controller button bit (byte 48 bit 0, remapped).
pub const BTN_MENU: u32 = 0x10;
/// Raw touch bit for the thumbstick (byte 49).
pub const TOUCH_STICK: u8 = 0x08;

/// Command ids used by the host.
pub mod cmd {
    /// Version read.
    pub const VERSION: u8 = 0x03;
    /// Controller vibration `58 09 07 SS AMP DUR_lo DUR_hi 00 00 00` (no reply; Windows capture 2026-10-15).
    pub const HAPTIC: u8 = 0x07;
    /// Calibration document read.
    pub const READ_DOC: u8 = 0x0a;
    /// Serial read.
    pub const SERIAL: u8 = 0x16;
    /// Wake / announce rate.
    pub const ANNOUNCE: u8 = 0x1a;
    /// Controller unpair `58 03 05 SS` (reply `4c 04 05 SS 01 07`; Windows capture 2026-10-15 redo take 3).
    pub const UNPAIR: u8 = 0x05;
    /// Headset restart, eSpiCmdSysReset `58 04 28 00 00` (reply `4c 04 28 00 01 07`; Windows capture take 2).
    pub const SYS_RESET: u8 = 0x28;
    /// Panel on request.
    pub const PANEL_ON: u8 = 0x29;
    /// Controller status heartbeat (device to host).
    pub const STATUS: u8 = 0x37;
    /// Keepalive.
    pub const KEEPALIVE: u8 = 0x39;
    /// Headset-local info (also part of the controller attach burst).
    pub const INFO: u8 = 0x50;
    /// Application unlock.
    pub const APP_UNLOCK: u8 = 0x5b;
}

pub(crate) fn be16(p: &[u8]) -> i16 {
    i16::from_be_bytes([p[0], p[1]])
}

pub(crate) fn le32(p: &[u8]) -> u32 {
    u32::from_le_bytes([p[0], p[1], p[2], p[3]])
}
