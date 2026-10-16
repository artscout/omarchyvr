//! Minimal V4L2 capture (raw ioctls, mmap streaming, no dependency besides libc) for the camera node of
//! `34a4:0014`. Struct layouts are checked against `<linux/videodev2.h>` (x86_64) by the unit tests.

use crate::source::{RawFrame, Source};
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::path::{Path, PathBuf};

const VIDIOC_QUERYCAP: u64 = 0x8068_5600;
const VIDIOC_G_FMT: u64 = 0xc0d0_5604;
const VIDIOC_S_FMT: u64 = 0xc0d0_5605;
const VIDIOC_REQBUFS: u64 = 0xc014_5608;
const VIDIOC_QUERYBUF: u64 = 0xc058_5609;
const VIDIOC_QBUF: u64 = 0xc058_560f;
const VIDIOC_DQBUF: u64 = 0xc058_5611;
const VIDIOC_STREAMON: u64 = 0x4004_5612;
const VIDIOC_STREAMOFF: u64 = 0x4004_5613;
const VIDIOC_G_PARM: u64 = 0xc0cc_5615;
const VIDIOC_S_PARM: u64 = 0xc0cc_5616;
const VIDIOC_ENUM_FRAMEINTERVALS: u64 = 0xc034_564b;

const BUF_TYPE_VIDEO_CAPTURE: u32 = 1;
const MEMORY_MMAP: u32 = 1;
const FIELD_NONE: u32 = 1;
/// `V4L2_PIX_FMT_YUYV`.
pub const PIX_FMT_YUYV: u32 = 0x5659_5559;
const CAP_VIDEO_CAPTURE: u32 = 0x1;
const CAP_STREAMING: u32 = 0x0400_0000;
const CAP_DEVICE_CAPS: u32 = 0x8000_0000;
const BUF_FLAG_ERROR: u32 = 0x40;
const BUF_FLAG_TIMESTAMP_MASK: u32 = 0xe000;
const BUF_FLAG_TIMESTAMP_MONOTONIC: u32 = 0x2000;
const FRMIVAL_TYPE_DISCRETE: u32 = 1;

/// USB ids of the camera.
pub const CAM_VID: &str = "34a4";
/// USB product id.
pub const CAM_PID: &str = "0014";

