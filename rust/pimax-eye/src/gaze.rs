//! Gaze notification (TTP stream 0x0505, 1242-byte frames, about 120.5 Hz) decoder.
//!
//! The payload is `00 00` followed by one compound `0x230bb8` (35 elements of type `0x0bb8`). Each element is
//! `0x20bb9` (compound, 2 items) `| tag2 field-number | value`, where the value is a scalar or a compound
//! (`0x31f42` vec3 S15.16, `0x21f43` vec2 S15.16, `0x21f40` two S22.42). The decoder walks the elements by *field
//! number*, so field order does not matter; the 364 full-size packets of `20261013-130650-full-packets` all use the order
//! returned by [`WIRE_ORDER`], which [`encode_payload`] reproduces byte for byte.
//!
//! The field meanings come from the Windows captures (headset not worn in the only full-size capture, worn in the
//! 485-byte snap-length captures where only eye A and the frame header are visible): see `analysis/overnight/eye.md`.
//! Eye B (fields 8-13, 17, 24, 25, 32, 33) has never been seen with valid data.

use crate::tlv::{
    fixed16_16_to_f64, fixed22_42_to_f64, push_compound, push_fixed16_16, push_fixed22_42,
    push_i32, push_i64, Items, Value,
};

/// Type id of the 35-element container.
pub const TYPE_GAZE: u16 = 0x0bb8;
/// Type id of one `(field number, value)` element.
pub const TYPE_FIELD: u16 = 0x0bb9;
/// Type id of a vec3 of S15.16 values.
pub const TYPE_VEC3_F16: u16 = 0x1f42;
/// Type id of a vec2 of S15.16 values.
pub const TYPE_VEC2_F16: u16 = 0x1f43;
/// Type id of a pair of S22.42 values.
pub const TYPE_PAIR_F22: u16 = 0x1f40;

/// Field numbers of the frame-level fields.
pub mod field {
    /// int64 device timestamp in microseconds.
    pub const TIMESTAMP: u8 = 1;
    /// int32, constant 0xFFFFF in every packet seen.
    pub const FIELD_14: u8 = 14;
    /// int32 frame counter, +1 per packet.
    pub const COUNTER: u8 = 15;
    /// vec3, sentinel (-1,-1,-1) in every packet seen.
    pub const AUX_POINT_1: u8 = 18;
    /// validity of [`AUX_POINT_1`].
    pub const AUX_VALID_1: u8 = 19;
    /// vec3, sentinel (-1,-1,-1) in every packet seen.
    pub const AUX_POINT_2: u8 = 20;
    /// validity of [`AUX_POINT_2`].
    pub const AUX_VALID_2: u8 = 21;
    /// int32 flag, always 0.
    pub const FLAG_26: u8 = 26;
    /// int32 flag, always 0.
    pub const FLAG_27: u8 = 27;
    /// int32 flag, always 0.
    pub const FLAG_28: u8 = 28;
    /// S15.16, -1.0 in every packet seen.
    pub const FIELD_29: u8 = 29;
    /// S15.16, constant 57.35359 in every packet seen.
    pub const FIELD_34: u8 = 34;
    /// int32, constant 1 in every packet seen.
    pub const FIELD_35: u8 = 35;
}

/// Field numbers of the two eye groups, in the order
/// `[origin, origin_valid, direction, direction_valid, pupil_diameter, openness, aux_pair, pupil_pos, pupil_pos_valid, flag_a, flag_b]`.
pub const EYE_FIELDS: [[u8; 11]; 2] = [
    [2, 3, 4, 5, 6, 7, 16, 23, 22, 30, 31],
    [8, 9, 10, 11, 12, 13, 17, 25, 24, 32, 33],
];

/// Field order used on the wire by every packet seen.
pub const WIRE_ORDER: [u8; 35] = [
    1, 2, 3, 4, 5, 6, 7, 16, 23, 22, 30, 31, 8, 9, 10, 11, 12, 13, 17, 25, 24, 32, 33, 14, 15, 18,
    19, 20, 21, 26, 27, 29, 28, 34, 35,
];

const FRAME_FIELDS: [u8; 13] = [1, 14, 15, 18, 19, 20, 21, 26, 27, 28, 29, 34, 35];

fn group_mask(g: usize) -> u64 {
    EYE_FIELDS[g].iter().fold(0u64, |m, f| m | (1u64 << f))
}

