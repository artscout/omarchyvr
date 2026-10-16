//! The GPU safety gate (plan section 4.3): one classifier for kernel and Monado log lines, and the gate latched per
//! `boot_id` in `$XDG_RUNTIME_DIR/pimaxd/gpu-gate`.
//!
//! * caution: an Xid 31 raised by WayVR's own process. User decision 2026-10-15: warn only, it blocks nothing.
//! * needs-reboot: any other Xid (or one without a process name), "Idling display engine timed out", "Stuck layer
//!   channel", a display acquire failure / VK_ERROR_INITIALIZATION_FAILED / a stuck flip in the Monado log, Monado
//!   still alive after its stop grace, in D state, or SIGKILLed. No GPU-client start (automatic or manual) without
//!   `--force` until a reboot.
use crate::model::GateLevel;
use std::path::Path;

/// A classified line.
#[derive(Debug, Clone, PartialEq)]
pub enum GpuEvent {
    Xid { code: u32, pid: Option<u32>, name: Option<String> },
    DisplayEngineIdle,
    StuckLayer,
    AcquireFailed,
    FlipStuck,
}

impl GpuEvent {
    pub fn level(&self) -> GateLevel {
        match self {
            GpuEvent::Xid { code: 31, name: Some(n), .. } if n == "wayvr" => GateLevel::Caution,
            _ => GateLevel::NeedsReboot,
        }
    }
    pub fn summary(&self) -> String {
        match self {
            GpuEvent::Xid { code, pid, name } => {
                let who = match (name, pid) {
                    (Some(n), Some(p)) => format!(" from {n} (pid {p})"),
                    (Some(n), None) => format!(" from {n}"),
                    _ => String::new(),
                };
                format!("Xid {code}{who}")
            }
            GpuEvent::DisplayEngineIdle => "Idling display engine timed out".into(),
            GpuEvent::StuckLayer => "Stuck layer channel".into(),
            GpuEvent::AcquireFailed => "display acquire failed (vkAcquireDrmDisplayEXT / VK_ERROR_INITIALIZATION_FAILED)".into(),
            GpuEvent::FlipStuck => "flip stuck".into(),
        }
    }
}

/// Classify a kernel log line (`journalctl -k` / dmesg).
pub fn kernel_line(line: &str) -> Option<GpuEvent> {
    if let Some(i) = line.find("Xid") {
        // "NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel ..." | "Xid 79" | "Xid: 13"
        let rest = &line[i + 3..];
        let rest = match rest.find("):") {
            Some(j) if rest.trim_start().starts_with('(') => &rest[j + 2..],
            _ => rest,
        };
        let digits: String = rest.trim_start_matches([' ', ':']).chars().take_while(|c| c.is_ascii_digit()).collect();
        let code = digits.parse().ok()?;
        let field = |k: &str| -> Option<String> {
            let j = rest.find(k)?;
            let v: String = rest[j + k.len()..].chars().take_while(|&c| c != ',' && c != ' ').collect();
            let v = v.trim_matches('\'').to_string();
            (!v.is_empty() && !v.starts_with('<')).then_some(v)
        };
        return Some(GpuEvent::Xid { code, pid: field("pid=").and_then(|p| p.parse().ok()), name: field("name=") });
    }
    if line.contains("Idling display engine") {
        return Some(GpuEvent::DisplayEngineIdle);
    }
    if line.contains("Stuck layer channel") {
        return Some(GpuEvent::StuckLayer);
    }
    None
}

/// Classify a Monado log line.
pub fn monado_line(line: &str) -> Option<GpuEvent> {
    let l = line;
    if ((l.contains("vkAcquireDrmDisplayEXT") || l.contains("acquire_drm_display")) && (l.contains("fail") || l.contains("ERROR") || l.contains("VK_ERROR")))
        || l.contains("VK_ERROR_INITIALIZATION_FAILED")
    {
        return Some(GpuEvent::AcquireFailed);
    }
    if l.contains("flip stuck") || l.contains("flipsafe: stuck") {
        return Some(GpuEvent::FlipStuck);
    }
    None
}

/// A kernel line naming a crashed process: `comm[pid]: segfault at ...` or `traps: comm[pid] ...`.
/// Returns (comm, pid).
pub fn crash_line(line: &str) -> Option<(String, u32)> {
    let key = if line.contains("segfault at") {
        "segfault at"
    } else if line.contains("traps: ") {
        "traps: "
    } else if line.contains("general protection") {
        "general protection"
    } else {
        return None;
    };
    let head: &str = if key == "traps: " { &line[line.find(key)? + key.len()..] } else { &line[..line.find(key)?] };
    // the last "name[pid]" token before the key (or the first after "traps: ")
    let tok = if key == "traps: " { head.split_whitespace().next()? } else { head.split_whitespace().last()? };
    let tok = tok.trim_end_matches(':');
    let (name, pid) = tok.rsplit_once('[')?;
    let pid: u32 = pid.trim_end_matches(']').parse().ok()?;
    Some((name.to_string(), pid))
}

