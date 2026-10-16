//! Restart headset: `omarchyvr vr reset-headset` as a job (the headset's own restart command `58 04 28 00 00`: back on USB
//! after about 8 s, picture after about 22 s; the slow heartbeat-timeout restart, up to 2 minutes 30, only if the
//! headset ignores it), refused while Monado runs, and its verdict line ("verdict: camera back after N s" or a failure) in words for people. The
//! headset card shows "Camera missing" when the HID chip (34a4:0044) is on USB but the camera (34a4:0014) is not
//! (from the facts' USB list, read off the UI thread). The app never tells anyone to unplug anything.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::job::{JobKind, JobSpec, Step};
use std::time::Duration;

/// The camera's USB product id.
pub const CAMERA_PID: u16 = 0x0014;

/// How long a restart takes (the progress bar's end): the headset's own restart command, 22 s to the picture.
pub const RESET_SECS: u64 = 30;
/// The bar's end when the headset ignored the command and the slow restart runs (15 s wait + the 150 s watch).
pub const RESET_SLOW_SECS: u64 = 165;

/// The job's kind.
pub const KIND: &str = "reset-headset";

/// The service restart's job kind.
pub const RESTART_KIND: &str = "restart-service";

/// How long a service restart may take (the progress bar's end: a 30 s stop grace, then the start).
pub const RESTART_SECS: u64 = 120;

/// The session watchdog's pid file in the session dir (omarchyvr-tools `supervise::WD_PID`).
pub const WATCHDOG_PID: &str = "watchdog.pid";
/// Its log (`supervise::WD_LOG`).
pub const WATCHDOG_LOG: &str = "watchdog.log";
/// How the session was started (`supervise::ARGV_FILE`).
pub const ARGV_FILE: &str = "start-argv.txt";

/// What a restart ended with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResetResult {
    /// The camera is back.
    pub ok: bool,
    /// For people.
    pub text: String,
}

/// Read the launcher's verdict line (`verdict: ...`) of a reset.
pub fn verdict(lines: &[String]) -> Option<ResetResult> {
    let v = lines.iter().rev().find_map(|l| l.split_once("verdict: ").map(|(_, v)| v.trim().to_string()))?;
    let num_after = |key: &str| v.split(key).nth(1).and_then(|r| r.split_whitespace().next()).and_then(|n| n.parse::<f64>().ok());
    Some(if v.starts_with("full power cycle") {
        let back = num_after("back at ").map(|s| format!(" after {s:.0} s")).unwrap_or_default();
        ResetResult { ok: true, text: format!("Headset restarted: the camera is back{back}.") }
    } else if v.starts_with("camera back after") {
        let back = num_after("camera back after ").map(|s| format!(" after {s:.0} s")).unwrap_or_default();
        ResetResult { ok: true, text: format!("Headset restarted: the camera is back{back}.") }
    } else if v.starts_with("interrupted") {
        ResetResult { ok: false, text: "The restart was interrupted before it finished.".into() }
    } else if v.contains("video4linux") || v.contains("uvcvideo") {
        ResetResult { ok: false, text: "The camera is back on USB but its video device did not appear: try Restart headset once more.".into() }
    } else {
        ResetResult { ok: false, text: "The camera did not come back: try Restart headset once more.".into() }
    })
}

/// Plain words for one `restart: ...` line of the launcher (None: nothing yet).
pub fn stage_text(line: Option<&str>) -> &'static str {
    match line {
        None => "Sending the restart command",
        Some(l) if l.contains("ignored the restart command") => "The headset did not act on the fast restart: using the slow one (up to 2 minutes 30)",
        Some(l) if l.contains("nothing left USB") => "The headset did not act on the fast restart: using the slow one (up to 2 minutes 30)",
        Some(l) if l.contains("is up") => "The headset is up",
        Some(l) if l.contains("camera is back") => "The camera is back; the picture comes on in a moment",
        Some(l) if l.contains("back on USB") => "The headset is back on USB; waiting for the camera and the picture",
        Some(l) if l.contains("left USB") => "The headset is restarting (it is off USB for a few seconds)",
        Some(_) => "The restart command was sent; waiting for the headset to shut down",
    }
}

impl AppCore {
    /// The HID chip is on USB but the camera is not.
    pub fn headset_camera_missing(&self) -> bool {
        let has = |pid: u16| self.facts.usb.contains(&(omarchyvr_doctor::PIMAX_VID, pid));
        self.facts.at != 0 && has(omarchyvr_doctor::DREAM_AIR_PID) && !has(CAMERA_PID)
    }

