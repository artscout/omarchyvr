//! The capture pipeline used by the C ABI and the CLI: source -> metadata -> tile rows -> timestamps -> stats,
//! with optional raw recording of every frame.

use crate::clock::HostClockMap;
use crate::frame::{self, FrameKind, Meta, RowSource, TileRows, TILE_COUNT, TILE_H, TILE_W};
use crate::record::AsyncRecorder;
use crate::source::Source;
use std::io;
use std::path::Path;

/// Frame info flag: metadata parsed and plausible.
pub const FLAG_META_VALID: u32 = 1;
/// Frame info flag: at least one tile row was found by scanning (metadata offset unusable).
pub const FLAG_ROWS_SCANNED: u32 = 2;
/// Frame info flag: frame comes from a recording.
pub const FLAG_REPLAY: u32 = 4;

/// Per-frame result (also the C struct `pimax_cam_frame_info`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FrameInfo {
    /// Host `CLOCK_MONOTONIC` ns when the driver saw the frame (V4L2 timestamp; rebased for replays).
    pub host_ts_ns: u64,
    /// Device timestamp (MCU µs, IMU clock).
    pub dev_ts_us: u64,
    /// Device time mapped to `CLOCK_MONOTONIC` by the minimum-delay filter (fallback without IMU reference).
    pub mono_ts_ns: u64,
    /// Metadata frame counter.
    pub counter: u32,
    /// Driver sequence.
    pub sequence: u32,
    /// Frames missing just before this one (counter gap minus one).
    pub dropped: u32,
    /// [`FrameKind`] as u32.
    pub kind: u32,
    /// Raw `frame_type` byte.
    pub frame_type: u32,
    /// Bit-per-byte stream index.
    pub stream_index: u32,
    /// Exposure per camera.
    pub exposure: [u16; 4],
    /// Gain per camera.
    pub gain: [u16; 4],
    /// Tile start rows actually used.
    pub y_offset: [u16; 4],
    /// 640.
    pub tile_width: u32,
    /// 480.
    pub tile_height: u32,
    /// `FLAG_*`.
    pub flags: u32,
    /// Reserved, zero.
    pub reserved: u32,
}

/// Running counters.
#[derive(Clone, Debug, Default)]
pub struct Stats {
    /// Frames delivered.
    pub frames: u64,
    /// Tracking-exposure frames.
    pub tracking: u64,
    /// LED-exposure frames.
    pub led: u64,
    /// Frames with unrecognised metadata.
    pub unknown: u64,
    /// Sum of counter gaps (lost frames).
    pub dropped: u64,
    /// Frames whose tile rows had to be scanned.
    pub scanned_rows: u64,
    /// Frames where a metadata row offset did not match the zero/non-zero boundary (diagnostic).
    pub row_mismatch: u64,
    /// Frames whose kind did not alternate (tracking after tracking or LED after LED).
    pub kind_repeats: u64,
    /// Frames the recorder could not queue (disk too slow).
    pub record_dropped: u64,
    /// Times the source came back from a reopened camera or a new broker (clock and counter tracking restarted).
    pub reopens: u64,
}

/// Capture pipeline.
pub struct Camera {
    src: Box<dyn Source>,
    clock: HostClockMap,
    recorder: Option<AsyncRecorder>,
    rows: Option<TileRows>,
    info: FrameInfo,
    last_counter: Option<u32>,
    last_kind: FrameKind,
    generation: u64,
    /// Check every frame's row offsets against the picture (costs ~4 row scans per frame).
    pub verify_rows: bool,
    /// Counters.
    pub stats: Stats,
}

impl Camera {
    /// Wrap a source.
    pub fn new(src: Box<dyn Source>) -> Camera {
        let generation = src.generation();
        Camera {
            src,
            clock: HostClockMap::default(),
            recorder: None,
            rows: None,
            info: FrameInfo::default(),
            last_counter: None,
            last_kind: FrameKind::Unknown,
            generation,
            verify_rows: true,
            stats: Stats::default(),
        }
    }

    /// Open by spec (see [`crate::source::open`]).
    pub fn open(spec: &str, fps: u32, replay: crate::source::ReplayOptions) -> io::Result<Camera> {
        Ok(Camera::new(crate::source::open(spec, fps, replay)?))
    }

    /// Source description.
    pub fn describe(&self) -> String {
        self.src.describe()
    }

    /// The underlying source.
    pub fn source(&self) -> &dyn Source {
        self.src.as_ref()
    }

    /// Start recording every following frame to `path` (`zlib`: per-frame compression level, None = raw).
    pub fn start_recording(&mut self, path: &Path, zlib: Option<u8>) -> io::Result<()> {
        self.recorder = Some(AsyncRecorder::start(path, frame::FRAME_W, zlib, 64)?);
        Ok(())
    }

    /// Stop recording; returns (frames, bytes).
    pub fn stop_recording(&mut self) -> io::Result<Option<(u64, u64)>> {
        match self.recorder.take() {
            Some(r) => {
                self.stats.record_dropped += r.dropped;
                r.finish().map(Some)
            }
            None => Ok(None),
        }
    }