/// Tracking state of one eye derived from the validity flags.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EyeState {
    /// No valid data (eye closed, not worn, or the group is missing from a truncated packet).
    Lost,
    /// The eye position is valid but there is no gaze direction (blink, partial view).
    OriginOnly,
    /// Origin, direction, pupil diameter and pupil position are all valid.
    Tracked,
}

/// One eye. Missing or invalid values hold the device sentinels (`0` for vectors, `-1` for scalars).
#[derive(Debug, Clone, PartialEq)]
pub struct Eye {
    /// Gaze origin in millimetres (fields 2 / 8). Eye A is about (+31, -2, -22).
    pub origin_mm: [f64; 3],
    /// Validity of `origin_mm` (fields 3 / 9).
    pub origin_valid: bool,
    /// Unit gaze direction (fields 4 / 10); +z is the viewing direction.
    pub direction: [f64; 3],
    /// Validity of `direction` (fields 5 / 11).
    pub direction_valid: bool,
    /// Pupil diameter in millimetres (fields 6 / 12), -1 when invalid.
    pub pupil_diameter_mm: f64,
    /// Fields 7 / 13: eye openness candidate. -1 in every packet of every capture, so "not provided".
    pub openness: f64,
    /// Pupil position in the sensor area, 0..1 (fields 23 / 25), -1 when invalid.
    pub pupil_pos: [f64; 2],
    /// Validity of `pupil_pos` (fields 22 / 24).
    pub pupil_pos_valid: bool,
    /// Fields 16 / 17: two S22.42 values, (-1, -1) in every packet seen.
    pub aux_pair: [f64; 2],
    /// Fields 30,31 / 32,33: flags. The first is always 1; the second is 0 whenever the eye is fully tracked and
    /// 1 whenever only the origin is valid (observed on eye A), so it may mean "pupil/glints lost".
    pub flags: [i32; 2],
    /// All 11 fields of this group were present in the packet.
    pub complete: bool,
}

impl Default for Eye {
    fn default() -> Eye {
        Eye {
            origin_mm: [0.0; 3],
            origin_valid: false,
            direction: [0.0; 3],
            direction_valid: false,
            pupil_diameter_mm: -1.0,
            openness: -1.0,
            pupil_pos: [-1.0; 2],
            pupil_pos_valid: false,
            aux_pair: [-1.0; 2],
            flags: [0; 2],
            complete: false,
        }
    }
}

impl Eye {
    /// Tracking state from the validity flags.
    pub fn state(&self) -> EyeState {
        if self.direction_valid {
            EyeState::Tracked
        } else if self.origin_valid {
            EyeState::OriginOnly
        } else {
            EyeState::Lost
        }
    }

    /// Pupil diameter when valid.
    pub fn pupil_diameter(&self) -> Option<f64> {
        (self.pupil_diameter_mm >= 0.0).then_some(self.pupil_diameter_mm)
    }

    /// Eye openness when the device provides it (never seen).
    pub fn openness(&self) -> Option<f64> {
        (self.openness >= 0.0).then_some(self.openness)
    }

    /// Gaze ray `(origin_mm, direction)` when both are valid.
    pub fn ray(&self) -> Option<([f64; 3], [f64; 3])> {
        (self.origin_valid && self.direction_valid).then_some((self.origin_mm, self.direction))
    }

    /// `(yaw, pitch)` of the gaze direction in degrees, `yaw = atan2(x, z)`, `pitch = atan2(y, hypot(x, z))`, when valid.
    /// The axis signs are those of the device frame (x toward eye A's side, +z forward); the sign of y is unverified.
    pub fn yaw_pitch_deg(&self) -> Option<(f64, f64)> {
        if !self.direction_valid {
            return None;
        }
        let [x, y, z] = self.direction;
        Some((x.atan2(z).to_degrees(), y.atan2(x.hypot(z)).to_degrees()))
    }
}

/// Frame-level fields other than timestamp and counter. All unexplained constants.
#[derive(Debug, Clone, PartialEq)]
pub struct FrameFields {
    /// Field 14.
    pub field_14: i32,
    /// Fields 18 and 20.
    pub aux_points: [[f64; 3]; 2],
    /// Fields 19 and 21.
    pub aux_valid: [bool; 2],
    /// Fields 26, 27, 28.
    pub flags: [i32; 3],
    /// Field 29.
    pub field_29: f64,
    /// Field 34.
    pub field_34: f64,
    /// Field 35.
    pub field_35: i32,
}