    /// Why a restart cannot run now (None: it can).
    pub fn restart_headset_blocker(&self) -> Option<String> {
        if !self.live.monado_pids.is_empty() || self.live.service.is_some() {
            return Some("Stop VR / desk mode first: the headset restart is refused while Monado runs.".into());
        }
        if self.busy() {
            return Some("Another action is running.".into());
        }
        None
    }

    /// Run `omarchyvr vr reset-headset` (a job, up to about two and a half minutes).
    pub fn restart_headset(&mut self) {
        if let Some(b) = self.restart_headset_blocker() {
            self.message = b;
            return;
        }
        self.headset_reset = None;
        let c = Cmd::with_args(self.launcher_bin().display().to_string(), vec!["vr".into(), "reset-headset".into()]);
        let spec = JobSpec {
            kind: JobKind::Other(KIND.into()),
            label: "Restart headset".into(),
            steps: vec![Step::Note("The headset restarts itself: keep it still and wait (about 30 seconds).".into()), Step::Run(c)],
        };
        if self.spawn(spec) {
            self.headset_reset_from = Some(self.log.len());
        }
    }

    /// The slow restart (heartbeat timeout) runs because the headset ignored the restart command.
    fn restart_headset_slow(&self) -> bool {
        let from = self.headset_reset_from.unwrap_or(0).min(self.log.len());
        self.log[from..].iter().any(|l| l.contains("ignored the restart command"))
    }

    /// The running restart's progress (0..1), if one runs.
    pub fn restart_headset_progress(&self) -> Option<f32> {
        let j = self.job.as_ref().filter(|j| j.kind == JobKind::Other(KIND.into()))?;
        let end = if self.restart_headset_slow() { RESET_SLOW_SECS } else { RESET_SECS };
        Some((j.started.elapsed().as_secs_f32() / Duration::from_secs(end).as_secs_f32()).min(1.0))
    }

    /// Seconds the progress bar counts to (30, or 165 after the headset ignored the command).
    pub fn restart_headset_total_secs(&self) -> u64 {
        if self.restart_headset_slow() {
            RESET_SLOW_SECS
        } else {
            RESET_SECS
        }
    }

    /// What the running restart is doing, in plain words (from the launcher's `restart: ...` lines).
    pub fn restart_headset_stage(&self) -> Option<String> {
        self.restart_headset_progress()?;
        let from = self.headset_reset_from.unwrap_or(0).min(self.log.len());
        let last = self.log[from..].iter().rev().find(|l| l.contains("restart: ") || l.contains("ignored the restart command"));
        Some(stage_text(last.map(String::as_str)).into())
    }

    /// Why Restart service cannot run now (None: it can).
    pub fn restart_service_blocker(&self) -> Option<String> {
        if !self.facts.caps.restart {
            return Some("The installed launcher has no restart yet: install the current one (Setup > Install).".into());
        }
        if self.busy() {
            return Some("Another action is running.".into());
        }
        if self.live.started_as.is_none() {
            return Some("No session was started yet (nothing to restart).".into());
        }
        None
    }

    /// `omarchyvr desk restart` or `omarchyvr vr restart`: stop (grace 30) and start again with the same switches (a job).
    pub fn restart_service(&mut self) {
        if let Some(b) = self.restart_service_blocker() {
            self.message = b;
            return;
        }
        let kind = self.live.started_as.clone().unwrap_or_else(|| "desk".into());
        let c = Cmd::with_args(self.launcher_bin().display().to_string(), vec![kind.clone(), "restart".into()]);
        let c = self.own_scope(c, &kind);
        self.spawn(JobSpec { kind: JobKind::Other(RESTART_KIND.into()), label: format!("Restart service (omarchyvr {kind} restart)"), steps: vec![Step::Run(c)] });
    }

    /// The running service restart's progress (0..1), if one runs.
    pub fn restart_service_progress(&self) -> Option<f32> {
        let j = self.job.as_ref().filter(|j| j.kind == JobKind::Other(RESTART_KIND.into()))?;
        Some((j.started.elapsed().as_secs_f32() / RESTART_SECS as f32).min(1.0))
    }

    /// The watchdog for people: "auto-reconnect on" and its last event, or None when it does not run.
    pub fn watchdog_state(&self) -> Option<String> {
        self.live.watchdog?;
        Some(match &self.live.watchdog_last {
            Some(l) => format!("Auto-reconnect on (last: {l})"),
            None => "Auto-reconnect on".into(),
        })
    }

