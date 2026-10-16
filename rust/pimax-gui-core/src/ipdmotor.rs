//! The Optics page's IPD: Apply saves the value for Monado (`~/.config/pimax/ipd.env`) and, when no session owns the
//! headset, also moves the lenses with the Windows-captured command `58 06 5c 05 01 <mm x100 LE>` through
//! `pimax-eye ipd --set MM` (clamped to 58-72 mm, one write, then the live lens IPD is read back). The current lens
//! IPD comes from the headset's info report (`pimax-eye ipd --read`). While a session runs the page says why the lenses
//! do not move (the saved value applies at the next start). Nothing else is sent; the eye tracker is not touched.
use crate::app::AppCore;
use crate::job::{JobKind, JobSpec, Step};
use std::path::PathBuf;

/// The lens move's job kind.
pub const KIND: &str = "ipd-motor";
/// Lowest / highest IPD the lenses are driven to, mm (the approved range, `ipd_motor` in device.toml).
pub const MIN_MM: f32 = 58.0;
pub const MAX_MM: f32 = 72.0;

/// The last lens move's result for people.
#[derive(Debug, Clone, PartialEq)]
pub struct IpdResult {
    pub ok: bool,
    pub text: String,
}

/// Read the output of `pimax-eye ipd --set` into plain words.
pub fn result_from(lines: &[String], ok_exit: bool) -> IpdResult {
    let final_mm = lines.iter().rev().find_map(|l| {
        l.split_once("final lens IPD: ").and_then(|(_, r)| r.split_whitespace().next()).and_then(|n| n.parse::<f32>().ok())
    });
    let target = lines.iter().rev().find_map(|l| {
        l.split_once("-> lens IPD ").and_then(|(_, r)| r.split_whitespace().next()).and_then(|n| n.parse::<f32>().ok())
    });
    if ok_exit {
        return IpdResult { ok: true, text: format!("Lenses set to {:.1} mm.", final_mm.or(target).unwrap_or(0.0)) };
    }
    if lines.iter().any(|l| l.contains("stop VR first")) {
        return IpdResult { ok: false, text: "A VR session is running: stop it first. The value is saved and used at the next start.".into() };
    }
    if lines.iter().any(|l| l.contains("headset not found")) {
        return IpdResult { ok: false, text: "The headset was not found on USB. The value is saved and used at the next start.".into() };
    }
    match (final_mm, target) {
        (Some(f), Some(t)) => IpdResult { ok: false, text: format!("The lenses stopped at {f:.1} mm, not {t:.1} mm: try Apply again.") },
        _ => IpdResult { ok: false, text: "The lenses did not move (see the output below). The value is saved and used at the next start.".into() },
    }
}

/// `lens IPD: 61.35 mm` -> 61.35.
pub fn parse_lens_line(out: &str) -> Option<f32> {
    out.lines().find_map(|l| l.strip_prefix("lens IPD: ")).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse().ok())
}

impl AppCore {
    /// Why the lenses cannot be moved now (None: they can). The saved value still applies at the next start.
    pub fn ipd_motor_blocker(&self) -> Option<String> {
        if !self.live.monado_pids.is_empty() || self.live.service.is_some() || self.live.steamvr.is_some() {
            return Some("The lenses move only while no VR session runs: stop it first. Apply still saves the value for the next start.".into());
        }
        if self.busy() {
            return Some("Another action is running.".into());
        }
        let has = |pid: u16| self.facts.usb.contains(&(pimax_doctor::PIMAX_VID, pid));
        if self.facts.at != 0 && !has(pimax_doctor::DREAM_AIR_PID) {
            return Some("The headset is not on USB.".into());
        }
        None
    }

    /// The `pimax-eye` binary: installed, else the build.
    fn eye_bin(&self) -> PathBuf {
        let inst = self.paths.installed_bin("pimax-eye");
        if self.facts.has(&inst) {
            inst
        } else {
            self.paths.bin("pimax-eye")
        }
    }

