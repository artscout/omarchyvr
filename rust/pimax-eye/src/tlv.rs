//! TLV items of a TTP payload.
//!
//! Every item is `tag u8 | len u32 BE | value[len]`. The tags below are the only ones seen in the Windows captures
//! (cold start, calibration, settings; 4 000+ messages). Unknown tags are carried as [`Value::Unknown`], never rejected.
//!
//! `tag 5` is a *compound header*: its 32-bit value is `count << 16 | type_id` and announces that the next `count`
//! items belong together (a vec3 is `0x31f42` followed by three fixed-point items; a gaze field is `0x20bb9` followed by
//! `field number` and the value). The count/type split is inferred from the data: it holds for all 364 full-size
//! gaze packets (`0x230bb8` = 35 fields, `0x20bb9` = 2 elements, `0x31f42` = 3, `0x21f43` = 2, `0x21f40` = 2).

/// Tag bytes seen on the wire.
pub mod tag {
    /// Unsigned 32-bit integer (seen only inside the configuration upload 0x852, whose capture is cut).
    pub const UINT32: u8 = 0x01;
    /// Signed 32-bit integer (counters, validity flags, field numbers, stream ids).
    pub const INT32: u8 = 0x02;
    /// Signed fixed point, 16 integer bits and 16 fraction bits (`raw / 65536`), 4 bytes.
    pub const FIXED_16_16: u8 = 0x03;
    /// Signed fixed point, 22 integer bits and 42 fraction bits (`raw / 2^42`), 8 bytes.
    pub const FIXED_22_42: u8 = 0x04;
    /// Compound header, 4 bytes: `count << 16 | type_id`.
    pub const COMPOUND: u8 = 0x05;
    /// Signed 64-bit integer (microsecond timestamps).
    pub const INT64: u8 = 0x06;
    /// String: `u32 BE length` followed by that many bytes.
    pub const STRING: u8 = 0x14;
    /// Blob: `u32 BE length` followed by that many bytes.
    pub const BLOB: u8 = 0x15;
    /// Array: `u32 BE count` followed by `count` 4-byte elements.
    pub const ARRAY: u8 = 0x17;
    /// Array of S22.42 values: `u32 BE count` followed by `count` 8-byte elements (seen only inside 0x852).
    pub const ARRAY_FIXED_22_42: u8 = 0x19;
}

/// Size of an item header (`tag` + `len`).
pub const ITEM_HEADER_LEN: usize = 5;

/// One undecoded item.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RawItem<'a> {
    /// Tag byte.
    pub tag: u8,
    /// Value bytes (exactly the length announced in the header).
    pub value: &'a [u8],
    /// Offset of the item header inside the buffer being iterated.
    pub offset: usize,
}

/// TLV walking error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TlvError {
    /// The item header or its value runs past the end of the buffer.
    Truncated {
        /// Offset of the offending item header.
        offset: usize,
        /// Bytes the item needs (header + value).
        needed: usize,
        /// Bytes left in the buffer from `offset`.
        available: usize,
    },
}

impl core::fmt::Display for TlvError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            TlvError::Truncated {
                offset,
                needed,
                available,
            } => write!(
                f,
                "TLV item at offset {offset} needs {needed} bytes but only {available} are left"
            ),
        }
    }
}

impl std::error::Error for TlvError {}

/// Iterator over the items of a buffer. After the first error the iterator ends.
#[derive(Debug, Clone)]
pub struct Items<'a> {
    buf: &'a [u8],
    pos: usize,
    failed: bool,
}

impl<'a> Items<'a> {
    /// Iterate `buf` from its first byte.
    pub fn new(buf: &'a [u8]) -> Self {
        Items {
            buf,
            pos: 0,
            failed: false,
        }
    }

    /// Offset of the next item header.
    pub fn position(&self) -> usize {
        self.pos
    }
}

impl<'a> Iterator for Items<'a> {
    type Item = Result<RawItem<'a>, TlvError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.failed || self.pos >= self.buf.len() {
            return None;
        }
        let rest = &self.buf[self.pos..];
        if rest.len() < ITEM_HEADER_LEN {
            self.failed = true;
            return Some(Err(TlvError::Truncated {
                offset: self.pos,
                needed: ITEM_HEADER_LEN,
                available: rest.len(),
            }));
        }
        let len = u32::from_be_bytes([rest[1], rest[2], rest[3], rest[4]]) as usize;
        let needed = ITEM_HEADER_LEN.saturating_add(len);
        if needed > rest.len() {
            self.failed = true;
            return Some(Err(TlvError::Truncated {
                offset: self.pos,
                needed,
                available: rest.len(),
            }));
        }
        let item = RawItem {
            tag: rest[0],
            value: &rest[ITEM_HEADER_LEN..needed],
            offset: self.pos,
        };
        self.pos += needed;
        Some(Ok(item))
    }
}

