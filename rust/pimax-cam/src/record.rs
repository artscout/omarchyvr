//! Raw recording format (`.pxcam`) for offline tests and replay into Monado.
//!
//! ```text
//! file header, 32 bytes, little endian:
//!   0  [u8; 8]  magic "PXCAMRAW"
//!   8  u16      version = 1
//!  10  u16      header length = 32
//!  12  u16      width  = 2560 (grey bytes per image row)
//!  14  u16      height = 832
//!  16  u32      stride of the stored rows (2560)
//!  20  u32      flags (bit 0: records may be zlib-compressed)
//!  24  [u8; 8]  reserved
//! record, 32-byte header + payload:
//!   0  u32  magic "FRAM" (0x4d415246)
//!   4  u32  flags (bit 0: payload is zlib)
//!   8  u32  stored payload length
//!  12  u32  raw length (stride * height)
//!  16  u64  host CLOCK_MONOTONIC ns (V4L2 buffer timestamp)
//!  24  u32  V4L2 sequence
//!  28  u32  reserved
//! ```
//! The payload is the complete frame as delivered by the driver (metadata rows, zero rows and all four tiles),
//! so a replay goes through exactly the same parsing as live capture. A truncated last record (recording killed)
//! is reported as the end of the file.

use crate::frame::{FRAME_H, FRAME_W};
use crate::source::{RawFrame, ReplayOptions, Source};
use std::fs::File;
use std::io::{self, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

/// File magic.
pub const MAGIC: &[u8; 8] = b"PXCAMRAW";
/// Record magic ("FRAM").
pub const REC_MAGIC: u32 = 0x4d41_5246;
const HDR_LEN: usize = 32;
const REC_LEN: usize = 32;
/// Record flag: payload is zlib.
pub const REC_ZLIB: u32 = 1;

/// Writer.
pub struct Recorder {
    w: BufWriter<File>,
    compress: Option<u8>,
    stride: usize,
    /// Frames written.
    pub frames: u64,
    /// Bytes written including headers.
    pub bytes: u64,
    path: PathBuf,
}

impl Recorder {
    /// Create `path`. `compress_level`: `None` = raw, `Some(1..=9)` = zlib per frame.
    pub fn create(path: &Path, stride: usize, compress_level: Option<u8>) -> io::Result<Recorder> {
        let f = File::create(path)?;
        let mut w = BufWriter::with_capacity(4 << 20, f);
        let mut h = [0u8; HDR_LEN];
        h[..8].copy_from_slice(MAGIC);
        h[8..10].copy_from_slice(&1u16.to_le_bytes());
        h[10..12].copy_from_slice(&(HDR_LEN as u16).to_le_bytes());
        h[12..14].copy_from_slice(&(FRAME_W as u16).to_le_bytes());
        h[14..16].copy_from_slice(&(FRAME_H as u16).to_le_bytes());
        h[16..20].copy_from_slice(&(stride as u32).to_le_bytes());
        h[20..24].copy_from_slice(&(compress_level.is_some() as u32).to_le_bytes());
        w.write_all(&h)?;
        Ok(Recorder {
            w,
            compress: compress_level,
            stride,
            frames: 0,
            bytes: HDR_LEN as u64,
            path: path.to_path_buf(),
        })
    }

    /// Append one frame (must have the stride given at creation).
    pub fn write(&mut self, f: &RawFrame) -> io::Result<()> {
        if f.stride != self.stride {
            return Err(io::Error::other(format!(
                "stride {} != recording stride {}",
                f.stride, self.stride
            )));
        }
        let raw_len = self.stride * (FRAME_H - 1) + FRAME_W;
        if f.data.len() < raw_len {
            return Err(io::Error::other("short frame"));
        }
        let raw = &f.data[..raw_len];
        let (payload, flags): (std::borrow::Cow<[u8]>, u32) = match self.compress {
            Some(level) => (
                miniz_oxide::deflate::compress_to_vec_zlib(raw, level).into(),
                REC_ZLIB,
            ),
            None => (raw.into(), 0),
        };
        let mut h = [0u8; REC_LEN];
        h[0..4].copy_from_slice(&REC_MAGIC.to_le_bytes());
        h[4..8].copy_from_slice(&flags.to_le_bytes());
        h[8..12].copy_from_slice(&(payload.len() as u32).to_le_bytes());
        h[12..16].copy_from_slice(&(raw_len as u32).to_le_bytes());
        h[16..24].copy_from_slice(&f.host_ts_ns.to_le_bytes());
        h[24..28].copy_from_slice(&f.sequence.to_le_bytes());
        self.w.write_all(&h)?;
        self.w.write_all(&payload)?;
        self.frames += 1;
        self.bytes += (REC_LEN + payload.len()) as u64;
        Ok(())
    }

    /// Flush and close.
    pub fn finish(mut self) -> io::Result<(PathBuf, u64, u64)> {
        self.w.flush()?;
        self.w.get_ref().sync_data()?;
        Ok((self.path.clone(), self.frames, self.bytes))
    }
}

/// Recorder on its own thread: the capture loop only copies the frame into a bounded queue, so disk stalls
/// never block capture. Frames are dropped (and counted) when the queue is full.
pub struct AsyncRecorder {
    tx: Option<std::sync::mpsc::SyncSender<(Vec<u8>, u64, u32)>>,
    pool: std::sync::Arc<std::sync::Mutex<Vec<Vec<u8>>>>,
    handle: Option<std::thread::JoinHandle<io::Result<(u64, u64)>>>,
    stride: usize,
    /// Frames dropped because the queue was full.
    pub dropped: u64,
    /// Frames queued.
    pub queued: u64,
}

impl AsyncRecorder {
    /// Start a writer thread for `path` with a queue of `depth` frames.
    pub fn start(
        path: &Path,
        stride: usize,
        compress_level: Option<u8>,
        depth: usize,
    ) -> io::Result<AsyncRecorder> {
        let mut rec = Recorder::create(path, stride, compress_level)?;
        let (tx, rx) = std::sync::mpsc::sync_channel::<(Vec<u8>, u64, u32)>(depth.max(1));
        let pool = std::sync::Arc::new(std::sync::Mutex::new(Vec::<Vec<u8>>::new()));
        let pool2 = pool.clone();
        let handle = std::thread::Builder::new()
            .name("pimax-cam-rec".into())
            .spawn(move || {
                let mut err = None;
                for (data, ts, seq) in rx {
                    if err.is_none() {
                        if let Err(e) = rec.write(&RawFrame {
                            data: &data,
                            stride,
                            host_ts_ns: ts,
                            sequence: seq,
                        }) {
                            err = Some(e);
                        }
                    }
                    let mut p = pool2.lock().unwrap_or_else(|e| e.into_inner());
                    if p.len() < 8 {
                        p.push(data);
                    }
                }
                if let Some(e) = err {
                    return Err(e);
                }
                rec.finish().map(|(_, f, b)| (f, b))
            })?;
        Ok(AsyncRecorder {
            tx: Some(tx),
            pool,
            handle: Some(handle),
            stride,
            dropped: 0,
            queued: 0,
        })
    }

    /// Queue a copy of the frame (never blocks).
    pub fn push(&mut self, f: &RawFrame) {
        let Some(tx) = &self.tx else { return };
        let len = self.stride * (FRAME_H - 1) + FRAME_W;
        if f.data.len() < len || f.stride != self.stride {
            self.dropped += 1;
            return;
        }
        let mut buf = self
            .pool
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pop()
            .unwrap_or_default();
        buf.clear();
        buf.extend_from_slice(&f.data[..len]);
        match tx.try_send((buf, f.host_ts_ns, f.sequence)) {
            Ok(()) => self.queued += 1,
            Err(_) => self.dropped += 1,
        }
    }

    /// Flush, stop the thread; returns (frames written, bytes).
    pub fn finish(mut self) -> io::Result<(u64, u64)> {
        self.tx = None;
        match self.handle.take() {
            Some(h) => h
                .join()
                .map_err(|_| io::Error::other("recorder thread panicked"))?,
            None => Ok((0, 0)),
        }
    }
}

/// One frame read back.
#[derive(Clone, Debug)]
pub struct Record {
    /// Full raw frame.
    pub data: Vec<u8>,
    /// Row stride.
    pub stride: usize,
    /// Recorded host timestamp.
    pub host_ts_ns: u64,
    /// Recorded sequence.
    pub sequence: u32,
}

/// Sequential reader.
pub struct Reader {
    r: BufReader<File>,
    /// Stored row stride.
    pub stride: usize,
    /// File-level flags.
    pub flags: u32,
    /// True if the last read hit a truncated record.
    pub truncated: bool,
}

fn rd_u32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

impl Reader {
    /// Open and validate the header.
    pub fn open(path: &Path) -> io::Result<Reader> {
        let mut r = BufReader::with_capacity(4 << 20, File::open(path)?);
        let mut h = [0u8; HDR_LEN];
        r.read_exact(&mut h)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "not a .pxcam file (short)"))?;
        if &h[..8] != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "not a .pxcam file (magic)",
            ));
        }
        let version = u16::from_le_bytes([h[8], h[9]]);
        let hlen = u16::from_le_bytes([h[10], h[11]]) as usize;
        let (w, hh) = (
            u16::from_le_bytes([h[12], h[13]]) as usize,
            u16::from_le_bytes([h[14], h[15]]) as usize,
        );
        let stride = rd_u32(&h, 16) as usize;
        if version != 1 || hlen < HDR_LEN || w != FRAME_W || hh != FRAME_H || stride < FRAME_W {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!(
                    "unsupported .pxcam: version {version} header {hlen} {w}x{hh} stride {stride}"
                ),
            ));
        }
        if hlen > HDR_LEN {
            r.seek(SeekFrom::Start(hlen as u64))?;
        }
        Ok(Reader {
            r,
            stride,
            flags: rd_u32(&h, 20),
            truncated: false,
        })
    }

    /// Rewind to the first record.
    pub fn rewind(&mut self) -> io::Result<()> {
        self.r.seek(SeekFrom::Start(HDR_LEN as u64))?;
        self.truncated = false;
        Ok(())
    }

    /// Next record or `None` at the end (also for a truncated tail, see [`Reader::truncated`]).
    pub fn next_record(&mut self) -> io::Result<Option<Record>> {
        let mut h = [0u8; REC_LEN];
        match read_full(&mut self.r, &mut h)? {
            0 => return Ok(None),
            n if n < REC_LEN => {
                self.truncated = true;
                return Ok(None);
            }
            _ => {}
        }
        if rd_u32(&h, 0) != REC_MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "bad record magic",
            ));
        }
        let flags = rd_u32(&h, 4);
        let stored = rd_u32(&h, 8) as usize;
        let raw_len = rd_u32(&h, 12) as usize;
        if raw_len != self.stride * (FRAME_H - 1) + FRAME_W || stored > raw_len + raw_len / 2 + 1024
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("bad record lengths {stored}/{raw_len}"),
            ));
        }
        let mut payload = vec![0u8; stored];
        if read_full(&mut self.r, &mut payload)? < stored {
            self.truncated = true;
            return Ok(None);
        }
        let data = if flags & REC_ZLIB != 0 {
            let d = miniz_oxide::inflate::decompress_to_vec_zlib_with_limit(&payload, raw_len)
                .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("zlib: {e:?}")))?;
            if d.len() != raw_len {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "zlib length mismatch",
                ));
            }
            d
        } else {
            payload
        };
        Ok(Some(Record {
            data,
            stride: self.stride,
            host_ts_ns: u64::from_le_bytes(h[16..24].try_into().unwrap()),
            sequence: rd_u32(&h, 24),
        }))
    }
}

