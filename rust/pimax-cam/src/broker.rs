//! Camera frame broker: one process streams the camera, any number of local processes read every frame.
//!
//! V4L2 lets one process stream `/dev/video0`, but passthrough, 6DoF head tracking (Monado), controller tracking (LED
//! frames) and hand tracking all want the same four cameras. `pimax-cam broker` opens the [`Source`] once and
//! publishes each frame into a shared-memory ring; a client uses the source spec `broker` (or `broker:<socket>`) and
//! gets a [`BrokerSource`], an ordinary [`Source`], so nothing else changes for it (`pimax-passthrough run --source
//! broker`, `PIMAX_CAM_SOURCE=broker` for the Monado driver, `pimax-cam stats --source broker`).
//!
//! Protocol (Unix `SOCK_SEQPACKET` socket, default `$XDG_RUNTIME_DIR/pimax-cam-broker.sock`):
//! * on connect the server sends one hello datagram ([`Hello`]) with the ring's memfd attached (`SCM_RIGHTS`);
//! * then one 8-byte datagram per published frame: its index `k` (u64 LE); the frame sits in slot `k % slots`.
//!
//! Each slot is guarded by a seqlock (`2k+1` while frame `k` is written, `2k+2` once complete). The client copies the
//! frame out and re-checks the lock, so a client that falls more than `slots` frames behind skips frames; nobody ever
//! waits for a client (non-blocking sends: a full client socket just loses that notice). Cost: one 2.1 MB copy into
//! the ring per frame and one per client (well under a millisecond each).
//!
//! All `unsafe` (memfd, mmap, `SCM_RIGHTS`) is in small helpers below with SAFETY notes; the frame copy out of the
//! shared mapping is a plain `memcpy` checked by the seqlock, like any lock-free shared-memory ring.

use crate::frame::FRAME_BYTES;
use crate::source::{RawFrame, Source};
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

/// First bytes of the hello datagram and of the ring header.
pub const MAGIC: [u8; 4] = *b"PXCB";
/// Protocol / ring layout version.
pub const VERSION: u32 = 1;
/// Default number of slots: 8 frames = 160 ms at 50 fps.
pub const DEFAULT_SLOTS: u32 = 8;
/// Hello flag: the server's source is a recording.
pub const FLAG_REPLAY: u32 = 1;

const PAGE: usize = 4096;
const HEADER_BYTES: usize = PAGE;
const SLOT_HEADER: usize = 64;
const LATEST_OFFSET: usize = 64; // u64 in the header page: newest complete frame index + 1 (0 = none yet)
const PUBLISHED_OFFSET: usize = 72; // u64: frames published
const CAM_GEN_OFFSET: usize = 80; // u64: the server's camera generation (changes on every camera reopen)
const CAM_STATE_OFFSET: usize = 88; // u64: 0 = unknown (older broker), 1 = camera streaming, 2 = camera reopening
const BEAT_OFFSET: usize = 96; // u64: server loop heartbeat, CLOCK_MONOTONIC ns (0 = older broker)
const MAX_DESC: usize = 200;
const HELLO_FIXED: usize = 4 + 6 * 4;

/// Default socket: `$XDG_RUNTIME_DIR/pimax-cam-broker.sock` (or `/tmp/pimax-cam-broker-<uid>.sock`).
pub fn default_socket() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) if !d.is_empty() => PathBuf::from(d).join("pimax-cam-broker.sock"),
        // SAFETY: getuid cannot fail.
        _ => PathBuf::from(format!("/tmp/pimax-cam-broker-{}.sock", unsafe {
            libc::getuid()
        })),
    }
}

/// What the server tells a new client.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Hello {
    /// Ring slots.
    pub slots: u32,
    /// Bytes per slot (header + data, page multiple).
    pub slot_bytes: u32,
    /// Largest frame a slot holds.
    pub data_bytes: u32,
    /// `FLAG_*` bits.
    pub flags: u32,
    /// The server's source, for logs.
    pub desc: String,
}

impl Hello {
    /// Wire form: magic, version, slots, slot_bytes, data_bytes, flags, desc_len (u32 LE), desc.
    pub fn encode(&self) -> Vec<u8> {
        let desc = truncate_utf8(&self.desc, MAX_DESC);
        let mut v = Vec::with_capacity(HELLO_FIXED + desc.len());
        v.extend_from_slice(&MAGIC);
        for x in [
            VERSION,
            self.slots,
            self.slot_bytes,
            self.data_bytes,
            self.flags,
            desc.len() as u32,
        ] {
            v.extend_from_slice(&x.to_le_bytes());
        }
        v.extend_from_slice(desc.as_bytes());
        v
    }

    /// Parse and validate a hello datagram.
    pub fn decode(b: &[u8]) -> io::Result<Hello> {
        let bad = |m: &str| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("camera broker hello: {m}"),
            )
        };
        if b.len() < HELLO_FIXED || b[..4] != MAGIC {
            return Err(bad("not a pimax-cam broker"));
        }
        let u = |i: usize| u32::from_le_bytes(b[4 + 4 * i..8 + 4 * i].try_into().unwrap());
        if u(0) != VERSION {
            return Err(bad(&format!(
                "version {} (this client speaks {VERSION})",
                u(0)
            )));
        }
        let (slots, slot_bytes, data_bytes, flags, dlen) = (u(1), u(2), u(3), u(4), u(5) as usize);
        if b.len() != HELLO_FIXED + dlen || dlen > MAX_DESC {
            return Err(bad("bad length"));
        }
        if slots == 0
            || slots > 1024
            || (slot_bytes as usize) < SLOT_HEADER + data_bytes as usize
            || !(slot_bytes as usize).is_multiple_of(PAGE)
        {
            return Err(bad("bad ring layout"));
        }
        Ok(Hello {
            slots,
            slot_bytes,
            data_bytes,
            flags,
            desc: String::from_utf8_lossy(&b[HELLO_FIXED..]).into_owned(),
        })
    }

    fn ring_len(&self) -> usize {
        HEADER_BYTES + self.slots as usize * self.slot_bytes as usize
    }
}

