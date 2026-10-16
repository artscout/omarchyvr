//! `pimax-eye ipd`: measure the user's IPD with the eye tracker (chip field 34, cross-checked with the distance between
//! the two gaze origins) and, only with `--apply`, drive the lens motor with the one command Windows sends.
//!
//! The command (`analysis/controllers-settings.md` R2.1, `eSpiCmdSetIPD`, fw log `motor_ipd_set type:1`) is the HID
//! output report `58 06 5c 05 01 <lo> <hi>`, zero padded to 64 bytes, on interface 0 of the headset (the same node and
//! write path as the keepalive and the eye-module enable in [`crate::power`]); `<lo> <hi>` is a u16 LE, IPD in mm x 100
//! (`70 17` = 0x1770 = 60.00 mm, `be 19` = 65.90 mm). The live lens IPD is read back from the headset info (IMU)
//! report on interface 2: `53 31 ..`, u16 LE at bytes 45..46, /100 mm. Nothing else is ever sent.

use crate::track::Sample;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Lowest / highest IPD the tool will command, mm.
pub const MIN_MM: f32 = 58.0;
pub const MAX_MM: f32 = 72.0;
/// Samples whose gaze origin is further than this (mm) from that eye's median are dropped.
pub const REJECT_MM: f32 = 2.5;
/// Field 34 and the origin distance may differ by at most this much (mm) for `--apply` without `--mm`.
pub const MAX_DISAGREE_MM: f32 = 3.0;
/// `--apply` without `--mm` refuses a measurement further than this (mm) from the current lens IPD.
pub const MAX_FROM_LENS_MM: f32 = 4.0;
/// Minimum good samples for `--apply` without `--mm`.
pub const MIN_GOOD: usize = 100;
/// The lens counts as arrived when within this many mm of the target.
pub const TOLERANCE_MM: f32 = 0.5;

/// u16 LE payload value for an IPD in mm (x100, rounded).
pub fn raw_from_mm(mm: f32) -> u16 {
    (mm * 100.0).round().clamp(0.0, 65535.0) as u16
}

/// Clamp to the allowed range.
pub fn clamp_mm(mm: f32) -> f32 {
    mm.clamp(MIN_MM, MAX_MM)
}

/// The captured set-IPD request as a 64-byte HID output report: `58 06 5c 05 01 lo hi`, zero padded.
pub fn motor_report(mm: f32) -> [u8; 64] {
    let v = raw_from_mm(mm).to_le_bytes();
    let mut r = [0u8; 64];
    r[..7].copy_from_slice(&[0x58, 0x06, 0x5c, 0x05, 0x01, v[0], v[1]]);
    r
}

/// Write the report to the headset command node (report id 0 prefix, as hidraw wants). Called once per `--apply`.
pub fn send_motor(node: &Path, mm: f32) -> io::Result<()> {
    use std::io::Write;
    let mut buf = [0u8; 65];
    buf[1..].copy_from_slice(&motor_report(mm));
    let mut f = std::fs::OpenOptions::new().write(true).open(node)?;
    let n = f.write(&buf)?;
    if n != buf.len() {
        return Err(io::Error::new(io::ErrorKind::WriteZero, format!("short hidraw write: {n} of {}", buf.len())));
    }
    Ok(())
}

/// Live lens IPD (mm) from a headset info report (`53 31 ..`), `None` for any other report.
pub fn lens_ipd_from_report(r: &[u8]) -> Option<f32> {
    if r.len() < 47 || r[0] != 0x53 || r[1] != 0x31 {
        return None;
    }
    Some(f32::from(u16::from_le_bytes([r[45], r[46]])) / 100.0)
}

/// hidraw node of USB interface `iface` (0 = commands, 2 = info/IMU) of the headset 34a4:0044.
pub fn headset_node(hidraw_class: &Path, dev: &Path, iface: &str) -> Option<PathBuf> {
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
        let uevent = std::fs::read_to_string(d.join("uevent")).unwrap_or_default();
        if !uevent.contains("HID_ID=0003:000034A4:00000044") {
            continue;
        }
        let Ok(real) = std::fs::canonicalize(&d) else { continue };
        let got = real
            .parent()
            .and_then(|p| std::fs::read_to_string(p.join("bInterfaceNumber")).ok());
        if got.as_deref().map(str::trim) == Some(iface) {
            return Some(dev.join(n));
        }
    }
    None
}

