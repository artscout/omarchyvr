//! Live controller poses for Monado: a 256-byte memory-mapped file with a seqlock.
//!
//! Layout (little endian), default path `$XDG_RUNTIME_DIR/pimax-ctrltrack.pose`. The Monado reader is
//! `pimax_ctrltrack_shm.h` in the Pimax driver (keep both in sync).
//!
//! ```text
//!   0 u32 magic 0x54435850 ("PXCT")   4 u32 version (1)   8 u32 seq (odd while writing)   12 u32 writer pid
//!  16 + 96*hand (0 = left, 1 = right):
//!    +0 u64 frame_ns    CLOCK_MONOTONIC time of the LED frame the pose comes from (0 = never seen)
//!    +8 u64 publish_ns  CLOCK_MONOTONIC time it was written
//!   +16 f32 pos[3]      head (body) frame position of the controller (LED model origin), metres;
//!                       body = OpenXR head axes (x right, y up, z back), the frame of Monado's head pose
//!   +28 f32 quat[4]     body_from_aim orientation, x y z w
//!   +44 f32 rms_px      joint reprojection RMS
//!   +48 u32 leds        matched LEDs over all cameras
//!   +52 u32 cams        cameras with matches
//!   +56 f32 aim_from_imu[4]  rotation x y z w (controller IMU axes in the aim frame, for gyro deltas)
//! 208 heartbeat (added 2026-10-15, same version: older readers ignore it; 0 = an older writer):
//!   208 u64 beat_ns     CLOCK_MONOTONIC time of the writer's last loop pass (at least every 0.5 s while alive,
//!                       with or without frames or controllers): the liveness signal for pimaxd / Monado
//!   216 u64 led_frames  LED frames processed since start
//!   224 u32 source_up   1 = camera frames arriving, 0 = source reopening (broker or camera gone)
//!   228 u32 reopens     times the source came back (broker restart, camera reopen)
//! ```

use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, AtomicU32, Ordering};

pub const MAGIC: u32 = 0x5443_5850;
pub const VERSION: u32 = 1;
pub const SIZE: usize = 256;
pub const HAND_OFF: usize = 16;
pub const HAND_STRIDE: usize = 96;
pub const BEAT_OFF: usize = HAND_OFF + 2 * HAND_STRIDE;
const _: () = assert!(BEAT_OFF + 24 <= SIZE);

/// The writer's heartbeat (offset [`BEAT_OFF`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Beat {
    pub beat_ns: u64,
    pub led_frames: u64,
    pub source_up: bool,
    pub reopens: u32,
}

impl Beat {
    /// Age of the heartbeat at `now_ns`; `None` when the writer never wrote one (older writer).
    pub fn age_ns(&self, now_ns: u64) -> Option<u64> {
        (self.beat_ns != 0).then(|| now_ns.saturating_sub(self.beat_ns))
    }
}

/// One hand's published state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HandPose {
    pub frame_ns: u64,
    pub publish_ns: u64,
    pub pos: [f32; 3],
    pub quat: [f32; 4],
    pub rms_px: f32,
    pub leds: u32,
    pub cams: u32,
    pub aim_from_imu: [f32; 4],
}

pub fn default_path() -> PathBuf {
    let d = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    d.join("pimax-ctrltrack.pose")
}

pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: valid pointer to a timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

struct Map {
    p: *mut u8,
}
// SAFETY: the mapping is only touched through the seqlock protocol below.
unsafe impl Send for Map {}