fn truncate_utf8(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Metadata of a slot read back by a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotInfo {
    /// Frame index `k`.
    pub index: u64,
    /// Host monotonic time of the frame (server's source), ns.
    pub host_ts_ns: u64,
    /// Driver sequence number.
    pub sequence: u32,
    /// Bytes of frame data.
    pub len: usize,
    /// Row stride.
    pub stride: usize,
}

/// The shared ring (memfd + mapping). The server maps it read-write, clients read-only.
pub struct Ring {
    ptr: *mut u8,
    len: usize,
    layout: Hello,
    fd: OwnedFd,
}

// SAFETY: the mapping is plain shared memory owned by this value; access goes through atomics and seqlock-checked
// copies, so moving the owner to another thread is fine.
unsafe impl Send for Ring {}

impl Drop for Ring {
    fn drop(&mut self) {
        // SAFETY: ptr/len come from a successful mmap of exactly this length.
        unsafe { libc::munmap(self.ptr.cast(), self.len) };
    }
}

impl Ring {
    /// Server side: a new memfd ring with `slots` slots of up to `data_bytes` each.
    pub fn create(slots: u32, data_bytes: usize, flags: u32, desc: &str) -> io::Result<Ring> {
        let slot_bytes = (SLOT_HEADER + data_bytes).div_ceil(PAGE) * PAGE;
        let layout = Hello {
            slots: slots.clamp(2, 1024),
            slot_bytes: slot_bytes as u32,
            data_bytes: data_bytes as u32,
            flags,
            desc: truncate_utf8(desc, MAX_DESC).to_string(),
        };
        let len = layout.ring_len();
        // SAFETY: valid NUL-terminated name; the returned fd is owned below.
        let raw = unsafe { libc::memfd_create(c"pimax-cam-broker".as_ptr(), libc::MFD_CLOEXEC) };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: raw is a fresh descriptor we own.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        // SAFETY: plain syscall on our descriptor.
        if unsafe { libc::ftruncate(fd.as_raw_fd(), len as libc::off_t) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let ptr = map(&fd, len, true)?;
        let ring = Ring {
            ptr,
            len,
            layout,
            fd,
        };
        // SAFETY: the header page lies inside the mapping; nobody else has the fd yet.
        unsafe {
            std::ptr::copy_nonoverlapping(MAGIC.as_ptr(), ring.ptr, 4);
            std::ptr::copy_nonoverlapping(VERSION.to_le_bytes().as_ptr(), ring.ptr.add(4), 4);
        }
        Ok(ring)
    }

    /// Client side: map a ring received from the server (read-only), after checking its size.
    pub fn open(fd: OwnedFd, layout: Hello) -> io::Result<Ring> {
        let len = layout.ring_len();
        // SAFETY: zeroed stat is a valid out-parameter for fstat.
        let mut st: libc::stat = unsafe { std::mem::zeroed() };
        // SAFETY: plain syscall on a descriptor we own.
        if unsafe { libc::fstat(fd.as_raw_fd(), &mut st) } != 0 {
            return Err(io::Error::last_os_error());
        }
        if (st.st_size as usize) < len {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "camera broker ring smaller than announced",
            ));
        }
        let ptr = map(&fd, len, false)?;
        let ring = Ring {
            ptr,
            len,
            layout,
            fd,
        };
        // SAFETY: the header page lies inside the mapping.
        let magic = unsafe { std::slice::from_raw_parts(ring.ptr, 4) };
        if magic != MAGIC {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "camera broker ring has no header",
            ));
        }
        Ok(ring)
    }

    /// The layout (as sent in the hello).
    pub fn layout(&self) -> &Hello {
        &self.layout
    }

    /// The memfd (sent to clients).
    pub fn fd(&self) -> RawFd {
        self.fd.as_raw_fd()
    }

    fn atomic(&self, offset: usize) -> &AtomicU64 {
        debug_assert!(offset.is_multiple_of(8) && offset + 8 <= self.len);
        // SAFETY: 8-aligned (page-aligned mapping, offsets multiples of 8), inside the mapping, and only ever
        // accessed atomically by every process.
        unsafe { &*(self.ptr.add(offset) as *const AtomicU64) }
    }

    fn slot_offset(&self, k: u64) -> usize {
        HEADER_BYTES + (k % self.layout.slots as u64) as usize * self.layout.slot_bytes as usize
    }

    /// Newest complete frame index, if any.
    pub fn latest(&self) -> Option<u64> {
        self.atomic(LATEST_OFFSET)
            .load(Ordering::Acquire)
            .checked_sub(1)
    }

    /// Frames published so far.
    pub fn published(&self) -> u64 {
        self.atomic(PUBLISHED_OFFSET).load(Ordering::Acquire)
    }

    /// Server: record the camera state for clients and probes (`up`, its reopen generation) and the loop heartbeat.
    pub fn set_camera(&self, up: bool, generation: u64, now_ns: u64) {
        self.atomic(CAM_GEN_OFFSET).store(generation, Ordering::Release);
        self.atomic(CAM_STATE_OFFSET)
            .store(if up { 1 } else { 2 }, Ordering::Release);
        self.atomic(BEAT_OFFSET).store(now_ns, Ordering::Release);
    }

    /// The server's camera state: (`Some(up)`, generation, heartbeat ns); `None` from a broker without the field.
    pub fn camera(&self) -> (Option<bool>, u64, u64) {
        let up = match self.atomic(CAM_STATE_OFFSET).load(Ordering::Acquire) {
            1 => Some(true),
            2 => Some(false),
            _ => None,
        };
        (
            up,
            self.atomic(CAM_GEN_OFFSET).load(Ordering::Acquire),
            self.atomic(BEAT_OFFSET).load(Ordering::Acquire),
        )
    }

    /// Server: write frame `k` into its slot. Frames bigger than the slot are refused.
    pub fn publish(&self, k: u64, f: &RawFrame<'_>) -> io::Result<()> {
        if f.data.len() > self.layout.data_bytes as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!(
                    "frame of {} B does not fit a {} B slot",
                    f.data.len(),
                    self.layout.data_bytes
                ),
            ));
        }
        let off = self.slot_offset(k);
        let lock = self.atomic(off);
        lock.store(2 * k + 1, Ordering::Relaxed);
        fence(Ordering::Release);
        // SAFETY: the slot (header fields + data) lies inside the read-write mapping; only this server writes it,
        // readers detect a concurrent write through the seqlock.
        unsafe {
            let p = self.ptr.add(off);
            std::ptr::copy_nonoverlapping(f.host_ts_ns.to_le_bytes().as_ptr(), p.add(8), 8);
            std::ptr::copy_nonoverlapping(f.sequence.to_le_bytes().as_ptr(), p.add(16), 4);
            std::ptr::copy_nonoverlapping(
                (f.data.len() as u32).to_le_bytes().as_ptr(),
                p.add(20),
                4,
            );
            std::ptr::copy_nonoverlapping((f.stride as u32).to_le_bytes().as_ptr(), p.add(24), 4);
            std::ptr::copy_nonoverlapping(k.to_le_bytes().as_ptr(), p.add(32), 8);
            std::ptr::copy_nonoverlapping(f.data.as_ptr(), p.add(SLOT_HEADER), f.data.len());
        }
        lock.store(2 * k + 2, Ordering::Release);
        self.atomic(LATEST_OFFSET).store(k + 1, Ordering::Release);
        self.atomic(PUBLISHED_OFFSET).fetch_add(1, Ordering::AcqRel);
        Ok(())
    }

    /// Client: copy frame `k` into `out` (which must hold `data_bytes`). `None` when the slot no longer (or not yet)
    /// holds frame `k`, or was overwritten during the copy.
    pub fn read(&self, k: u64, out: &mut [u8]) -> Option<SlotInfo> {
        let off = self.slot_offset(k);
        let lock = self.atomic(off);
        let s1 = lock.load(Ordering::Acquire);
        if s1 != 2 * k + 2 {
            return None;
        }
        let mut hdr = [0u8; 40];
        // SAFETY: inside the mapping; a concurrent overwrite is detected by the second lock read below.
        let len = unsafe {
            std::ptr::copy_nonoverlapping(self.ptr.add(off), hdr.as_mut_ptr(), 40);
            let len = u32::from_le_bytes(hdr[20..24].try_into().unwrap()) as usize;
            if len > out.len() || len > self.layout.data_bytes as usize {
                return None;
            }
            std::ptr::copy_nonoverlapping(self.ptr.add(off + SLOT_HEADER), out.as_mut_ptr(), len);
            len
        };
        fence(Ordering::Acquire);
        if lock.load(Ordering::Relaxed) != s1 {
            return None; // torn: the server reused the slot while we copied
        }
        let u64_at = |i: usize| u64::from_le_bytes(hdr[i..i + 8].try_into().unwrap());
        let u32_at = |i: usize| u32::from_le_bytes(hdr[i..i + 4].try_into().unwrap());
        (u64_at(32) == k).then_some(SlotInfo {
            index: k,
            host_ts_ns: u64_at(8),
            sequence: u32_at(16),
            len,
            stride: u32_at(24) as usize,
        })
    }
}

