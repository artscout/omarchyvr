//! Frame layout and metadata of the Dream Air camera stream (UVC `34a4:0014`, `/dev/video0`).
//!
//! The device advertises `YUYV 1280x832` (bytesperline 2560, 2 129 920 B per frame) but every byte is one
//! 8-bit grey pixel: the picture is **2560x832 grey**, four 640-px tiles side by side (factory camera ids 0..3,
//! left to right). Rows 0..7 carry a 96-byte metadata block. Each tile's full 640x480 OV7251 image starts at its
//! own row, `y_offset`, which the metadata reports (Linux captures: 278 for tracking frames, 10 for LED frames;
//! the readout position depends on the exposure, like the Rift S). Every other row is zero.
//!
//! Frames alternate between two exposure sets (byte 32): `0x86` = tracking/SLAM exposure (usable for hands and
//! SLAM) and `0x06` = short controller-LED exposure (almost black, LED blobs only). At 50 fps each kind runs at
//! 25 Hz. This is the same scheme as the Rift S (`rift_s_camera.c`: frame_type 0x86/0x06, u64 µs timestamp).

/// Width of the grey picture in pixels (= bytes per row).
pub const FRAME_W: usize = 2560;
/// Rows per frame.
pub const FRAME_H: usize = 832;
/// Bytes per raw frame (`sizeimage` of the UVC format).
pub const FRAME_BYTES: usize = FRAME_W * FRAME_H;
/// Number of camera tiles.
pub const TILE_COUNT: usize = 4;
/// Tile width (one camera).
pub const TILE_W: usize = 640;
/// Tile height (full OV7251 sensor height, matches the factory calibration size 640x480).
pub const TILE_H: usize = 480;
/// Rows that repeat the metadata block.
pub const META_ROWS: usize = 8;
/// Bytes of metadata parsed from row 0.
pub const META_LEN: usize = 96;
/// Lowest and highest legal tile start rows.
pub const Y_MIN: usize = META_ROWS;
/// Highest legal tile start row (the tile must fit in the frame).
pub const Y_MAX: usize = FRAME_H - TILE_H;
/// Constant bytes seen at offset 92..96 of the metadata on Linux.
pub const META_MAGIC: [u8; 4] = [0x3c, 0x3d, 0x3e, 0x3f];

/// Which exposure set a frame was taken with.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum FrameKind {
    /// Metadata missing or not recognised.
    #[default]
    Unknown = 0,
    /// `frame_type` bit 7 set (0x86): normal tracking exposure. Feed these to hand tracking / SLAM.
    Tracking = 1,
    /// `frame_type` bit 7 clear (0x06): short exposure for the controller LEDs.
    Led = 2,
}

impl FrameKind {
    /// Short lower-case name.
    pub fn name(self) -> &'static str {
        match self {
            FrameKind::Unknown => "unknown",
            FrameKind::Tracking => "tracking",
            FrameKind::Led => "led",
        }
    }
}

/// Parsed metadata block (row 0 of the frame).
///
/// | bytes | field | confidence |
/// |---|---|---|
/// | 0..12 | `stream_index`: one bit per byte (0/1), LSB first; counts frames since the stream started | M (5 frames) |
/// | 32 | `frame_type` 0x86 tracking / 0x06 LED | H |
/// | 33..41 | `ts_us` u64 LE, MCU microseconds (the IMU's u32 timestamp is its low 32 bits) | H (low 32), M (high) |
/// | 41..45 | `counter` u32 LE, +1 per frame, continuous across streams | H |
/// | 45..53 | `exposure[4]` u16 LE per camera | H |
/// | 53..61 | `gain[4]` u16 LE per camera | H |
/// | 61..69 | `y_offset[4]` u16 LE: first image row of each tile | H on Linux |
/// | 92..96 | `3c 3d 3e 3f` | constant |
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Meta {
    /// Bit-per-byte counter from bytes 0..12.
    pub stream_index: u16,
    /// Byte 32.
    pub frame_type: u8,
    /// Device timestamp in microseconds (MCU clock, same as the IMU).
    pub ts_us: u64,
    /// Frame counter.
    pub counter: u32,
    /// Exposure per camera (device units).
    pub exposure: [u16; 4],
    /// Gain per camera (device units).
    pub gain: [u16; 4],
    /// First row of each tile in the frame.
    pub y_offset: [u16; 4],
    /// Bytes 92..96 match [`META_MAGIC`].
    pub magic_ok: bool,
}

