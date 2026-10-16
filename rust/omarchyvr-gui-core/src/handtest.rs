//! The guided hand test (docs/HANDS.md "Live test", run from the app): (re)start desk mode with `--6dof --hands`,
//! talk the user through it IN THE HEADSET, record the cameras, measure.
//!
//! * In the headset: the captions panel under the main screen (WayVR, `wayvrctl panel-modify captions ...`). The
//!   session is started with `--voice` so that panel exists; the live captions are switched off for the test (their
//!   flag file) so they do not overwrite the instructions, and back on afterwards when they were on. No desktop
//!   notification (invisible in the headset), no TTS (none installed).
//! * Steps: get ready, left hand 10 s, right hand 10 s, both 10 s, open/close and pinch 10 s, while
//!   `pimax-cam record <file> --seconds 40 --source broker` records into `~/src/cam-data` (it reads the broker and
//!   does not open the camera).
//! * Then `nice -n 19 pimax_ht_bench <file>` (the hands build's offline bench) gives the detection rate, and the
//!   `hands (running)` lines of monado.log give the live rate. Both are shown on Home.
//! * The session keeps running afterwards (with hands); Play stops it as usual.
use crate::app::AppCore;
use crate::cmd::{quote, Cmd};
use crate::guards;
use crate::job::{JobKind, JobSpec, Step};
use crate::launcher::{self, DeskToggles};
use crate::preflight::Gate;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The job kind (its end is handled in `AppCore::job_finished`).
pub const KIND: &str = "hand test";

/// What the headset shows, and for how long (seconds). The recording covers the four 10 s steps.
pub const STEPS: [(&str, &str, u32); 5] = [
    ("Hand test: get ready", "Put the controllers down. Bare hands, a bright room. Starting in 8 seconds.", 8),
    ("Hand test 1/4: LEFT hand", "Hold your left hand in front of you, 25-50 cm away. Move it slowly.", 10),
    ("Hand test 2/4: RIGHT hand", "Now your right hand, 25-50 cm away. Move it slowly.", 10),
    ("Hand test 3/4: BOTH hands", "Both hands in front of you.", 10),
    ("Hand test 4/4: open, close, pinch", "Open and close your hands, then pinch thumb and index finger.", 10),
];

/// The test's state and result.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandTestView {
    /// The recording of the running or last test.
    pub file: Option<PathBuf>,
    /// Where the running test's output starts in the app log.
    pub log_from: Option<usize>,
    /// The last result.
    pub result: Option<HandTestResult>,
}

/// What the last test measured.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HandTestResult {
    /// The job ran to the end.
    pub ok: bool,
    /// The bench's `detection: left .. %, right .. %, any .. %, both .. %` line.
    pub detection: Option<String>,
    /// The last `hands (running)` lines of monado.log (live rate during the test).
    pub live: Vec<String>,
    /// Problems (a step failed, no stats).
    pub note: Option<String>,
}

/// The shell script of the guided part: instructions on the captions panel, the recording in parallel.
pub fn script(wayvrctl: &Path, pimax_cam: &Path, file: &Path, flag: &Path) -> String {
    let c = quote(&wayvrctl.display().to_string());
    let f = quote(&flag.display().to_string());
    let mut s = String::new();
    s += &format!("if [ -e {f} ]; then had=1; else had=0; touch {f}; fi\n");
    // the captions process hides its panel when the flag appears (it checks twice a second)
    s += "sleep 1\n";
    s += &format!("{c} panel-modify captions cap_l2 set-color '#ffffffff'\n");
    for (i, (title, text, secs)) in STEPS.iter().enumerate() {
        if i == 1 {
            s += &format!(
                "{} record {} --seconds 40 --source broker &\nrec=$!\n",
                quote(&pimax_cam.display().to_string()),
                quote(&file.display().to_string())
            );
        }
        s += &format!("echo {}\n", quote(&format!("hand test: {title}")));
        s += &format!("{c} panel-modify captions cap_l1 set-text -- {}\n", quote(title));
        s += &format!("{c} panel-modify captions cap_l2 set-text -- {}\n", quote(text));
        s += &format!("{c} panel-modify captions cap_box set-visible 1\n");
        s += &format!("sleep {secs}\n");
    }
    s += &format!("{c} panel-modify captions cap_l1 set-text -- 'Hand test: done'\n");
    s += &format!("{c} panel-modify captions cap_l2 set-text -- 'Measuring... the result is in the Max app.'\n");
    s += "wait $rec\nst=$?\n";
    s += "sleep 4\n";
    s += &format!("{c} panel-modify captions cap_box set-visible 0\n");
    s += &format!("if [ \"$had\" = 0 ]; then rm -f {f}; fi\n");
    s += "exit $st\n";
    s
}