fn map(fd: &OwnedFd, len: usize, write: bool) -> io::Result<*mut u8> {
    let prot = if write {
        libc::PROT_READ | libc::PROT_WRITE
    } else {
        libc::PROT_READ
    };
    // SAFETY: mapping a descriptor we own with a length the caller checked; the result is checked.
    let p = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            len,
            prot,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if p == libc::MAP_FAILED {
        return Err(io::Error::last_os_error());
    }
    Ok(p.cast())
}

/*
 * Sockets (SOCK_SEQPACKET, Unix).
 */

fn sockaddr(path: &Path) -> io::Result<(libc::sockaddr_un, libc::socklen_t)> {
    use std::os::unix::ffi::OsStrExt;
    let b = path.as_os_str().as_bytes();
    // SAFETY: an all-zero sockaddr_un is valid.
    let mut a: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    if b.is_empty() || b.len() >= a.sun_path.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("socket path too long or empty: {}", path.display()),
        ));
    }
    a.sun_family = libc::AF_UNIX as libc::sa_family_t;
    for (d, s) in a.sun_path.iter_mut().zip(b) {
        *d = *s as libc::c_char;
    }
    let len = std::mem::size_of::<libc::sa_family_t>() + b.len() + 1;
    Ok((a, len as libc::socklen_t))
}