/// A decoded item value. Fixed-point values keep their raw integer so nothing is lost.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Value<'a> {
    /// `tag 1`.
    Uint32(u32),
    /// `tag 2`.
    Int32(i32),
    /// `tag 3`, raw value; real value is `raw / 65536`.
    Fixed16_16(i32),
    /// `tag 4`, raw value; real value is `raw / 2^42`.
    Fixed22_42(i64),
    /// `tag 5`.
    Compound {
        /// Number of items that follow and belong to this compound.
        count: u16,
        /// Type id.
        type_id: u16,
    },
    /// `tag 6`.
    Int64(i64),
    /// `tag 0x14`, without the length prefix.
    Str(&'a [u8]),
    /// `tag 0x15`, without the length prefix.
    Blob(&'a [u8]),
    /// `tag 0x17`: `count` 4-byte elements.
    Array {
        /// Element count.
        count: u32,
        /// `count * 4` bytes.
        elements: &'a [u8],
    },
    /// `tag 0x19`: `count` S22.42 elements of 8 bytes each.
    ArrayFixed22_42 {
        /// Element count.
        count: u32,
        /// `count * 8` bytes.
        elements: &'a [u8],
    },
    /// Unknown tag, or a known tag with an unexpected length.
    Unknown {
        /// Tag byte.
        tag: u8,
        /// Value bytes.
        value: &'a [u8],
    },
}

impl<'a> RawItem<'a> {
    /// Decode the value according to the tag table.
    pub fn decode(&self) -> Value<'a> {
        let v = self.value;
        let unknown = Value::Unknown {
            tag: self.tag,
            value: v,
        };
        match (self.tag, v.len()) {
            (tag::UINT32, 4) => Value::Uint32(u32::from_be_bytes([v[0], v[1], v[2], v[3]])),
            (tag::INT32, 4) => Value::Int32(i32::from_be_bytes([v[0], v[1], v[2], v[3]])),
            (tag::FIXED_16_16, 4) => {
                Value::Fixed16_16(i32::from_be_bytes([v[0], v[1], v[2], v[3]]))
            }
            (tag::FIXED_22_42, 8) => Value::Fixed22_42(i64::from_be_bytes([
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
            ])),
            (tag::COMPOUND, 4) => {
                let w = u32::from_be_bytes([v[0], v[1], v[2], v[3]]);
                Value::Compound {
                    count: (w >> 16) as u16,
                    type_id: (w & 0xffff) as u16,
                }
            }
            (tag::INT64, 8) => Value::Int64(i64::from_be_bytes([
                v[0], v[1], v[2], v[3], v[4], v[5], v[6], v[7],
            ])),
            (tag::STRING | tag::BLOB, n) if n >= 4 => {
                let l = u32::from_be_bytes([v[0], v[1], v[2], v[3]]) as usize;
                if l == n - 4 {
                    if self.tag == tag::STRING {
                        Value::Str(&v[4..])
                    } else {
                        Value::Blob(&v[4..])
                    }
                } else {
                    unknown
                }
            }
            (tag::ARRAY | tag::ARRAY_FIXED_22_42, n) if n >= 4 => {
                let c = u32::from_be_bytes([v[0], v[1], v[2], v[3]]);
                let width = if self.tag == tag::ARRAY { 4 } else { 8 };
                if (c as usize).checked_mul(width) == Some(n - 4) {
                    if self.tag == tag::ARRAY {
                        Value::Array {
                            count: c,
                            elements: &v[4..],
                        }
                    } else {
                        Value::ArrayFixed22_42 {
                            count: c,
                            elements: &v[4..],
                        }
                    }
                } else {
                    unknown
                }
            }
            _ => unknown,
        }
    }
}

/// `raw / 65536`.
pub fn fixed16_16_to_f64(raw: i32) -> f64 {
    f64::from(raw) / 65536.0
}

/// `raw / 2^42`.
pub fn fixed22_42_to_f64(raw: i64) -> f64 {
    raw as f64 / 4_398_046_511_104.0
}

/// Append one item.
///
/// # Panics
/// Panics if `value` is longer than `u32::MAX` bytes.
pub fn push_item(out: &mut Vec<u8>, tag: u8, value: &[u8]) {
    out.push(tag);
    let len = u32::try_from(value.len()).expect("TLV value longer than u32::MAX");
    out.extend_from_slice(&len.to_be_bytes());
    out.extend_from_slice(value);
}

/// Append a `tag 2` item.
pub fn push_i32(out: &mut Vec<u8>, v: i32) {
    push_item(out, tag::INT32, &v.to_be_bytes());
}

/// Append a `tag 3` item with the given raw S15.16 value.
pub fn push_fixed16_16(out: &mut Vec<u8>, raw: i32) {
    push_item(out, tag::FIXED_16_16, &raw.to_be_bytes());
}

/// Append a `tag 4` item with the given raw S22.42 value.
pub fn push_fixed22_42(out: &mut Vec<u8>, raw: i64) {
    push_item(out, tag::FIXED_22_42, &raw.to_be_bytes());
}

