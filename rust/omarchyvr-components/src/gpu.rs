//! The GPU safety gate (plan section 4.3): one classifier for kernel and Monado log lines, and the gate latched per
//! `boot_id` in `$XDG_RUNTIME_DIR/omarchyvrd/gpu-gate`.
//!
//! * caution: an Xid 31 raised by WayVR's own process. User decision 2026-10-15: warn only, it blocks nothing.
//! * needs-reboot: any other Xid (or one without a process name), "Idling display engine timed out", "Stuck layer
//!   channel", a display acquire failure / VK_ERROR_INITIALIZATION_FAILED / a stuck flip in the Monado log, Monado
//!   still alive after its stop grace, in D state, or SIGKILLed. No GPU-client start (automatic or manual) without
//!   `--force` until a reboot.
use crate::model::GateLevel;
use std::path::Path;

/// A needs-reboot raised ONLY by process-kill evidence (Monado SIGKILLed, or still alive after its stop grace) is
/// lowered to caution after Monado has run this long, healthy and without new GPU-fault evidence (seconds).
pub const CLEAN_SESSION_S: f64 = 300.0;

/// Identity of one Monado process (pid, start ticks): a restart is a new key.
pub type MonadoKey = (u32, u64);

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
    /// Any GPU-fault evidence was seen (Xid, display engine, stuck layer, D state, acquire failure ...). False: the
    /// level comes from process kills only. Files without a `cause=` key load as true (strict).
    pub fault: bool,
    /// Runtime only: the Monado process the clean window follows, and when the window began.
    clean: Option<(MonadoKey, f64)>,
    /// Runtime only: the last Monado seen, and the one that was alive when a kill was latched (it never counts).
    last_key: Option<MonadoKey>,
    skip_key: Option<MonadoKey>,
}

impl Gate {
    pub fn new(boot_id: &str) -> Gate {
        Gate { boot_id: boot_id.into(), ..Default::default() }
    }

