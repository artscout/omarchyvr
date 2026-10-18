//! Powering the eye-tracker module: the headset MCU (34a4:0044) switches the Tobii EyeChip (2104:0561) on when the
//! host sends one HID report. Windows sends exactly this report (`analysis/init-sequence.md`, poweron capture
//! 20261013, t = 61.645 s, log `eSpiCmdeyetkctrl`, reply `4c 04 3c 05 00 00`); the chip enumerates about 1.6 s
//! later. Without it, a headset that was power-cycled under Linux leaves the eye module off (seen 14 Oct 13:56).
//!
//! Only the *enable* report is ever sent. The disable variant (`.. 00 00`) was never captured and is not in this
//! code. Nothing here writes settings or flash: it is the same one-report request Pimax's service sends.
//!
//! The Crystal Super (34a4:0040) takes the same report on its interface 0: Pimax Play sent exactly these bytes in the
//! Crystal Super capture (take cold2 of 2026-10-17, t = 27.543 s; analysis/crystal-super-capture-20261017.md).
//!
//! [`EnableGate`] is the rate limit used by the live tracker: at most one report per enumeration of the headset
//! (identified by its USB device number), and only when the headset is present and the EyeChip has been absent for
//! more than [`ABSENT_BEFORE_ENABLE`].

use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// The captured eye-module enable request, as a 64-byte HID output report (`58 05 3c 05 00 01`, zero padded).
pub fn enable_report() -> [u8; 64] {
    let mut r = [0u8; 64];
    r[..6].copy_from_slice(&[0x58, 0x05, 0x3c, 0x05, 0x00, 0x01]);
    r
}

/// The EyeChip must have been missing this long (with the headset present) before the enable is sent.
pub const ABSENT_BEFORE_ENABLE: Duration = Duration::from_secs(5);

/// A USB device seen in sysfs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UsbId {
    /// sysfs name, e.g. `1-5.2.4.1`.
    pub name: String,
    /// `devnum` (changes on every enumeration).
    pub devnum: u32,
}

fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Devices with this VID:PID under `root` (normally `/sys/bus/usb/devices`).
pub fn find_usb(root: &Path, vid: u16, pid: u16) -> Vec<UsbId> {
    let (v, p) = (format!("{vid:04x}"), format!("{pid:04x}"));
    let mut out = Vec::new();
    for e in std::fs::read_dir(root).into_iter().flatten().flatten() {
        let d = e.path();
        if read_trim(&d.join("idVendor")).as_deref() == Some(v.as_str())
            && read_trim(&d.join("idProduct")).as_deref() == Some(p.as_str())
        {
            let devnum = read_trim(&d.join("devnum"))
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            out.push(UsbId {
                name: e.file_name().to_string_lossy().into_owned(),
                devnum,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// Headset HID chips whose interface 0 takes the eye-module enable: the Dream Air (0044), the Crystal Super (0040).
pub const HEADSET_PIDS: [u16; 2] = [0x0044, 0x0040];

/// The headset (34a4:0044 or 34a4:0040), if enumerated.
pub fn headset(root: &Path) -> Option<UsbId> {
    HEADSET_PIDS.iter().find_map(|&pid| find_usb(root, 0x34a4, pid).into_iter().next())
}

/// Whether an EyeChip (2104:0561) is enumerated.
pub fn eye_chip_present(root: &Path) -> bool {
    !find_usb(root, 0x2104, 0x0561).is_empty()
}

/// hidraw node of interface 0 (the command channel) of the headset (34a4:0044 or 0040): `hidraw_class` is normally
/// `/sys/class/hidraw`,
/// `dev` is `/dev`.
pub fn headset_command_node(hidraw_class: &Path, dev: &Path) -> Option<PathBuf> {
    let mut names: Vec<String> = std::fs::read_dir(hidraw_class)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("hidraw"))
        .collect();
    names.sort();
    for n in names {
        let d = hidraw_class.join(&n).join("device");
        let Some(uevent) = read_trim(&d.join("uevent")) else {
            continue;
        };
        if !HEADSET_PIDS
            .iter()
            .any(|pid| uevent.contains(&format!("HID_ID=0003:000034A4:0000{pid:04X}")))
        {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&d) else {
            continue;
        };
        let iface = real
            .parent()
            .and_then(|p| read_trim(&p.join("bInterfaceNumber")));
        if iface.as_deref() == Some("00") {
            return Some(dev.join(n));
        }
    }
    None
}

/// Write the enable report to the headset command node (report id 0 prefix, as hidraw wants).
pub fn send_enable(node: &Path) -> io::Result<()> {
    use std::io::Write;
    let mut buf = [0u8; 65];
    buf[1..].copy_from_slice(&enable_report());
    let mut f = std::fs::OpenOptions::new().write(true).open(node)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(
            io::ErrorKind::WriteZero,
            format!("short hidraw write: {n} of {}", buf.len()),
        ));
    }
    Ok(())
}

/// Rate limit of the automatic enable (see the module docs).
#[derive(Debug, Default)]
pub struct EnableGate {
    absent_since: Option<Instant>,
    sent_for: Option<UsbId>,
    headset_seen: Option<UsbId>,
}

/// What [`EnableGate::step`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Gate {
    /// The EyeChip is there, or the headset is not: nothing to do.
    Idle,
    /// Waiting: the chip has been absent for less than [`ABSENT_BEFORE_ENABLE`].
    Waiting,
    /// Send the enable now (to this headset enumeration).
    Send(UsbId),
    /// Already sent for this headset enumeration.
    AlreadySent,
}

impl EnableGate {
    /// Decide from the current bus state; `Send` is returned at most once per headset enumeration.
    pub fn step(&mut self, now: Instant, headset: Option<&UsbId>, chip_present: bool) -> Gate {
        let Some(h) = headset else {
            self.absent_since = None;
            self.headset_seen = None;
            return Gate::Idle;
        };
        if self.headset_seen.as_ref() != Some(h) {
            // a new enumeration of the headset: the chip's absence is counted from now
            self.headset_seen = Some(h.clone());
            self.absent_since = None;
        }
        if chip_present {
            self.absent_since = None;
            return Gate::Idle;
        }
        let since = *self.absent_since.get_or_insert(now);
        if self.sent_for.as_ref() == Some(h) {
            return Gate::AlreadySent;
        }
        if now.duration_since(since) <= ABSENT_BEFORE_ENABLE {
            return Gate::Waiting;
        }
        self.sent_for = Some(h.clone());
        Gate::Send(h.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn report_is_the_captured_bytes() {
        let r = enable_report();
        assert_eq!(&r[..6], &[0x58, 0x05, 0x3c, 0x05, 0x00, 0x01]);
        assert!(r[6..].iter().all(|&b| b == 0));
        // the HID framing rule (length byte = data length + 3) holds: data `00 01`
        assert_eq!(usize::from(r[1]), 2 + 3);
    }

    fn id(n: u32) -> UsbId {
        UsbId {
            name: "1-5.2.4.1".into(),
            devnum: n,
        }
    }

    #[test]
    fn gate_waits_sends_once_per_enumeration() {
        let mut g = EnableGate::default();
        let t0 = Instant::now();
        assert_eq!(g.step(t0, None, false), Gate::Idle);
        assert_eq!(g.step(t0, Some(&id(21)), true), Gate::Idle);
        assert_eq!(g.step(t0, Some(&id(21)), false), Gate::Waiting);
        assert_eq!(
            g.step(t0 + Duration::from_secs(3), Some(&id(21)), false),
            Gate::Waiting
        );
        assert_eq!(
            g.step(t0 + Duration::from_secs(6), Some(&id(21)), false),
            Gate::Send(id(21))
        );
        for s in 7..30 {
            assert_eq!(
                g.step(t0 + Duration::from_secs(s), Some(&id(21)), false),
                Gate::AlreadySent
            );
        }
        // chip appeared then vanished again on the same enumeration: still no second report
        assert_eq!(g.step(t0 + Duration::from_secs(31), Some(&id(21)), true), Gate::Idle);
        assert_eq!(
            g.step(t0 + Duration::from_secs(40), Some(&id(21)), false),
            Gate::AlreadySent
        );
        // the headset re-enumerated (new devnum): one more report after the 5 s grace
        assert_eq!(
            g.step(t0 + Duration::from_secs(50), Some(&id(30)), false),
            Gate::Waiting
        );
        assert_eq!(
            g.step(t0 + Duration::from_secs(56), Some(&id(30)), false),
            Gate::Send(id(30))
        );
    }

    #[test]
    fn gate_restarts_the_grace_when_the_headset_goes_away() {
        let mut g = EnableGate::default();
        let t0 = Instant::now();
        assert_eq!(g.step(t0, Some(&id(5)), false), Gate::Waiting);
        assert_eq!(g.step(t0 + Duration::from_secs(4), None, false), Gate::Idle);
        assert_eq!(
            g.step(t0 + Duration::from_secs(6), Some(&id(6)), false),
            Gate::Waiting
        );
        assert_eq!(
            g.step(t0 + Duration::from_secs(12), Some(&id(6)), false),
            Gate::Send(id(6))
        );
    }

    #[test]
    fn sysfs_discovery_on_a_fake_tree() {
        let dir = std::env::temp_dir().join(format!("pimax-power-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let usb = dir.join("usb");
        let mk = |name: &str, v: &str, p: &str, devnum: &str| {
            let d = usb.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("idVendor"), format!("{v}\n")).unwrap();
            std::fs::write(d.join("idProduct"), format!("{p}\n")).unwrap();
            std::fs::write(d.join("devnum"), format!("{devnum}\n")).unwrap();
        };
        mk("1-5.2.4.1", "34a4", "0044", "21");
        mk("1-5.2.4.3.3", "34a4", "0043", "24");
        assert_eq!(headset(&usb), Some(id(21)));
        assert!(!eye_chip_present(&usb));
        mk("1-5.2.4.3.4", "2104", "0561", "27");
        assert!(eye_chip_present(&usb));
        // hidraw: interface 0 of 0044 is chosen, the bridge and other interfaces are not
        let class = dir.join("class");
        let ifdir = |iface: &str| {
            let p = dir.join(format!("devices/if{iface}"));
            std::fs::create_dir_all(p.join("hid")).unwrap();
            std::fs::write(p.join("bInterfaceNumber"), format!("{iface}\n")).unwrap();
            p.join("hid")
        };
        let link = |n: &str, target: &Path, hid_id: &str| {
            let e = class.join(n);
            std::fs::create_dir_all(&e).unwrap();
            std::fs::write(target.join("uevent"), format!("HID_ID={hid_id}\nHID_NAME=x\n")).unwrap();
            std::os::unix::fs::symlink(target, e.join("device")).unwrap();
        };
        let h0 = ifdir("00");
        let h1 = ifdir("01");
        let hb = dir.join("devices/bridge/hid");
        std::fs::create_dir_all(&hb).unwrap();
        std::fs::write(dir.join("devices/bridge/bInterfaceNumber"), "00\n").unwrap();
        link("hidraw1", &h1, "0003:000034A4:00000044");
        link("hidraw2", &hb, "0003:000034A4:00000043");
        link("hidraw3", &h0, "0003:000034A4:00000044");
        assert_eq!(
            headset_command_node(&class, Path::new("/dev")),
            Some(PathBuf::from("/dev/hidraw3"))
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