impl Map {
    fn open(path: &Path, write: bool) -> io::Result<Map> {
        use std::os::unix::fs::OpenOptionsExt;
        let f = std::fs::OpenOptions::new()
            .read(true)
            .write(write)
            .create(write)
            .truncate(false)
            .mode(0o644)
            .open(path)?;
        if write {
            f.set_len(SIZE as u64)?;
        } else if f.metadata()?.len() < SIZE as u64 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "pose file too short",
            ));
        }
        use std::os::fd::AsRawFd;
        let prot = libc::PROT_READ | if write { libc::PROT_WRITE } else { 0 };
        // SAFETY: mapping a regular file of at least SIZE bytes.
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                SIZE,
                prot,
                libc::MAP_SHARED,
                f.as_raw_fd(),
                0,
            )
        };
        if p == libc::MAP_FAILED {
            return Err(io::Error::last_os_error());
        }
        Ok(Map { p: p as *mut u8 })
    }
    fn seq(&self) -> &AtomicU32 {
        // SAFETY: offset 8 is 4-byte aligned inside the mapping.
        unsafe { &*(self.p.add(8) as *const AtomicU32) }
    }
}

impl Drop for Map {
    fn drop(&mut self) {
        // SAFETY: unmapping what open() mapped.
        unsafe { libc::munmap(self.p as *mut libc::c_void, SIZE) };
    }
}

fn put(p: *mut u8, off: usize, b: &[u8]) {
    // SAFETY: callers stay inside SIZE.
    unsafe { std::ptr::copy_nonoverlapping(b.as_ptr(), p.add(off), b.len()) };
}

fn get<const N: usize>(p: *const u8, off: usize) -> [u8; N] {
    let mut b = [0u8; N];
    // SAFETY: callers stay inside SIZE.
    unsafe { std::ptr::copy_nonoverlapping(p.add(off), b.as_mut_ptr(), N) };
    b
}

/// Writer side (`pimax-ctrltrack live`).
pub struct PoseWriter {
    m: Map,
    hands: [HandPose; 2],
    beat: Beat,
}

impl PoseWriter {
    pub fn create(path: &Path) -> io::Result<PoseWriter> {
        let m = Map::open(path, true)?;
        put(m.p, 0, &MAGIC.to_le_bytes());
        put(m.p, 4, &VERSION.to_le_bytes());
        put(m.p, 12, &std::process::id().to_le_bytes());
        let w = PoseWriter {
            m,
            hands: [HandPose::default(); 2],
            beat: Beat::default(),
        };
        w.m.seq().store(0, Ordering::Release);
        w.flush();
        Ok(w)
    }

    /// Replace one hand (0 left, 1 right) and publish both.
    pub fn set(&mut self, hand: usize, h: HandPose) {
        self.hands[hand] = h;
        self.flush();
    }

    /// Update the heartbeat and publish.
    pub fn beat(&mut self, b: Beat) {
        self.beat = b;
        self.flush();
    }

    fn flush(&self) {
        let s = self.m.seq().load(Ordering::Relaxed);
        self.m.seq().store(s.wrapping_add(1) | 1, Ordering::Relaxed);
        fence(Ordering::Release);
        for (i, h) in self.hands.iter().enumerate() {
            let o = HAND_OFF + i * HAND_STRIDE;
            put(self.m.p, o, &h.frame_ns.to_le_bytes());
            put(self.m.p, o + 8, &h.publish_ns.to_le_bytes());
            for k in 0..3 {
                put(self.m.p, o + 16 + 4 * k, &h.pos[k].to_le_bytes());
            }
            for k in 0..4 {
                put(self.m.p, o + 28 + 4 * k, &h.quat[k].to_le_bytes());
                put(self.m.p, o + 56 + 4 * k, &h.aim_from_imu[k].to_le_bytes());
            }
            put(self.m.p, o + 44, &h.rms_px.to_le_bytes());
            put(self.m.p, o + 48, &h.leds.to_le_bytes());
            put(self.m.p, o + 52, &h.cams.to_le_bytes());
        }
        let b = &self.beat;
        put(self.m.p, BEAT_OFF, &b.beat_ns.to_le_bytes());
        put(self.m.p, BEAT_OFF + 8, &b.led_frames.to_le_bytes());
        put(self.m.p, BEAT_OFF + 16, &(b.source_up as u32).to_le_bytes());
        put(self.m.p, BEAT_OFF + 20, &b.reopens.to_le_bytes());
        fence(Ordering::Release);
        let s = self.m.seq().load(Ordering::Relaxed);
        self.m
            .seq()
            .store(s.wrapping_add(1) & !1, Ordering::Release);
    }
}