/// The bench's detection line.
pub fn detection_line<'a>(lines: impl IntoIterator<Item = &'a String>) -> Option<String> {
    lines.into_iter().filter_map(|l| l.trim().strip_prefix("detection:").map(|r| r.trim().to_string())).last()
}

/// The last `hands` lines of monado.log (stats every 5 s, plus the "Mercury up" line).
pub fn live_lines(log: &str, n: usize) -> Vec<String> {
    let v: Vec<String> = log.lines().filter(|l| l.contains("hands (running)") || l.contains("hands: ")).map(|l| l.trim().to_string()).collect();
    v[v.len().saturating_sub(n)..].to_vec()
}

impl AppCore {
    /// The Monado build a start with hands uses (the one build; from the launcher's plan).
    pub fn hands_build(&self) -> PathBuf {
        let t = DeskToggles { sixdof: true, hands: true, ..self.toggles };
        match self.plan_for_toggles(&t) {
            Some(Ok(p)) => p.monado_build,
            _ => self.paths.desk_build().dir.clone(),
        }
    }

    /// The offline bench of that build.
    pub fn hands_bench(&self) -> PathBuf {
        self.hands_build().join("src/xrt/drivers/pimax_ht_bench")
    }

    /// Why the Camera hands switch is greyed out (None = it can be switched on).
    pub fn hands_unavailable(&self) -> Option<String> {
        let b = self.hands_build();
        let markers = self.facts.build(&b).and_then(|i| i.markers);
        if !self.facts.caps.hands {
            Some("The installed pimax launcher has no --hands yet (Setup > Install).".into())
        } else if markers.is_some_and(|k| !k.hands) {
            Some(format!("The Monado build ({}) has no camera hand tracking code (PIMAX_HT_HZ missing).", b.display()))
        } else if markers.is_none() {
            Some(format!("The Monado build ({}) was not checked yet.", b.display()))
        } else if !self.facts.hands_models {
            Some("The hand models are not installed (~/.local/share/monado/hand-tracking-models, docs/HANDS.md).".into())
        } else {
            None
        }
    }

    /// The switches of the test session: desk mode, 6DoF, hands (no controller roles), the captions panel.
    pub fn hand_test_toggles(&self) -> DeskToggles {
        DeskToggles { sixdof: true, hands: true, hands_roles: false, ctrl_track: false, voice: true, ..self.toggles }
    }

    /// Why the hand test cannot start now (empty = it can).
    pub fn hand_test_blockers(&self) -> Vec<String> {
        let mut v = Vec::new();
        if self.busy() {
            v.push("another action is running".to_string());
        }
        if let Some(why) = self.hands_unavailable() {
            v.push(why);
        }
        if !self.facts.caps.voice {
            v.push("the installed pimax launcher has no --voice (the in-headset text needs the captions panel)".into());
        }
        if !self.demo && !self.hands_bench().is_file() {
            v.push(format!("{} is not built", self.hands_bench().display()));
        }
        if !self.facts.headset {
            v.push("the headset is not connected".into());
        }
        if !self.live.foreign_monado.is_empty() {
            v.push("a monado-service the app did not start is running".into());
        }
        match self.hand_test_gate() {
            Gate::Ready => {}
            Gate::NeedsConsent(r) if !self.consent_force => v.extend(r.into_iter().map(|x| format!("needs your OK (tick Start anyway): {x}"))),
            Gate::NeedsConsent(_) => {}
            Gate::Blocked(r) => v.extend(r),
        }
        v
    }