    /// True while recording.
    pub fn recording(&self) -> bool {
        self.recorder.is_some()
    }

    /// Wait for the next frame. `Ok(None)`: timeout / end of a replay.
    pub fn next_frame(&mut self, timeout_ms: i32) -> io::Result<Option<FrameInfo>> {
        if !self.src.advance(timeout_ms)? {
            return Ok(None);
        }
        let generation = self.src.generation();
        if generation != self.generation {
            // reopened camera or new broker: the device clock and the frame counter may have restarted
            self.generation = generation;
            self.clock = HostClockMap::default();
            self.last_counter = None;
            self.last_kind = FrameKind::Unknown;
            self.stats.reopens += 1;
        }
        let replay = self.src.is_replay();
        let raw = self
            .src
            .current()
            .ok_or_else(|| io::Error::other("source has no current frame"))?;
        let stride = frame::check_raw(raw.data.len(), raw.stride).map_err(io::Error::other)?;
        if let Some(rec) = self.recorder.as_mut() {
            rec.push(&raw);
        }
        let meta = Meta::parse(&raw.data[..frame::META_LEN]).filter(|m| m.looks_valid());
        let rows = frame::tile_rows(meta.as_ref(), raw.data, stride);
        let mut flags = if replay { FLAG_REPLAY } else { 0 };
        let st = &mut self.stats;
        st.frames += 1;
        if rows.source == RowSource::Scan {
            flags |= FLAG_ROWS_SCANNED;
            st.scanned_rows += 1;
        } else if self.verify_rows
            && (0..TILE_COUNT)
                .any(|t| !frame::tile_start_consistent(raw.data, stride, t, rows.y[t]))
        {
            st.row_mismatch += 1;
        }
        let m = meta.unwrap_or_default();
        let kind = if meta.is_some() {
            flags |= FLAG_META_VALID;
            m.kind()
        } else {
            FrameKind::Unknown
        };
        match kind {
            FrameKind::Tracking => st.tracking += 1,
            FrameKind::Led => st.led += 1,
            FrameKind::Unknown => st.unknown += 1,
        }
        if kind != FrameKind::Unknown && kind == self.last_kind {
            st.kind_repeats += 1;
        }
        self.last_kind = kind;
        let mut dropped = 0;
        if meta.is_some() {
            if let Some(prev) = self.last_counter {
                let gap = m.counter.wrapping_sub(prev);
                // a jump backwards or by more than 10 s worth of frames is a restart, not a loss
                if (1..=500).contains(&gap) {
                    dropped = gap - 1;
                }
            }
            self.last_counter = Some(m.counter);
        }
        st.dropped += dropped as u64;
        let mono = if meta.is_some() {
            self.clock.map(m.ts_us, raw.host_ts_ns)
        } else {
            raw.host_ts_ns
        };
        self.rows = Some(rows);
        self.info = FrameInfo {
            host_ts_ns: raw.host_ts_ns,
            dev_ts_us: m.ts_us,
            mono_ts_ns: mono,
            counter: m.counter,
            sequence: raw.sequence,
            dropped,
            kind: kind as u32,
            frame_type: m.frame_type as u32,
            stream_index: m.stream_index as u32,
            exposure: m.exposure,
            gain: m.gain,
            y_offset: rows.y.map(|y| y as u16),
            tile_width: TILE_W as u32,
            tile_height: TILE_H as u32,
            flags,
            reserved: 0,
        };
        Ok(Some(self.info))
    }

    /// Info of the current frame.
    pub fn info(&self) -> &FrameInfo {
        &self.info
    }

    /// Copy tile `t` of the current frame into `dst` (row pitch `dst_stride` >= 640, 480 rows).
    pub fn copy_tile(&self, t: usize, dst: &mut [u8], dst_stride: usize) -> io::Result<()> {
        if t >= TILE_COUNT {
            return Err(io::Error::other("tile index out of range"));
        }
        if dst_stride < TILE_W || dst.len() < dst_stride * (TILE_H - 1) + TILE_W {
            return Err(io::Error::other("destination too small"));
        }
        let raw = self
            .src
            .current()
            .ok_or_else(|| io::Error::other("no current frame"))?;
        let rows = self
            .rows
            .ok_or_else(|| io::Error::other("no current frame"))?;
        frame::copy_tile(raw.data, raw.stride, t, rows.y[t], dst, dst_stride);
        Ok(())
    }