fn read_full(r: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// A recording played back as a [`Source`]. Host timestamps are rebased onto the current monotonic clock.
pub struct Replay {
    reader: Reader,
    opts: ReplayOptions,
    path: PathBuf,
    cur: Option<Record>,
    base_rec_ns: Option<u64>,
    base_mono_ns: u64,
    last_out_ns: u64,
    loop_offset_ns: u64,
    done: bool,
    /// Number of times the file was restarted.
    pub loops: u32,
}

impl Replay {
    /// Open a recording.
    pub fn open(path: &Path, opts: ReplayOptions) -> io::Result<Replay> {
        Ok(Replay {
            reader: Reader::open(path)?,
            opts,
            path: path.to_path_buf(),
            cur: None,
            base_rec_ns: None,
            base_mono_ns: 0,
            last_out_ns: 0,
            loop_offset_ns: 0,
            done: false,
            loops: 0,
        })
    }
}

impl Source for Replay {
    fn advance(&mut self, timeout_ms: i32) -> io::Result<bool> {
        if self.done {
            return Ok(false);
        }
        let rec = match self.reader.next_record()? {
            Some(r) => r,
            None if self.opts.looping => {
                self.reader.rewind()?;
                self.loops += 1;
                // continue the timeline one frame period after the last frame
                self.loop_offset_ns =
                    self.last_out_ns.saturating_sub(self.base_mono_ns) + 20_000_000;
                self.base_rec_ns = None;
                match self.reader.next_record()? {
                    Some(r) => r,
                    None => {
                        self.done = true;
                        return Ok(false);
                    }
                }
            }
            None => {
                self.done = true;
                return Ok(false);
            }
        };
        let now = crate::clock::monotonic_ns();
        let base_rec = match self.base_rec_ns {
            Some(b) => b,
            None => {
                if self.loops == 0 {
                    self.base_mono_ns = now;
                }
                self.base_rec_ns = Some(rec.host_ts_ns);
                rec.host_ts_ns
            }
        };
        let mut out =
            self.base_mono_ns + rec.host_ts_ns.saturating_sub(base_rec) + self.loop_offset_ns;
        if self.opts.realtime && out > now {
            let max_wait = (timeout_ms.max(0) as u64).max(1000) * 1_000_000;
            std::thread::sleep(std::time::Duration::from_nanos((out - now).min(max_wait)));
        }
        if out <= self.last_out_ns {
            out = self.last_out_ns + 1;
        }
        self.last_out_ns = out;
        self.cur = Some(Record {
            host_ts_ns: out,
            ..rec
        });
        Ok(true)
    }

    fn current(&self) -> Option<RawFrame<'_>> {
        self.cur.as_ref().map(|r| RawFrame {
            data: &r.data,
            stride: r.stride,
            host_ts_ns: r.host_ts_ns,
            sequence: r.sequence,
        })
    }

    fn describe(&self) -> String {
        format!(
            "replay {} (loop {}, realtime {}, zlib {})",
            self.path.display(),
            self.opts.looping,
            self.opts.realtime,
            self.reader.flags & 1 != 0
        )
    }

    fn finished(&self) -> bool {
        self.done
    }

    fn is_replay(&self) -> bool {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::FRAME_BYTES;

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-cam-test-{}-{name}", std::process::id()));
        let _ = std::fs::remove_file(&d);
        d
    }

    fn frame(seed: u8) -> Vec<u8> {
        let mut v = vec![0u8; FRAME_BYTES];
        for (i, b) in v.iter_mut().enumerate().step_by(97) {
            *b = (i as u8).wrapping_mul(seed) | 1;
        }
        v
    }

    #[test]
    fn roundtrip_raw_and_zlib() {
        for level in [None, Some(1u8)] {
            let p = tmp(&format!("rt{level:?}"));
            let mut rec = Recorder::create(&p, FRAME_W, level).unwrap();
            let frames: Vec<Vec<u8>> = (1..4).map(frame).collect();
            for (i, f) in frames.iter().enumerate() {
                rec.write(&RawFrame {
                    data: f,
                    stride: FRAME_W,
                    host_ts_ns: 1000 + i as u64 * 20_000_000,
                    sequence: i as u32,
                })
                .unwrap();
            }
            let (_, n, bytes) = rec.finish().unwrap();
            assert_eq!(n, 3);
            assert_eq!(bytes, std::fs::metadata(&p).unwrap().len());
            if level.is_some() {
                assert!(bytes < 3 * FRAME_BYTES as u64 / 4, "compressed {bytes}");
            }
            let mut r = Reader::open(&p).unwrap();
            for (i, f) in frames.iter().enumerate() {
                let x = r.next_record().unwrap().unwrap();
                assert_eq!(&x.data, f);
                assert_eq!(x.sequence, i as u32);
                assert_eq!(x.host_ts_ns, 1000 + i as u64 * 20_000_000);
            }
            assert!(r.next_record().unwrap().is_none());
            assert!(!r.truncated);
            std::fs::remove_file(&p).unwrap();
        }
    }

    #[test]
    fn async_recorder_writes_everything() {
        let p = tmp("async");
        let mut rec = AsyncRecorder::start(&p, FRAME_W, None, 16).unwrap();
        let f = frame(5);
        for i in 0..10u32 {
            rec.push(&RawFrame {
                data: &f,
                stride: FRAME_W,
                host_ts_ns: i as u64,
                sequence: i,
            });
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(rec.dropped, 0);
        let (n, _) = rec.finish().unwrap();
        assert_eq!(n, 10);
        let mut r = Reader::open(&p).unwrap();
        for i in 0..10u32 {
            assert_eq!(r.next_record().unwrap().unwrap().sequence, i);
        }
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn truncated_tail_and_garbage() {
        let p = tmp("trunc");
        let mut rec = Recorder::create(&p, FRAME_W, None).unwrap();
        let f = frame(7);
        for i in 0..2 {
            rec.write(&RawFrame {
                data: &f,
                stride: FRAME_W,
                host_ts_ns: i,
                sequence: i as u32,
            })
            .unwrap();
        }
        rec.finish().unwrap();
        let len = std::fs::metadata(&p).unwrap().len();
        let file = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
        file.set_len(len - 1000).unwrap();
        let mut r = Reader::open(&p).unwrap();
        assert!(r.next_record().unwrap().is_some());
        assert!(r.next_record().unwrap().is_none());
        assert!(r.truncated);
        std::fs::write(&p, b"PXCAMRAWgarbage").unwrap();
        assert!(Reader::open(&p).is_err());
        std::fs::write(&p, b"nope").unwrap();
        assert!(Reader::open(&p).is_err());
        std::fs::remove_file(&p).unwrap();
    }

    #[test]
    fn replay_rebases_and_loops() {
        let p = tmp("loop");
        let mut rec = Recorder::create(&p, FRAME_W, Some(1)).unwrap();
        let f = frame(3);
        for i in 0..3u64 {
            rec.write(&RawFrame {
                data: &f,
                stride: FRAME_W,
                host_ts_ns: 7_000_000_000 + i * 20_000_000,
                sequence: i as u32,
            })
            .unwrap();
        }
        rec.finish().unwrap();
        let mut rp = Replay::open(
            &p,
            ReplayOptions {
                looping: true,
                realtime: false,
            },
        )
        .unwrap();
        let mut ts = Vec::new();
        for _ in 0..7 {
            let fr = rp.next(0).unwrap().unwrap();
            assert_eq!(fr.data.len(), FRAME_BYTES);
            ts.push(fr.host_ts_ns);
        }
        assert!(ts.windows(2).all(|w| w[1] > w[0]), "{ts:?}");
        assert_eq!(ts[1] - ts[0], 20_000_000);
        assert_eq!(rp.loops, 2);
        let mut once = Replay::open(&p, ReplayOptions::default()).unwrap();
        for _ in 0..3 {
            assert!(once.next(0).unwrap().is_some());
        }
        assert!(once.next(0).unwrap().is_none());
        assert!(once.finished());
        std::fs::remove_file(&p).unwrap();
    }
}
