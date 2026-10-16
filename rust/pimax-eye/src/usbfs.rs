//! Linux usbfs (`/dev/bus/usb/BBB/DDD`) access to one device, and sysfs discovery.
//!
//! This is the only module with `unsafe` code: a few ioctls on a file descriptor we own. It never detaches a kernel
//! driver (`USBDEVFS_DISCONNECT` is not used) and never changes the configuration; claiming an interface that a driver
//! owns simply fails with EBUSY. [`reset_node`] issues `USBDEVFS_RESET`, a host-side USB port reset (the hub drives a
//! bus reset on the device's port and the kernel re-enumerates it, as on every plug-in): no request or data is sent to
//! the device firmware. The live tracker uses it only to recover a chip that stopped answering.
#![allow(unsafe_code)]

use crate::transport::Transport;
use std::fs::{File, OpenOptions};
use std::io;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::time::Duration;

const IOC_READ: u64 = 2;
const IOC_RW: u64 = 3;

const fn ioc(dir: u64, nr: u64, size: usize) -> u64 {
    (dir << 30) | ((size as u64) << 16) | ((b'U' as u64) << 8) | nr
}

#[repr(C)]
struct CtrlTransfer {
    b_request_type: u8,
    b_request: u8,
    w_value: u16,
    w_index: u16,
    w_length: u16,
    timeout: u32,
    data: *mut libc::c_void,
}

#[repr(C)]
struct BulkTransfer {
    ep: u32,
    len: u32,
    timeout: u32,
    data: *mut libc::c_void,
}

/// `USBDEVFS_CONTROL`.
pub const USBDEVFS_CONTROL: u64 = ioc(IOC_RW, 0, std::mem::size_of::<CtrlTransfer>());
/// `USBDEVFS_BULK`.
pub const USBDEVFS_BULK: u64 = ioc(IOC_RW, 2, std::mem::size_of::<BulkTransfer>());
/// `USBDEVFS_CLAIMINTERFACE`.
pub const USBDEVFS_CLAIMINTERFACE: u64 = ioc(IOC_READ, 15, 4);
/// `USBDEVFS_RELEASEINTERFACE`.
pub const USBDEVFS_RELEASEINTERFACE: u64 = ioc(IOC_READ, 16, 4);
/// `USBDEVFS_RESET` (`_IO('U', 20)`).
pub const USBDEVFS_RESET: u64 = ioc(0, 20, 0);

fn ioctl<T>(file: &File, request: u64, arg: *mut T) -> io::Result<i32> {
    loop {
        // SAFETY: `arg` points to a live, correctly laid out struct (or u32) for this request, and the descriptor is
        // valid for the lifetime of `file`. The kernel reads/writes only through the pointers inside the struct, which
        // callers set to buffers that outlive the call.
        let rc = unsafe { libc::ioctl(file.as_raw_fd(), request as _, arg) };
        if rc >= 0 {
            return Ok(rc);
        }
        let e = io::Error::last_os_error();
        if e.kind() != io::ErrorKind::Interrupted {
            return Err(e);
        }
    }
}

fn millis(t: Duration) -> u32 {
    u32::try_from(t.as_millis()).unwrap_or(u32::MAX).max(1)
}

/// One opened usbfs node.
pub struct UsbFs {
    file: File,
    claimed: Option<u32>,
}

impl UsbFs {
    /// Open the node read/write.
    pub fn open(path: &Path) -> io::Result<UsbFs> {
        let file = OpenOptions::new().read(true).write(true).open(path)?;
        Ok(UsbFs {
            file,
            claimed: None,
        })
    }

    /// Claim an interface. Fails with EBUSY when a kernel driver is bound; nothing is detached.
    pub fn claim_interface(&mut self, interface: u32) -> io::Result<()> {
        let mut n = interface;
        ioctl(&self.file, USBDEVFS_CLAIMINTERFACE, &mut n)?;
        self.claimed = Some(interface);
        Ok(())
    }
}