impl Default for FrameFields {
    fn default() -> FrameFields {
        FrameFields {
            field_14: 0,
            aux_points: [[-1.0; 3]; 2],
            aux_valid: [false; 2],
            flags: [0; 3],
            field_29: -1.0,
            field_34: 0.0,
            field_35: 0,
        }
    }
}

/// One decoded gaze notification.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GazeSample {
    /// Device timestamp in microseconds (field 1); +8300 per packet nominally, same crystal as the IMU clock.
    pub timestamp_us: i64,
    /// Frame counter (field 15), `None` when the packet was cut before it.
    pub frame_counter: Option<u32>,
    /// Eye A (fields 2-7, 16, 22, 23, 30, 31) and eye B (fields 8-13, 17, 24, 25, 32, 33).
    pub eyes: [Eye; 2],
    /// Remaining fields.
    pub frame: FrameFields,
    /// The payload ended before all announced elements were read (capture snap length, or a short read).
    pub truncated: bool,
    /// Bit `n` is set when field `n` was decoded.
    pub fields: u64,
    /// Elements with a field number this decoder does not know (kept out of the typed struct).
    pub unknown_fields: u32,
}

impl GazeSample {
    /// Whether field `n` was decoded.
    pub fn has(&self, n: u8) -> bool {
        n < 64 && self.fields & (1u64 << n) != 0
    }

    /// Whether all 35 known fields were decoded.
    pub fn is_complete(&self) -> bool {
        let all = WIRE_ORDER.iter().fold(0u64, |m, f| m | (1u64 << f));
        self.fields & all == all
    }
}

/// Decoder errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GazeError {
    /// No payload prefix, or nothing decodable (not even the timestamp).
    TooShort,
    /// The first item is not a `0x...0bb8` container.
    NotGaze,
    /// An element or a field value has the wrong shape.
    BadField {
        /// Field number, or -1 when the element header itself is wrong.
        field: i32,
        /// What was wrong.
        reason: &'static str,
    },
    /// Strict mode: the payload ends before the announced elements do.
    Truncated {
        /// Elements read.
        found: u16,
        /// Elements announced by the container.
        announced: u16,
    },
    /// Strict mode: items remain after the announced elements.
    Trailing {
        /// Offset (inside the TLV area) of the first unexpected item.
        offset: usize,
    },
}

impl core::fmt::Display for GazeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            GazeError::TooShort => write!(f, "payload too short to hold a gaze sample"),
            GazeError::NotGaze => write!(f, "payload does not start with a 0x..0bb8 container"),
            GazeError::BadField { field, reason } => write!(f, "field {field}: {reason}"),
            GazeError::Truncated { found, announced } => {
                write!(f, "payload truncated: {found} of {announced} elements")
            }
            GazeError::Trailing { offset } => write!(f, "trailing items at offset {offset}"),
        }
    }
}

impl std::error::Error for GazeError {}

enum Val {
    I32(i32),
    I64(i64),
    F16(i32),
    Vec3([i32; 3]),
    Vec2([i32; 2]),
    Pair22([i64; 2]),
}

struct Cursor<'a> {
    items: Items<'a>,
    cut: bool,
}

impl<'a> Cursor<'a> {
    fn next(&mut self) -> Option<Value<'a>> {
        match self.items.next() {
            Some(Ok(i)) => Some(i.decode()),
            Some(Err(_)) => {
                self.cut = true;
                None
            }
            None => None,
        }
    }
}

fn bad(field: i32, reason: &'static str) -> GazeError {
    GazeError::BadField { field, reason }
}