fn seqpacket(nonblock: bool) -> io::Result<OwnedFd> {
    let flags =
        libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC | if nonblock { libc::SOCK_NONBLOCK } else { 0 };
    // SAFETY: plain syscall; the descriptor is owned below.
    let fd = unsafe { libc::socket(libc::AF_UNIX, flags, 0) };
    if fd < 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: fresh descriptor we own.
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn connect(path: &Path) -> io::Result<OwnedFd> {
    let (a, len) = sockaddr(path)?;
    let fd = seqpacket(false)?;
    // SAFETY: a/len describe a valid sockaddr_un.
    if unsafe { libc::connect(fd.as_raw_fd(), (&a as *const libc::sockaddr_un).cast(), len) } != 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(fd)
}

/// Bind and listen (non-blocking). A stale socket file (no server answering) is replaced; a live one is an error.
fn listen(path: &Path) -> io::Result<OwnedFd> {
    if path.exists() {
        if connect(path).is_ok() {
            return Err(io::Error::new(
                io::ErrorKind::AddrInUse,
                format!("a camera broker already serves {}", path.display()),
            ));
        }
        std::fs::remove_file(path)?;
    }
    let (a, len) = sockaddr(path)?;
    let fd = seqpacket(true)?;
    // SAFETY: a/len describe a valid sockaddr_un; plain syscalls on our descriptor.
    unsafe {
        if libc::bind(fd.as_raw_fd(), (&a as *const libc::sockaddr_un).cast(), len) != 0
            || libc::listen(fd.as_raw_fd(), 16) != 0
        {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(fd)
}

/// `Ok(None)` when no connection is pending.
fn accept(listener: &OwnedFd) -> io::Result<Option<OwnedFd>> {
    // SAFETY: plain syscall; NULL address out-parameters are allowed.
    let fd = unsafe {
        libc::accept4(
            listener.as_raw_fd(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            libc::SOCK_CLOEXEC | libc::SOCK_NONBLOCK,
        )
    };
    if fd < 0 {
        let e = io::Error::last_os_error();
        return match e.raw_os_error() {
            Some(libc::EAGAIN) | Some(libc::EINTR) | Some(libc::ECONNABORTED) => Ok(None),
            _ => Err(e),
        };
    }
    // SAFETY: fresh descriptor we own.
    Ok(Some(unsafe { OwnedFd::from_raw_fd(fd) }))
}

// cmsghdr-aligned control buffer for one descriptor.
#[repr(C, align(8))]
struct Cmsg([u8; 64]);

fn send_with_fd(sock: &OwnedFd, data: &[u8], fd: RawFd) -> io::Result<()> {
    let mut iov = libc::iovec {
        iov_base: data.as_ptr() as *mut libc::c_void,
        iov_len: data.len(),
    };
    let mut cbuf = Cmsg([0; 64]);
    // SAFETY: msghdr/cmsghdr set up per cmsg(3) in a buffer large and aligned enough for one descriptor.
    unsafe {
        let space = libc::CMSG_SPACE(std::mem::size_of::<RawFd>() as u32) as usize;
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.0.as_mut_ptr().cast();
        msg.msg_controllen = space as _;
        let cm = libc::CMSG_FIRSTHDR(&msg);
        (*cm).cmsg_level = libc::SOL_SOCKET;
        (*cm).cmsg_type = libc::SCM_RIGHTS;
        (*cm).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<RawFd>() as u32) as _;
        std::ptr::write_unaligned(libc::CMSG_DATA(cm).cast::<RawFd>(), fd);
        if libc::sendmsg(sock.as_raw_fd(), &msg, libc::MSG_NOSIGNAL) < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn recv_with_fd(sock: &OwnedFd, buf: &mut [u8]) -> io::Result<(usize, Option<OwnedFd>)> {
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    let mut cbuf = Cmsg([0; 64]);
    // SAFETY: msghdr/cmsghdr per cmsg(3); a received descriptor is taken over exactly once.
    unsafe {
        let mut msg: libc::msghdr = std::mem::zeroed();
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = cbuf.0.as_mut_ptr().cast();
        msg.msg_controllen = cbuf.0.len() as _;
        let n = libc::recvmsg(sock.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC);
        if n < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut fd = None;
        let cm = libc::CMSG_FIRSTHDR(&msg);
        if !cm.is_null()
            && (*cm).cmsg_level == libc::SOL_SOCKET
            && (*cm).cmsg_type == libc::SCM_RIGHTS
        {
            let raw = std::ptr::read_unaligned(libc::CMSG_DATA(cm).cast::<RawFd>());
            fd = Some(OwnedFd::from_raw_fd(raw));
        }
        if msg.msg_flags & (libc::MSG_TRUNC | libc::MSG_CTRUNC) != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "camera broker message truncated",
            ));
        }
        Ok((n as usize, fd))
    }
}

/// Wait up to `timeout_ms` (< 0: forever) for input on `fd`.
fn poll_in(fd: &OwnedFd, timeout_ms: i32) -> io::Result<bool> {
    let mut p = libc::pollfd {
        fd: fd.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    loop {
        // SAFETY: one valid pollfd.
        let r = unsafe { libc::poll(&mut p, 1, timeout_ms) };
        if r < 0 {
            let e = io::Error::last_os_error();
            if e.raw_os_error() == Some(libc::EINTR) {
                continue;
            }
            return Err(e);
        }
        return Ok(r > 0);
    }
}

enum Sent {
    Ok,
    Full,
    Gone,
}

fn send_notice(sock: &OwnedFd, k: u64) -> Sent {
    let b = k.to_le_bytes();
    // SAFETY: sending 8 bytes from a live buffer.
    let r = unsafe {
        libc::send(
            sock.as_raw_fd(),
            b.as_ptr().cast(),
            b.len(),
            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
        )
    };
    if r == 8 {
        return Sent::Ok;
    }
    match io::Error::last_os_error().raw_os_error() {
        Some(libc::EAGAIN) | Some(libc::ENOBUFS) | Some(libc::EINTR) => Sent::Full,
        _ => Sent::Gone,
    }
}

/*
 * Server.
 */

/// Options of [`serve`].
#[derive(Clone, Debug)]
pub struct ServeOptions {
    /// Socket path ([`default_socket`]).
    pub socket: PathBuf,
    /// Ring slots ([`DEFAULT_SLOTS`]).
    pub slots: u32,
    /// How often `on_status` is called.
    pub status_every: Duration,
}

impl Default for ServeOptions {
    fn default() -> Self {
        ServeOptions {
            socket: default_socket(),
            slots: DEFAULT_SLOTS,
            status_every: Duration::from_secs(5),
        }
    }
}

/// Counters of a running / finished broker.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ServeStats {
    /// Frames published.
    pub frames: u64,
    /// Clients connected now.
    pub clients: usize,
    /// Clients ever connected.
    pub clients_total: u64,
    /// Frame notices not delivered because a client's socket was full (that client skips those frames).
    pub notices_dropped: u64,
    /// Source read timeouts (no frame within 100 ms; live camera only).
    pub timeouts: u64,
    /// The camera is streaming (false while it is being reopened).
    pub camera_up: bool,
    /// Camera reopens so far.
    pub camera_reopens: u64,
}

/// Serve frames from `src` until `stop` is set, the source ends (replay without loop) or fails. The socket file is
/// removed on return. `on_status` is called every `status_every` with the counters.
///
/// A live camera opened through [`crate::source::open`] never fails here: it reopens itself after an unplug or a
/// stall ([`crate::resilient`]), and the broker keeps its socket, its ring and its clients meanwhile; the camera
/// state, its reopen generation and a heartbeat are published in the ring header for clients and probes
/// ([`probe`]).
pub fn serve(
    mut src: Box<dyn Source>,
    opts: &ServeOptions,
    stop: &AtomicBool,
    mut on_status: impl FnMut(&ServeStats),
) -> io::Result<ServeStats> {
    let flags = if src.is_replay() { FLAG_REPLAY } else { 0 };
    let ring = Ring::create(opts.slots, FRAME_BYTES, flags, &src.describe())?;
    let hello = ring.layout().encode();
    let listener = listen(&opts.socket)?;
    struct Unlink<'a>(&'a Path);
    impl Drop for Unlink<'_> {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(self.0);
        }
    }
    let _unlink = Unlink(&opts.socket);

    let mut clients: Vec<OwnedFd> = Vec::new();
    let mut st = ServeStats::default();
    let mut next_status = Instant::now() + opts.status_every;
    let mut k: u64 = 0;
    let mut gen0 = src.generation();
    let mut was_up = src.up();
    ring.set_camera(was_up, 0, crate::clock::monotonic_ns());
    while !stop.load(Ordering::Relaxed) {
        while let Some(c) = accept(&listener)? {
            if send_with_fd(&c, &hello, ring.fd()).is_ok() {
                clients.push(c);
                st.clients_total += 1;
            }
        }
        let got = src.advance(100)?;
        if got {
            if let Some(f) = src.current() {
                ring.publish(k, &f)?;
                clients.retain(|c| match send_notice(c, k) {
                    Sent::Ok => true,
                    Sent::Full => {
                        st.notices_dropped += 1;
                        true
                    }
                    Sent::Gone => false,
                });
                k += 1;
                st.frames = k;
            }
        } else if src.finished() {
            break;
        } else if !src.is_replay() {
            st.timeouts += 1;
        }
        let (up, gen) = (src.up(), src.generation());
        if gen != gen0 {
            st.camera_reopens += 1;
            gen0 = gen;
        }
        if up != was_up {
            was_up = up;
            st.clients = clients.len();
            on_status(&ServeStats { camera_up: up, ..st }); // report the change at once
        }
        st.camera_up = up;
        ring.set_camera(up, st.camera_reopens, crate::clock::monotonic_ns());
        if Instant::now() >= next_status {
            next_status += opts.status_every;
            st.clients = clients.len();
            on_status(&st);
        }
    }
    st.clients = clients.len();
    Ok(st)
}