/// A USB disconnect line (`usb 2-4: USB disconnect, device number 7`): the port.
pub fn usb_disconnect_line(line: &str) -> Option<String> {
    let i = line.find("USB disconnect")?;
    let head = line[..i].trim_end().trim_end_matches(':');
    let port = head.rsplit("usb ").next()?.trim();
    (!port.is_empty()).then(|| port.to_string())
}

/// `/proc/sys/kernel/random/boot_id` under `proc_root`.
pub fn boot_id(proc_root: &Path) -> String {
    std::fs::read_to_string(proc_root.join("sys/kernel/random/boot_id")).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The latched gate.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Gate {
    pub level: GateLevel,
    pub boot_id: String,
    /// The lines that raised it (at most 8, newest last).
    pub evidence: Vec<String>,
    /// The user acknowledged a caution (it is shown as ok but the evidence stays).
    pub acked: bool,
}

impl Gate {
    pub fn new(boot_id: &str) -> Gate {
        Gate { boot_id: boot_id.into(), ..Default::default() }
    }

    /// The effective level (an acknowledged caution reads as ok).
    pub fn effective(&self) -> GateLevel {
        if self.level == GateLevel::Caution && self.acked {
            GateLevel::Ok
        } else {
            self.level
        }
    }

    /// Raise to `level` (never lowers). True when the effective level went up.
    pub fn raise(&mut self, level: GateLevel, evidence: &str) -> bool {
        let before = self.effective();
        if level == GateLevel::Ok {
            return false;
        }
        if self.evidence.len() >= 8 {
            self.evidence.remove(0);
        }
        self.evidence.push(evidence.chars().take(300).collect());
        if level > self.level {
            self.level = level;
            if level == GateLevel::NeedsReboot {
                self.acked = false;
            }
        } else if level == GateLevel::Caution && self.level == GateLevel::Caution {
            // a new WayVR fault needs a new acknowledgement
            self.acked = false;
        }
        self.effective() > before
    }

    /// The user acknowledged a caution. needs-reboot is never acknowledged away.
    pub fn ack(&mut self) -> bool {
        if self.level == GateLevel::Caution {
            self.acked = true;
            true
        } else {
            false
        }
    }

    /// May a GPU client start? `force`: the user passed `--force` (allowed past needs-reboot, with a warning).
    pub fn allows_gpu_start(&self, automatic: bool, force: bool) -> bool {
        match self.effective() {
            GateLevel::Ok | GateLevel::Caution => true,
            GateLevel::NeedsReboot => !automatic && force,
        }
    }

    pub fn to_text(&self) -> String {
        // the format of pimax-tools vr/gate.rs (boot_id, level, why) plus pimaxd's own keys (acked, evidence)
        let mut s = format!(
            "boot_id={}\nlevel={}\nwhy={}\nacked={}\n",
            self.boot_id,
            self.level.name(),
            self.evidence.last().map(|e| e.replace('\n', " ")).unwrap_or_default(),
            self.acked
        );
        for e in &self.evidence {
            s.push_str("evidence=");
            s.push_str(&e.replace('\n', " "));
            s.push('\n');
        }
        s
    }

    /// Parse the latch file; a different boot means a fresh gate.
    pub fn from_text(text: &str, current_boot: &str) -> Gate {
        let mut g = Gate::new(current_boot);
        let mut boot = String::new();
        let mut why = None;
        for l in text.lines() {
            match l.split_once('=') {
                Some(("boot_id", v)) => boot = v.to_string(),
                Some(("level", v)) => g.level = GateLevel::parse(v).unwrap_or_default(),
                Some(("acked", v)) => g.acked = v == "true",
                Some(("evidence", v)) => g.evidence.push(v.to_string()),
                Some(("why", v)) if !v.is_empty() => why = Some(v.to_string()),
                _ => {}
            }
        }
        if boot != current_boot || current_boot.is_empty() {
            return Gate::new(current_boot);
        }
        // written by the launcher (no evidence= lines): its why= is the evidence
        if g.evidence.is_empty() {
            g.evidence.extend(why);
        }
        g
    }

    pub fn load(path: &Path, current_boot: &str) -> Gate {
        std::fs::read_to_string(path).map(|t| Gate::from_text(&t, current_boot)).unwrap_or_else(|_| Gate::new(current_boot))
    }

    /// Atomic write.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        crate::util::write_atomic(path, self.to_text().as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAYVR: &str = "Oct 14 14:20:00 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel 0x00000016, intr 00000000. MMU Fault";
    const GAME: &str = "Oct 13 23:16:30 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=128586, name=DoE.exe, channel 0x0000001a";
    const FELL: &str = "Oct 13 09:01:02 mau kernel: NVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.";
    const UNKNOWN: &str = "kernel: NVRM: Xid (PCI:0000:01:00): 31, pid='<unknown>', name=<unknown>, channel 0x1";

    #[test]
    fn classifies_kernel_lines() {
        assert_eq!(
            kernel_line(WAYVR),
            Some(GpuEvent::Xid { code: 31, pid: Some(2499196), name: Some("wayvr".into()) })
        );
        assert_eq!(kernel_line(WAYVR).unwrap().level(), GateLevel::Caution);
        assert_eq!(kernel_line(GAME).unwrap().level(), GateLevel::NeedsReboot);
        assert_eq!(kernel_line(FELL), Some(GpuEvent::Xid { code: 79, pid: None, name: None }));
        assert_eq!(kernel_line(UNKNOWN).unwrap(), GpuEvent::Xid { code: 31, pid: None, name: None });
        assert_eq!(kernel_line(UNKNOWN).unwrap().level(), GateLevel::NeedsReboot);
        assert_eq!(kernel_line("kernel: NVRM: Xid 13").unwrap().level(), GateLevel::NeedsReboot);
        assert_eq!(kernel_line("nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x0000c67e:0:0:1128"), Some(GpuEvent::DisplayEngineIdle));
        assert_eq!(kernel_line("nvidia-modeset: Stuck layer channel"), Some(GpuEvent::StuckLayer));
        assert_eq!(kernel_line("usb 2-4: new SuperSpeed USB device"), None);
        assert_eq!(kernel_line(WAYVR).unwrap().summary(), "Xid 31 from wayvr (pid 2499196)");
    }

    #[test]
    fn classifies_monado_and_crash_lines() {
        assert_eq!(monado_line("ERROR [vk_acquire] vkAcquireDrmDisplayEXT failed: VK_ERROR_INITIALIZATION_FAILED"), Some(GpuEvent::AcquireFailed));
        assert_eq!(monado_line("comp: flip stuck for 2000 ms"), Some(GpuEvent::FlipStuck));
        assert_eq!(monado_line("gyro bias done: 0.1 0.2"), None);
        assert_eq!(crash_line("kernel: wayvr[1234]: segfault at 0 ip 000 sp 000 error 4 in libx.so"), Some(("wayvr".into(), 1234)));
        assert_eq!(crash_line("kernel: traps: pimax-voice[99] general protection fault ip:1"), Some(("pimax-voice".into(), 99)));
        assert_eq!(crash_line("kernel: usb 2-4: USB disconnect"), None);
        assert_eq!(usb_disconnect_line("[ 812.1] usb 2-4: USB disconnect, device number 7"), Some("2-4".into()));
        assert_eq!(usb_disconnect_line("usb 2-4: new device"), None);
    }

    #[test]
    fn the_gate_latches_per_boot() {
        let mut g = Gate::new("b1");
        assert!(g.allows_gpu_start(true, false));
        assert!(g.raise(GateLevel::Caution, WAYVR));
        assert_eq!(g.effective(), GateLevel::Caution);
        assert!(g.allows_gpu_start(true, false), "caution warns only");
        assert!(g.ack());
        assert_eq!(g.effective(), GateLevel::Ok);
        assert!(g.raise(GateLevel::Caution, WAYVR), "a new fault needs a new ack");
        assert!(g.raise(GateLevel::NeedsReboot, FELL));
        assert!(!g.ack());
        assert!(!g.raise(GateLevel::Caution, WAYVR), "never lowers");
        assert!(!g.allows_gpu_start(true, true), "automatic never passes needs-reboot");
        assert!(!g.allows_gpu_start(false, false));
        assert!(g.allows_gpu_start(false, true), "--force passes with a warning");
        let t = g.to_text();
        assert_eq!(Gate::from_text(&t, "b1"), g);
        assert_eq!(Gate::from_text(&t, "b2"), Gate::new("b2"), "a reboot clears it");
        // the launcher's latch (pimax-tools vr/gate.rs) reads the same file
        let launcher = Gate::from_text("boot_id=b1\nlevel=needs-reboot\nwhy=Monado hung after its stop grace\n", "b1");
        assert_eq!(launcher.level, GateLevel::NeedsReboot);
        assert_eq!(launcher.evidence, ["Monado hung after its stop grace"]);
        assert!(g.to_text().contains("\nwhy="));
        let d = std::env::temp_dir().join(format!("pimax-gate-test-{}", std::process::id()));
        let p = d.join("gpu-gate");
        g.save(&p).unwrap();
        assert_eq!(Gate::load(&p, "b1"), g);
        let _ = std::fs::remove_dir_all(&d);
    }
}
