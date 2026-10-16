//! The shared-memory ring between Monado's recorder tap (the producer, `comp_recorder.c`) and this recorder (the
//! consumer). Byte-for-byte the layout of `src/xrt/compositor/main/comp_recorder_shm.h` (the Monado fork).
//!
//! The consumer creates the file under a temporary name, fills the request and renames it into place, so the
//! producer never maps a half-written header. Live fields are only touched through atomics. Slot states: 0 free,
//! 1 producer writing, 2 ready, 3 consumer reading.
use std::fs::OpenOptions;
use std::io;
use std::os::fd::AsRawFd;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

pub const MAGIC: u64 = 0x3143_4552_5841_4d50; // "PMAXREC1"
pub const VERSION: u32 = 1;
pub const MAX_SLOTS: u32 = 8;
pub const HEADER_SIZE: u32 = 4096;

pub const SLOT_FREE: u32 = 0;
pub const SLOT_WRITING: u32 = 1;
pub const SLOT_READY: u32 = 2;
pub const SLOT_READING: u32 = 3;

#[repr(C)]
pub struct Slot {
    pub state: u32,
    pub width: u32,
    pub height: u32,
    pub _pad: u32,
    pub seq: u64,
    pub timestamp_ns: u64,
}

#[repr(C)]
pub struct Header {
    pub magic: u64,
    pub version: u32,
    pub header_size: u32,
    pub eyes: u32,
    pub fps: u32,
    pub max_height: u32,
    pub scale_permille: u32,
    pub slot_count: u32,
    pub _pad0: u32,
    pub slot_size: u64,
    pub consumer_heartbeat_ns: u64,
    pub stop: u32,
    pub producer_pid: u32,
    pub producer_heartbeat_ns: u64,
    pub width: u32,
    pub height: u32,
    pub src_width: u32,
    pub src_height: u32,
    pub produced: u64,
    pub dropped_busy: u64,
    pub dropped_full: u64,
    pub skipped: u64,
    pub producer_note: [u8; 64],
    pub slots: [Slot; MAX_SLOTS as usize],
}

/// What the consumer asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Request {
    pub eyes: u32,
    pub fps: u32,
    pub max_height: u32,
    pub scale_permille: u32,
    pub slot_count: u32,
    pub slot_size: u64,
}

impl Request {
    /// Slots big enough for any eye aspect up to 1.25 at `max_height` (the producer shrinks the frame if needed).
    pub fn new(eyes: u32, fps: u32, max_height: u32, scale_permille: u32) -> Request {
        let w = (max_height as u64 * 5).div_ceil(4) * eyes as u64;
        Request { eyes, fps, max_height, scale_permille, slot_count: 4, slot_size: (w * max_height as u64 * 4).div_ceil(4096) * 4096 }
    }
    pub fn file_size(&self) -> u64 {
        HEADER_SIZE as u64 + self.slot_count as u64 * self.slot_size
    }
}

/// `CLOCK_MONOTONIC` in ns (the producer's clock).
pub fn monotonic_ns() -> u64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: clock_gettime writes into ts.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

/// `$PIMAX_RECORD_SHM`, else `$XDG_RUNTIME_DIR/pimax-record.shm` (Monado computes the same).
pub fn default_path(runtime_dir: &Path) -> PathBuf {
    match std::env::var_os("PIMAX_RECORD_SHM").filter(|v| !v.is_empty()) {
        Some(p) => PathBuf::from(p),
        None => runtime_dir.join("pimax-record.shm"),
    }
}

/// A mapping of the ring (either side).
pub struct Ring {
    base: *mut u8,
    len: usize,
    pub path: PathBuf,
    /// The consumer owns (and removes) the file.
    owner: bool,
}

// SAFETY: the mapping is shared memory accessed through atomics (live fields) or after a slot-state handshake.
unsafe impl Send for Ring {}
unsafe impl Sync for Ring {}

/// A frame the consumer holds (slot state READING) until dropped.
pub struct Frame<'a> {
    ring: &'a Ring,
    index: u32,
    pub width: u32,
    pub height: u32,
    pub seq: u64,
    pub timestamp_ns: u64,
}