/// Reader side (tests, `pimax-ctrltrack show`); Monado has the same in C.
pub struct PoseReader {
    m: Map,
}

impl PoseReader {
    pub fn open(path: &Path) -> io::Result<PoseReader> {
        Ok(PoseReader {
            m: Map::open(path, false)?,
        })
    }

    pub fn read(&self) -> Option<[HandPose; 2]> {
        self.read_all().map(|(h, _)| h)
    }

    /// Both hands and the heartbeat, one consistent snapshot.
    pub fn read_all(&self) -> Option<([HandPose; 2], Beat)> {
        for _ in 0..1000 {
            let s1 = self.m.seq().load(Ordering::Acquire);
            if s1 & 1 == 1 {
                std::hint::spin_loop();
                continue;
            }
            if u32::from_le_bytes(get(self.m.p, 0)) != MAGIC {
                return None;
            }
            let mut out = [HandPose::default(); 2];
            for (i, h) in out.iter_mut().enumerate() {
                let o = HAND_OFF + i * HAND_STRIDE;
                let f = |k: usize| f32::from_le_bytes(get(self.m.p, o + k));
                h.frame_ns = u64::from_le_bytes(get(self.m.p, o));
                h.publish_ns = u64::from_le_bytes(get(self.m.p, o + 8));
                h.pos = [f(16), f(20), f(24)];
                h.quat = [f(28), f(32), f(36), f(40)];
                h.rms_px = f(44);
                h.leds = u32::from_le_bytes(get(self.m.p, o + 48));
                h.cams = u32::from_le_bytes(get(self.m.p, o + 52));
                h.aim_from_imu = [f(56), f(60), f(64), f(68)];
            }
            let beat = Beat {
                beat_ns: u64::from_le_bytes(get(self.m.p, BEAT_OFF)),
                led_frames: u64::from_le_bytes(get(self.m.p, BEAT_OFF + 8)),
                source_up: u32::from_le_bytes(get(self.m.p, BEAT_OFF + 16)) != 0,
                reopens: u32::from_le_bytes(get(self.m.p, BEAT_OFF + 20)),
            };
            fence(Ordering::Acquire);
            if self.m.seq().load(Ordering::Relaxed) == s1 {
                return Some((out, beat));
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn write_read_roundtrip() {
        let p =
            std::env::temp_dir().join(format!("pimax-ctrltrack-test-{}.pose", std::process::id()));
        let mut w = PoseWriter::create(&p).unwrap();
        let h = HandPose {
            frame_ns: 123,
            publish_ns: 456,
            pos: [0.1, -0.2, -0.3],
            quat: [0.0, 0.0, 0.0, 1.0],
            rms_px: 0.7,
            leds: 11,
            cams: 3,
            aim_from_imu: [0.0, 1.0, 0.0, 0.0],
        };
        w.set(1, h);
        let r = PoseReader::open(&p).unwrap();
        let got = r.read().unwrap();
        assert_eq!(got[1], h);
        assert_eq!(got[0].frame_ns, 0);
        let (_, b0) = r.read_all().unwrap();
        assert_eq!(b0.age_ns(1000), None, "no heartbeat yet");
        let b = Beat {
            beat_ns: 5_000,
            led_frames: 77,
            source_up: true,
            reopens: 2,
        };
        w.beat(b);
        let (hands, got_b) = r.read_all().unwrap();
        assert_eq!(got_b, b);
        assert_eq!(hands[1], h, "the heartbeat leaves the poses alone");
        assert_eq!(got_b.age_ns(6_000), Some(1_000));
        drop(w);
        let _ = std::fs::remove_file(&p);
    }
}