    /// The preflight gate of the test session, judged as if our session were stopped (the test restarts it).
    fn hand_test_gate(&self) -> Gate {
        let t = self.hand_test_toggles();
        let mut live = self.live.clone();
        let session: Vec<u32> = live.session.iter().filter_map(|s| s.pid).collect();
        live.service = None;
        for s in live.session.iter_mut() {
            s.alive = false;
        }
        live.monado_pids.retain(|p| !session.contains(p));
        let mut facts = self.facts.clone();
        facts.camera_holders.retain(|h| !session.contains(&h.pid));
        facts.eye_holders.retain(|h| !session.contains(&h.pid));
        self.preflight_for(&live, &facts, &t).gate
    }

    /// The test job and its recording file.
    pub fn hand_test_job(&self) -> Result<(JobSpec, PathBuf), String> {
        let b = self.hand_test_blockers();
        if !b.is_empty() {
            return Err(format!("Hand test not started: {}", b.join("; ")));
        }
        let t = self.hand_test_toggles();
        let dir = self.paths.src.join("cam-data");
        let stamp = omarchyvr_config::store::stamp(self.machine.now_unix());
        let file = dir.join(format!("hands-test-{stamp}.pxcam"));
        let launcher_bin = self.launcher_bin().display().to_string();
        let mut steps = Vec::new();
        if self.live.service.is_some() || self.live.session_helpers_alive() {
            let args = if self.live.desk_side() { launcher::desk_stop_args() } else { launcher::vr_stop_args(false) };
            steps.push(Step::RunAllowFail(Cmd::with_args(launcher_bin.clone(), args)));
        }
        steps.push(Step::WaitNoMonado(Duration::from_secs(45)));
        steps.push(Step::Guard("camera and eye chip still free", guards::devices_free(true, t.eye)));
        steps.push(Step::Run(self.own_scope(self.desk_start_cmd(&t, self.consent_force && matches!(self.hand_test_gate(), Gate::NeedsConsent(_))), "desk")));
        steps.push(Step::Run(Cmd::new("mkdir", &["-p", &dir.display().to_string()])));
        let flag = self.paths.runtime_dir.join(crate::subtitles::OFF_FLAG);
        steps.push(Step::Run(Cmd::shell(&script(&self.wayvr_paths.ctl, &omarchyvr_tools::vr::config::tools_dir(&self.paths.home).join("pimax-cam"), &file, &flag))));
        steps.push(Step::Note("measuring the recording (pimax_ht_bench)...".into()));
        steps.push(Step::Run(Cmd::with_args("nice", vec!["-n".into(), "19".into(), self.hands_bench().display().to_string(), file.display().to_string()])));
        Ok((JobSpec { kind: JobKind::Other(KIND.into()), label: "Hand test (desk mode with camera hands, 4 steps in the headset)".into(), steps }, file))
    }

    /// Start the guided hand test.
    pub fn start_hand_test(&mut self) {
        match self.hand_test_job() {
            Ok((j, file)) => {
                let from = self.log.len();
                if self.spawn(j) {
                    self.play.hand_test = HandTestView { file: Some(file), log_from: Some(from), result: None };
                }
            }
            Err(e) => self.message = e,
        }
    }