/// Read info reports from `node` for `timeout` (opened read-only) and return the newest lens IPD seen.
pub fn read_lens_ipd(node: &Path, timeout: Duration) -> Option<f32> {
    use std::io::Read;
    use std::os::unix::io::AsRawFd;
    let mut f = std::fs::File::open(node).ok()?;
    let end = Instant::now() + timeout;
    let mut last = None;
    let mut buf = [0u8; 64];
    while Instant::now() < end {
        let left = end.saturating_duration_since(Instant::now()).as_millis().min(200) as i32;
        let mut p = libc::pollfd { fd: f.as_raw_fd(), events: libc::POLLIN, revents: 0 };
        #[allow(unsafe_code)]
        // SAFETY: one valid pollfd, count 1.
        let r = unsafe { libc::poll(&mut p, 1, left) };
        if r > 0 {
            if let Ok(n) = f.read(&mut buf) {
                if let Some(v) = lens_ipd_from_report(&buf[..n]) {
                    last = Some(v);
                }
            }
        }
    }
    last
}

/// Result of the busy check.
#[derive(Debug, Clone, PartialEq)]
pub enum Busy {
    Free,
    /// Proceed, but tell the user.
    Warn(String),
    /// Refuse.
    Refuse(String),
}

/// Whether a running `monado-service` has the eye device node open. Only that is a conflict: Monado in desk mode
/// without `--eye` never opens the eye chip, and its whole-report keepalive writes to the headset hidraw coexist
/// with our single whole-report motor write. When `/proc/<pid>/fd` cannot be read the answer is a warning.
pub fn busy_check(proc_root: &Path, eye_node: Option<&Path>) -> Busy {
    let mut warn = None;
    for e in std::fs::read_dir(proc_root).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if std::fs::read_to_string(e.path().join("comm")).map(|c| c.trim() != "monado-service").unwrap_or(true) {
            continue;
        }
        let fds = match std::fs::read_dir(e.path().join("fd")) {
            Ok(d) => d,
            Err(err) => {
                warn = Some(format!("monado-service (pid {pid}) is running and its fds are unreadable ({err}): cannot tell whether it holds the eye tracker"));
                continue;
            }
        };
        if let Some(node) = eye_node {
            for fd in fds.flatten() {
                if std::fs::read_link(fd.path()).is_ok_and(|t| t == node) {
                    return Busy::Refuse(format!(
                        "monado-service (pid {pid}) has the eye tracker open ({}): restart it without --eye; nothing sent",
                        node.display()
                    ));
                }
            }
        } else {
            warn = Some(format!("monado-service (pid {pid}) is running and the eye device node is unknown: cannot tell whether it holds the eye tracker"));
        }
    }
    warn.map_or(Busy::Free, Busy::Warn)
}

/// Pid of a running `monado-service` (any owner): a session owns the headset, so the lens motor is not driven then.
pub fn session_running(proc_root: &Path) -> Option<u32> {
    for e in std::fs::read_dir(proc_root).into_iter().flatten().flatten() {
        let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
        if std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim() == "monado-service") {
            return Some(pid);
        }
    }
    None
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        0.0
    } else if v.len() % 2 == 1 {
        v[v.len() / 2]
    } else {
        0.5 * (v[v.len() / 2 - 1] + v[v.len() / 2])
    }
}

type Row = ([f32; 3], [f32; 3], f32);

/// Collects samples; [`Measure::result`] applies the filters.
#[derive(Default)]
pub struct Measure {
    /// (left origin mm, right origin mm, field 34 mm) of every sample with both origins valid.
    rows: Vec<Row>,
    pub seen: usize,
}

/// Outcome of a measurement.
#[derive(Debug, Clone, PartialEq)]
pub struct Measurement {
    pub seen: usize,
    /// Both origins valid and field 34 plausible.
    pub valid: usize,
    /// Valid and within [`REJECT_MM`] of the per-eye median origin.
    pub good: usize,
    pub field34_mm: f32,
    pub origin_mm: f32,
}

impl Measure {
    pub fn push(&mut self, s: &Sample) {
        self.seen += 1;
        let [l, r] = &s.eyes;
        if !(l.origin_valid && r.origin_valid) || !(20.0..=100.0).contains(&s.ipd_mm) {
            return;
        }
        let mm = |o: [f32; 3]| [o[0] * 1000.0, o[1] * 1000.0, o[2] * 1000.0];
        self.rows.push((mm(l.origin), mm(r.origin), s.ipd_mm));
    }

    pub fn result(&self) -> Measurement {
        let med3 = |sel: &dyn Fn(&Row) -> [f32; 3]| {
            let mut m = [0.0; 3];
            for (k, mk) in m.iter_mut().enumerate() {
                let mut v: Vec<f32> = self.rows.iter().map(|r| sel(r)[k]).collect();
                *mk = median(&mut v);
            }
            m
        };
        let (ml, mr) = (med3(&|r| r.0), med3(&|r| r.1));
        let d = |a: [f32; 3], b: [f32; 3]| ((a[0] - b[0]).powi(2) + (a[1] - b[1]).powi(2) + (a[2] - b[2]).powi(2)).sqrt();
        let good: Vec<_> = self.rows.iter().filter(|r| d(r.0, ml) <= REJECT_MM && d(r.1, mr) <= REJECT_MM).collect();
        let mut f34: Vec<f32> = good.iter().map(|r| r.2).collect();
        let mut od: Vec<f32> = good.iter().map(|r| d(r.0, r.1)).collect();
        Measurement {
            seen: self.seen,
            valid: self.rows.len(),
            good: good.len(),
            field34_mm: median(&mut f34),
            origin_mm: median(&mut od),
        }
    }
}