/// Read one element. `Ok(None)` means the payload ended inside or before it.
fn read_element(cur: &mut Cursor<'_>) -> Result<Option<(i32, Val)>, GazeError> {
    let Some(head) = cur.next() else {
        return Ok(None);
    };
    match head {
        Value::Compound {
            count: 2,
            type_id: TYPE_FIELD,
        } => {}
        _ => return Err(bad(-1, "element does not start with a 0x20bb9 compound")),
    }
    let no = match cur.next() {
        Some(Value::Int32(n)) => n,
        Some(_) => return Err(bad(-1, "element field number is not an int32")),
        None => return Ok(None),
    };
    let Some(v) = cur.next() else {
        return Ok(None);
    };
    let val = match v {
        Value::Int32(x) => Val::I32(x),
        Value::Int64(x) => Val::I64(x),
        Value::Fixed16_16(x) => Val::F16(x),
        Value::Compound {
            count: 3,
            type_id: TYPE_VEC3_F16,
        } => {
            let mut a = [0i32; 3];
            for slot in &mut a {
                match cur.next() {
                    Some(Value::Fixed16_16(x)) => *slot = x,
                    Some(_) => return Err(bad(no, "vec3 component is not S15.16")),
                    None => return Ok(None),
                }
            }
            Val::Vec3(a)
        }
        Value::Compound {
            count: 2,
            type_id: TYPE_VEC2_F16,
        } => {
            let mut a = [0i32; 2];
            for slot in &mut a {
                match cur.next() {
                    Some(Value::Fixed16_16(x)) => *slot = x,
                    Some(_) => return Err(bad(no, "vec2 component is not S15.16")),
                    None => return Ok(None),
                }
            }
            Val::Vec2(a)
        }
        Value::Compound {
            count: 2,
            type_id: TYPE_PAIR_F22,
        } => {
            let mut a = [0i64; 2];
            for slot in &mut a {
                match cur.next() {
                    Some(Value::Fixed22_42(x)) => *slot = x,
                    Some(_) => return Err(bad(no, "pair component is not S22.42")),
                    None => return Ok(None),
                }
            }
            Val::Pair22(a)
        }
        Value::Compound { .. } => return Err(bad(no, "unknown compound type")),
        _ => return Err(bad(no, "unsupported value tag")),
    };
    Ok(Some((no, val)))
}

fn v3(a: [i32; 3]) -> [f64; 3] {
    [
        fixed16_16_to_f64(a[0]),
        fixed16_16_to_f64(a[1]),
        fixed16_16_to_f64(a[2]),
    ]
}

/// Store a decoded value. Returns `Ok(false)` for an unknown field number.
fn apply(s: &mut GazeSample, no: i32, val: Val) -> Result<bool, GazeError> {
    let Ok(n) = u8::try_from(no) else {
        return Ok(false);
    };
    // locate the field: eye group slot, frame field, or unknown
    let mismatch = || bad(no, "value type does not match the field");
    let slot = EYE_FIELDS
        .iter()
        .enumerate()
        .find_map(|(g, f)| f.iter().position(|x| *x == n).map(|i| (g, i)));
    if let Some((g, i)) = slot {
        let e = &mut s.eyes[g];
        match (i, val) {
            (0, Val::Vec3(a)) => e.origin_mm = v3(a),
            (1, Val::I32(x)) => e.origin_valid = x != 0,
            (2, Val::Vec3(a)) => e.direction = v3(a),
            (3, Val::I32(x)) => e.direction_valid = x != 0,
            (4, Val::F16(x)) => e.pupil_diameter_mm = fixed16_16_to_f64(x),
            (5, Val::F16(x)) => e.openness = fixed16_16_to_f64(x),
            (6, Val::Pair22(a)) => e.aux_pair = [fixed22_42_to_f64(a[0]), fixed22_42_to_f64(a[1])],
            (7, Val::Vec2(a)) => e.pupil_pos = [fixed16_16_to_f64(a[0]), fixed16_16_to_f64(a[1])],
            (8, Val::I32(x)) => e.pupil_pos_valid = x != 0,
            (9, Val::I32(x)) => e.flags[0] = x,
            (10, Val::I32(x)) => e.flags[1] = x,
            _ => return Err(mismatch()),
        }
    } else {
        let f = &mut s.frame;
        match (n, val) {
            (field::TIMESTAMP, Val::I64(x)) => s.timestamp_us = x,
            (field::COUNTER, Val::I32(x)) => s.frame_counter = Some(x as u32),
            (field::FIELD_14, Val::I32(x)) => f.field_14 = x,
            (field::AUX_POINT_1, Val::Vec3(a)) => f.aux_points[0] = v3(a),
            (field::AUX_VALID_1, Val::I32(x)) => f.aux_valid[0] = x != 0,
            (field::AUX_POINT_2, Val::Vec3(a)) => f.aux_points[1] = v3(a),
            (field::AUX_VALID_2, Val::I32(x)) => f.aux_valid[1] = x != 0,
            (field::FLAG_26, Val::I32(x)) => f.flags[0] = x,
            (field::FLAG_27, Val::I32(x)) => f.flags[1] = x,
            (field::FLAG_28, Val::I32(x)) => f.flags[2] = x,
            (field::FIELD_29, Val::F16(x)) => f.field_29 = fixed16_16_to_f64(x),
            (field::FIELD_34, Val::F16(x)) => f.field_34 = fixed16_16_to_f64(x),
            (field::FIELD_35, Val::I32(x)) => f.field_35 = x,
            _ if FRAME_FIELDS.contains(&n) => return Err(mismatch()),
            _ => return Ok(false),
        }
    }
    s.fields |= 1u64 << n;
    Ok(true)
}