/// Append a `tag 5` compound header.
pub fn push_compound(out: &mut Vec<u8>, count: u16, type_id: u16) {
    let w = (u32::from(count) << 16) | u32::from(type_id);
    push_item(out, tag::COMPOUND, &w.to_be_bytes());
}

/// Append a `tag 6` item.
pub fn push_i64(out: &mut Vec<u8>, v: i64) {
    push_item(out, tag::INT64, &v.to_be_bytes());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_every_known_tag() {
        let mut b = Vec::new();
        push_i32(&mut b, -7);
        push_fixed16_16(&mut b, -65536);
        push_fixed22_42(&mut b, -(1i64 << 42));
        push_compound(&mut b, 3, 0x1f42);
        push_i64(&mut b, 50_376_465);
        push_item(&mut b, tag::STRING, &[0, 0, 0, 2, b'h', b'i']);
        push_item(&mut b, tag::BLOB, &[0, 0, 0, 1, 0xaa]);
        push_item(&mut b, tag::ARRAY, &[0, 0, 0, 1, 1, 2, 3, 4]);
        push_item(&mut b, tag::UINT32, &[0xff, 0, 0, 1]);
        push_item(
            &mut b,
            tag::ARRAY_FIXED_22_42,
            &[0, 0, 0, 1, 1, 2, 3, 4, 5, 6, 7, 8],
        );
        push_item(&mut b, 0x77, &[9]);
        let v: Vec<_> = Items::new(&b).map(|i| i.unwrap().decode()).collect();
        assert_eq!(
            v,
            vec![
                Value::Int32(-7),
                Value::Fixed16_16(-65536),
                Value::Fixed22_42(-(1i64 << 42)),
                Value::Compound {
                    count: 3,
                    type_id: 0x1f42
                },
                Value::Int64(50_376_465),
                Value::Str(b"hi"),
                Value::Blob(&[0xaa]),
                Value::Array {
                    count: 1,
                    elements: &[1, 2, 3, 4]
                },
                Value::Uint32(0xff00_0001),
                Value::ArrayFixed22_42 {
                    count: 1,
                    elements: &[1, 2, 3, 4, 5, 6, 7, 8]
                },
                Value::Unknown {
                    tag: 0x77,
                    value: &[9]
                },
            ]
        );
    }

    #[test]
    fn wrong_lengths_become_unknown() {
        // tag 2 with 3 bytes, string whose prefix disagrees, array whose count disagrees
        for (t, val) in [
            (tag::INT32, vec![0, 0, 1]),
            (tag::STRING, vec![0, 0, 0, 5, b'a']),
            (tag::ARRAY, vec![0, 0, 0, 2, 1, 2, 3, 4]),
            (tag::ARRAY_FIXED_22_42, vec![0, 0, 0, 1, 1, 2, 3, 4]),
            (tag::UINT32, vec![0; 8]),
            (tag::INT64, vec![0; 4]),
            (tag::COMPOUND, vec![0; 8]),
        ] {
            let mut b = Vec::new();
            push_item(&mut b, t, &val);
            let it = Items::new(&b).next().unwrap().unwrap();
            assert!(matches!(it.decode(), Value::Unknown { .. }), "tag {t}");
        }
    }

    #[test]
    fn truncation_is_reported_once() {
        let mut b = Vec::new();
        push_i32(&mut b, 1);
        push_i32(&mut b, 2);
        for cut in 1..b.len() {
            let mut it = Items::new(&b[..cut]);
            let first = it.next();
            if cut == 9 {
                // clean end after the first item
                assert!(first.unwrap().is_ok());
                assert!(it.next().is_none());
                continue;
            } else if cut > 9 {
                assert!(first.unwrap().is_ok());
                let second = it.next().unwrap();
                assert!(
                    matches!(second, Err(TlvError::Truncated { .. })),
                    "cut {cut}"
                );
            } else {
                assert!(matches!(first, Some(Err(TlvError::Truncated { .. }))));
            }
            assert!(it.next().is_none(), "iterator must stop after an error");
        }
    }

    #[test]
    fn huge_length_does_not_overflow() {
        let b = [0x02, 0xff, 0xff, 0xff, 0xff, 0, 0];
        let e = Items::new(&b).next().unwrap().unwrap_err();
        assert!(matches!(e, TlvError::Truncated { needed, .. } if needed >= 0xffff_ffff));
    }

    #[test]
    fn fixed_point_conversions() {
        assert_eq!(fixed16_16_to_f64(65536), 1.0);
        assert_eq!(fixed16_16_to_f64(-65536), -1.0);
        assert_eq!(fixed16_16_to_f64(0x0003_0000 + 0x8000), 3.5);
        assert_eq!(fixed22_42_to_f64(-(1i64 << 42)), -1.0);
        assert_eq!(fixed22_42_to_f64(1i64 << 47), 32.0);
    }
}