/*
 * Client.
 */

/// A [`Source`] that reads frames from a running broker.
pub struct BrokerSource {
    sock: OwnedFd,
    ring: Ring,
    path: PathBuf,
    buf: Vec<u8>,
    cur: Option<SlotInfo>,
    skipped: u64,
    eof: bool,
}

impl BrokerSource {
    /// Connect to the broker at `path` and map its ring.
    pub fn connect(path: &Path) -> io::Result<BrokerSource> {
        let sock = connect(path).map_err(|e| {
            io::Error::new(
                e.kind(),
                format!(
                    "camera broker {}: {e} (start one with `pimax-cam broker`)",
                    path.display()
                ),
            )
        })?;
        if !poll_in(&sock, 3000)? {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                "camera broker: no hello within 3 s",
            ));
        }
        let mut b = [0u8; HELLO_FIXED + MAX_DESC];
        let (n, fd) = recv_with_fd(&sock, &mut b)?;
        let hello = Hello::decode(&b[..n])?;
        let fd = fd.ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                "camera broker hello without ring",
            )
        })?;
        let data_bytes = hello.data_bytes as usize;
        let ring = Ring::open(fd, hello)?;
        Ok(BrokerSource {
            sock,
            ring,
            path: path.to_path_buf(),
            buf: vec![0; data_bytes],
            cur: None,
            skipped: 0,
            eof: false,
        })
    }

    /// Frames this client missed (it fell more than the ring behind, or its notices were dropped).
    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    /// The server's hello.
    pub fn hello(&self) -> &Hello {
        self.ring.layout()
    }

    /// Frames the server published so far.
    pub fn published(&self) -> u64 {
        self.ring.published()
    }

    /// The server's camera state (see [`Ring::camera`]).
    pub fn camera(&self) -> (Option<bool>, u64, u64) {
        self.ring.camera()
    }

    fn recv_notice(&mut self) -> io::Result<Option<u64>> {
        let mut b = [0u8; 8];
        // SAFETY: receiving into a live 8-byte buffer.
        let n = unsafe {
            libc::recv(
                self.sock.as_raw_fd(),
                b.as_mut_ptr().cast(),
                b.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if n == 0 {
            self.eof = true;
            return Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                format!("camera broker {} stopped", self.path.display()),
            ));
        }
        if n < 0 {
            let e = io::Error::last_os_error();
            return match e.raw_os_error() {
                Some(libc::EAGAIN) | Some(libc::EINTR) => Ok(None),
                _ => Err(e),
            };
        }
        if n != 8 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "camera broker notice size",
            ));
        }
        Ok(Some(u64::from_le_bytes(b)))
    }
}

impl Source for BrokerSource {
    fn advance(&mut self, timeout_ms: i32) -> io::Result<bool> {
        let deadline =
            (timeout_ms >= 0).then(|| Instant::now() + Duration::from_millis(timeout_ms as u64));
        loop {
            let wait = match deadline {
                None => -1,
                Some(d) => d.saturating_duration_since(Instant::now()).as_millis() as i32,
            };
            if !poll_in(&self.sock, wait)? {
                return Ok(false);
            }
            let Some(k) = self.recv_notice()? else {
                if deadline.is_some_and(|d| Instant::now() >= d) {
                    return Ok(false);
                }
                continue;
            };
            let last = self.cur.map(|c| c.index);
            // Fell more than the ring behind: jump to the newest frame instead of reading overwritten slots.
            let newest = self.ring.latest().unwrap_or(k);
            let want = if newest >= k + self.ring.layout().slots as u64 - 1 {
                newest
            } else {
                k
            };
            if last.is_some_and(|l| want <= l) {
                continue; // an older notice for a frame we already passed
            }
            match self.ring.read(want, &mut self.buf) {
                Some(info) => {
                    if let Some(l) = last {
                        self.skipped += want - l - 1;
                    }
                    self.cur = Some(info);
                    return Ok(true);
                }
                None => continue, // overwritten while we got to it: take the next notice
            }
        }
    }

