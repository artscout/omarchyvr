//! The gaze for other processes: one small file in shared memory (`/dev/shm/omarchyvr-eye-gaze`), rewritten for every
//! sample by `omarchyvr eyed`. Readers: the SteamVR driver (CustomHeadsetOpenVR with the Linux patch, which hands the
//! gaze to SteamVR's eye tracking component and so to OpenXR `XR_EXT_eye_gaze_interaction`) and anything else that
//! wants the gaze without owning the eye chip. The C layout is `linux/crystal-super/omarchyvr_eye_gaze.h`.
//!
//! Layout (little endian, [`SIZE`] bytes), head frame of OpenXR (x right, y up, -z forward, metres):
//!
//! | off | type | |
//! |---|---|---|
//! | 0 | u64 | `seq_begin` |
//! | 8 | u32 | magic [`MAGIC`] (`OVEG`) |
//! | 12 | u32 | version [`VERSION`] |
//! | 16 | i64 | sample time, `CLOCK_MONOTONIC` ns (latency corrected) |
//! | 24 | i64 | time it was published, `CLOCK_MONOTONIC` ns |
//! | 32 | u32 | flags: [`LEFT`], [`RIGHT`], [`COMBINED`], [`LEFT_ORIGIN`], [`RIGHT_ORIGIN`] |
//! | 36 | f32 | IPD from the chip, mm (0 = unknown) |
//! | 40 | f32 x3 x2 | left origin, left direction |
//! | 64 | f32 x3 x2 | right origin, right direction |
//! | 88 | f32 x3 x2 | combined origin, combined direction |
//! | 112 | u64 | tracker sample number |
//! | 120 | u32 | tracker state (0 searching, 1 connecting, 2 streaming, ...) |
//! | 124 | u32 | reserved (0) |
//! | 128 | u64 | `seq_end` |
//!
//! Consistency without locks: the writer stores an odd `seq_begin`, then the body, then the new even value in
//! `seq_end`, then in `seq_begin`. A reader loads `seq_end`, the body, then `seq_begin`, and keeps the body only when
//! both are equal and even (else it retries). Each store is its own `pwrite`, so they reach readers in this order.
use crate::track::Sample;
use std::fs::File;
use std::io;
use std::os::unix::fs::FileExt;
use std::path::{Path, PathBuf};

/// `OVEG` as a little-endian u32.
pub const MAGIC: u32 = u32::from_le_bytes(*b"OVEG");
/// Layout version.
pub const VERSION: u32 = 1;
/// Size of the file.
pub const SIZE: usize = 136;
/// Flag: left direction valid.
pub const LEFT: u32 = 1;
/// Flag: right direction valid.
pub const RIGHT: u32 = 2;
/// Flag: combined direction valid.
pub const COMBINED: u32 = 4;
/// Flag: left origin valid.
pub const LEFT_ORIGIN: u32 = 8;
/// Flag: right origin valid.
pub const RIGHT_ORIGIN: u32 = 16;

/// The default file.
pub fn default_path() -> PathBuf {
    PathBuf::from("/dev/shm/omarchyvr-eye-gaze")
}

/// What a reader gets.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Gaze {
    pub t_ns: i64,
    pub written_ns: i64,
    pub flags: u32,
    pub ipd_mm: f32,
    /// left, right, combined
    pub origin: [[f32; 3]; 3],
    pub direction: [[f32; 3]; 3],
    pub sample: u64,
    pub state: u32,
}

/// The body (bytes 8..128) of a sample.
pub fn body(s: Option<&Sample>, state: u32, written_ns: i64) -> [u8; 120] {
    let mut b = [0u8; 120];
    let mut put = |off: usize, v: &[u8]| b[off - 8..off - 8 + v.len()].copy_from_slice(v);
    put(8, &MAGIC.to_le_bytes());
    put(12, &VERSION.to_le_bytes());
    put(24, &written_ns.to_le_bytes());
    put(120, &state.to_le_bytes());
    let Some(s) = s else {
        return b;
    };
    put(16, &s.host_ts_ns.to_le_bytes());
    let mut flags = 0;
    let v3 = |v: [f32; 3]| {
        let mut o = [0u8; 12];
        for (i, x) in v.iter().enumerate() {
            o[i * 4..i * 4 + 4].copy_from_slice(&x.to_le_bytes());
        }
        o
    };
    for (e, (off, dbit, obit)) in [(40usize, LEFT, LEFT_ORIGIN), (64, RIGHT, RIGHT_ORIGIN)].into_iter().enumerate() {
        let r = &s.eyes[e];
        if r.origin_valid {
            flags |= obit;
            put(off, &v3(r.origin));
        }
        if r.direction_valid {
            flags |= dbit;
            put(off + 12, &v3(r.direction));
        }
    }
    if let Some(g) = &s.gaze {
        flags |= COMBINED;
        put(88, &v3(g.origin));
        put(100, &v3(g.direction));
    }
    put(32, &flags.to_le_bytes());
    put(36, &s.ipd_mm.max(0.0).to_le_bytes());
    put(112, &s.seq.to_le_bytes());
    b
}