fn parse_inner(payload: &[u8], strict: bool) -> Result<GazeSample, GazeError> {
    if payload.len() < 2 {
        return Err(GazeError::TooShort);
    }
    let mut cur = Cursor {
        items: Items::new(&payload[2..]),
        cut: false,
    };
    let announced = match cur.next() {
        Some(Value::Compound {
            count,
            type_id: TYPE_GAZE,
        }) => count,
        Some(_) => return Err(GazeError::NotGaze),
        None => return Err(GazeError::TooShort),
    };
    let mut s = GazeSample::default();
    let mut found = 0u16;
    while found < announced {
        match read_element(&mut cur)? {
            Some((no, val)) => {
                if !apply(&mut s, no, val)? {
                    s.unknown_fields += 1;
                }
                found += 1;
            }
            None => break,
        }
    }
    s.truncated = found < announced;
    if strict {
        if s.truncated {
            return Err(GazeError::Truncated { found, announced });
        }
        if cur.next().is_some() || cur.cut {
            return Err(GazeError::Trailing {
                offset: cur.items.position(),
            });
        }
    } else if !s.has(field::TIMESTAMP) {
        return Err(GazeError::TooShort);
    }
    for (g, e) in s.eyes.iter_mut().enumerate() {
        e.complete = s.fields & group_mask(g) == group_mask(g);
    }
    Ok(s)
}

/// Strict decode of a complete payload (the 2-byte prefix plus all announced elements, nothing after them).
pub fn parse_payload(payload: &[u8]) -> Result<GazeSample, GazeError> {
    parse_inner(payload, true)
}

/// Lenient decode for payloads cut by a capture snap length or a short read: every complete element before the cut
/// is kept, `truncated` is set, and absent fields keep their invalid defaults. Needs at least the timestamp.
pub fn parse_payload_lenient(payload: &[u8]) -> Result<GazeSample, GazeError> {
    parse_inner(payload, false)
}