    /// The test job ended: the detection line from its output, the live lines from monado.log.
    pub(crate) fn hand_test_done(&mut self, ok: bool) {
        let from = self.play.hand_test.log_from.take().unwrap_or(self.log.len()).min(self.log.len());
        let detection = detection_line(&self.log[from..]);
        let (m, log) = (self.machine.clone(), self.paths.session_dir.join("monado.log"));
        self.task(None, move || {
            let text = m.read(&log).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
            let live = live_lines(&text, 6);
            Box::new(move |c: &mut AppCore| {
                let note = match (&detection, live.is_empty()) {
                    (None, true) => Some("No measurement: see the output panel (did the session start with hands?).".to_string()),
                    (None, false) => Some("The bench gave no detection line (see the output panel).".to_string()),
                    (Some(_), true) => Some("monado.log has no hands stats lines.".to_string()),
                    _ => None,
                };
                c.play.hand_test.result = Some(HandTestResult { ok, detection, live, note });
            })
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_script_is_visible_to_the_safety_checks_and_restores_the_captions() {
        let s = script(Path::new("/w/wayvrctl"), Path::new("/r/pimax-cam"), Path::new("/home/u/src/cam-data/h.pxcam"), Path::new("/run/user/1000/omarchyvr-voice-captions.off"));
        assert_eq!(crate::safety::check(&Cmd::shell(&s)), Ok(()));
        assert!(s.contains("/r/pimax-cam record /home/u/src/cam-data/h.pxcam --seconds 40 --source broker &"));
        assert!(s.contains("panel-modify captions cap_l1 set-text -- 'Hand test 1/4: LEFT hand'"));
        assert!(s.contains("rm -f /run/user/1000/omarchyvr-voice-captions.off"));
        let rec = s.find(" record ").unwrap();
        assert!(s.find("LEFT hand").unwrap() > rec, "the recording covers the four steps");
        assert_eq!(STEPS[1..].iter().map(|x| x.2).sum::<u32>(), 40);
    }

    #[test]
    fn results_are_read_from_the_bench_and_the_log() {
        let out = vec!["pairs: 1200".to_string(), "detection: left 61.0 %, right 58.2 %, any 80.1 %, both 40.3 %".to_string()];
        assert_eq!(detection_line(&out).as_deref(), Some("left 61.0 %, right 58.2 %, any 80.1 %, both 40.3 %"));
        let log = "x\nhands: Mercury up\nfoo\nhands (running): 30 pairs/s processed, left active 50 %, right 20 %\n";
        assert_eq!(live_lines(log, 6).len(), 2);
    }

    #[test]
    fn the_demo_job_restarts_desk_mode_with_hands_and_voice() {
        let d = std::env::temp_dir().join(format!("pimax-handtest-{}", std::process::id()));
        let mut a = AppCore::demo(true, omarchyvr_config::Store::new(d.join("c"), d.join("k")));
        a.facts.hands_models = true;
        a.facts.caps.hands = true;
        let e = a.hand_test_job().unwrap_err();
        assert!(e.contains("no camera hand tracking code"), "the demo build has no hands: {e}");
        for b in a.facts.builds.iter_mut() {
            if let Some(k) = b.markers.as_mut() {
                k.hands = true;
            }
        }
        assert!(a.hand_test_job().unwrap_err().contains("needs your OK"), "the demo kernel log has WayVR Xid 31 faults");
        a.consent_force = true;
        let (j, f) = a.hand_test_job().unwrap();
        assert!(f.starts_with("/home/u/src/cam-data"));
        let text: Vec<String> = j.steps.iter().map(|s| format!("{s:?}")).collect();
        assert!(text[0].contains("desk stop --grace 30"), "{text:?}");
        assert!(text.iter().any(|s| s.contains("desk start") && s.contains("--6dof --hands") && s.contains("--voice")), "{text:?}");
        assert!(text.last().unwrap().contains("nice -n 19") && text.last().unwrap().contains("pimax_ht_bench"));
        for s in &j.steps {
            if let Step::Run(c) | Step::RunAllowFail(c) = s {
                assert_eq!(crate::safety::check(c), Ok(()), "{}", c.display());
            }
        }
        a.facts.hands_models = false;
        assert!(a.hand_test_job().is_err());
    }
}
