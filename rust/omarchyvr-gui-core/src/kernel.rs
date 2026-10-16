//! This boot's kernel log, classified (LESSONS row 9).
//!
//! Xid 31 (MMU fault) raised by WayVR's own process is a per-application fault the GPU recovers from: yellow, and the
//! user may start anyway (`--force`). Any other Xid, an Xid without a process name, "Idling display engine timed out"
//! or "Stuck layer channel" means the display engine may be wedged: red, reboot before the next session.
//! The launcher refuses on any of them; the app adds the distinction and the consent.

/// One GPU-related kernel line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KernelEvent {
    /// `NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel ...`
    Xid {
        /// Xid number.
        code: u32,
        /// Process id, if printed.
        pid: Option<u32>,
        /// Process name, if printed (truncated to 15 characters by the kernel).
        name: Option<String>,
        /// The whole line.
        line: String,
    },
    /// `nvidia-modeset: ERROR: GPU:0: Idling display engine timed out`.
    DisplayEngine(String),
    /// `Stuck layer channel` or another "Idling" line.
    Stuck(String),
}

impl KernelEvent {
    /// The line as logged.
    pub fn line(&self) -> &str {
        match self {
            KernelEvent::Xid { line, .. } | KernelEvent::DisplayEngine(line) | KernelEvent::Stuck(line) => line,
        }
    }

    /// Xid 31 from WayVR: the only event the user may start over (with `--force`).
    pub fn is_wayvr_mmu_fault(&self) -> bool {
        matches!(self, KernelEvent::Xid { code: 31, name: Some(n), .. } if n == "wayvr")
    }

    /// Short text for the list (`Xid 31 from wayvr (pid 2499196) at Oct 14 14:20:00`).
    pub fn summary(&self) -> String {
        let when = timestamp(self.line());
        let at = if when.is_empty() { String::new() } else { format!(" at {when}") };
        match self {
            KernelEvent::Xid { code, pid, name, .. } => {
                let who = match (name, pid) {
                    (Some(n), Some(p)) => format!(" from {n} (pid {p})"),
                    (Some(n), None) => format!(" from {n}"),
                    (None, Some(p)) => format!(" from pid {p}"),
                    (None, None) => " (no process named)".into(),
                };
                format!("Xid {code}{who}{at}")
            }
            KernelEvent::DisplayEngine(_) => format!("display engine timed out{at}"),
            KernelEvent::Stuck(_) => format!("stuck display channel{at}"),
        }
    }
}

/// `Oct 14 14:20:00` from the start of a journalctl short line, if present.
fn timestamp(line: &str) -> String {
    let w: Vec<&str> = line.split_whitespace().take(3).collect();
    if w.len() == 3 && w[2].matches(':').count() == 2 && w[1].chars().all(|c| c.is_ascii_digit()) {
        w.join(" ")
    } else {
        String::new()
    }
}

/// Classify one line; `None` if it is not GPU trouble.
pub fn parse_line(line: &str) -> Option<KernelEvent> {
    let l = line.trim_end();
    if let Some(i) = l.find("Xid") {
        let rest = &l[i + 3..];
        // "Xid (PCI:0000:01:00): 31, pid=..., name=..., ..." or "Xid 79" or "Xid: 13"
        let after = match rest.find("):") {
            Some(j) if rest.trim_start().starts_with('(') => &rest[j + 2..],
            _ => rest.trim_start_matches([':', ' ']),
        };
        let code = after
            .trim_start()
            .split(|c: char| !c.is_ascii_digit())
            .next()
            .and_then(|n| n.parse::<u32>().ok());
        let Some(code) = code else {
            return Some(KernelEvent::Stuck(l.to_string()));
        };
        // values run up to the next ", " (a process name may contain spaces)
        let field = |key: &str| -> Option<String> {
            let k = after.find(key)?;
            let v = &after[k + key.len()..];
            let v = v.split(", ").next()?.trim().trim_matches(['\'', '"']);
            (!v.is_empty() && v != "<unknown>").then(|| v.to_string())
        };
        return Some(KernelEvent::Xid {
            code,
            pid: field("pid=").and_then(|p| p.parse().ok()),
            name: field("name="),
            line: l.to_string(),
        });
    }
    if l.contains("Idling display engine") {
        return Some(KernelEvent::DisplayEngine(l.to_string()));
    }
    if l.contains("Stuck layer channel") || l.contains("Idling") {
        return Some(KernelEvent::Stuck(l.to_string()));
    }
    None
}

/// All events of a log text.
pub fn scan(text: &str) -> Vec<KernelEvent> {
    text.lines().filter_map(parse_line).collect()
}

/// What the events mean for the next start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verdict {
    /// Nothing found.
    Clean,
    /// Only WayVR Xid 31 faults: yellow, the user may start anyway with `--force`.
    WayvrOnly(Vec<KernelEvent>),
    /// Anything else: red, reboot recommended (the events that caused it first, then the WayVR ones).
    Reboot(Vec<KernelEvent>),
}