/// Decide the target for `--apply`. `mm_arg` is `--mm`. Returns (target mm, note about clamping).
pub fn decide(m: &Measurement, mm_arg: Option<f32>, lens: Option<f32>) -> Result<(f32, Option<String>), String> {
    let raw = match mm_arg {
        Some(x) if x.is_finite() => x,
        Some(_) => return Err("--mm is not a finite number".into()),
        None => {
            if m.good < MIN_GOOD {
                return Err(format!("only {} good samples (need {MIN_GOOD}); refusing. Wear the headset or pass --mm", m.good));
            }
            let diff = (m.field34_mm - m.origin_mm).abs();
            if diff > MAX_DISAGREE_MM {
                return Err(format!(
                    "field 34 ({:.2} mm) and origin distance ({:.2} mm) disagree by {diff:.2} mm (> {MAX_DISAGREE_MM}); refusing. Pass --mm to override",
                    m.field34_mm, m.origin_mm
                ));
            }
            if !(MIN_MM..=MAX_MM).contains(&m.field34_mm) {
                return Err(format!(
                    "measurement looks wrong: {:.2} mm is outside {MIN_MM}-{MAX_MM} mm; refusing. Pass --mm to override",
                    m.field34_mm
                ));
            }
            if let Some(l) = lens.filter(|l| l.is_finite() && *l > 0.0) {
                if (m.field34_mm - l).abs() > MAX_FROM_LENS_MM {
                    return Err(format!(
                        "measurement looks wrong: {:.2} mm is more than {MAX_FROM_LENS_MM} mm from the current lens IPD {l:.2} mm; refusing. Pass --mm to override",
                        m.field34_mm
                    ));
                }
            }
            m.field34_mm
        }
    };
    let t = clamp_mm(raw);
    let note = ((t - raw).abs() > 1e-6).then(|| format!("{raw:.2} mm clamped to {t:.2} mm (allowed {MIN_MM}-{MAX_MM})"));
    Ok((t, note))
}

/// Whether the final lens reading counts as success.
pub fn arrived(final_mm: f32, target_mm: f32) -> bool {
    (final_mm - target_mm).abs() <= TOLERANCE_MM
}