    fn current(&self) -> Option<RawFrame<'_>> {
        self.cur.map(|c| RawFrame {
            data: &self.buf[..c.len],
            stride: c.stride,
            host_ts_ns: c.host_ts_ns,
            sequence: c.sequence,
        })
    }

    fn describe(&self) -> String {
        format!(
            "broker {} ({})",
            self.path.display(),
            self.ring.layout().desc
        )
    }

    fn finished(&self) -> bool {
        self.eof && self.is_replay()
    }

    fn is_replay(&self) -> bool {
        self.ring.layout().flags & FLAG_REPLAY != 0
    }

    fn generation(&self) -> u64 {
        self.ring.camera().1
    }

    fn up(&self) -> bool {
        self.ring.camera().0 != Some(false)
    }
}

/// What [`probe`] saw.
#[derive(Clone, Debug, PartialEq)]
pub struct Probe {
    /// The server's source description.
    pub desc: String,
    /// Camera state (`None`: a broker without the field).
    pub camera_up: Option<bool>,
    /// Camera reopens so far.
    pub camera_reopens: u64,
    /// Frames published at the end of the window.
    pub published: u64,
    /// Frames per second over the window.
    pub fps: f64,
    /// Age of the server's loop heartbeat at the end of the window, s (`None`: older broker).
    pub heartbeat_age_s: Option<f64>,
}

impl Probe {
    /// One JSON object (for pimaxd and shells).
    pub fn to_json(&self) -> String {
        let opt = |v: Option<String>| v.unwrap_or_else(|| "null".into());
        format!(
            "{{\"camera_up\":{},\"camera_reopens\":{},\"published\":{},\"fps\":{:.1},\"heartbeat_age_s\":{},\"desc\":\"{}\"}}",
            opt(self.camera_up.map(|b| b.to_string())),
            self.camera_reopens,
            self.published,
            self.fps,
            opt(self.heartbeat_age_s.map(|a| format!("{a:.2}"))),
            self.desc.replace('\\', "\\\\").replace('"', "\\\"")
        )
    }
}