/// Classify a list of events.
pub fn verdict(events: &[KernelEvent]) -> Verdict {
    if events.is_empty() {
        return Verdict::Clean;
    }
    let (benign, bad): (Vec<KernelEvent>, Vec<KernelEvent>) =
        events.iter().cloned().partition(KernelEvent::is_wayvr_mmu_fault);
    if bad.is_empty() {
        Verdict::WayvrOnly(benign)
    } else {
        let mut v = bad;
        v.extend(benign);
        Verdict::Reboot(v)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WAYVR: &str = "Oct 14 14:20:00 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=2499196, name=wayvr, channel 0x00000016, intr 00000000. MMU Fault: ENGINE GR0_PBDMA0 HUBCLIENT_ESC faulted @ 0x0_0e8e0000. Fault is of type FAULT_PTE ACCESS_TYPE_VIRT_WRITE";
    const GAME: &str = "Oct 13 23:16:30 mau kernel: NVRM: Xid (PCI:0000:01:00): 31, pid=128586, name=DoE.exe, channel 0x0000001a, intr 00000000. MMU Fault: ENGINE GRAPHICS GPC0 GPCCLIENT_PROP_0 faulted @ 0x0_82e12000. Fault is of type FAULT_PDE ACCESS_TYPE_VIRT_READ";
    const IDLE: &str = "Oct 13 23:16:54 mau kernel: nvidia-modeset: ERROR: GPU:0: Idling display engine timed out: 0x0000c67e:0:0:1174";
    const FELL: &str = "Oct 13 09:01:02 mau kernel: NVRM: Xid (PCI:0000:01:00): 79, GPU has fallen off the bus.";

    #[test]
    fn parses_the_real_line_formats() {
        match parse_line(WAYVR).unwrap() {
            KernelEvent::Xid { code, pid, name, .. } => {
                assert_eq!((code, pid, name.as_deref()), (31, Some(2_499_196), Some("wayvr")));
            }
            e => panic!("{e:?}"),
        }
        let e = parse_line(GAME).unwrap();
        assert!(!e.is_wayvr_mmu_fault());
        assert_eq!(e.summary(), "Xid 31 from DoE.exe (pid 128586) at Oct 13 23:16:30");
        assert!(matches!(parse_line(IDLE), Some(KernelEvent::DisplayEngine(_))));
        match parse_line(FELL).unwrap() {
            KernelEvent::Xid { code: 79, pid: None, name: None, .. } => {}
            e => panic!("{e:?}"),
        }
        assert!(matches!(parse_line("kernel: NVRM: Xid 13"), Some(KernelEvent::Xid { code: 13, .. })));
        assert!(matches!(
            parse_line("kernel: nvidia-modeset: Stuck layer channel 3"),
            Some(KernelEvent::Stuck(_))
        ));
        assert_eq!(parse_line("kernel: usb 1-5: new high-speed USB device"), None);
        assert_eq!(parse_line(WAYVR).unwrap().summary(), "Xid 31 from wayvr (pid 2499196) at Oct 14 14:20:00");
    }

    #[test]
    fn only_wayvr_xid31_is_a_consentable_warning() {
        assert_eq!(verdict(&[]), Verdict::Clean);
        let log = format!("{WAYVR}\nOct 14 14:22:55 mau kernel: usb 1-1: reset\n{WAYVR}\n");
        match verdict(&scan(&log)) {
            Verdict::WayvrOnly(v) => assert_eq!(v.len(), 2),
            v => panic!("{v:?}"),
        }
        for bad in [GAME, IDLE, FELL] {
            let log = format!("{WAYVR}\n{bad}\n");
            match verdict(&scan(&log)) {
                Verdict::Reboot(v) => {
                    assert_eq!(v.len(), 2);
                    assert_eq!(v[0].line(), bad, "the event that forces the reboot comes first");
                }
                v => panic!("{bad}: {v:?}"),
            }
        }
        // an Xid 31 of a process whose name is unknown is not WayVR
        let unknown = "kernel: NVRM: Xid (PCI:0000:01:00): 31, pid='<unknown>', name=<unknown>, channel 0x1";
        assert!(matches!(verdict(&scan(unknown)), Verdict::Reboot(_)));
        // only the process named exactly wayvr: wayvr-dashboard or a name with a space is another program
        for other in ["name=wayvr-dashboard,", "name=wayvr foo,", "name=wayvr_x,"] {
            let l = WAYVR.replace("name=wayvr,", other);
            assert!(matches!(verdict(&scan(&l)), Verdict::Reboot(_)), "{other}");
        }
        // other Xid codes from wayvr are not the known benign fault either
        let w43 = WAYVR.replace("): 31,", "): 43,");
        assert!(matches!(verdict(&scan(&w43)), Verdict::Reboot(_)));
    }
}