/// Re-encode a sample's payload in [`WIRE_ORDER`] (all fields; absent ones are written with their defaults).
/// Used to synthesise notifications for tests and the fake chip.
pub fn encode_payload(s: &GazeSample) -> Vec<u8> {
    let raw16 = |x: f64| (x * 65536.0).round() as i32;
    let raw22 = |x: f64| (x * 4_398_046_511_104.0).round() as i64;
    let mut items = Vec::new();
    push_compound(&mut items, WIRE_ORDER.len() as u16, TYPE_GAZE);
    for &n in &WIRE_ORDER {
        push_compound(&mut items, 2, TYPE_FIELD);
        push_i32(&mut items, i32::from(n));
        let slot = EYE_FIELDS
            .iter()
            .enumerate()
            .find_map(|(g, f)| f.iter().position(|x| *x == n).map(|i| (g, i)));
        let vec3 = |items: &mut Vec<u8>, v: [f64; 3]| {
            push_compound(items, 3, TYPE_VEC3_F16);
            for c in v {
                push_fixed16_16(items, raw16(c));
            }
        };
        if let Some((g, i)) = slot {
            let e = &s.eyes[g];
            match i {
                0 => vec3(&mut items, e.origin_mm),
                1 => push_i32(&mut items, i32::from(e.origin_valid)),
                2 => vec3(&mut items, e.direction),
                3 => push_i32(&mut items, i32::from(e.direction_valid)),
                4 => push_fixed16_16(&mut items, raw16(e.pupil_diameter_mm)),
                5 => push_fixed16_16(&mut items, raw16(e.openness)),
                6 => {
                    push_compound(&mut items, 2, TYPE_PAIR_F22);
                    for c in e.aux_pair {
                        push_fixed22_42(&mut items, raw22(c));
                    }
                }
                7 => {
                    push_compound(&mut items, 2, TYPE_VEC2_F16);
                    for c in e.pupil_pos {
                        push_fixed16_16(&mut items, raw16(c));
                    }
                }
                8 => push_i32(&mut items, i32::from(e.pupil_pos_valid)),
                9 => push_i32(&mut items, e.flags[0]),
                _ => push_i32(&mut items, e.flags[1]),
            }
        } else {
            let f = &s.frame;
            match n {
                field::TIMESTAMP => push_i64(&mut items, s.timestamp_us),
                field::COUNTER => push_i32(&mut items, s.frame_counter.unwrap_or(0) as i32),
                field::FIELD_14 => push_i32(&mut items, f.field_14),
                field::AUX_POINT_1 => vec3(&mut items, f.aux_points[0]),
                field::AUX_VALID_1 => push_i32(&mut items, i32::from(f.aux_valid[0])),
                field::AUX_POINT_2 => vec3(&mut items, f.aux_points[1]),
                field::AUX_VALID_2 => push_i32(&mut items, i32::from(f.aux_valid[1])),
                field::FLAG_26 => push_i32(&mut items, f.flags[0]),
                field::FLAG_27 => push_i32(&mut items, f.flags[1]),
                field::FLAG_28 => push_i32(&mut items, f.flags[2]),
                field::FIELD_29 => push_fixed16_16(&mut items, raw16(f.field_29)),
                field::FIELD_34 => push_fixed16_16(&mut items, raw16(f.field_34)),
                _ => push_i32(&mut items, f.field_35),
            }
        }
    }
    let mut payload = vec![0, 0];
    payload.extend_from_slice(&items);
    payload
}

/// Running statistics over a stream of samples.
#[derive(Debug, Clone, Default)]
pub struct GazeStats {
    /// Samples seen.
    pub samples: u64,
    /// Samples per eye in each [`EyeState`] (`[Lost, OriginOnly, Tracked]`).
    pub states: [[u64; 3]; 2],
    /// Frames missing according to jumps of the frame counter.
    pub counter_gaps: u64,
    /// Timestamp deltas above 1.5 nominal periods.
    pub timestamp_gaps: u64,
    /// Timestamp going backwards or standing still.
    pub timestamp_regressions: u64,
    /// First and last device timestamps.
    pub first_ts: Option<i64>,
    /// Last device timestamp.
    pub last_ts: Option<i64>,
    last_counter: Option<u32>,
}

/// Nominal period of the gaze stream in microseconds (mode of the timestamp deltas, 120.48 Hz).
pub const NOMINAL_PERIOD_US: i64 = 8300;

impl GazeStats {
    /// Account for one sample.
    pub fn add(&mut self, s: &GazeSample) {
        self.samples += 1;
        for (g, e) in s.eyes.iter().enumerate() {
            let i = match e.state() {
                EyeState::Lost => 0,
                EyeState::OriginOnly => 1,
                EyeState::Tracked => 2,
            };
            self.states[g][i] += 1;
        }
        if let (Some(prev), Some(c)) = (self.last_counter, s.frame_counter) {
            let d = c.wrapping_sub(prev);
            if d > 1 && d < 1 << 31 {
                self.counter_gaps += u64::from(d - 1);
            }
        }
        if s.frame_counter.is_some() {
            self.last_counter = s.frame_counter;
        }
        if let Some(prev) = self.last_ts {
            let d = s.timestamp_us - prev;
            if d <= 0 {
                self.timestamp_regressions += 1;
            } else if d * 2 > NOMINAL_PERIOD_US * 3 {
                self.timestamp_gaps += 1;
            }
        }
        self.first_ts.get_or_insert(s.timestamp_us);
        self.last_ts = Some(s.timestamp_us);
    }

    /// Rate from the device timestamps, in Hz.
    pub fn device_rate_hz(&self) -> Option<f64> {
        let (a, b) = (self.first_ts?, self.last_ts?);
        (self.samples > 1 && b > a).then(|| (self.samples - 1) as f64 / ((b - a) as f64 * 1e-6))
    }
}