    /// Borrowed view of tile `t` of the current frame (no copy; for statistics such as auto-exposure).
    pub fn tile_view(&self, t: usize) -> io::Result<crate::ae::TileView<'_>> {
        if t >= TILE_COUNT {
            return Err(io::Error::other("tile index out of range"));
        }
        let raw = self
            .src
            .current()
            .ok_or_else(|| io::Error::other("no current frame"))?;
        let rows = self
            .rows
            .ok_or_else(|| io::Error::other("no current frame"))?;
        let need = (rows.y[t] + TILE_H - 1) * raw.stride + t * TILE_W + TILE_W;
        if raw.data.len() < need {
            return Err(io::Error::other("tile outside the frame"));
        }
        Ok(crate::ae::TileView {
            data: raw.data,
            stride: raw.stride,
            x0: t * TILE_W,
            y0: rows.y[t],
            w: TILE_W,
            h: TILE_H,
        })
    }

    /// Current tile as an owned 640x480 buffer.
    pub fn tile(&self, t: usize) -> io::Result<Vec<u8>> {
        let mut v = vec![0u8; TILE_W * TILE_H];
        self.copy_tile(t, &mut v, TILE_W)?;
        Ok(v)
    }

    /// The current raw frame bytes.
    pub fn raw(&self) -> Option<&[u8]> {
        self.src.current().map(|r| r.data)
    }

    /// Corrupt buffers reported by the driver.
    pub fn corrupt_frames(&self) -> u64 {
        self.src.corrupt_frames()
    }

    /// Current host-device clock offset of the fallback mapping.
    pub fn clock_offset_ns(&self) -> i64 {
        self.clock.offset_ns()
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::frame::{FRAME_BYTES, FRAME_W, META_LEN, META_MAGIC, META_ROWS};
    use crate::source::RawFrame;

    /// In-memory source for tests.
    pub(crate) struct VecSource {
        pub frames: Vec<(Vec<u8>, u64)>,
        pub pos: usize,
        pub cur: Option<usize>,
    }

    impl Source for VecSource {
        fn advance(&mut self, _t: i32) -> io::Result<bool> {
            if self.pos >= self.frames.len() {
                return Ok(false);
            }
            self.cur = Some(self.pos);
            self.pos += 1;
            Ok(true)
        }
        fn current(&self) -> Option<RawFrame<'_>> {
            self.cur.map(|i| RawFrame {
                data: &self.frames[i].0,
                stride: FRAME_W,
                host_ts_ns: self.frames[i].1,
                sequence: i as u32,
            })
        }
        fn describe(&self) -> String {
            "vec".into()
        }
    }

    pub(crate) fn synth(kind_bit: bool, counter: u32, ts_us: u64, off: u16) -> Vec<u8> {
        let mut f = vec![0u8; FRAME_BYTES];
        let mut meta = [0u8; META_LEN];
        meta[32] = if kind_bit { 0x86 } else { 0x06 };
        meta[33..41].copy_from_slice(&ts_us.to_le_bytes());
        meta[41..45].copy_from_slice(&counter.to_le_bytes());
        for i in 0..4 {
            meta[61 + 2 * i..63 + 2 * i].copy_from_slice(&off.to_le_bytes());
        }
        meta[92..96].copy_from_slice(&META_MAGIC);
        for r in 0..META_ROWS {
            f[r * FRAME_W..r * FRAME_W + META_LEN].copy_from_slice(&meta);
        }
        for y in 0..TILE_H {
            for x in 0..FRAME_W {
                f[(off as usize + y) * FRAME_W + x] =
                    1 + ((x / TILE_W) as u8) * 50 + (y % 40) as u8;
            }
        }
        f
    }

    #[test]
    fn pipeline_counts_kinds_drops_and_times() {
        let mut frames = Vec::new();
        let mut counter = 100u32;
        for i in 0..20u64 {
            if i == 10 {
                counter += 2; // two frames lost
            }
            let tracking = counter.is_multiple_of(2);
            let off = if tracking { 278 } else { 10 };
            frames.push((
                synth(tracking, counter, 5_000_000 + counter as u64 * 20_000, off),
                9_000_000_000 + counter as u64 * 20_000_000 + 11_000_000,
            ));
            counter += 1;
        }
        let mut cam = Camera::new(Box::new(VecSource {
            frames,
            pos: 0,
            cur: None,
        }));
        let mut infos = Vec::new();
        while let Some(i) = cam.next_frame(0).unwrap() {
            infos.push(i);
        }
        assert_eq!(infos.len(), 20);
        assert_eq!(cam.stats.dropped, 2);
        assert_eq!(infos[10].dropped, 2);
        assert_eq!(cam.stats.tracking + cam.stats.led, 20);
        assert_eq!(cam.stats.row_mismatch, 0);
        assert_eq!(cam.stats.scanned_rows, 0);
        assert_eq!(cam.stats.kind_repeats, 0); // losing two frames keeps the tracking/LED alternation
        let t = infos
            .iter()
            .find(|i| i.kind == FrameKind::Tracking as u32)
            .unwrap();
        assert_eq!(t.y_offset, [278; 4]);
        assert!(infos.iter().all(|i| i.flags & FLAG_META_VALID != 0));
        assert!(infos.windows(2).all(|w| w[1].mono_ts_ns > w[0].mono_ts_ns));
        // tiles come out with the right content
        let tile2 = cam.tile(2).unwrap();
        assert_eq!(tile2[0], 1 + 100);
        assert_eq!(tile2[5 * TILE_W + 7], 1 + 100 + 5);
        assert!(cam.copy_tile(4, &mut [0u8; 10], 640).is_err());
    }
}
