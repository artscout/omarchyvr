//! Command framing (`58 LL CMD OPT ..`) and reply parsing (`4c LL CMD OPT .. CK`).
use crate::{cmd, Report, MAX_DATA, REPORT_SIZE, SLOT_LEFT, SLOT_RIGHT};

/// Framing errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    /// Payload longer than [`MAX_DATA`].
    TooLong,
}

impl core::fmt::Display for FrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            FrameError::TooLong => write!(f, "command payload longer than {MAX_DATA} bytes"),
        }
    }
}

/// Build `58 LL cmd opt data`, `LL = len + 3`, zero padded to 64 bytes.
pub fn build_command(cmd: u8, opt: u8, data: &[u8]) -> Result<Report, FrameError> {
    if data.len() > MAX_DATA {
        return Err(FrameError::TooLong);
    }
    let mut r = [0u8; REPORT_SIZE];
    r[0] = 0x58;
    r[1] = (data.len() + 3) as u8;
    r[2] = cmd;
    r[3] = opt;
    r[4..4 + data.len()].copy_from_slice(data);
    Ok(r)
}

/// Prefix the unnumbered report id 0, as required by a hidraw `write()`.
pub fn to_write_buf(r: &Report) -> [u8; REPORT_SIZE + 1] {
    let mut b = [0u8; REPORT_SIZE + 1];
    b[1..].copy_from_slice(r);
    b
}

fn fixed(cmd: u8, opt: u8, data: &[u8]) -> Report {
    // all callers pass short constant payloads
    build_command(cmd, opt, data).unwrap_or([0; REPORT_SIZE])
}

/// Wake / "PC alive": `58 06 1a 05 01 00 00`.
pub fn wake() -> Report {
    fixed(cmd::ANNOUNCE, 0x05, &[0x01, 0x00, 0x00])
}

/// Headset restart (eSpiCmdSysReset): `58 04 28 00 00`. The firmware answers `4c 04 28 00 01 07`, drops off USB about
/// 1 s later and is back (HID) after about 8 s, panel after about 22 s (Windows capture 2026-10-15 take 2).
pub fn sys_reset() -> Report {
    fixed(cmd::SYS_RESET, 0x00, &[0x00])
}

/// Controller unpair: `58 03 05 SS` (SS 2 left, 3 right), no data.
pub fn unpair(slot: u8) -> Report {
    fixed(cmd::UNPAIR, slot, &[])
}

/// Keepalive carrying the video rate, in the exact wire form Windows and `pimax_keepalived.py` use:
/// `58 04 39 00 RR 00` (LL = 4: the trailing zero is the high byte of the u16 rate, outside LL).
/// The C libpimax sends the same with LL = 5; the firmware accepts both.
pub fn keepalive(rate: u8) -> Report {
    let mut r = fixed(cmd::KEEPALIVE, 0x00, &[rate]);
    r[5] = 0x00;
    r
}

/// Video preparation sequence (`pimax_video_prepare`): `5b(02 02)`, `29(01)`, `1a(02 FPS)`, `1a(05 00)`.
pub fn video_prepare_commands(fps: u8) -> [Report; 4] {
    let mut d1a = [0u8; 10];
    d1a[0] = 0x02;
    d1a[1] = fps;
    let mut d1a_gpu = [0u8; 10];
    d1a_gpu[0] = 0x05;
    [
        fixed(cmd::APP_UNLOCK, 0x05, &[0x02, 0x02]),
        fixed(cmd::PANEL_ON, 0x00, &[0x01]),
        fixed(cmd::ANNOUNCE, 0x05, &d1a),
        fixed(cmd::ANNOUNCE, 0x05, &d1a_gpu),
    ]
}