/// One-line human readable form used by `pimax-eye stream --print`.
pub fn format_line(s: &GazeSample) -> String {
    let mut out = format!(
        "ts={:>12}us ctr={:>6}",
        s.timestamp_us,
        s.frame_counter
            .map_or_else(|| "-".to_string(), |c| c.to_string())
    );
    for (name, e) in ["A", "B"].iter().zip(&s.eyes) {
        out.push_str(&format!(" | {name}: "));
        match e.state() {
            EyeState::Tracked => {
                let (yaw, pitch) = e.yaw_pitch_deg().unwrap_or((0.0, 0.0));
                out.push_str(&format!(
                    "org({:+6.1},{:+6.1},{:+6.1}) dir({:+.3},{:+.3},{:+.3}) yaw{:+6.1} pitch{:+6.1} pupil{:5.2}mm",
                    e.origin_mm[0], e.origin_mm[1], e.origin_mm[2],
                    e.direction[0], e.direction[1], e.direction[2],
                    yaw, pitch, e.pupil_diameter_mm
                ));
            }
            EyeState::OriginOnly => out.push_str(&format!(
                "org({:+6.1},{:+6.1},{:+6.1}) no gaze",
                e.origin_mm[0], e.origin_mm[1], e.origin_mm[2]
            )),
            EyeState::Lost => out.push_str(if e.complete { "invalid" } else { "n/a" }),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Canonical sample with eye A tracked.
    #[allow(clippy::field_reassign_with_default)]
    pub(crate) fn sample() -> GazeSample {
        let mut s = GazeSample::default();
        s.timestamp_us = 50_376_465;
        s.frame_counter = Some(5412);
        s.frame.field_14 = 0xfffff;
        s.frame.field_34 = fixed16_16_to_f64(0x0039_5a85);
        s.frame.field_35 = 1;
        let a = &mut s.eyes[0];
        a.origin_mm = [30.75, -4.5, -27.25];
        a.origin_valid = true;
        a.direction = [0.3125, 0.140625, 0.9375];
        a.direction_valid = true;
        a.pupil_diameter_mm = 3.5;
        a.pupil_pos = [0.5, 0.625];
        a.pupil_pos_valid = true;
        a.flags = [1, 0];
        s.eyes[1].flags = [1, 0];
        s
    }

    #[test]
    fn wire_order_covers_each_field_once() {
        let mut seen = WIRE_ORDER.to_vec();
        seen.sort_unstable();
        assert_eq!(seen, (1..=35).collect::<Vec<u8>>());
    }

    #[test]
    fn encode_then_parse_is_identity() {
        let mut s = sample();
        let p = encode_payload(&s);
        let back = parse_payload(&p).unwrap();
        s.fields = back.fields;
        s.eyes[0].complete = true;
        s.eyes[1].complete = true;
        assert_eq!(back, s);
        assert!(back.is_complete());
        assert_eq!(
            encode_payload(&back),
            p,
            "bytes must survive a decode/encode cycle"
        );
    }

    #[test]
    fn truncation_keeps_complete_elements_only() {
        let p = encode_payload(&sample());
        let full = parse_payload(&p).unwrap();
        for cut in 0..p.len() {
            match parse_payload_lenient(&p[..cut]) {
                Ok(s) => {
                    assert!(s.truncated, "cut {cut}");
                    assert!(parse_payload(&p[..cut]).is_err());
                    // every decoded field equals the full decode
                    if s.has(field::TIMESTAMP) {
                        assert_eq!(s.timestamp_us, full.timestamp_us);
                    }
                    if s.eyes[0].complete {
                        assert_eq!(s.eyes[0].origin_mm, full.eyes[0].origin_mm);
                    }
                    assert!(s.fields & !full.fields == 0);
                }
                Err(e) => assert!(
                    matches!(e, GazeError::TooShort | GazeError::NotGaze),
                    "cut {cut}: {e:?}"
                ),
            }
        }
        assert!(!parse_payload_lenient(&p).unwrap().truncated);
    }

    #[test]
    fn strict_rejects_trailing_items_and_short_containers() {
        let p = encode_payload(&sample());
        // an item after the announced elements
        let mut extra = p.clone();
        push_i32(&mut extra, 1);
        assert!(matches!(
            parse_payload(&extra),
            Err(GazeError::Trailing { .. })
        ));
        // container announces one element more than present
        let mut more = p.clone();
        let pos = more.windows(5).position(|w| w == [5, 0, 0, 0, 4]).unwrap();
        more[pos + 5..pos + 9]
            .copy_from_slice(&((36u32 << 16) | u32::from(TYPE_GAZE)).to_be_bytes());
        assert_eq!(
            parse_payload(&more),
            Err(GazeError::Truncated {
                found: 35,
                announced: 36
            })
        );
        let lenient = parse_payload_lenient(&more).unwrap();
        assert!(lenient.truncated && lenient.is_complete());
        // wrong value type for a known field: the timestamp as int32
        let mut wrong = Vec::new();
        push_compound(&mut wrong, 1, TYPE_GAZE);
        push_compound(&mut wrong, 2, TYPE_FIELD);
        push_i32(&mut wrong, 1);
        push_i32(&mut wrong, 5);
        let mut wp = vec![0, 0];
        wp.extend_from_slice(&wrong);
        assert!(matches!(
            parse_payload(&wp),
            Err(GazeError::BadField { field: 1, .. })
        ));
    }

    #[test]
    fn unknown_fields_are_counted_not_fatal() {
        let mut items = Vec::new();
        push_compound(&mut items, 2, TYPE_GAZE);
        push_compound(&mut items, 2, TYPE_FIELD);
        push_i32(&mut items, 1);
        push_i64(&mut items, 77);
        push_compound(&mut items, 2, TYPE_FIELD);
        push_i32(&mut items, 99);
        push_i32(&mut items, 5);
        let mut p = vec![0, 0];
        p.extend_from_slice(&items);
        let s = parse_payload(&p).unwrap();
        assert_eq!((s.timestamp_us, s.unknown_fields), (77, 1));
        assert!(!s.is_complete());
    }

    #[test]
    fn non_gaze_payloads_are_rejected() {
        assert_eq!(parse_payload(&[]), Err(GazeError::TooShort));
        assert_eq!(parse_payload(&[0, 0]), Err(GazeError::TooShort));
        let mut items = Vec::new();
        push_i32(&mut items, 3);
        let mut p = vec![0, 0];
        p.extend_from_slice(&items);
        assert_eq!(parse_payload(&p), Err(GazeError::NotGaze));
    }

    #[test]
    fn eye_helpers() {
        let s = sample();
        assert_eq!(s.eyes[0].state(), EyeState::Tracked);
        assert_eq!(s.eyes[1].state(), EyeState::Lost);
        let (yaw, pitch) = s.eyes[0].yaw_pitch_deg().unwrap();
        assert!((yaw - (0.3125f64).atan2(0.9375).to_degrees()).abs() < 1e-12);
        assert!(pitch > 0.0);
        assert_eq!(s.eyes[0].pupil_diameter(), Some(3.5));
        assert_eq!(s.eyes[1].pupil_diameter(), None);
        assert_eq!(s.eyes[0].openness(), None);
        assert!(s.eyes[1].ray().is_none());
        assert!(s.eyes[0].ray().is_some());
    }

    #[test]
    fn stats_detect_gaps_and_rate() {
        let mut st = GazeStats::default();
        let mut s = sample();
        for i in 0..100u32 {
            s.timestamp_us += NOMINAL_PERIOD_US;
            s.frame_counter = Some(5412 + i);
            if i == 50 {
                s.timestamp_us += 2 * NOMINAL_PERIOD_US;
                s.frame_counter = Some(5412 + i + 2);
            }
            if i > 50 {
                s.frame_counter = Some(5412 + i + 2);
            }
            st.add(&s);
        }
        assert_eq!(st.samples, 100);
        assert_eq!(st.counter_gaps, 2);
        assert_eq!(st.timestamp_gaps, 1);
        assert_eq!(st.timestamp_regressions, 0);
        let hz = st.device_rate_hz().unwrap();
        assert!((hz - 1e6 / 8300.0 * 99.0 / 101.0).abs() < 0.5, "{hz}");
        assert_eq!(st.states[0], [0, 0, 100]);
        assert_eq!(st.states[1], [100, 0, 0]);
    }

    #[test]
    fn line_format_mentions_both_eyes() {
        let decoded = parse_payload(&encode_payload(&sample())).unwrap();
        let l = format_line(&decoded);
        assert!(l.contains("A: org(") && l.contains("B: invalid"), "{l}");
    }
}