fn le16(b: &[u8], o: usize) -> u16 {
    u16::from_le_bytes([b[o], b[o + 1]])
}

impl Meta {
    /// Parse the metadata from the first row of a frame. `None` if the row is too short.
    pub fn parse(row: &[u8]) -> Option<Meta> {
        if row.len() < META_LEN {
            return None;
        }
        let mut stream_index = 0u16;
        for (i, b) in row[..12].iter().enumerate() {
            if *b != 0 {
                stream_index |= 1 << i;
            }
        }
        let arr = |o: usize| {
            [
                le16(row, o),
                le16(row, o + 2),
                le16(row, o + 4),
                le16(row, o + 6),
            ]
        };
        let mut ts = [0u8; 8];
        ts.copy_from_slice(&row[33..41]);
        Some(Meta {
            stream_index,
            frame_type: row[32],
            ts_us: u64::from_le_bytes(ts),
            counter: u32::from_le_bytes([row[41], row[42], row[43], row[44]]),
            exposure: arr(45),
            gain: arr(53),
            y_offset: arr(61),
            magic_ok: row[92..96] == META_MAGIC,
        })
    }

    /// Plausibility: either the constant tail is present or the frame type looks like 0x06/0x86.
    pub fn looks_valid(&self) -> bool {
        self.magic_ok || (self.frame_type & 0x7f) == 0x06
    }

    /// Exposure set of the frame.
    pub fn kind(&self) -> FrameKind {
        if !self.looks_valid() {
            FrameKind::Unknown
        } else if self.frame_type & 0x80 != 0 {
            FrameKind::Tracking
        } else {
            FrameKind::Led
        }
    }

    /// Low 32 bits of the device timestamp (what the IMU reports).
    pub fn ts_us_u32(&self) -> u32 {
        self.ts_us as u32
    }
}

/// How the tile start rows of a frame were determined.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowSource {
    /// All four from the metadata `y_offset`.
    Metadata,
    /// At least one tile was located by scanning for its first non-zero row.
    Scan,
}

/// Start row of each tile.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileRows {
    /// First image row of tile `i` in the frame.
    pub y: [usize; TILE_COUNT],
    /// Where the values came from.
    pub source: RowSource,
}

/// Row `y` of tile `t` (640 bytes).
pub fn tile_row(frame: &[u8], stride: usize, t: usize, y: usize) -> &[u8] {
    let o = y * stride + t * TILE_W;
    &frame[o..o + TILE_W]
}

/// First row >= [`Y_MIN`] of tile `t` holding any non-zero byte, clamped to [`Y_MAX`].
pub fn scan_tile_start(frame: &[u8], stride: usize, t: usize) -> Option<usize> {
    (Y_MIN..FRAME_H)
        .find(|&y| tile_row(frame, stride, t, y).iter().any(|&b| b != 0))
        .map(|y| y.min(Y_MAX))
}

/// Locate the four tiles: metadata `y_offset` when in range, else a scan for the first non-zero row
/// (falls back to [`Y_MIN`] for an all-zero tile).
pub fn tile_rows(meta: Option<&Meta>, frame: &[u8], stride: usize) -> TileRows {
    let mut y = [Y_MIN; TILE_COUNT];
    let mut source = RowSource::Metadata;
    for (t, yt) in y.iter_mut().enumerate() {
        let from_meta = meta
            .filter(|m| m.looks_valid())
            .map(|m| m.y_offset[t] as usize)
            .filter(|v| (Y_MIN..=Y_MAX).contains(v));
        *yt = match from_meta {
            Some(v) => v,
            None => {
                source = RowSource::Scan;
                scan_tile_start(frame, stride, t).unwrap_or(Y_MIN)
            }
        };
    }
    TileRows { y, source }
}

