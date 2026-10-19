//! sysfs discovery of the headset interfaces (same algorithm as `pimax_find_interfaces` / `find_interfaces()`).
use pimax_proto::{Model, MODELS, NUM_IF, VID};
use std::path::{Path, PathBuf};

/// Where to look. Defaults to the real `/sys/class/hidraw` and `/dev`; overridable for tests with
/// `PIMAX_SYSFS_ROOT` (replaces `/sys`) and `PIMAX_DEV_ROOT` (replaces `/dev`).
#[derive(Debug, Clone)]
pub struct Roots {
    /// Directory holding the `hidrawN` entries.
    pub hidraw_class: PathBuf,
    /// Directory holding the device nodes.
    pub dev: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            hidraw_class: "/sys/class/hidraw".into(),
            dev: "/dev".into(),
        }
    }
}

impl Roots {
    /// Defaults plus the `PIMAX_SYSFS_ROOT` / `PIMAX_DEV_ROOT` environment overrides.
    pub fn from_env() -> Self {
        let mut r = Self::default();
        if let Some(s) = std::env::var_os("PIMAX_SYSFS_ROOT") {
            r.hidraw_class = Path::new(&s).join("class/hidraw");
        }
        if let Some(d) = std::env::var_os("PIMAX_DEV_ROOT") {
            r.dev = d.into();
        }
        r
    }
}

fn hid_id(vid: u16, pid: u16) -> String {
    format!("HID_ID=0003:{vid:08X}:{pid:08X}")
}

fn entries(roots: &Roots) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(&roots.hidraw_class)
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| n.starts_with("hidraw"))
        .collect();
    v.sort();
    v
}

/// Device node of each USB interface 0..3 of the headset (the first model of [`MODELS`] that is present).
pub fn find_interfaces(roots: &Roots) -> [Option<PathBuf>; NUM_IF] {
    find_headset(roots).map(|(_, f)| f).unwrap_or_default()
}

/// The headset model on the bus and the device node of each of its interfaces 0..3.
pub fn find_headset(roots: &Roots) -> Option<(&'static Model, [Option<PathBuf>; NUM_IF])> {
    let names = entries(roots);
    MODELS.iter().find_map(|m| {
        let f = interfaces_of(roots, &names, m.pid);
        (mask(&f) != 0).then_some((m, f))
    })
}

fn interfaces_of(roots: &Roots, names: &[String], pid: u16) -> [Option<PathBuf>; NUM_IF] {
    let mut out: [Option<PathBuf>; NUM_IF] = Default::default();
    let want = hid_id(VID, pid);
    for name in names {
        let dev = roots.hidraw_class.join(name).join("device");
        let Ok(uevent) = std::fs::read_to_string(dev.join("uevent")) else {
            continue;
        };
        if !uevent.contains(&want) {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&dev) else {
            continue;
        };
        let Some(iface_dir) = real.parent() else {
            continue;
        };
        let Ok(num) = std::fs::read_to_string(iface_dir.join("bInterfaceNumber")) else {
            continue;
        };
        let Ok(n) = usize::from_str_radix(num.trim(), 16) else {
            continue;
        };
        if n < NUM_IF {
            out[n] = Some(roots.dev.join(name));
        }
    }
    out
}

/// Bitmask of the interfaces found.
pub fn mask(found: &[Option<PathBuf>; NUM_IF]) -> u8 {
    found
        .iter()
        .enumerate()
        .fold(0, |m, (i, p)| if p.is_some() { m | (1 << i) } else { m })
}

/// hidraw node of the display bridge MCU (34a4:0043 on the Dream Air, 0041 on the Crystal Super; EP 0x81 = ASCII log).
pub fn find_bridge_log(roots: &Roots) -> Option<PathBuf> {
    let names = entries(roots);
    MODELS.iter().find_map(|m| {
        let want = hid_id(VID, m.pid_bridge);
        names.iter().find_map(|name| {
            let u = std::fs::read_to_string(roots.hidraw_class.join(name).join("device/uevent")).ok()?;
            u.contains(&want).then(|| roots.dev.join(name))
        })
    })
}

/// USB device number of the headset (for "did it re-enumerate" diagnostics), from `/sys/bus/usb/devices`.
pub fn usb_devnum() -> Option<String> {
    for e in std::fs::read_dir("/sys/bus/usb/devices").ok()?.flatten() {
        let p = e.path();
        let rd = |f: &str| {
            std::fs::read_to_string(p.join(f))
                .ok()
                .map(|s| s.trim().to_string())
        };
        let is_headset = rd("idProduct")
            .and_then(|p| u16::from_str_radix(&p, 16).ok())
            .is_some_and(|p| pimax_proto::model::by_pid(p).is_some());
        if rd("idVendor").as_deref() == Some("34a4") && is_headset {
            return rd("devnum");
        }
    }
    None
}