/// Decode a consistent snapshot (the bytes of the whole file); `None` when torn, empty or of another layout.
pub fn decode(f: &[u8]) -> Option<Gaze> {
    if f.len() < SIZE {
        return None;
    }
    let u64_at = |o: usize| u64::from_le_bytes(f[o..o + 8].try_into().unwrap());
    let u32_at = |o: usize| u32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let f32_at = |o: usize| f32::from_le_bytes(f[o..o + 4].try_into().unwrap());
    let (b, e) = (u64_at(0), u64_at(128));
    if b != e || b % 2 != 0 || b == 0 || u32_at(8) != MAGIC || u32_at(12) != VERSION {
        return None;
    }
    let v3 = |o: usize| [f32_at(o), f32_at(o + 4), f32_at(o + 8)];
    Some(Gaze {
        t_ns: u64_at(16) as i64,
        written_ns: u64_at(24) as i64,
        flags: u32_at(32),
        ipd_mm: f32_at(36),
        origin: [v3(40), v3(64), v3(88)],
        direction: [v3(52), v3(76), v3(100)],
        sample: u64_at(112),
        state: u32_at(120),
    })
}

/// The writer.
pub struct Publisher {
    file: File,
    seq: u64,
}

impl Publisher {
    /// Create (or take over) the file, mode 0600.
    pub fn create(path: &Path) -> io::Result<Publisher> {
        use std::os::unix::fs::OpenOptionsExt;
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(path)?;
        file.set_len(SIZE as u64)?;
        // continue after what is there, so a reader never sees the same sequence number for other data
        let mut old = [0u8; 8];
        let _ = file.read_exact_at(&mut old, 128);
        let seq = (u64::from_le_bytes(old) + 2) & !1;
        Ok(Publisher { file, seq })
    }

    /// Publish one state (`sample` None: no gaze right now).
    pub fn publish(&mut self, sample: Option<&Sample>, state: u32, written_ns: i64) -> io::Result<()> {
        let b = body(sample, state, written_ns);
        self.file.write_all_at(&(self.seq + 1).to_le_bytes(), 0)?;
        self.file.write_all_at(&b, 8)?;
        self.seq += 2;
        self.file.write_all_at(&self.seq.to_le_bytes(), 128)?;
        self.file.write_all_at(&self.seq.to_le_bytes(), 0)
    }
}

/// Read the file the way readers must (`seq_end`, body, `seq_begin`; a few retries when torn).
pub fn read(path: &Path) -> Option<Gaze> {
    let file = File::open(path).ok()?;
    for _ in 0..8 {
        let mut f = [0u8; SIZE];
        file.read_exact_at(&mut f[128..136], 128).ok()?;
        file.read_exact_at(&mut f[8..128], 8).ok()?;
        file.read_exact_at(&mut f[0..8], 0).ok()?;
        if let Some(g) = decode(&f) {
            return Some(g);
        }
        std::thread::yield_now();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{EyeRay, Gaze as TGaze};

    fn sample() -> Sample {
        let mut s = Sample { seq: 42, host_ts_ns: 123_456_789, ipd_mm: 64.5, ..Sample::default() };
        s.eyes[0] = EyeRay {
            origin: [-0.032, 0.0, 0.0],
            direction: [0.1, 0.05, -0.99],
            origin_valid: true,
            direction_valid: true,
            ..EyeRay::default()
        };
        s.eyes[1] = EyeRay { direction: [0.08, 0.05, -0.99], direction_valid: true, ..EyeRay::default() };
        s.gaze = Some(TGaze { origin: [0.0, 0.0, 0.0], direction: [0.09, 0.05, -0.99], eyes: 3 });
        s
    }

    #[test]
    fn publish_and_read_back() {
        let dir = std::env::temp_dir().join(format!("pimax-eye-shm-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("gaze");
        let mut w = Publisher::create(&p).unwrap();
        assert!(read(&p).is_none(), "nothing published yet");
        w.publish(Some(&sample()), 2, 999).unwrap();
        let g = read(&p).unwrap();
        assert_eq!(g.t_ns, 123_456_789);
        assert_eq!(g.written_ns, 999);
        assert_eq!(g.flags, LEFT | RIGHT | COMBINED | LEFT_ORIGIN);
        assert_eq!(g.direction[0], [0.1, 0.05, -0.99]);
        assert_eq!(g.direction[1], [0.08, 0.05, -0.99]);
        assert_eq!(g.origin[0], [-0.032, 0.0, 0.0]);
        assert_eq!(g.ipd_mm, 64.5);
        assert_eq!((g.sample, g.state), (42, 2));
        // no gaze: flags clear, state kept
        w.publish(None, 1, 1000).unwrap();
        let g = read(&p).unwrap();
        assert_eq!((g.flags, g.state), (0, 1));
        // a new writer continues the sequence
        let before = u64::from_le_bytes(std::fs::read(&p).unwrap()[0..8].try_into().unwrap());
        let mut w2 = Publisher::create(&p).unwrap();
        w2.publish(Some(&sample()), 2, 1).unwrap();
        let after = u64::from_le_bytes(std::fs::read(&p).unwrap()[0..8].try_into().unwrap());
        assert!(after > before && after % 2 == 0);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn torn_snapshots_are_refused() {
        let mut f = [0u8; SIZE];
        f[8..128].copy_from_slice(&body(Some(&sample()), 2, 5));
        f[0..8].copy_from_slice(&4u64.to_le_bytes());
        f[128..136].copy_from_slice(&4u64.to_le_bytes());
        assert!(decode(&f).is_some());
        f[0..8].copy_from_slice(&5u64.to_le_bytes()); // writer in the middle
        assert!(decode(&f).is_none());
        f[0..8].copy_from_slice(&6u64.to_le_bytes()); // begin newer than end
        assert!(decode(&f).is_none());
        f[0..8].copy_from_slice(&4u64.to_le_bytes());
        f[8] ^= 1; // another layout
        assert!(decode(&f).is_none());
    }
}