/// Per-slot controller attach burst: `16(1e)`, `50`, `0a(00 00 00 10)`, `03`. The host spaces them by 10 ms.
pub fn attach_commands(slot: u8) -> [Report; 4] {
    [
        fixed(cmd::SERIAL, slot, &[0x1e]),
        fixed(cmd::INFO, slot, &[]),
        fixed(cmd::READ_DOC, slot, &[0, 0, 0, 0x10]),
        fixed(cmd::VERSION, slot, &[]),
    ]
}

/// Duration games use for a streamed pulse (Windows capture 2026-10-15 take 15: 11-12 ms, re-sent every 8-10 ms).
pub const HAPTIC_MIN_MS: u16 = 12;

/// Controller vibration: `58 09 07 SS AMP DUR_lo DUR_hi 00 00 00`, zero padded (LL = 9: six data bytes, the last three
/// always zero). `amp` is percent (clamped to 1..=100), `dur_ms` milliseconds (at least 1). The headset sends no reply.
/// Captured on Windows 2026-10-15: Pimax Play's test pulse `58 09 07 03 19 2c 01` (right, 25 %, 300 ms).
pub fn haptic_command(slot: u8, amp: u8, dur_ms: u16) -> Report {
    let [lo, hi] = dur_ms.max(1).to_le_bytes();
    fixed(cmd::HAPTIC, slot, &[amp.clamp(1, 100), lo, hi, 0, 0, 0])
}

/// `f32::round` (half away from zero) for `0 <= x < 2^23` without `std` (no_std has no float rounding): the
/// fraction `x - trunc(x)` is exact in f32 there, so this is bit-for-bit the same result.
pub fn round_pos(x: f32) -> u32 {
    let t = x as u32;
    if x - t as f32 >= 0.5 {
        t + 1
    } else {
        t
    }
}

/// OpenXR `XrHapticVibration` (amplitude 0..1, duration in ns, `XR_MIN_HAPTIC_DURATION` = -1) to `(amp %, ms)`.
/// `None` means "stop": amplitude 0 (or NaN). The headset never got AMP 0 on Windows, a burst just ends, so a stop
/// sends nothing. Duration <= 0 (minimum) -> [`HAPTIC_MIN_MS`]; longer ones are rounded up to whole ms, max 65535.
pub fn haptic_from_openxr(amplitude: f32, duration_ns: i64) -> Option<(u8, u16)> {
    if amplitude.is_nan() || amplitude <= 0.0 {
        return None;
    }
    let amp = (round_pos(amplitude.min(1.0) * 100.0) as u8).clamp(1, 100);
    let ms = if duration_ns <= 0 {
        HAPTIC_MIN_MS
    } else {
        ((duration_ns + 999_999) / 1_000_000).clamp(1, u16::MAX as i64) as u16
    };
    Some((amp, ms))
}

/// A parsed `4c LL cmd opt data.. CK` reply.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reply<'a> {
    /// Command id.
    pub cmd: u8,
    /// Option byte (usually the slot or 0).
    pub opt: u8,
    /// Payload without the trailing check byte (`LL - 3` bytes, clamped to the report).
    pub data: &'a [u8],
}

/// Parse a report from IF0. `None` when it lacks the `4c` magic.
pub fn parse_reply(r: &Report) -> Option<Reply<'_>> {
    if r[0] != 0x4c {
        return None;
    }
    let dlen = (r[1] as usize).saturating_sub(3).min(REPORT_SIZE - 4);
    Some(Reply {
        cmd: r[2],
        opt: r[3],
        data: &r[4..4 + dlen],
    })
}

/// Serial reply data (`<len> <ASCII serial> [tail]`) to the ASCII serial bytes.
pub fn parse_serial(data: &[u8]) -> &[u8] {
    if data.is_empty() {
        return &[];
    }
    let n = (data[0] as usize).min(data.len() - 1);
    &data[1..1 + n]
}

/// True for controller slots 2 and 3.
pub fn is_slot(s: u8) -> bool {
    s == SLOT_LEFT || s == SLOT_RIGHT
}