impl Frame<'_> {
    pub fn pixels(&self) -> &[u8] {
        let n = self.width as usize * self.height as usize * 4;
        // SAFETY: the slot is READING: the producer does not touch it; bounds checked when the frame was taken.
        unsafe { std::slice::from_raw_parts(self.ring.slot_ptr(self.index), n) }
    }
}

impl Drop for Frame<'_> {
    fn drop(&mut self) {
        self.ring.slot_state(self.index).store(SLOT_FREE, Ordering::Release);
    }
}

fn map(file: &std::fs::File, len: usize) -> io::Result<*mut u8> {
    // SAFETY: plain mmap of a file we opened read-write; checked for MAP_FAILED.
    let p = unsafe {
        libc::mmap(std::ptr::null_mut(), len, libc::PROT_READ | libc::PROT_WRITE, libc::MAP_SHARED, file.as_raw_fd(), 0)
    };
    if p == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(p as *mut u8)
}

impl Ring {
    /// Consumer: create the file (mode 0600) under a temporary name, write the request, rename it to `path`.
    /// Fails if `path` exists and a live producer or consumer still uses it.
    pub fn create(path: &Path, req: Request) -> io::Result<Ring> {
        if path.exists() {
            if let Ok(old) = Ring::open(path) {
                let h = old.hdr();
                let fresh = |t: u64| t != 0 && monotonic_ns().saturating_sub(t) < 3_000_000_000;
                if fresh(old.load64(&h.consumer_heartbeat_ns)) {
                    return Err(io::Error::new(io::ErrorKind::AlreadyExists, "another recorder is running"));
                }
            }
            let _ = std::fs::remove_file(path);
        }
        let tmp = path.with_extension(format!("tmp{}", std::process::id()));
        let f = OpenOptions::new().read(true).write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        let len = req.file_size();
        f.set_len(len)?;
        let base = map(&f, len as usize).inspect_err(|_| {
            let _ = std::fs::remove_file(&tmp);
        })?;
        let ring = Ring { base, len: len as usize, path: path.to_path_buf(), owner: true };
        {
            // SAFETY: nobody else can see the temporary file yet; the mapping covers the header.
            let h = unsafe { &mut *(base as *mut Header) };
            h.magic = MAGIC;
            h.version = VERSION;
            h.header_size = HEADER_SIZE;
            h.eyes = req.eyes;
            h.fps = req.fps;
            h.max_height = req.max_height;
            h.scale_permille = req.scale_permille;
            h.slot_count = req.slot_count;
            h.slot_size = req.slot_size;
            h.consumer_heartbeat_ns = monotonic_ns();
        }
        std::fs::rename(&tmp, path)?;
        Ok(ring)
    }