    /// Move the lenses to `mm` (a job: `pimax-eye ipd --set`). Does not save ipd.env itself (the tool does after the
    /// lens arrived; [`AppCore::set_ipd`] saves first).
    pub fn move_lenses(&mut self, mm: f32) {
        if let Some(b) = self.ipd_motor_blocker() {
            self.message = b;
            return;
        }
        let mm = mm.clamp(MIN_MM, MAX_MM);
        self.ipd_result = None;
        let c = crate::cmd::Cmd::with_args(self.eye_bin().display().to_string(), vec!["ipd".into(), "--set".into(), format!("{mm:.2}")]);
        let spec = JobSpec {
            kind: JobKind::Other(KIND.into()),
            label: format!("Set the lenses to {mm:.1} mm"),
            steps: vec![Step::Note("The lenses move at about 1.8 mm per second.".into()), Step::Run(c)],
        };
        if self.spawn(spec) {
            self.ipd_job_from = Some(self.log.len());
        }
    }

    pub(crate) fn ipd_motor_done(&mut self, ok: bool) {
        let from = self.ipd_job_from.take().unwrap_or(0).min(self.log.len());
        self.ipd_result = Some(result_from(&self.log[from..], ok));
        if ok {
            self.lens_ipd_asked = false; // read the new position
        }
    }

    /// Is the lens move running?
    pub fn moving_lenses(&self) -> bool {
        self.job.as_ref().is_some_and(|j| j.kind == JobKind::Other(KIND.into()))
    }

    /// Read the live lens IPD once (background; `pimax-eye ipd --read`). Skipped while a session runs (it also reads
    /// the info report) or when already asked since the last change.
    pub fn read_lens_ipd(&mut self) {
        if self.lens_ipd_asked {
            return;
        }
        self.lens_ipd_asked = true;
        if self.demo {
            self.lens_ipd = self.facts.ipd_mm.or(Some(63.0));
            return;
        }
        if !self.live.monado_pids.is_empty() || self.live.service.is_some() {
            return;
        }
        let bin = self.eye_bin();
        self.task(Some("lens-ipd"), move || {
            let v = std::process::Command::new(&bin)
                .args(["ipd", "--read"])
                .stdin(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .output()
                .ok()
                .filter(|o| o.status.success())
                .and_then(|o| parse_lens_line(&String::from_utf8_lossy(&o.stdout)));
            Box::new(move |c: &mut AppCore| c.lens_ipd = v)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn results_in_plain_words() {
        let ok = l(&["lens IPD now: 61.35 mm", "sent once: 58 06 5c 05 01 a8 16 -> lens IPD 58.00 mm", "final lens IPD: 58.03 mm (OK)"]);
        assert_eq!(result_from(&ok, true), IpdResult { ok: true, text: "Lenses set to 58.0 mm.".into() });
        let short = l(&["sent once: 58 06 5c 05 01 a8 16 -> lens IPD 58.00 mm", "final lens IPD: 60.50 mm (NOT within 0.5 mm of target)"]);
        assert!(result_from(&short, false).text.contains("stopped at 60.5 mm, not 58.0 mm"));
        let busy = l(&["error: monado-service (pid 5) is running: stop VR first, the lenses are set only while no session owns the headset; nothing sent"]);
        assert!(result_from(&busy, false).text.contains("stop it first"));
        assert!(result_from(&l(&["headset not found (no hidraw node)"]), false).text.contains("not found"));
        assert!(!result_from(&l(&[]), false).ok);
    }

    #[test]
    fn blocked_while_a_session_runs_and_demo_shows_the_lens() {
        let d = std::env::temp_dir().join(format!("pimax-ipdm-{}", std::process::id()));
        let mut a = AppCore::demo(true, pimax_config::Store::new(d.join("a"), d.join("b")));
        a.live.monado_pids = vec![4242];
        assert!(a.ipd_motor_blocker().unwrap().contains("stop it first"));
        a.move_lenses(60.0);
        assert!(a.message.contains("no VR session"), "{}", a.message);
        a.live.monado_pids.clear();
        a.live.service = None;
        a.live.steamvr = None;
        a.set_ipd(61.5);
        assert_eq!(a.facts.ipd_mm, Some(61.5));
        assert_eq!(a.lens_ipd, Some(61.5));
        a.lens_ipd_asked = false;
        a.read_lens_ipd();
        assert!(a.lens_ipd.is_some());
    }

    #[test]
    fn lens_line() {
        assert_eq!(parse_lens_line("lens IPD: 61.35 mm\n"), Some(61.35));
        assert_eq!(parse_lens_line("nothing"), None);
    }
}