#[repr(C)]
#[derive(Clone, Copy)]
struct Capability {
    driver: [u8; 16],
    card: [u8; 32],
    bus_info: [u8; 32],
    version: u32,
    capabilities: u32,
    device_caps: u32,
    reserved: [u32; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
struct PixFormat {
    width: u32,
    height: u32,
    pixelformat: u32,
    field: u32,
    bytesperline: u32,
    sizeimage: u32,
    colorspace: u32,
    priv_: u32,
    flags: u32,
    enc: u32,
    quantization: u32,
    xfer_func: u32,
}

#[repr(C)]
#[derive(Clone, Copy)]
union FormatUnion {
    pix: PixFormat,
    raw: [u8; 200],
    _align: [u64; 25],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Format {
    type_: u32,
    fmt: FormatUnion,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RequestBuffers {
    count: u32,
    type_: u32,
    memory: u32,
    capabilities: u32,
    flags: u8,
    reserved: [u8; 3],
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Timecode {
    type_: u32,
    flags: u32,
    frames: u8,
    seconds: u8,
    minutes: u8,
    hours: u8,
    userbits: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Buffer {
    index: u32,
    type_: u32,
    bytesused: u32,
    flags: u32,
    field: u32,
    timestamp: libc::timeval,
    timecode: Timecode,
    sequence: u32,
    memory: u32,
    m: u64,
    length: u32,
    reserved2: u32,
    request_fd: u32,
}

impl Buffer {
    fn new(index: u32) -> Buffer {
        // SAFETY: plain-old-data, all-zero is valid.
        let mut b: Buffer = unsafe { std::mem::zeroed() };
        b.index = index;
        b.type_ = BUF_TYPE_VIDEO_CAPTURE;
        b.memory = MEMORY_MMAP;
        b
    }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
struct Fract {
    numerator: u32,
    denominator: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct CaptureParm {
    capability: u32,
    capturemode: u32,
    timeperframe: Fract,
    extendedmode: u32,
    readbuffers: u32,
    reserved: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
union ParmUnion {
    capture: CaptureParm,
    raw: [u8; 200],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct StreamParm {
    type_: u32,
    parm: ParmUnion,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct FrmIvalEnum {
    index: u32,
    pixel_format: u32,
    width: u32,
    height: u32,
    type_: u32,
    discrete: Fract,
    _stepwise_rest: [u32; 4],
    reserved: [u32; 2],
}

fn ioctl<T>(fd: i32, req: u64, arg: &mut T) -> io::Result<()> {
    loop {
        // SAFETY: `arg` points to a properly sized repr(C) struct matching `req`.
        let r = unsafe { libc::ioctl(fd, req as _, arg as *mut T) };
        if r >= 0 {
            return Ok(());
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn cstr(b: &[u8]) -> String {
    let end = b.iter().position(|&c| c == 0).unwrap_or(b.len());
    String::from_utf8_lossy(&b[..end]).into_owned()
}

/// Static facts about an opened device.
#[derive(Clone, Debug)]
pub struct DeviceInfo {
    /// Path opened.
    pub path: PathBuf,
    /// Driver name (`uvcvideo`).
    pub driver: String,
    /// Card name.
    pub card: String,
    /// Bus info.
    pub bus: String,
    /// Negotiated width x height (in YUYV pixels).
    pub width: u32,
    /// Negotiated height.
    pub height: u32,
    /// Bytes per line.
    pub bytesperline: u32,
    /// Bytes per frame.
    pub sizeimage: u32,
    /// Negotiated frame interval as fps (0 if unknown).
    pub fps: f64,
    /// Frame intervals offered for YUYV at this size (fps).
    pub offered_fps: Vec<f64>,
}

struct Mapping {
    ptr: *mut libc::c_void,
    len: usize,
}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: ptr/len came from a successful mmap.
        unsafe { libc::munmap(self.ptr, self.len) };
    }
}

/// An open, streaming capture device.
pub struct V4l2Capture {
    fd: OwnedFd,
    maps: Vec<Mapping>,
    held: Option<Held>,
    streaming: bool,
    /// What was negotiated.
    pub info: DeviceInfo,
    /// Buffers dequeued with `V4L2_BUF_FLAG_ERROR` or short `bytesused` (skipped).
    pub corrupt: u64,
    /// Frames whose timestamp was not `CLOCK_MONOTONIC` (should stay 0).
    pub non_monotonic_ts: u64,
}

// SAFETY: the mappings are only touched through &mut self.
unsafe impl Send for V4l2Capture {}

/// One `/dev/videoN` candidate for [`pick_node`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeEntry {
    /// Device path.
    pub path: PathBuf,
    /// USB vendor id (lower case hex), empty if unknown.
    pub vid: String,
    /// USB product id (lower case hex), empty if unknown.
    pub pid: String,
    /// V4L2 capability bits (device_caps when present); `None` if they could not be read.
    pub caps: Option<u32>,
    /// `index` from sysfs (0 = first node of the interface), `None` if unknown.
    pub index: Option<u32>,
}

/// Pure selection: of the camera's nodes (USB 34a4:0014) pick the video-capture + streaming one (not the UVC
/// metadata node); when caps are unknown fall back to sysfs index 0. Lowest node number wins.
pub fn pick_node(entries: &[NodeEntry]) -> Option<PathBuf> {
    entries
        .iter()
        .filter(|e| e.vid == CAM_VID && e.pid == CAM_PID)
        .find(|e| match e.caps {
            Some(c) => c & CAP_VIDEO_CAPTURE != 0 && c & CAP_STREAMING != 0,
            None => e.index.unwrap_or(0) == 0,
        })
        .map(|e| e.path.clone())
}

/// Read the capability bits of `path` with a short-lived descriptor (closed before returning; QUERYCAP does not
/// claim the device).
fn probe_caps(path: &Path) -> Option<u32> {
    let c = CString::new(path.as_os_str().as_encoded_bytes()).ok()?;
    // SAFETY: valid C string.
    let raw = unsafe {
        libc::open(
            c.as_ptr(),
            libc::O_RDONLY | libc::O_NONBLOCK | libc::O_CLOEXEC,
        )
    };
    if raw < 0 {
        return None;
    }
    // SAFETY: we own the new descriptor; dropped (closed) at the end of this function.
    let fd = unsafe { OwnedFd::from_raw_fd(raw) };
    // SAFETY: POD.
    let mut cap: Capability = unsafe { std::mem::zeroed() };
    ioctl(fd.as_raw_fd(), VIDIOC_QUERYCAP, &mut cap).ok()?;
    Some(if cap.capabilities & CAP_DEVICE_CAPS != 0 {
        cap.device_caps
    } else {
        cap.capabilities
    })
}

/// Find `/dev/videoN` of the camera (USB 34a4:0014, capture capable). `None` when not plugged in.
pub fn find_device() -> Option<PathBuf> {
    let mut nodes: Vec<(u32, PathBuf)> = std::fs::read_dir("/sys/class/video4linux")
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter_map(|p| {
            let n = p.file_name()?.to_string_lossy().into_owned();
            let num = n.strip_prefix("video")?.parse::<u32>().ok()?;
            Some((num, p))
        })
        .collect();
    nodes.sort();
    let mut entries = Vec::new();
    for (num, n) in nodes {
        let usb = n.join("device/..");
        let rd = |f: &str| {
            std::fs::read_to_string(usb.join(f))
                .map(|s| s.trim().to_lowercase())
                .unwrap_or_default()
        };
        let (vid, pid) = (rd("idVendor"), rd("idProduct"));
        if vid != CAM_VID || pid != CAM_PID {
            continue;
        }
        let path = PathBuf::from(format!("/dev/video{num}"));
        entries.push(NodeEntry {
            caps: probe_caps(&path),
            index: std::fs::read_to_string(n.join("index"))
                .ok()
                .and_then(|s| s.trim().parse().ok()),
            path,
            vid,
            pid,
        });
    }
    pick_node(&entries)
}

impl V4l2Capture {
    /// Open `path`, set YUYV 1280x832, request `fps` (0 = keep the driver default), map `nbufs` buffers, start
    /// streaming.
    pub fn open(path: &Path, fps: u32, nbufs: u32) -> io::Result<V4l2Capture> {
        let c = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| io::Error::other("bad path"))?;
        // SAFETY: valid C string.
        let raw = unsafe {
            libc::open(
                c.as_ptr(),
                libc::O_RDWR | libc::O_NONBLOCK | libc::O_CLOEXEC,
            )
        };
        if raw < 0 {
            return Err(io::Error::last_os_error());
        }
        // SAFETY: we own the new descriptor.
        let fd = unsafe { OwnedFd::from_raw_fd(raw) };
        let f = fd.as_raw_fd();

        // SAFETY: POD.
        let mut cap: Capability = unsafe { std::mem::zeroed() };
        ioctl(f, VIDIOC_QUERYCAP, &mut cap)?;
        let caps = if cap.capabilities & CAP_DEVICE_CAPS != 0 {
            cap.device_caps
        } else {
            cap.capabilities
        };
        if caps & CAP_VIDEO_CAPTURE == 0 || caps & CAP_STREAMING == 0 {
            return Err(io::Error::other(format!(
                "{}: not a streaming capture node",
                path.display()
            )));
        }

        let mut fmt = Format {
            type_: BUF_TYPE_VIDEO_CAPTURE,
            fmt: FormatUnion { raw: [0; 200] },
        };
        ioctl(f, VIDIOC_G_FMT, &mut fmt)?;
        // SAFETY: the pix member is valid for VIDEO_CAPTURE.
        let mut pix = unsafe { fmt.fmt.pix };
        pix.width = 1280;
        pix.height = 832;
        pix.pixelformat = PIX_FMT_YUYV;
        pix.field = FIELD_NONE;
        fmt.fmt = FormatUnion { raw: [0; 200] };
        fmt.fmt.pix = pix;
        ioctl(f, VIDIOC_S_FMT, &mut fmt)?;
        // SAFETY: as above.
        let pix = unsafe { fmt.fmt.pix };
        if pix.pixelformat != PIX_FMT_YUYV || pix.width != 1280 || pix.height != 832 {
            return Err(io::Error::other(format!(
                "driver chose {:#x} {}x{}, expected YUYV 1280x832",
                pix.pixelformat, pix.width, pix.height
            )));
        }

        let mut offered = Vec::new();
        for i in 0..16 {
            let mut e = FrmIvalEnum {
                index: i,
                pixel_format: PIX_FMT_YUYV,
                width: 1280,
                height: 832,
                ..Default::default()
            };
            if ioctl(f, VIDIOC_ENUM_FRAMEINTERVALS, &mut e).is_err() {
                break;
            }
            if e.type_ == FRMIVAL_TYPE_DISCRETE && e.discrete.numerator > 0 {
                offered.push(e.discrete.denominator as f64 / e.discrete.numerator as f64);
            }
        }

        if fps > 0 {
            let mut p = StreamParm {
                type_: BUF_TYPE_VIDEO_CAPTURE,
                parm: ParmUnion { raw: [0; 200] },
            };
            p.parm.capture = CaptureParm {
                timeperframe: Fract {
                    numerator: 1,
                    denominator: fps,
                },
                ..Default::default()
            };
            ioctl(f, VIDIOC_S_PARM, &mut p)?;
        }
        let mut p = StreamParm {
            type_: BUF_TYPE_VIDEO_CAPTURE,
            parm: ParmUnion { raw: [0; 200] },
        };
        let got_fps = match ioctl(f, VIDIOC_G_PARM, &mut p) {
            // SAFETY: capture member valid for VIDEO_CAPTURE.
            Ok(()) => {
                let t = unsafe { p.parm.capture.timeperframe };
                if t.numerator > 0 {
                    t.denominator as f64 / t.numerator as f64
                } else {
                    0.0
                }
            }
            Err(_) => 0.0,
        };

        let mut rb = RequestBuffers {
            count: nbufs.max(2),
            type_: BUF_TYPE_VIDEO_CAPTURE,
            memory: MEMORY_MMAP,
            ..Default::default()
        };
        ioctl(f, VIDIOC_REQBUFS, &mut rb)?;
        if rb.count < 2 {
            return Err(io::Error::other("driver granted fewer than 2 buffers"));
        }
        let mut maps = Vec::new();
        for i in 0..rb.count {
            let mut b = Buffer::new(i);
            ioctl(f, VIDIOC_QUERYBUF, &mut b)?;
            // SAFETY: mapping the buffer the driver described.
            let ptr = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    b.length as usize,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED,
                    f,
                    b.m as u32 as libc::off_t,
                )
            };
            if ptr == libc::MAP_FAILED {
                return Err(io::Error::last_os_error());
            }
            maps.push(Mapping {
                ptr,
                len: b.length as usize,
            });
        }
        let info = DeviceInfo {
            path: path.to_path_buf(),
            driver: cstr(&cap.driver),
            card: cstr(&cap.card),
            bus: cstr(&cap.bus_info),
            width: pix.width,
            height: pix.height,
            bytesperline: pix.bytesperline,
            sizeimage: pix.sizeimage,
            fps: got_fps,
            offered_fps: offered,
        };
        let mut c = V4l2Capture {
            fd,
            maps,
            held: None,
            streaming: false,
            info,
            corrupt: 0,
            non_monotonic_ts: 0,
        };
        for i in 0..c.maps.len() as u32 {
            c.queue(i)?;
        }
        let mut t = BUF_TYPE_VIDEO_CAPTURE as i32;
        ioctl(c.fd.as_raw_fd(), VIDIOC_STREAMON, &mut t)?;
        c.streaming = true;
        Ok(c)
    }

    fn queue(&mut self, index: u32) -> io::Result<()> {
        let mut b = Buffer::new(index);
        ioctl(self.fd.as_raw_fd(), VIDIOC_QBUF, &mut b)
    }

    fn requeue_held(&mut self) -> io::Result<()> {
        if let Some(h) = self.held.take() {
            self.queue(h.index)?;
        }
        Ok(())
    }
}

/// The buffer currently handed out.
#[derive(Clone, Copy, Debug)]
struct Held {
    index: u32,
    len: usize,
    host_ts_ns: u64,
    sequence: u32,
}

impl Source for V4l2Capture {
    fn advance(&mut self, timeout_ms: i32) -> io::Result<bool> {
        self.requeue_held()?;
        let deadline = crate::clock::monotonic_ns() + (timeout_ms.max(0) as u64) * 1_000_000;
        loop {
            let now = crate::clock::monotonic_ns();
            let left_ms = if now >= deadline {
                0
            } else {
                ((deadline - now) / 1_000_000) as i32 + 1
            };
            let mut pfd = libc::pollfd {
                fd: self.fd.as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            };
            // SAFETY: one valid pollfd.
            let r = unsafe { libc::poll(&mut pfd, 1, left_ms) };
            if r < 0 {
                let e = io::Error::last_os_error();
                if e.kind() == io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(e);
            }
            if r == 0 {
                return Ok(false);
            }
            if pfd.revents & (libc::POLLERR | libc::POLLHUP | libc::POLLNVAL) != 0
                && pfd.revents & libc::POLLIN == 0
            {
                return Err(io::Error::other("camera device error (unplugged?)"));
            }
            let mut b = Buffer::new(0);
            match ioctl(self.fd.as_raw_fd(), VIDIOC_DQBUF, &mut b) {
                Ok(()) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => continue,
                Err(e) => return Err(e),
            }
            let need = self.info.sizeimage as usize;
            if b.flags & BUF_FLAG_ERROR != 0
                || (b.bytesused as usize) < need
                || b.index as usize >= self.maps.len()
            {
                self.corrupt += 1;
                if (b.index as usize) < self.maps.len() {
                    self.queue(b.index)?;
                }
                if crate::clock::monotonic_ns() >= deadline {
                    return Ok(false);
                }
                continue;
            }
            if b.flags & BUF_FLAG_TIMESTAMP_MASK != BUF_FLAG_TIMESTAMP_MONOTONIC {
                self.non_monotonic_ts += 1;
            }
            let m = &self.maps[b.index as usize];
            self.held = Some(Held {
                index: b.index,
                len: (b.bytesused as usize).min(m.len),
                host_ts_ns: b.timestamp.tv_sec as u64 * 1_000_000_000
                    + b.timestamp.tv_usec as u64 * 1000,
                sequence: b.sequence,
            });
            return Ok(true);
        }
    }

    fn current(&self) -> Option<RawFrame<'_>> {
        let h = self.held?;
        let m = &self.maps[h.index as usize];
        // SAFETY: the driver filled `len` bytes of this mapping; it stays ours until re-queued by `advance`.
        let data = unsafe { std::slice::from_raw_parts(m.ptr as *const u8, h.len) };
        Some(RawFrame {
            data,
            stride: self.info.bytesperline as usize,
            host_ts_ns: h.host_ts_ns,
            sequence: h.sequence,
        })
    }

    fn describe(&self) -> String {
        format!(
            "{} ({}, {}, {}) {}x{} YUYV bpl {} size {} at {:.2} fps (offered {:?})",
            self.info.path.display(),
            self.info.driver,
            self.info.card,
            self.info.bus,
            self.info.width,
            self.info.height,
            self.info.bytesperline,
            self.info.sizeimage,
            self.info.fps,
            self.info.offered_fps
        )
    }

    fn corrupt_frames(&self) -> u64 {
        self.corrupt
    }
}

impl Drop for V4l2Capture {
    fn drop(&mut self) {
        if self.streaming {
            let mut t = BUF_TYPE_VIDEO_CAPTURE as i32;
            let _ = ioctl(self.fd.as_raw_fd(), VIDIOC_STREAMOFF, &mut t);
        }
        self.maps.clear();
        let mut rb = RequestBuffers {
            count: 0,
            type_: BUF_TYPE_VIDEO_CAPTURE,
            memory: MEMORY_MMAP,
            ..Default::default()
        };
        let _ = ioctl(self.fd.as_raw_fd(), VIDIOC_REQBUFS, &mut rb);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::mem::{offset_of, size_of};

    #[test]
    fn abi_matches_videodev2_h() {
        // Values printed from <linux/videodev2.h> with gcc on this machine (x86_64).
        assert_eq!(size_of::<Capability>(), 104);
        assert_eq!(offset_of!(Capability, device_caps), 88);
        assert_eq!(size_of::<Format>(), 208);
        assert_eq!(offset_of!(Format, fmt), 8);
        assert_eq!(size_of::<PixFormat>(), 48);
        assert_eq!(size_of::<RequestBuffers>(), 20);
        assert_eq!(size_of::<Buffer>(), 88);
        assert_eq!(offset_of!(Buffer, timestamp), 24);
        assert_eq!(offset_of!(Buffer, timecode), 40);
        assert_eq!(offset_of!(Buffer, sequence), 56);
        assert_eq!(offset_of!(Buffer, memory), 60);
        assert_eq!(offset_of!(Buffer, m), 64);
        assert_eq!(offset_of!(Buffer, length), 72);
        assert_eq!(offset_of!(Buffer, request_fd), 80);
        assert_eq!(size_of::<StreamParm>(), 204);
        assert_eq!(offset_of!(StreamParm, parm), 4);
        assert_eq!(size_of::<CaptureParm>(), 40);
        assert_eq!(offset_of!(CaptureParm, timeperframe), 8);
        assert_eq!(size_of::<FrmIvalEnum>(), 52);
        // ioctl numbers encode the struct sizes
        assert_eq!((VIDIOC_QBUF >> 16) & 0x3fff, 88);
        assert_eq!((VIDIOC_S_FMT >> 16) & 0x3fff, 208);
        assert_eq!((VIDIOC_S_PARM >> 16) & 0x3fff, 204);
        assert_eq!((VIDIOC_REQBUFS >> 16) & 0x3fff, 20);
        assert_eq!((VIDIOC_QUERYCAP >> 16) & 0x3fff, 104);
        assert_eq!((VIDIOC_ENUM_FRAMEINTERVALS >> 16) & 0x3fff, 52);
    }

    fn entry(n: u32, caps: Option<u32>, index: Option<u32>, pid: &str) -> NodeEntry {
        NodeEntry {
            path: PathBuf::from(format!("/dev/video{n}")),
            vid: CAM_VID.into(),
            pid: pid.into(),
            caps,
            index,
        }
    }

    #[test]
    fn pick_node_prefers_capture_over_metadata() {
        let cap = CAP_VIDEO_CAPTURE | CAP_STREAMING | CAP_DEVICE_CAPS;
        let meta = 0x0000_0008 | CAP_STREAMING | CAP_DEVICE_CAPS; // V4L2_CAP_META_CAPTURE
                                                                  // metadata node listed first must be skipped
        let v = [
            entry(0, Some(meta), Some(0), CAM_PID),
            entry(1, Some(cap), Some(1), CAM_PID),
        ];
        assert_eq!(pick_node(&v), Some(PathBuf::from("/dev/video1")));
        let v = [
            entry(0, Some(cap), Some(0), CAM_PID),
            entry(1, Some(meta), Some(1), CAM_PID),
        ];
        assert_eq!(pick_node(&v), Some(PathBuf::from("/dev/video0")));
        // unknown caps: sysfs index 0
        let v = [
            entry(1, None, Some(1), CAM_PID),
            entry(0, None, Some(0), CAM_PID),
        ];
        assert_eq!(pick_node(&v), Some(PathBuf::from("/dev/video0")));
        // other devices ignored, nothing capable = None
        assert_eq!(pick_node(&[entry(2, Some(cap), Some(0), "ffff")]), None);
        assert_eq!(pick_node(&[entry(0, Some(meta), Some(0), CAM_PID)]), None);
        assert_eq!(pick_node(&[]), None);
    }

    #[test]
    fn open_nonexistent_fails() {
        assert!(V4l2Capture::open(Path::new("/dev/does-not-exist-pimax"), 0, 4).is_err());
    }
}
