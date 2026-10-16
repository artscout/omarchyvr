//! The GPU safety gate of the automatic paths (resilience plan section 4.3, P0).
//!
//! Levels: `ok`; `caution` (an Xid 31 raised by WayVR's own process: warn, do not block, user decision 3);
//! `needs-reboot` (any other Xid or an Xid without a process name, "Idling display engine", "Stuck layer channel",
//! a display acquire failure in the Monado log, Monado hung after its stop grace or SIGKILLed). After needs-reboot
//! nothing restarts the session automatically: a forced kill or a stuck flip leaves the connector leased to a dead
//! file, and every later `vkAcquireDrmDisplayEXT` fails until a reboot (`analysis/nvidia-flip-hang.md` section 0).
//!
//! The verdict is latched in `$XDG_RUNTIME_DIR/omarchyvrd/gpu-gate` together with the boot id, so it clears only on a
//! reboot (the file is on tmpfs, and a latch of another boot is ignored). File format, one `key=value` per line:
//! `boot_id=<id>`, `level=caution|needs-reboot`, `why=<one line>`. A latch is only ever raised, never lowered.
//! omarchyvrd (P4) reads and writes the same file.
use std::path::{Path, PathBuf};

/// Gate level, ordered: a higher level wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Ok,
    Caution,
    NeedsReboot,
}

impl Level {
    pub fn as_str(self) -> &'static str {
        match self {
            Level::Ok => "ok",
            Level::Caution => "caution",
            Level::NeedsReboot => "needs-reboot",
        }
    }
    fn parse(s: &str) -> Option<Self> {
        match s {
            "ok" => Some(Level::Ok),
            "caution" => Some(Level::Caution),
            "needs-reboot" => Some(Level::NeedsReboot),
            _ => None,
        }
    }
}

/// `name=` of an NVRM Xid line (`..., pid=2499196, name=wayvr, channel ...`).
fn xid_name(line: &str) -> Option<&str> {
    let i = line.find("name=")? + 5;
    let rest = &line[i..];
    let end = rest.find(',').unwrap_or(rest.len());
    Some(rest[..end].trim())
}

/// Xid code of an NVRM line: `Xid (PCI:0000:01:00): 31, ...`, `Xid: 13`, `Xid 79`.
fn xid_code(line: &str) -> Option<u32> {
    let rest = &line[line.find("Xid")? + 3..];
    let rest = match rest.trim_start().strip_prefix('(') {
        Some(r) => &r[r.find(')')? + 1..],
        None => rest,
    };
    let digits: String = rest
        .trim_start_matches(|c: char| c == ':' || c.is_whitespace())
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// One kernel log line.
pub fn classify_kernel_line(line: &str) -> Level {
    if line.contains("Idling display engine") || line.contains("Stuck layer channel") {
        return Level::NeedsReboot;
    }
    if !line.contains("Xid") {
        return Level::Ok;
    }
    if xid_code(line) == Some(31) && xid_name(line) == Some("wayvr") {
        Level::Caution
    } else {
        Level::NeedsReboot
    }
}

/// One Monado log line: the display acquire failure (`vkAcquireDrmDisplayEXT failed: VK_ERROR_INITIALIZATION_FAILED`).
pub fn classify_monado_line(line: &str) -> Level {
    let acquire = line.contains("vkAcquireDrmDisplayEXT") || line.contains("acquire_drm_display");
    if acquire && (line.contains("fail") || line.contains("VK_ERROR")) {
        Level::NeedsReboot
    } else {
        Level::Ok
    }
}

/// Worst level of a set of lines and the first line at that level.
pub fn worst<'a>(lines: impl IntoIterator<Item = &'a str>, f: fn(&str) -> Level) -> (Level, Option<String>) {
    let mut best = (Level::Ok, None);
    for l in lines {
        let lv = f(l);
        if lv > best.0 {
            best = (lv, Some(l.trim().to_string()));
        }
    }
    best
}

/// `$XDG_RUNTIME_DIR/omarchyvrd/gpu-gate` (`None` without XDG_RUNTIME_DIR: nothing is latched then).
pub fn gate_path(env: &dyn Fn(&str) -> Option<String>) -> Option<PathBuf> {
    env("XDG_RUNTIME_DIR")
        .filter(|s| !s.is_empty())
        .map(|d| PathBuf::from(d).join("omarchyvrd").join("gpu-gate"))
}

/// This boot's id (`<proc>/sys/kernel/random/boot_id`; `PIMAX_PROC_ROOT` replaces `/proc`).
pub fn boot_id(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let root = env("PIMAX_PROC_ROOT").filter(|s| !s.is_empty()).unwrap_or_else(|| "/proc".into());
    let s = std::fs::read_to_string(Path::new(&root).join("sys/kernel/random/boot_id")).ok()?;
    let s = s.trim();
    (!s.is_empty()).then(|| s.to_string())
}

/// The latch of this boot, if any.
pub fn read_latch(path: &Path, boot_id: &str) -> Option<(Level, String)> {
    let s = std::fs::read_to_string(path).ok()?;
    let (mut b, mut lv, mut why) = (None, None, String::new());
    for l in s.lines() {
        match l.split_once('=') {
            Some(("boot_id", v)) => b = Some(v.trim()),
            Some(("level", v)) => lv = Level::parse(v.trim()),
            Some(("why", v)) => why = v.trim().to_string(),
            _ => {}
        }
    }
    (b == Some(boot_id)).then_some(())?;
    lv.filter(|l| *l > Level::Ok).map(|l| (l, why))
}