    /// Producer (and tests): map an existing ring.
    pub fn open(path: &Path) -> io::Result<Ring> {
        let f = OpenOptions::new().read(true).write(true).open(path)?;
        let len = f.metadata()?.len() as usize;
        if len < HEADER_SIZE as usize {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "too small"));
        }
        let base = map(&f, len)?;
        let ring = Ring { base, len, path: path.to_path_buf(), owner: false };
        let h = ring.hdr();
        if h.magic != MAGIC || h.version != VERSION || h.slot_count > MAX_SLOTS || ring.need(h.slot_count) > len as u64 {
            return Err(io::Error::new(io::ErrorKind::InvalidData, "not a omarchyvr-record ring"));
        }
        Ok(ring)
    }

    fn need(&self, slots: u32) -> u64 {
        HEADER_SIZE as u64 + slots as u64 * self.hdr().slot_size
    }

    pub fn hdr(&self) -> &Header {
        // SAFETY: the mapping is at least HEADER_SIZE bytes; plain fields are only read after the handshake.
        unsafe { &*(self.base as *const Header) }
    }

    fn a32(&self, f: &u32) -> &AtomicU32 {
        // SAFETY: f points into our mapping, 4-byte aligned (repr(C) layout checked by the tests).
        unsafe { AtomicU32::from_ptr(f as *const u32 as *mut u32) }
    }
    fn a64(&self, f: &u64) -> &AtomicU64 {
        // SAFETY: as above, 8-byte aligned.
        unsafe { AtomicU64::from_ptr(f as *const u64 as *mut u64) }
    }
    pub fn load32(&self, f: &u32) -> u32 {
        self.a32(f).load(Ordering::Acquire)
    }
    pub fn load64(&self, f: &u64) -> u64 {
        self.a64(f).load(Ordering::Acquire)
    }
    fn slot_state(&self, i: u32) -> &AtomicU32 {
        self.a32(&self.hdr().slots[i as usize].state)
    }
    fn slot_ptr(&self, i: u32) -> *mut u8 {
        // SAFETY: i < slot_count, checked at open/create against the mapping length.
        unsafe { self.base.add(HEADER_SIZE as usize + i as usize * self.hdr().slot_size as usize) }
    }

    pub fn heartbeat(&self) {
        self.a64(&self.hdr().consumer_heartbeat_ns).store(monotonic_ns(), Ordering::Release);
    }
    /// Ask the producer to detach.
    pub fn request_stop(&self) {
        self.a32(&self.hdr().stop).store(1, Ordering::Release);
    }
    pub fn producer_pid(&self) -> u32 {
        self.load32(&self.hdr().producer_pid)
    }
    /// Seconds since the producer's last heartbeat (None before it attached).
    pub fn producer_age_s(&self) -> Option<f64> {
        let t = self.load64(&self.hdr().producer_heartbeat_ns);
        (t != 0).then(|| monotonic_ns().saturating_sub(t) as f64 / 1e9)
    }
    pub fn counters(&self) -> Counters {
        let h = self.hdr();
        Counters {
            produced: self.load64(&h.produced),
            dropped_busy: self.load64(&h.dropped_busy),
            dropped_full: self.load64(&h.dropped_full),
            skipped: self.load64(&h.skipped),
        }
    }
    pub fn size(&self) -> (u32, u32) {
        (self.load32(&self.hdr().width), self.load32(&self.hdr().height))
    }
    pub fn producer_note(&self) -> String {
        let n = &self.hdr().producer_note;
        String::from_utf8_lossy(&n[..n.iter().position(|b| *b == 0).unwrap_or(n.len())]).into_owned()
    }

    /// Consumer: the oldest ready frame, if any (its slot is held until the frame is dropped).
    pub fn take(&self) -> Option<Frame<'_>> {
        let h = self.hdr();
        let mut best: Option<(u32, u64)> = None;
        for i in 0..h.slot_count.min(MAX_SLOTS) {
            if self.slot_state(i).load(Ordering::Acquire) == SLOT_READY {
                let seq = h.slots[i as usize].seq;
                if best.is_none_or(|(_, s)| seq < s) {
                    best = Some((i, seq));
                }
            }
        }
        let (i, _) = best?;
        self.slot_state(i).compare_exchange(SLOT_READY, SLOT_READING, Ordering::AcqRel, Ordering::Acquire).ok()?;
        let s = &h.slots[i as usize];
        let (w, ht) = (s.width, s.height);
        if w as u64 * ht as u64 * 4 > h.slot_size || w == 0 || ht == 0 {
            self.slot_state(i).store(SLOT_FREE, Ordering::Release);
            return None;
        }
        Some(Frame { ring: self, index: i, width: w, height: ht, seq: s.seq, timestamp_ns: s.timestamp_ns })
    }

    /// Producer side (the fake producer of the tests and the self-test): attach with an output size.
    pub fn attach_producer(&self, width: u32, height: u32, note: &str) {
        // SAFETY: producer-owned plain fields, written before producer_pid is published.
        let h = unsafe { &mut *(self.base as *mut Header) };
        h.width = width;
        h.height = height;
        h.src_width = width;
        h.src_height = height;
        let n = note.as_bytes();
        h.producer_note = [0; 64];
        h.producer_note[..n.len().min(63)].copy_from_slice(&n[..n.len().min(63)]);
        self.a64(&h.producer_heartbeat_ns).store(monotonic_ns(), Ordering::Release);
        self.a32(&h.producer_pid).store(std::process::id(), Ordering::Release);
    }
    pub fn producer_heartbeat(&self) {
        self.a64(&self.hdr().producer_heartbeat_ns).store(monotonic_ns(), Ordering::Release);
    }
    /// Producer side: publish a frame into a free slot; false (and `dropped_full` counted) if every slot is busy.
    pub fn publish(&self, width: u32, height: u32, seq: u64, timestamp_ns: u64, fill: &mut dyn FnMut(&mut [u8])) -> bool {
        let (count, slot_size) = (self.hdr().slot_count.min(MAX_SLOTS), self.hdr().slot_size);
        for i in 0..count {
            if self.slot_state(i).compare_exchange(SLOT_FREE, SLOT_WRITING, Ordering::AcqRel, Ordering::Acquire).is_ok() {
                let n = width as usize * height as usize * 4;
                assert!(n as u64 <= slot_size);
                // SAFETY: the slot is WRITING (ours) and n <= slot_size.
                fill(unsafe { std::slice::from_raw_parts_mut(self.slot_ptr(i), n) });
                // SAFETY: plain slot fields of a WRITING slot, written through a raw pointer into the mapping.
                unsafe {
                    let sp = std::ptr::addr_of_mut!((*(self.base as *mut Header)).slots[i as usize]);
                    (*sp).width = width;
                    (*sp).height = height;
                    (*sp).seq = seq;
                    (*sp).timestamp_ns = timestamp_ns;
                }
                self.slot_state(i).store(SLOT_READY, Ordering::Release);
                self.a64(&self.hdr().produced).fetch_add(1, Ordering::Relaxed);
                return true;
            }
        }
        self.a64(&self.hdr().dropped_full).fetch_add(1, Ordering::Relaxed);
        false
    }
    /// Producer side: detach (as Monado does on `stop`).
    pub fn detach_producer(&self) {
        self.a32(&self.hdr().producer_pid).store(0, Ordering::Release);
    }
    pub fn stop_requested(&self) -> bool {
        self.load32(&self.hdr().stop) != 0
    }
}