/// USB port reset of the device behind a usbfs node (`USBDEVFS_RESET`). The kernel re-enumerates the device;
/// interfaces claimed through usbfs are released, so every handle has to be reopened afterwards.
pub fn reset_node(path: &Path) -> io::Result<()> {
    let file = OpenOptions::new().read(true).write(true).open(path)?;
    ioctl(&file, USBDEVFS_RESET, std::ptr::null_mut::<u8>())?;
    Ok(())
}

impl Drop for UsbFs {
    fn drop(&mut self) {
        if let Some(mut n) = self.claimed.take() {
            let _ = ioctl(&self.file, USBDEVFS_RELEASEINTERFACE, &mut n);
        }
    }
}

impl Transport for UsbFs {
    fn control(&mut self, setup: [u8; 8], out: &[u8], timeout: Duration) -> io::Result<Vec<u8>> {
        let is_in = setup[0] & 0x80 != 0;
        let w_length = u16::from_le_bytes([setup[6], setup[7]]);
        if !is_in && out.len() != usize::from(w_length) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "control OUT data length differs from wLength",
            ));
        }
        let mut buf = if is_in {
            vec![0u8; usize::from(w_length)]
        } else {
            out.to_vec()
        };
        let mut t = CtrlTransfer {
            b_request_type: setup[0],
            b_request: setup[1],
            w_value: u16::from_le_bytes([setup[2], setup[3]]),
            w_index: u16::from_le_bytes([setup[4], setup[5]]),
            w_length,
            timeout: millis(timeout),
            data: buf.as_mut_ptr().cast(),
        };
        let n = ioctl(&self.file, USBDEVFS_CONTROL, &mut t)?;
        if is_in {
            buf.truncate(usize::try_from(n).unwrap_or(0));
            Ok(buf)
        } else {
            Ok(Vec::new())
        }
    }

    fn bulk_out(&mut self, ep: u8, data: &[u8], timeout: Duration) -> io::Result<()> {
        let mut buf = data.to_vec();
        let mut t = BulkTransfer {
            ep: u32::from(ep),
            len: u32::try_from(buf.len())
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bulk OUT too large"))?,
            timeout: millis(timeout),
            data: buf.as_mut_ptr().cast(),
        };
        let n = ioctl(&self.file, USBDEVFS_BULK, &mut t)?;
        if usize::try_from(n).ok() != Some(data.len()) {
            return Err(io::Error::new(
                io::ErrorKind::WriteZero,
                format!("short bulk OUT: {n} of {} bytes", data.len()),
            ));
        }
        Ok(())
    }

    fn bulk_in(
        &mut self,
        ep: u8,
        max_len: usize,
        timeout: Duration,
    ) -> io::Result<Option<Vec<u8>>> {
        let mut buf = vec![0u8; max_len];
        let mut t = BulkTransfer {
            ep: u32::from(ep),
            len: u32::try_from(max_len)
                .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "bulk IN too large"))?,
            timeout: millis(timeout),
            data: buf.as_mut_ptr().cast(),
        };
        match ioctl(&self.file, USBDEVFS_BULK, &mut t) {
            Ok(n) => {
                buf.truncate(usize::try_from(n).unwrap_or(0));
                Ok(Some(buf))
            }
            Err(e) if e.raw_os_error() == Some(libc::ETIMEDOUT) => Ok(None),
            Err(e) => Err(e),
        }
    }
}

/// A USB device as seen in sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SysDevice {
    /// `/sys/bus/usb/devices/<name>`.
    pub sysfs: PathBuf,
    /// Vendor id.
    pub vid: u16,
    /// Product id.
    pub pid: u16,
    /// Bus number.
    pub bus: u32,
    /// Device number on the bus.
    pub devnum: u32,
    /// Product string.
    pub product: Option<String>,
    /// Serial string.
    pub serial: Option<String>,
    /// Negotiated speed in Mb/s as text.
    pub speed: Option<String>,
    /// `/dev/bus/usb/BBB/DDD`.
    pub devnode: PathBuf,
}

impl SysDevice {
    /// Name of the kernel driver bound to interface 0, if any.
    pub fn interface_driver(&self) -> Option<String> {
        let name = self.sysfs.file_name()?.to_string_lossy().into_owned();
        let link = std::fs::read_link(self.sysfs.join(format!("{name}:1.0/driver"))).ok()?;
        Some(link.file_name()?.to_string_lossy().into_owned())
    }
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p)
        .ok()
        .map(|s| s.trim().to_string())
}