/// Raise the latch to `level` (never lowers it). Best effort: the file is a record for the next automatic action.
pub fn latch(path: &Path, boot_id: &str, level: Level, why: &str) -> bool {
    if level == Level::Ok || read_latch(path, boot_id).is_some_and(|(l, _)| l >= level) {
        return false;
    }
    if let Some(dir) = path.parent() {
        if std::fs::create_dir_all(dir).is_err() {
            return false;
        }
    }
    let why = why.replace(['\n', '\r'], " ");
    let tmp = path.with_extension("tmp");
    let body = format!("boot_id={boot_id}\nlevel={}\nwhy={why}\n", level.as_str());
    std::fs::write(&tmp, body).is_ok() && std::fs::rename(&tmp, path).is_ok()
}

/// Latch through the caller's environment (`XDG_RUNTIME_DIR`, `PIMAX_PROC_ROOT`); no-op when either is unknown.
pub fn latch_env(env: &dyn Fn(&str) -> Option<String>, level: Level, why: &str) -> bool {
    match (gate_path(env), boot_id(env)) {
        (Some(p), Some(b)) => latch(&p, &b, level, why),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAYVR: &str = "Oct 14 14:20:00 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel 0x00000016, intr 00000000. MMU Fault: ENGINE GR0_PBDMA0";
    const GAME: &str = "Oct 13 23:16:30 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=128586, name=DoE.exe, channel 0x0000001a";

    #[test]
    fn kernel_lines() {
        assert_eq!(classify_kernel_line(WAYVR), Level::Caution);
        assert_eq!(classify_kernel_line(GAME), Level::NeedsReboot);
        assert_eq!(classify_kernel_line(&WAYVR.replace("): 31,", "): 43,")), Level::NeedsReboot);
        assert_eq!(classify_kernel_line(&WAYVR.replace("name=wayvr,", "name=wayvr-dashboard,")), Level::NeedsReboot);
        assert_eq!(classify_kernel_line("kernel: NVRM: Xid (PCI:0000:01:00): 31, pid='<unknown>', name=<unknown>, channel 0x1"), Level::NeedsReboot);
        assert_eq!(classify_kernel_line("NVRM: Xid 79"), Level::NeedsReboot);
        assert_eq!(classify_kernel_line("nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x0000c67e"), Level::NeedsReboot);
        assert_eq!(classify_kernel_line("nvidia-modeset: Stuck layer channel 3 (acquire)"), Level::NeedsReboot);
        assert_eq!(classify_kernel_line("usb 2-4: USB disconnect, device number 9"), Level::Ok);
        let (lv, line) = worst([WAYVR, "x", GAME], classify_kernel_line);
        assert_eq!(lv, Level::NeedsReboot);
        assert!(line.unwrap().contains("DoE.exe"));
        assert_eq!(worst([WAYVR], classify_kernel_line).0, Level::Caution);
    }

    #[test]
    fn monado_lines() {
        assert_eq!(classify_monado_line("ERROR [vk_create_display] vkAcquireDrmDisplayEXT failed: VK_ERROR_INITIALIZATION_FAILED"), Level::NeedsReboot);
        assert_eq!(classify_monado_line("INFO [comp] vkAcquireDrmDisplayEXT ok"), Level::Ok);
        assert_eq!(classify_monado_line("gyro bias done"), Level::Ok);
    }

    #[test]
    fn latch_is_per_boot_and_only_raises() {
        let d = crate::vr::test_dir("gate-latch");
        let p = d.join("omarchyvrd/gpu-gate");
        assert_eq!(read_latch(&p, "b1"), None);
        assert!(latch(&p, "b1", Level::Caution, "Xid 31 wayvr"));
        assert_eq!(read_latch(&p, "b1"), Some((Level::Caution, "Xid 31 wayvr".into())));
        assert!(!latch(&p, "b1", Level::Caution, "again"));
        assert!(latch(&p, "b1", Level::NeedsReboot, "monado\nhung"));
        assert_eq!(read_latch(&p, "b1"), Some((Level::NeedsReboot, "monado hung".into())));
        assert!(!latch(&p, "b1", Level::Caution, "lower"));
        assert_eq!(read_latch(&p, "b1").unwrap().0, Level::NeedsReboot);
        // another boot: the old latch means nothing, and a new one replaces it
        assert_eq!(read_latch(&p, "b2"), None);
        assert!(latch(&p, "b2", Level::Caution, "new boot"));
        assert_eq!(read_latch(&p, "b1"), None);
    }

    #[test]
    fn latch_env_needs_the_runtime_dir_and_a_boot_id() {
        let d = crate::vr::test_dir("gate-env");
        let proc_root = d.join("proc");
        std::fs::create_dir_all(proc_root.join("sys/kernel/random")).unwrap();
        std::fs::write(proc_root.join("sys/kernel/random/boot_id"), "abc-123\n").unwrap();
        let run = d.join("run");
        let (r, pr) = (run.display().to_string(), proc_root.display().to_string());
        let env = move |k: &str| match k {
            "XDG_RUNTIME_DIR" => Some(r.clone()),
            "PIMAX_PROC_ROOT" => Some(pr.clone()),
            _ => None,
        };
        assert!(latch_env(&env, Level::NeedsReboot, "x"));
        assert_eq!(read_latch(&run.join("omarchyvrd/gpu-gate"), "abc-123").unwrap().0, Level::NeedsReboot);
        assert!(!latch_env(&|_: &str| None, Level::NeedsReboot, "x"));
    }
}