/// Diagnostics: does the picture of tile `t` really start at `y`? True when the row above (if it is not a
/// metadata row) is all zero and row `y` has data. Dark rows can be legitimately zero, so treat a `false` as a hint.
pub fn tile_start_consistent(frame: &[u8], stride: usize, t: usize, y: usize) -> bool {
    if !(Y_MIN..=Y_MAX).contains(&y) {
        return false;
    }
    let above_zero = y == Y_MIN || tile_row(frame, stride, t, y - 1).iter().all(|&b| b == 0);
    let here_data = tile_row(frame, stride, t, y).iter().any(|&b| b != 0);
    above_zero && here_data
}

/// Copy tile `t` (640x480 starting at row `y`) into `dst` with row pitch `dst_stride` (>= 640).
pub fn copy_tile(
    frame: &[u8],
    stride: usize,
    t: usize,
    y: usize,
    dst: &mut [u8],
    dst_stride: usize,
) {
    assert!(dst_stride >= TILE_W && dst.len() >= dst_stride * (TILE_H - 1) + TILE_W);
    for r in 0..TILE_H {
        dst[r * dst_stride..r * dst_stride + TILE_W].copy_from_slice(tile_row(
            frame,
            stride,
            t,
            y + r,
        ));
    }
}

/// Check that a raw buffer has the expected geometry; returns the row stride.
pub fn check_raw(len: usize, stride: usize) -> Result<usize, String> {
    if stride < FRAME_W {
        return Err(format!("row stride {stride} < {FRAME_W}"));
    }
    let need = stride * (FRAME_H - 1) + FRAME_W;
    if len < need {
        return Err(format!(
            "frame has {len} bytes, need {need} (stride {stride})"
        ));
    }
    Ok(stride)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real metadata rows (Linux capture 2026-10-13 19:59, frames 0 and 1 of raw_1280x832_yuyv.bin).
    pub(crate) const META_F0: &str = "00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
        00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 86 77 36 e5 a9 01 00 00 00 ec f9 04 00 aa 28 aa \
        28 aa 28 aa 28 c1 01 c1 01 c1 01 c1 01 16 01 16 01 16 01 16 01 00 00 00 00 00 00 00 00 00 00 00 \
        00 00 00 00 00 00 00 00 00 00 00 00 3c 3d 3e 3f";
    pub(crate) const META_F1: &str = "01 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 \
        00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 06 8b 84 e5 a9 01 00 00 00 ed f9 04 00 28 00 28 \
        00 28 00 28 00 3e 00 3e 00 3e 00 3e 00 0a 00 0a 00 0a 00 0a 00 00 00 00 00 00 00 00 00 00 00 00 \
        00 00 00 00 00 00 00 00 00 00 00 00 3c 3d 3e 3f";

    pub(crate) fn hex(s: &str) -> Vec<u8> {
        let s: String = s.chars().filter(|c| !c.is_whitespace()).collect();
        (0..s.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap())
            .collect()
    }

    #[test]
    fn real_metadata_tracking_frame() {
        let row = hex(META_F0);
        assert_eq!(row.len(), META_LEN);
        let m = Meta::parse(&row).unwrap();
        assert_eq!(m.kind(), FrameKind::Tracking);
        assert_eq!(m.frame_type, 0x86);
        assert_eq!(m.ts_us_u32(), 2_850_371_191);
        assert_eq!(m.ts_us, (1u64 << 32) + 2_850_371_191);
        assert_eq!(m.counter, 0x0004_f9ec);
        assert_eq!(m.counter as u16, 63980);
        assert_eq!(m.exposure, [0x28aa; 4]);
        assert_eq!(m.gain, [0x01c1; 4]);
        assert_eq!(m.y_offset, [278; 4]);
        assert_eq!(m.stream_index, 0);
        assert!(m.magic_ok);
    }

    #[test]
    fn real_metadata_led_frame() {
        let m = Meta::parse(&hex(META_F1)).unwrap();
        assert_eq!(m.kind(), FrameKind::Led);
        assert_eq!(m.ts_us_u32(), 2_850_391_179);
        assert_eq!(m.ts_us_u32() - 2_850_371_191, 19_988);
        assert_eq!(m.counter as u16, 63981);
        assert_eq!(m.exposure, [40; 4]);
        assert_eq!(m.gain, [62; 4]);
        assert_eq!(m.y_offset, [10; 4]);
        assert_eq!(m.stream_index, 1);
    }

    #[test]
    fn short_or_garbage_metadata() {
        assert!(Meta::parse(&[0u8; 95]).is_none());
        let m = Meta::parse(&[0u8; 96]).unwrap();
        assert_eq!(m.kind(), FrameKind::Unknown);
        let mut row = vec![0u8; 96];
        row[32] = 0x86; // frame type alone is enough
        assert_eq!(Meta::parse(&row).unwrap().kind(), FrameKind::Tracking);
    }

    /// Synthetic frame: metadata row block plus four tiles with per-tile offsets and a pattern.
    pub(crate) fn synth_frame(
        offsets: [u16; 4],
        kind_bit: bool,
        counter: u32,
        ts_us: u64,
    ) -> Vec<u8> {
        let mut f = vec![0u8; FRAME_BYTES];
        let mut meta = [0u8; META_LEN];
        meta[32] = if kind_bit { 0x86 } else { 0x06 };
        meta[33..41].copy_from_slice(&ts_us.to_le_bytes());
        meta[41..45].copy_from_slice(&counter.to_le_bytes());
        for i in 0..4 {
            meta[45 + 2 * i..47 + 2 * i].copy_from_slice(&(1000u16 + i as u16).to_le_bytes());
            meta[53 + 2 * i..55 + 2 * i].copy_from_slice(&(10u16 + i as u16).to_le_bytes());
            meta[61 + 2 * i..63 + 2 * i].copy_from_slice(&offsets[i].to_le_bytes());
        }
        meta[92..96].copy_from_slice(&META_MAGIC);
        for r in 0..META_ROWS {
            f[r * FRAME_W..r * FRAME_W + META_LEN].copy_from_slice(&meta);
        }
        for (t, &off) in offsets.iter().enumerate() {
            for y in 0..TILE_H {
                for x in 0..TILE_W {
                    // never zero, encodes tile, row and column
                    f[(off as usize + y) * FRAME_W + t * TILE_W + x] =
                        1 + ((t * 61 + y * 7 + x * 3) % 250) as u8;
                }
            }
        }
        f
    }

    #[test]
    fn split_uses_metadata_offsets() {
        let offs = [278, 10, 100, 352];
        let f = synth_frame(offs, true, 5, 123);
        let m = Meta::parse(&f[..META_LEN]).unwrap();
        let rows = tile_rows(Some(&m), &f, FRAME_W);
        assert_eq!(rows.source, RowSource::Metadata);
        assert_eq!(rows.y, [278, 10, 100, 352]);
        for t in 0..4 {
            assert!(tile_start_consistent(&f, FRAME_W, t, rows.y[t]));
            let mut dst = vec![0u8; 700 * TILE_H];
            copy_tile(&f, FRAME_W, t, rows.y[t], &mut dst, 700);
            for &(y, x) in &[(0usize, 0usize), (0, 639), (479, 0), (479, 639), (123, 456)] {
                assert_eq!(
                    dst[y * 700 + x],
                    1 + ((t * 61 + y * 7 + x * 3) % 250) as u8,
                    "tile {t} ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn split_falls_back_to_scan() {
        let f = synth_frame([278, 10, 100, 200], false, 5, 123);
        let mut bad = Meta::parse(&f[..META_LEN]).unwrap();
        bad.y_offset = [0, 9999, 100, 200];
        let rows = tile_rows(Some(&bad), &f, FRAME_W);
        assert_eq!(rows.source, RowSource::Scan);
        assert_eq!(rows.y, [278, 10, 100, 200]);
        let rows = tile_rows(None, &f, FRAME_W);
        assert_eq!(rows.y, [278, 10, 100, 200]);
        // all-zero tile
        let z = vec![0u8; FRAME_BYTES];
        assert_eq!(tile_rows(None, &z, FRAME_W).y, [Y_MIN; 4]);
    }

    #[test]
    fn stride_check() {
        assert!(check_raw(FRAME_BYTES, FRAME_W).is_ok());
        assert!(check_raw(FRAME_BYTES - 1, FRAME_W).is_err());
        assert!(check_raw(FRAME_BYTES, 100).is_err());
    }
}