/// List the USB devices under `sysfs_root` (normally `/sys/bus/usb/devices`); nodes are placed under `dev_root`
/// (normally `/dev/bus/usb`).
pub fn scan_sysfs(sysfs_root: &Path, dev_root: &Path) -> Vec<SysDevice> {
    let Ok(rd) = std::fs::read_dir(sysfs_root) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in rd.flatten() {
        let p = entry.path();
        let hex =
            |name: &str| read_trim(&p.join(name)).and_then(|s| u16::from_str_radix(&s, 16).ok());
        let num = |name: &str| read_trim(&p.join(name)).and_then(|s| s.parse::<u32>().ok());
        let (Some(vid), Some(pid), Some(bus), Some(devnum)) = (
            hex("idVendor"),
            hex("idProduct"),
            num("busnum"),
            num("devnum"),
        ) else {
            continue;
        };
        out.push(SysDevice {
            vid,
            pid,
            bus,
            devnum,
            product: read_trim(&p.join("product")),
            serial: read_trim(&p.join("serial")),
            speed: read_trim(&p.join("speed")),
            devnode: dev_root
                .join(format!("{bus:03}"))
                .join(format!("{devnum:03}")),
            sysfs: p,
        });
    }
    out.sort_by_key(|d| (d.bus, d.devnum));
    out
}

/// EyeChips (2104:0561) currently on the bus.
pub fn find_eye_chips() -> Vec<SysDevice> {
    scan_sysfs(Path::new("/sys/bus/usb/devices"), Path::new("/dev/bus/usb"))
        .into_iter()
        .filter(|d| d.vid == crate::replay::EYE_VID && d.pid == crate::replay::EYE_PID)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ioctl_numbers_match_linux_usbdevice_fs_h() {
        // values printed by a C program including <linux/usbdevice_fs.h> on x86_64
        if cfg!(target_pointer_width = "64") {
            assert_eq!(std::mem::size_of::<CtrlTransfer>(), 24);
            assert_eq!(std::mem::size_of::<BulkTransfer>(), 24);
            assert_eq!(USBDEVFS_CONTROL, 0xc018_5500);
            assert_eq!(USBDEVFS_BULK, 0xc018_5502);
        }
        assert_eq!(USBDEVFS_CLAIMINTERFACE, 0x8004_550f);
        assert_eq!(USBDEVFS_RELEASEINTERFACE, 0x8004_5510);
        assert_eq!(USBDEVFS_RESET, 0x5514);
    }

    #[test]
    fn sysfs_scan_reads_a_fake_tree() {
        let dir = std::env::temp_dir().join(format!("pimax-eye-sysfs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dev = dir.join("1-5.1.4.3.4");
        std::fs::create_dir_all(&dev).unwrap();
        for (k, v) in [
            ("idVendor", "2104\n"),
            ("idProduct", "0561\n"),
            ("busnum", "1\n"),
            ("devnum", "12\n"),
            ("product", "EyeChip\n"),
            ("serial", "XR5DA-TEST\n"),
            ("speed", "480\n"),
        ] {
            std::fs::write(dev.join(k), v).unwrap();
        }
        std::fs::create_dir_all(dir.join("usb1")).unwrap(); // no idVendor: skipped
        let found = scan_sysfs(&dir, Path::new("/dev/bus/usb"));
        assert_eq!(found.len(), 1);
        let d = &found[0];
        assert_eq!((d.vid, d.pid, d.bus, d.devnum), (0x2104, 0x0561, 1, 12));
        assert_eq!(d.devnode, Path::new("/dev/bus/usb/001/012"));
        assert_eq!(d.product.as_deref(), Some("EyeChip"));
        assert_eq!(d.interface_driver(), None);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn missing_sysfs_root_is_empty() {
        assert!(scan_sysfs(Path::new("/nonexistent-pimax-eye"), Path::new("/dev")).is_empty());
    }
}