    /// The cause as written to the file.
    pub fn cause(&self) -> &'static str {
        if self.fault {
            "gpu-fault"
        } else {
            "process-kill"
        }
    }

    /// Needs-reboot from process kills alone (the only level a clean session can release).
    pub fn kill_only(&self) -> bool {
        self.level == GateLevel::NeedsReboot && !self.fault
    }

    /// Raise by process-kill evidence (Monado SIGKILLed / alive after its stop grace): not GPU-fault evidence.
    pub fn raise_kill(&mut self, level: GateLevel, evidence: &str) -> bool {
        let fault = self.fault;
        let up = self.raise(level, evidence);
        self.fault = fault;
        self.skip_key = self.last_key;
        up
    }

    /// Follow Monado for the evidence-based release. `monado`: the running, healthy Monado (None: down or unwell).
    /// A restart or an unhealthy moment restarts the window. Returns true when the level was lowered.
    pub fn clean_tick(&mut self, now: f64, monado: Option<MonadoKey>) -> bool {
        let Some(key) = monado else {
            self.clean = None;
            return false;
        };
        self.last_key = Some(key);
        if self.skip_key == Some(key) {
            return false;
        }
        let since = match self.clean {
            Some((k, t)) if k == key => t,
            _ => {
                self.clean = Some((key, now));
                now
            }
        };
        if self.kill_only() && now - since >= CLEAN_SESSION_S {
            self.level = GateLevel::Caution;
            self.acked = false;
            if self.evidence.len() >= 8 {
                self.evidence.remove(0);
            }
            self.evidence.push(format!("a clean {}-minute session after the kill: lowered to caution", (CLEAN_SESSION_S / 60.0).round() as u32));
            self.clean = None;
            return true;
        }
        false
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
        self.fault = true;
        self.clean = None;
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
        // the format of omarchyvr-tools vr/gate.rs (boot_id, level, why) plus omarchyvrd's own keys (acked, evidence)
        let mut s = format!(
            "boot_id={}\nlevel={}\nwhy={}\nacked={}\ncause={}\n",
            self.boot_id,
            self.level.name(),
            self.evidence.last().map(|e| e.replace('\n', " ")).unwrap_or_default(),
            self.acked,
            self.cause()
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
        g.fault = true; // no cause= key: strict unless every evidence line is a process kill (below)
        let has_cause = text.lines().any(|l| l.starts_with("cause="));
        for l in text.lines() {
            match l.split_once('=') {
                Some(("boot_id", v)) => boot = v.to_string(),
                Some(("level", v)) => g.level = GateLevel::parse(v).unwrap_or_default(),
                Some(("acked", v)) => g.acked = v == "true",
                Some(("cause", v)) => g.fault = v.trim() != "process-kill",
                Some(("evidence", v)) => g.evidence.push(v.to_string()),
                Some(("why", v)) if !v.is_empty() => why = Some(v.to_string()),
                _ => {}
            }
        }
        if boot != current_boot || current_boot.is_empty() {
            return Gate::new(current_boot);
        }
        if g.level == GateLevel::Ok {
            g.fault = false;
        }
        // written by the launcher (no evidence= lines): its why= is the evidence
        if g.evidence.is_empty() {
            g.evidence.extend(why);
        }
        // a file from before the cause= key: kill-only when every evidence line is a Monado kill or hang
        if !has_cause && !g.evidence.is_empty() && g.evidence.iter().all(|e| is_kill_evidence(e)) {
            g.fault = false;
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

/// Process-kill evidence written before the `cause=` key existed: Monado SIGKILLed, hung after SIGINT, still alive
/// after its stop grace, or not exiting after an automatic stop. Anything else (Xid, display engine, D state) is a fault.
fn is_kill_evidence(e: &str) -> bool {
    let e = e.to_ascii_lowercase();
    let kill = e.contains("was sigkilled") || e.contains("hung after sigint") || e.contains("still alive after its stop grace") || e.contains("did not exit and was not killed");
    let fault = e.contains("xid") || e.contains("display engine") || e.contains("stuck layer") || e.contains("d state") || e.contains("uninterruptible");
    kill && !fault
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
        assert_eq!(crash_line("kernel: traps: omarchyvr-voice[99] general protection fault ip:1"), Some(("omarchyvr-voice".into(), 99)));
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
        // the launcher's latch (omarchyvr-tools vr/gate.rs) reads the same file
        let launcher = Gate::from_text("boot_id=b1\nlevel=needs-reboot\nwhy=Monado hung after its stop grace\n", "b1");
        assert_eq!(launcher.level, GateLevel::NeedsReboot);
        assert_eq!(launcher.evidence, ["Monado hung after its stop grace"]);
        assert!(g.to_text().contains("\nwhy="));
        let d = std::env::temp_dir().join(format!("omarchyvr-gate-test-{}", std::process::id()));
        let p = d.join("gpu-gate");
        g.save(&p).unwrap();
        assert_eq!(Gate::load(&p, "b1"), g);
        let _ = std::fs::remove_dir_all(&d);
    }

    const KILL: &str = "monado-service was SIGKILLed (pid 43968)";
    const M1: Option<MonadoKey> = Some((100, 1));
    const M2: Option<MonadoKey> = Some((200, 2));

    fn killed() -> Gate {
        let mut g = Gate::new("b1");
        assert!(g.raise_kill(GateLevel::NeedsReboot, KILL));
        assert!(g.kill_only());
        g
    }

    #[test]
    fn kill_only_is_released_by_a_clean_session_and_then_ackable() {
        let mut g = killed();
        assert!(!g.clean_tick(1000.0, M1));
        assert!(!g.clean_tick(1000.0 + CLEAN_SESSION_S - 1.0, M1), "under 5 minutes: unchanged");
        assert_eq!(g.level, GateLevel::NeedsReboot);
        assert!(g.clean_tick(1000.0 + CLEAN_SESSION_S, M1));
        assert_eq!(g.level, GateLevel::Caution);
        assert!(!g.acked);
        assert!(g.evidence.last().unwrap().contains("lowered to caution"));
        assert_eq!(g.effective(), GateLevel::Caution);
        assert!(g.allows_gpu_start(true, false));
        assert!(g.ack());
        assert_eq!(g.effective(), GateLevel::Ok);
        let t = g.to_text();
        assert!(t.contains("cause=process-kill"));
        assert_eq!(Gate::from_text(&t, "b1").level, GateLevel::Caution);
    }

    #[test]
    fn kill_plus_a_gpu_fault_stays_needs_reboot() {
        let mut g = killed();
        g.raise(GateLevel::NeedsReboot, FELL);
        assert!(!g.kill_only());
        g.clean_tick(0.0, M1);
        assert!(!g.clean_tick(10_000.0, M1));
        assert_eq!(g.level, GateLevel::NeedsReboot);
        // the fault before the kill counts too
        let mut g = Gate::new("b1");
        g.raise(GateLevel::NeedsReboot, FELL);
        g.raise_kill(GateLevel::NeedsReboot, KILL);
        g.clean_tick(0.0, M1);
        assert!(!g.clean_tick(10_000.0, M1));
    }

    #[test]
    fn a_monado_restart_or_an_unwell_monado_resets_the_clean_timer() {
        let mut g = killed();
        g.clean_tick(0.0, M1);
        g.clean_tick(250.0, M1);
        assert!(!g.clean_tick(290.0, M2), "a new process starts a new window");
        assert!(!g.clean_tick(500.0, M2));
        assert!(g.clean_tick(290.0 + CLEAN_SESSION_S, M2));
        let mut g = killed();
        g.clean_tick(0.0, M1);
        g.clean_tick(200.0, None);
        assert!(!g.clean_tick(350.0, M1), "down in between");
        assert!(g.clean_tick(350.0 + CLEAN_SESSION_S, M1));
    }

    #[test]
    fn the_monado_alive_at_the_kill_does_not_count() {
        let mut g = Gate::new("b1");
        g.clean_tick(0.0, M1);
        g.raise_kill(GateLevel::NeedsReboot, "monado-service hung after SIGINT");
        assert!(!g.clean_tick(10_000.0, M1));
        assert!(!g.clean_tick(10_000.0, M2));
        assert!(g.clean_tick(10_000.0 + CLEAN_SESSION_S, M2));
    }

    #[test]
    fn an_old_file_without_the_cause_key_is_strict_unless_its_evidence_is_only_a_kill() {
        // the live file of 2026-10-17 13:29: only a SIGKILL, so it may be released by a clean session
        let old = "boot_id=b1\nlevel=needs-reboot\nwhy=monado-service was SIGKILLed (pid 43968)\nacked=false\nevidence=monado-service was SIGKILLed (pid 43968)\n";
        let g = Gate::from_text(old, "b1");
        assert!(!g.fault && g.kill_only());
        // with a GPU fault next to the kill, or unknown evidence, it stays strict
        for extra in ["evidence=NVRM: Xid (PCI:0000:01:00): 31", "evidence=something unknown"] {
            let mut g = Gate::from_text(&format!("{old}{extra}\n"), "b1");
            assert!(g.fault && !g.kill_only(), "{extra}");
            g.clean_tick(0.0, M1);
            assert!(!g.clean_tick(10_000.0, M1));
            assert_eq!(g.level, GateLevel::NeedsReboot);
        }
    }

    #[test]
    fn a_new_fault_after_the_release_raises_again() {
        let mut g = killed();
        g.clean_tick(0.0, M1);
        assert!(g.clean_tick(CLEAN_SESSION_S, M1));
        assert!(g.raise(GateLevel::NeedsReboot, FELL));
        assert_eq!(g.effective(), GateLevel::NeedsReboot);
        assert!(!g.kill_only());
        assert!(!g.ack());
        // a fault resets the window of a later kill-only level as well
        let mut g = killed();
        g.clean_tick(0.0, M1);
        g.raise(GateLevel::Caution, WAYVR);
        assert!(!g.kill_only(), "any fault evidence makes it strict");
    }
}