/// The Monado hook line (read once per session by `oxr_session.c`, default 63).
pub fn monado_line(mm: f32) -> String {
    format!("OXR_DEBUG_IPD_MM={mm:.1}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::EyeRay;

    fn sample(lx: f32, rx: f32, f34: f32) -> Sample {
        let e = |x: f32| EyeRay { origin: [x / 1000.0, 0.0, 0.0], origin_valid: true, direction_valid: true, ..Default::default() };
        Sample { seq: 0, device_ts_us: 0, host_ts_ns: 0, arrival_ns: 0, eyes: [e(lx), e(rx)], gaze: None, ipd_mm: f34 }
    }

    fn m(good: usize, f: f32, o: f32) -> Measurement {
        Measurement { seen: good, valid: good, good, field34_mm: f, origin_mm: o }
    }

    #[test]
    fn busy_check_looks_at_monado_fds() {
        let d = std::env::temp_dir().join(format!("pimax-busy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let p = d.join("123");
        std::fs::create_dir_all(p.join("fd")).unwrap();
        std::fs::write(p.join("comm"), "monado-service\n").unwrap();
        let eye = Path::new("/dev/bus/usb/001/007");
        std::os::unix::fs::symlink("/dev/hidraw3", p.join("fd/5")).unwrap();
        assert_eq!(busy_check(&d, Some(eye)), Busy::Free);
        assert!(matches!(busy_check(&d, None), Busy::Warn(_)));
        std::os::unix::fs::symlink(eye, p.join("fd/6")).unwrap();
        assert!(matches!(busy_check(&d, Some(eye)), Busy::Refuse(m) if m.contains("pid 123")));
        std::fs::remove_dir_all(p.join("fd")).unwrap();
        assert!(matches!(busy_check(&d, Some(eye)), Busy::Warn(_)));
        std::fs::remove_dir_all(&d).unwrap();
        assert_eq!(busy_check(&d, Some(eye)), Busy::Free);
    }

    #[test]
    fn session_running_finds_monado() {
        let d = std::env::temp_dir().join(format!("pimax-sess-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("77")).unwrap();
        std::fs::write(d.join("77/comm"), "bash\n").unwrap();
        assert_eq!(session_running(&d), None);
        std::fs::create_dir_all(d.join("123")).unwrap();
        std::fs::write(d.join("123/comm"), "monado-service\n").unwrap();
        assert_eq!(session_running(&d), Some(123));
        std::fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn windows_capture_58_00_mm() {
        // windows-capture-20261015.md take 6 t=10.397: 58 06 5c 05 01 a8 16 (0x16a8 = 5800 = 58.00 mm), reply 4c 04 5c 05 01 07
        assert_eq!(&motor_report(58.0)[..7], &[0x58, 0x06, 0x5c, 0x05, 0x01, 0xa8, 0x16]);
        assert_eq!(raw_from_mm(58.0), 0x16a8);
        assert_eq!(&motor_report(clamp_mm(40.0))[5..7], &[0xa8, 0x16], "below the range is clamped to 58.00");
        assert_eq!(&motor_report(clamp_mm(90.0))[5..7], &7200u16.to_le_bytes());
    }

    #[test]
    fn frames_match_capture() {
        let r = motor_report(60.0);
        assert_eq!(&r[..7], &[0x58, 0x06, 0x5c, 0x05, 0x01, 0x70, 0x17]);
        assert!(r[7..].iter().all(|&b| b == 0) && r.len() == 64);
        assert_eq!(&motor_report(63.5)[..7], &[0x58, 0x06, 0x5c, 0x05, 0x01, 0xce, 0x18]); // 6350 = 0x18ce
    }

    #[test]
    fn frame_65_9_is_the_captured_one() {
        assert_eq!(&motor_report(65.9)[..7], &[0x58, 0x06, 0x5c, 0x05, 0x01, 0xbe, 0x19]);
    }

    #[test]
    fn clamping() {
        assert_eq!(clamp_mm(50.0), 58.0);
        assert_eq!(clamp_mm(80.0), 72.0);
        assert_eq!(clamp_mm(63.5), 63.5);
        let (t, note) = decide(&m(0, 0.0, 0.0), Some(80.0), None).unwrap();
        assert_eq!(t, 72.0);
        assert!(note.is_some());
    }

    #[test]
    fn gating() {
        assert!(decide(&m(99, 63.0, 63.0), None, None).is_err());
        assert_eq!(decide(&m(100, 63.0, 62.0), None, None).unwrap().0, 63.0);
        assert!(decide(&m(500, 63.0, 59.9), None, None).is_err());
        assert!(decide(&m(500, 63.0, 60.0), None, None).is_ok());
        // --mm skips both checks
        assert_eq!(decide(&m(0, 0.0, 50.0), Some(61.0), Some(66.0)).unwrap().0, 61.0);
        // out of range or far from the lens: refuse instead of clamping (live run: 52.95 mm measured, lens 66.01 mm)
        let e = decide(&m(706, 52.95, 52.04), None, Some(66.01)).unwrap_err();
        assert!(e.contains("looks wrong"), "{e}");
        assert!(decide(&m(500, 40.0, 40.0), None, None).unwrap_err().contains("looks wrong"));
        assert!(decide(&m(500, 73.0, 73.0), None, None).is_err());
        assert!(decide(&m(500, 62.0, 62.0), None, Some(66.5)).unwrap_err().contains("lens IPD"));
        assert_eq!(decide(&m(500, 62.5, 62.5), None, Some(66.0)).unwrap().0, 62.5);
        // --mm still overrides, and is clamped
        assert_eq!(decide(&m(706, 52.95, 52.04), Some(66.0), Some(66.01)).unwrap().0, 66.0);
        assert_eq!(decide(&m(706, 52.95, 52.04), Some(50.0), Some(66.01)).unwrap().0, 58.0);
    }

    #[test]
    fn reads_lens_ipd() {
        let mut r = [0u8; 64];
        r[0] = 0x53;
        r[1] = 0x31;
        r[45] = 0x70;
        r[46] = 0x17;
        assert_eq!(lens_ipd_from_report(&r), Some(60.0));
        r[0] = 0;
        assert_eq!(lens_ipd_from_report(&r), None);
        assert!(arrived(60.3, 60.0) && !arrived(61.0, 60.0));
    }

    #[test]
    fn rejects_bad_right_eye_state() {
        let mut me = Measure::default();
        for i in 0..200 {
            let j = (i % 5) as f32 * 0.1;
            me.push(&sample(-31.0 + j, 31.0 + j, 62.0));
        }
        for _ in 0..40 {
            me.push(&sample(-31.0, 45.0, 62.0)); // bad: 14 mm jump
        }
        let r = me.result();
        assert_eq!(r.valid, 240);
        assert_eq!(r.good, 200);
        assert!((r.origin_mm - 62.0).abs() < 0.3 && (r.field34_mm - 62.0).abs() < 1e-3);
    }
}