/// Health probe of a running broker: connect (read-only client), count frames over `window`, read the camera
/// state. Sends nothing to the headset; the server just sees one short-lived client.
pub fn probe(path: &Path, window: Duration) -> io::Result<Probe> {
    let c = BrokerSource::connect(path)?;
    let n0 = c.published();
    let t0 = Instant::now();
    std::thread::sleep(window);
    let n1 = c.published();
    let (up, gen, beat) = c.camera();
    let now = crate::clock::monotonic_ns();
    Ok(Probe {
        desc: c.hello().desc.clone(),
        camera_up: up,
        camera_reopens: gen,
        published: n1,
        fps: (n1 - n0) as f64 / t0.elapsed().as_secs_f64().max(1e-3),
        heartbeat_age_s: (beat != 0).then(|| now.saturating_sub(beat) as f64 * 1e-9),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    /// Synthetic camera: `n` frames of `len` bytes, byte `i` of frame `k` = `(k * 7 + i) % 251`.
    struct Fake {
        k: u64,
        n: u64,
        len: usize,
        buf: Vec<u8>,
        period: Duration,
        replay: bool,
    }

    impl Fake {
        fn new(n: u64, len: usize, period_ms: u64) -> Fake {
            Fake {
                k: 0,
                n,
                len,
                buf: vec![0; len],
                period: Duration::from_millis(period_ms),
                replay: true,
            }
        }
    }

    fn pattern(k: u64, i: usize) -> u8 {
        ((k as usize * 7 + i) % 251) as u8
    }

    impl Source for Fake {
        fn advance(&mut self, _t: i32) -> io::Result<bool> {
            if self.k >= self.n {
                return Ok(false);
            }
            std::thread::sleep(self.period);
            for (i, b) in self.buf.iter_mut().enumerate() {
                *b = pattern(self.k, i);
            }
            self.k += 1;
            Ok(true)
        }
        fn current(&self) -> Option<RawFrame<'_>> {
            (self.k > 0).then(|| RawFrame {
                data: &self.buf[..self.len],
                stride: 2560,
                host_ts_ns: 1000 + self.k,
                sequence: self.k as u32 - 1,
            })
        }
        fn describe(&self) -> String {
            "fake camera".into()
        }
        fn finished(&self) -> bool {
            self.k >= self.n
        }
        fn is_replay(&self) -> bool {
            self.replay
        }
    }

    fn sock_path(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("pxcb-test-{}-{name}.sock", std::process::id()))
    }

    #[test]
    fn hello_roundtrip_and_rejects() {
        let h = Hello {
            slots: 8,
            slot_bytes: 4096 * 521,
            data_bytes: FRAME_BYTES as u32,
            flags: FLAG_REPLAY,
            desc: "x".repeat(300),
        };
        let d = Hello::decode(&h.encode()).unwrap();
        assert_eq!(d.desc.len(), MAX_DESC);
        assert_eq!((d.slots, d.flags), (8, FLAG_REPLAY));
        let mut b = h.encode();
        b[0] = b'X';
        assert!(Hello::decode(&b).is_err());
        let mut b = h.encode();
        b[4] = 9; // version
        assert!(Hello::decode(&b).is_err());
        assert!(Hello::decode(&h.encode()[..10]).is_err());
        let bad = Hello {
            slot_bytes: 100,
            ..h
        };
        assert!(Hello::decode(&bad.encode()).is_err());
    }

    #[test]
    fn ring_publish_read_overwrite() {
        let ring = Ring::create(3, 1000, 0, "t").unwrap();
        let mut out = vec![0u8; 1000];
        assert_eq!(ring.latest(), None);
        assert!(ring.read(0, &mut out).is_none());
        let data: Vec<u8> = (0..1000).map(|i| pattern(5, i)).collect();
        for k in 0..5u64 {
            let f = RawFrame {
                data: &data[..900],
                stride: 2560,
                host_ts_ns: 42 + k,
                sequence: k as u32,
            };
            ring.publish(k, &f).unwrap();
        }
        assert_eq!((ring.latest(), ring.published()), (Some(4), 5));
        // 3 slots: frames 2, 3, 4 are there, 0 and 1 were overwritten
        assert!(ring.read(1, &mut out).is_none());
        let i = ring.read(4, &mut out).unwrap();
        assert_eq!(
            (i.index, i.len, i.host_ts_ns, i.sequence, i.stride),
            (4, 900, 46, 4, 2560)
        );
        assert_eq!(&out[..900], &data[..900]);
        assert!(ring.read(5, &mut out).is_none(), "not published yet");
        let big = vec![0u8; 2000];
        let f = RawFrame {
            data: &big,
            stride: 2560,
            host_ts_ns: 0,
            sequence: 0,
        };
        assert!(ring.publish(5, &f).is_err());
    }

    #[test]
    fn client_maps_the_servers_ring_read_only() {
        let ring = Ring::create(2, 100, FLAG_REPLAY, "d").unwrap();
        // SAFETY: dup of a live descriptor, owned by the new OwnedFd.
        let dup = unsafe { OwnedFd::from_raw_fd(libc::dup(ring.fd())) };
        let client = Ring::open(dup, ring.layout().clone()).unwrap();
        let d = [9u8; 50];
        ring.publish(
            0,
            &RawFrame {
                data: &d,
                stride: 10,
                host_ts_ns: 7,
                sequence: 1,
            },
        )
        .unwrap();
        let mut out = [0u8; 100];
        let i = client.read(0, &mut out).unwrap();
        assert_eq!((i.len, i.host_ts_ns), (50, 7));
        assert_eq!(&out[..50], &d);
    }

    #[test]
    fn end_to_end_two_clients_get_every_frame_and_the_end() {
        let path = sock_path("e2e");
        let stop = Arc::new(AtomicBool::new(false));
        let n = 40;
        let server = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                // wait for both clients so no frame is published before they listen
                let opts = ServeOptions {
                    socket: path,
                    slots: 8,
                    status_every: Duration::from_secs(60),
                };
                serve(Box::new(Fake::new(n, 5000, 15)), &opts, &stop, |_| {})
            })
        };
        let mut clients = vec![];
        for _ in 0..2 {
            let t0 = Instant::now();
            loop {
                match BrokerSource::connect(&path) {
                    Ok(c) => break clients.push(c),
                    Err(_) if t0.elapsed() < Duration::from_secs(5) => {
                        std::thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("{e}"),
                }
            }
        }
        for c in &clients {
            assert!(c.is_replay() && c.describe().contains("fake camera"));
        }
        // both clients read at the same time, like passthrough and Monado would
        let readers: Vec<_> = clients
            .into_iter()
            .map(|mut c| {
                std::thread::spawn(move || {
                    let mut seen = vec![];
                    loop {
                        match c.advance(2000) {
                            Ok(true) => {
                                let f = c.current().unwrap();
                                let k = f.sequence as u64;
                                assert_eq!(f.data.len(), 5000);
                                assert!(f
                                    .data
                                    .iter()
                                    .enumerate()
                                    .all(|(i, b)| *b == pattern(k, i)));
                                assert_eq!(f.host_ts_ns, 1001 + k);
                                seen.push(k);
                            }
                            Ok(false) => panic!("timeout"),
                            Err(e) => {
                                assert_eq!(e.kind(), io::ErrorKind::ConnectionAborted);
                                assert!(c.finished());
                                return seen;
                            }
                        }
                    }
                })
            })
            .collect();
        let seen: Vec<Vec<u64>> = readers.into_iter().map(|r| r.join().unwrap()).collect();
        let st = server.join().unwrap().unwrap();
        assert_eq!(st.frames, n);
        for s in &seen {
            // in order, no duplicates; the first frames may predate the connection
            assert!(s.windows(2).all(|w| w[1] > w[0]));
            assert_eq!(*s.last().unwrap(), n - 1);
            assert!(s.len() as u64 >= n - 10, "{} of {n}", s.len());
        }
        assert!(!path.exists(), "socket removed at exit");
    }

    #[test]
    fn a_slow_client_skips_but_never_blocks_the_server() {
        let path = sock_path("slow");
        let stop = Arc::new(AtomicBool::new(false));
        let n = 60;
        let server = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                let opts = ServeOptions {
                    socket: path,
                    slots: 4,
                    status_every: Duration::from_secs(60),
                };
                let t0 = Instant::now();
                let r = serve(Box::new(Fake::new(n, 1000, 5)), &opts, &stop, |_| {});
                (r, t0.elapsed())
            })
        };
        let t0 = Instant::now();
        let mut c = loop {
            match BrokerSource::connect(&path) {
                Ok(c) => break c,
                Err(_) if t0.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(2))
                }
                Err(e) => panic!("{e}"),
            }
        };
        let mut got = vec![];
        while let Ok(true) = c.advance(2000) {
            got.push(c.current().unwrap().sequence);
            std::thread::sleep(Duration::from_millis(40)); // 8x slower than the camera
        }
        let (r, took) = server.join().unwrap();
        assert_eq!(r.unwrap().frames, n);
        assert!(
            took < Duration::from_millis(n * 5 + 1500),
            "server was slowed down: {took:?}"
        );
        assert!(got.windows(2).all(|w| w[1] > w[0]), "in order: {got:?}");
        assert!(c.skipped() > 0 && got.len() < n as usize / 2, "{got:?}");
    }

    #[test]
    fn second_broker_on_the_same_socket_is_refused_and_stale_files_are_replaced() {
        let path = sock_path("dup");
        std::fs::write(&path, b"stale").unwrap();
        let l = listen(&path).unwrap(); // stale file replaced
        let e = listen(&path).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::AddrInUse);
        drop(l);
        let _ = std::fs::remove_file(&path);
        let e = BrokerSource::connect(&path).err().unwrap();
        assert!(e.to_string().contains("pimax-cam broker"), "{e}");
    }

    /// A live camera that errors (unplug) after `fail_after` frames.
    struct Flaky {
        inner: Fake,
        fail_after: u64,
    }

    impl Source for Flaky {
        fn advance(&mut self, t: i32) -> io::Result<bool> {
            if self.inner.k >= self.fail_after {
                return Err(io::Error::other("camera device error (unplugged?)"));
            }
            self.inner.advance(t)
        }
        fn current(&self) -> Option<RawFrame<'_>> {
            self.inner.current()
        }
        fn describe(&self) -> String {
            "flaky camera".into()
        }
    }

    fn live(n: u64, fail_after: u64) -> Box<dyn Source> {
        let mut f = Fake::new(n, 1000, 4);
        f.replay = false;
        Box::new(Flaky { inner: f, fail_after })
    }

    fn connect_retry(path: &Path) -> Box<dyn Source> {
        let t0 = Instant::now();
        loop {
            match crate::source::open(
                &format!("broker:{}", path.display()),
                0,
                crate::source::ReplayOptions::default(),
            ) {
                Ok(c) => return c,
                Err(_) if t0.elapsed() < Duration::from_secs(5) => {
                    std::thread::sleep(Duration::from_millis(5))
                }
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn broker_reopens_a_dropped_camera_and_clients_keep_reading() {
        use crate::resilient::{Policy, Resilient};
        let path = sock_path("reopen");
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let (path, stop) = (path.clone(), stop.clone());
            std::thread::spawn(move || {
                // the camera drops after 10 frames, is absent for 10 tries (~200 ms), then streams again
                let mut tries = 0;
                let open: crate::resilient::Opener = Box::new(move || {
                    tries += 1;
                    if tries <= 10 {
                        Err(io::Error::new(io::ErrorKind::NotFound, "camera 34a4:0014 not found"))
                    } else {
                        Ok(live(u64::MAX, u64::MAX))
                    }
                });
                let policy = Policy {
                    stall: Some(Duration::from_secs(5)),
                    backoff_min: Duration::from_millis(10),
                    backoff_max: Duration::from_millis(20),
                    log_every: Duration::from_secs(30),
                };
                let src = Resilient::new(live(u64::MAX, 10), open, policy, "camera").quiet();
                let opts = ServeOptions {
                    socket: path,
                    slots: 8,
                    status_every: Duration::from_secs(60),
                };
                let mut downs = 0;
                let st = serve(Box::new(src), &opts, &stop, |s| downs += !s.camera_up as u32);
                (st, downs)
            })
        };
        let mut c = connect_retry(&path);
        let g0 = c.generation();
        let mut seen = 0;
        let mut gens = std::collections::BTreeSet::new();
        let t0 = Instant::now();
        while seen < 40 && t0.elapsed() < Duration::from_secs(5) {
            if c.advance(100).expect("a live broker client never errors") {
                seen += 1;
                gens.insert(c.generation());
            }
        }
        assert!(seen >= 40, "frames kept coming after the drop: {seen}");
        assert!(gens.len() >= 2 && c.generation() != g0, "generation changed: {gens:?}");
        let p = probe(&path, Duration::from_millis(100)).unwrap();
        assert_eq!(p.camera_up, Some(true));
        assert_eq!(p.camera_reopens, 1);
        assert!(p.fps > 10.0, "{p:?}");
        assert!(p.heartbeat_age_s.unwrap() < 1.0);
        assert!(p.to_json().contains("\"camera_up\":true"), "{}", p.to_json());
        stop.store(true, Ordering::Relaxed);
        let (st, downs) = server.join().unwrap();
        let st = st.unwrap();
        assert_eq!((st.camera_reopens, st.camera_up, downs), (1, true, 1));
    }

    #[test]
    fn client_reconnects_to_a_restarted_broker() {
        let path = sock_path("restart");
        let run = |stop: Arc<AtomicBool>| {
            let path = path.clone();
            std::thread::spawn(move || {
                let opts = ServeOptions {
                    socket: path,
                    slots: 8,
                    status_every: Duration::from_secs(60),
                };
                serve(live(u64::MAX, u64::MAX), &opts, &stop, |_| {})
            })
        };
        let stop1 = Arc::new(AtomicBool::new(false));
        let b1 = run(stop1.clone());
        let mut c = connect_retry(&path);
        let mut n = 0;
        while n < 5 {
            n += c.advance(1000).unwrap() as u32;
        }
        let g1 = c.generation();
        stop1.store(true, Ordering::Relaxed);
        b1.join().unwrap().unwrap();
        // broker gone: after the frames still queued, the client only sees timeouts, no error
        let mut quiet = 0;
        while quiet < 5 {
            let got = c.advance(20).expect("no error while the broker is away");
            quiet = if got { 0 } else { quiet + 1 };
        }
        assert!(!c.up());
        let stop2 = Arc::new(AtomicBool::new(false));
        let b2 = run(stop2.clone());
        let t0 = Instant::now();
        let mut got = 0;
        while got < 5 && t0.elapsed() < Duration::from_secs(5) {
            got += c.advance(100).unwrap() as u32;
        }
        assert_eq!(got, 5, "frames from the new broker");
        assert_ne!(c.generation(), g1);
        assert!(c.up());
        stop2.store(true, Ordering::Relaxed);
        b2.join().unwrap().unwrap();
    }

    #[test]
    fn replay_broker_end_is_still_an_end_for_wrapped_clients() {
        let path = sock_path("replay-end");
        let stop = Arc::new(AtomicBool::new(false));
        let server = {
            let path = path.clone();
            std::thread::spawn(move || {
                let opts = ServeOptions {
                    socket: path,
                    slots: 8,
                    status_every: Duration::from_secs(60),
                };
                serve(Box::new(Fake::new(30, 1000, 10)), &opts, &stop, |_| {})
            })
        };
        let mut c = connect_retry(&path);
        assert!(c.is_replay());
        let t0 = Instant::now();
        let end = loop {
            match c.advance(1000) {
                Ok(_) if t0.elapsed() < Duration::from_secs(5) => {}
                Ok(_) => panic!("no end"),
                Err(e) => break e,
            }
        };
        assert_eq!(end.kind(), io::ErrorKind::ConnectionAborted);
        assert!(c.finished());
        server.join().unwrap().unwrap();
    }
}
