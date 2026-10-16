//! Controller report (EP 0x82, usually seen on IF1): `53 32 08 SS 43 | sx sy trig | ts @8 | s0 @16 | s1 @30 | grip @44 | batt @46 | btn @47,48 | touch @49`.
use crate::{
    be16, le32, Report, BTN_A, BTN_B, BTN_MENU, BTN_STICK, REPORT_SIZE, SLOT_LEFT, SLOT_RIGHT,
};

/// Decoded controller report.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerReport {
    /// 2 = left, 3 = right.
    pub slot: u8,
    /// Device uptime clock, microseconds.
    pub ts_us: u32,
    /// -128..127, right positive.
    pub stick_x: i32,
    /// -128..127, up positive.
    pub stick_y: i32,
    /// 0..255.
    pub trigger: u8,
    /// 0..255.
    pub grip: u8,
    /// Percent: `byte46 & 0x7f`.
    pub battery: u8,
    /// `byte46` bit 7.
    pub charging: bool,
    /// `BTN_*` bits.
    pub buttons: u32,
    /// Raw byte 49.
    pub touch: u8,
    /// About 1024 LSB/g, big-endian on the wire.
    pub raw_accel: [i16; 3],
    /// Unit unknown.
    pub raw_gyro: [i16; 3],
    /// The whole report.
    pub raw: Report,
}

/// Capacitive touch flags derived from byte 49 (`analysis/controllers-linux.md`, corrected by the Windows redo
/// take 5, `analysis/windows-capture-20261015.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TouchBits {
    /// Bit 0. Not a per-button touch: bits 0 and 1 rise together for either face button (thumb near the buttons).
    pub a: bool,
    /// Bit 1 (see `a`; use [`TouchBits::thumb`] for "thumb on the face buttons").
    pub b: bool,
    /// Bit 3 (thumb on the stick).
    pub stick: bool,
    /// Bit 4: "hand on the controller" on BOTH hands (set while it is held, 0 when put down). Not a left/right rest
    /// state: the old decoder treated it as resting at 1 on the left and 0 on the right, which misfired on the right.
    pub hand_on: bool,
}

impl TouchBits {
    /// Thumb on either face button (bits 0 or 1).
    pub fn thumb(&self) -> bool {
        self.a || self.b
    }
}

impl ControllerReport {
    /// Touch flags for this report.
    pub fn touch_bits(&self) -> TouchBits {
        TouchBits {
            a: self.touch & 0x01 != 0,
            b: self.touch & 0x02 != 0,
            stick: self.touch & 0x08 != 0,
            hand_on: self.touch & 0x10 != 0,
        }
    }

    /// Names of the pressed buttons in `B, A, stick, menu` order (python `decode()["buttons"]` order).
    pub fn pressed(&self) -> impl Iterator<Item = &'static str> + '_ {
        [
            (BTN_B, "B"),
            (BTN_A, "A"),
            (BTN_STICK, "stick"),
            (BTN_MENU, "menu"),
        ]
        .into_iter()
        .filter(move |(m, _)| self.buttons & m != 0)
        .map(|(_, n)| n)
    }
}

/// Decode a controller report; `None` for anything else (other magic, other slots).
pub fn decode_controller(r: &Report) -> Option<ControllerReport> {
    if r[0] != 0x53 || r[1] != 0x32 || r[2] != 0x08 {
        return None;
    }
    if r[3] != SLOT_LEFT && r[3] != SLOT_RIGHT {
        return None;
    }
    let s0 = &r[16..28];
    let s1 = &r[30..42];
    // the second sample is zero when the packet holds one
    let s = if s0.iter().any(|&b| b != 0) { s0 } else { s1 };
    let mut raw_accel = [0i16; 3];
    let mut raw_gyro = [0i16; 3];
    for k in 0..3 {
        raw_accel[k] = be16(&s[2 * k..]);
        raw_gyro[k] = be16(&s[6 + 2 * k..]);
    }
    let mut buttons = u32::from(r[47]) & (BTN_B | BTN_A | BTN_STICK);
    if r[48] & 1 != 0 {
        buttons |= BTN_MENU;
    }
    let mut raw = [0u8; REPORT_SIZE];
    raw.copy_from_slice(r);
    Some(ControllerReport {
        slot: r[3],
        ts_us: le32(&r[8..]),
        stick_x: i32::from(r[5]) - 128,
        stick_y: i32::from(r[6]) - 128,
        trigger: r[7],
        grip: r[44],
        battery: r[46] & 0x7f,
        charging: r[46] & 0x80 != 0,
        buttons,
        touch: r[49],
        raw_accel,
        raw_gyro,
        raw,
    })
}