    /// A restart job ended: read its verdict.
    pub(crate) fn restart_headset_done(&mut self, ok: bool) {
        let from = self.headset_reset_from.take().unwrap_or(0).min(self.log.len());
        self.headset_reset = Some(match (verdict(&self.log[from..]), ok) {
            (Some(v), _) => v,
            (None, true) => ResetResult { ok: true, text: "Headset restarted.".into() },
            (None, false) => ResetResult { ok: false, text: "The restart did not run (see the output below).".into() },
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(s: &str) -> ResetResult {
        verdict(&["12:00:01 reset sent".to_string(), format!("verdict: {s}")]).unwrap()
    }

    #[test]
    fn stages_in_plain_words() {
        assert_eq!(stage_text(None), "Sending the restart command");
        assert!(stage_text(Some("restart: command sent. The headset restarts itself")).contains("waiting for the headset to shut down"));
        assert!(stage_text(Some("restart: the headset left USB (it is restarting)")).contains("off USB"));
        assert!(stage_text(Some("restart: the headset is back on USB; waiting for the camera and the picture")).contains("back on USB"));
        assert!(stage_text(Some("restart: the camera is back")).contains("picture"));
        assert_eq!(stage_text(Some("restart: the headset is up (the picture is on by now)")), "The headset is up");
        assert!(stage_text(Some("the headset ignored the restart command: using the slow restart")).contains("slow"));
    }

    #[test]
    fn verdicts_in_words_without_unplugging() {
        let ok = v("camera back after 74 s (it was missing at the start; video node present): recovered, no unplugging needed");
        assert_eq!(ok, ResetResult { ok: true, text: "Headset restarted: the camera is back after 74 s.".into() });
        assert!(v("full power cycle (camera dropped at 1.2 s, back at 38.4 s, video node present)").text.ends_with("after 38 s."));
        for bad in [
            "camera 34a4:0014 still missing after 150 s: run reset-headset again; only if that fails twice, cut the headset power at the junction box once",
            "camera dropped at 1.0 s and did not return within 150 s (headset powered off? press its button, or run reset-headset again)",
            "camera on USB at 40 s but /sys/class/video4linux has no 34a4:0014 node after 150 s (uvcvideo not bound?)",
            "interrupted before a verdict",
            "only the HID chip re-enumerated; no full reset within 150 s",
        ] {
            let r = v(bad);
            assert!(!r.ok, "{bad}");
            let t = r.text.to_lowercase();
            assert!(!t.contains("unplug") && !t.contains("power at") && !t.contains("cut"), "{}", r.text);
        }
        assert_eq!(verdict(&["no verdict".to_string()]), None);
    }

    #[test]
    fn restart_service_needs_the_launcher_command_and_a_recorded_start() {
        let d = std::env::temp_dir().join(format!("pimax-restart-{}", std::process::id()));
        let mut a = AppCore::demo(true, omarchyvr_config::Store::new(d.join("a"), d.join("b")));
        a.facts.caps.restart = false;
        assert!(a.restart_service_blocker().unwrap().contains("install the current one"));
        a.facts.caps.restart = true;
        a.live.started_as = None;
        assert!(a.restart_service_blocker().unwrap().contains("nothing to restart"));
        a.live.started_as = Some("desk".into());
        assert!(a.restart_service_blocker().is_none());
        a.restart_service();
        assert!(a.message.contains("demo mode: would run Restart service (omarchyvr desk restart)"), "{}", a.message);
        assert_eq!(a.watchdog_state(), None);
        a.live.watchdog = Some(7);
        a.live.watchdog_last = Some("12:00:01 headset back".into());
        assert_eq!(a.watchdog_state().unwrap(), "Auto-reconnect on (last: 12:00:01 headset back)");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn camera_missing_and_the_restart_is_refused_while_monado_runs() {
        let d = std::env::temp_dir().join(format!("pimax-reset-{}", std::process::id()));
        let mut a = AppCore::demo(false, omarchyvr_config::Store::new(d.join("a"), d.join("b")));
        a.facts.usb = vec![(0x34a4, 0x0044), (0x34a4, 0x0014)];
        assert!(!a.headset_camera_missing());
        a.facts.usb = vec![(0x34a4, 0x0044)];
        assert!(a.headset_camera_missing());
        assert!(a.restart_headset_blocker().is_none());
        a.restart_headset();
        assert!(a.message.contains("demo mode: would run Restart headset") && a.job.is_none());
        let mut r = AppCore::demo(true, omarchyvr_config::Store::new(d.join("c"), d.join("e")));
        assert!(r.restart_headset_blocker().unwrap().contains("refused while Monado runs"));
        r.restart_headset();
        assert!(r.message.contains("refused"));
        let _ = std::fs::remove_dir_all(d);
    }
}