/// The producer's counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Counters {
    pub produced: u64,
    /// GPU readback still busy (the compositor never waits for it).
    pub dropped_busy: u64,
    /// Every slot full: this recorder (or the encoder) was behind.
    pub dropped_full: u64,
    /// No squashed eye image that frame (the first frame after attaching, or no layers).
    pub skipped: u64,
}

impl Drop for Ring {
    fn drop(&mut self) {
        if self.owner {
            let _ = std::fs::remove_file(&self.path);
        }
        // SAFETY: base/len are the mapping created in create/open.
        unsafe { libc::munmap(self.base as *mut libc::c_void, self.len) };
    }
}

/// The output size Monado picks for an eye (`pick_eye_extent` in comp_recorder.c; kept in step by the test).
pub fn pick_eye_extent(src: (u32, u32), max_height: u32, scale_permille: u32, eyes: u32, slot_size: u64) -> Option<(u32, u32)> {
    let mut h = src.1 as f64;
    if scale_permille > 0 && scale_permille <= 1000 {
        h = h * scale_permille as f64 / 1000.0;
    }
    if max_height > 0 && h > max_height as f64 {
        h = max_height as f64;
    }
    let aspect = src.0 as f64 / src.1 as f64;
    loop {
        let eh = (h as u32) & !1;
        let ew = ((h * aspect + 0.5) as u32) & !1;
        if eh < 64 || ew < 64 {
            return None;
        }
        if ew as u64 * eyes as u64 * eh as u64 * 4 <= slot_size && ew * eyes <= 4096 {
            return Some((ew, eh));
        }
        h -= 16.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn layout_matches_the_c_header() {
        // the _Static_asserts of comp_recorder_shm.h
        assert_eq!(offset_of!(Header, consumer_heartbeat_ns), 48);
        assert_eq!(offset_of!(Header, stop), 56);
        assert_eq!(offset_of!(Header, producer_pid), 60);
        assert_eq!(offset_of!(Header, producer_heartbeat_ns), 64);
        assert_eq!(offset_of!(Header, width), 72);
        assert_eq!(offset_of!(Header, produced), 88);
        assert_eq!(offset_of!(Header, skipped), 112);
        assert_eq!(offset_of!(Header, producer_note), 120);
        assert_eq!(offset_of!(Header, slots), 184);
        assert_eq!(size_of::<Slot>(), 32);
        assert!(size_of::<Header>() <= HEADER_SIZE as usize);
        assert_eq!(&MAGIC.to_le_bytes(), b"PMAXREC1");
    }

    fn tmp(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-record-shm-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d.join("ring.shm")
    }

    #[test]
    fn frames_flow_in_order_and_slots_are_recycled() {
        let p = tmp("flow");
        let req = Request::new(1, 30, 64, 0);
        let c = Ring::create(&p, req).unwrap();
        let prod = Ring::open(&p).unwrap();
        assert_eq!(c.producer_pid(), 0);
        prod.attach_producer(64, 64, "fake");
        assert_eq!(c.producer_pid(), std::process::id());
        assert_eq!(c.size(), (64, 64));
        assert_eq!(c.producer_note(), "fake");
        for seq in 0..3u64 {
            assert!(prod.publish(64, 64, seq, 1000 + seq, &mut |b| b.fill(seq as u8)));
        }
        for seq in 0..3u64 {
            let f = c.take().unwrap();
            assert_eq!((f.seq, f.timestamp_ns), (seq, 1000 + seq));
            assert!(f.pixels().iter().all(|b| *b == seq as u8));
        }
        assert!(c.take().is_none());
        // 4 slots: the fifth publish without a take is dropped and counted
        for seq in 3..8u64 {
            prod.publish(64, 64, seq, seq, &mut |_| {});
        }
        let k = c.counters();
        assert_eq!((k.produced, k.dropped_full), (7, 1));
        prod.detach_producer();
        c.request_stop();
        assert!(prod.stop_requested());
        drop(prod);
        drop(c);
        assert!(!p.exists(), "the consumer removes the file");
    }

    #[test]
    fn a_live_ring_is_not_taken_over_but_a_stale_one_is() {
        let p = tmp("live");
        let a = Ring::create(&p, Request::new(1, 30, 64, 0)).unwrap();
        assert_eq!(Ring::create(&p, Request::new(1, 30, 64, 0)).err().map(|e| e.kind()), Some(io::ErrorKind::AlreadyExists));
        // a dead consumer: heartbeat zeroed
        a.a64(&a.hdr().consumer_heartbeat_ns).store(1, Ordering::Release);
        std::mem::forget(a); // as if it crashed: the file stays
        let b = Ring::create(&p, Request::new(1, 30, 64, 0)).unwrap();
        assert!(b.hdr().magic == MAGIC);
    }

    #[test]
    fn eye_extent_matches_monado() {
        let req = Request::new(1, 30, 1080, 0);
        // a 3840x3552 eye at 1080: aspect kept, even sizes
        assert_eq!(pick_eye_extent((3840, 3552), 1080, 0, 1, req.slot_size), Some((1168, 1080)));
        // half scale of a small eye is not capped
        assert_eq!(pick_eye_extent((2000, 2000), 1080, 500, 1, req.slot_size), Some((1000, 1000)));
        // both eyes fit the slot sized for two
        let two = Request::new(2, 30, 1080, 0);
        assert_eq!(pick_eye_extent((3840, 3552), 1080, 0, 2, two.slot_size), Some((1168, 1080)));
        // a wider eye than the slot was sized for shrinks
        let (w, h) = pick_eye_extent((4000, 2000), 1080, 0, 1, req.slot_size).unwrap();
        assert!(w as u64 * h as u64 * 4 <= req.slot_size && h < 1080);
        // H.264 width limit
        let (w, _) = pick_eye_extent((3840, 3552), 2160, 0, 2, Request::new(2, 30, 2160, 0).slot_size).unwrap();
        assert!(w * 2 <= 4096);
    }
}
