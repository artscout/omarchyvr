//! Onboarding. Since 2026-10-17 the system part (find the headset, check this computer, fix, udev/NVIDIA/EDID/setcap)
//! is the automatic setup ([`crate::autosetup`]); what is left here are the steps that need the person in the headset
//! (IPD, controller pairing, eye calibration, room map): optional, each with one button, after Ready.
//!
//! Old text: the steps of the headset's module (`[onboarding] steps` of its descriptor: find the headset, check this
//! computer, fix what the checks found, udev access, IPD, controller pairing, eye calibration, room map, done),
//! resumable through `~/.local/state/omarchyvr/onboarding.state` (`pimax_device::onboarding::Progress`).
//!
//! Every step has Run (its action through the existing tools: the checks, the catalogue fixes, `pimax-eye ipd`,
//! pairing through auto mode, `pimax-eyecal`, the Room row), Skip and Back; Next marks it done once its condition holds
//! ([`AppCore::onboarding_blocker`]). A failed step comes back first. The progress file is read and written in the
//! background; nothing here sends anything to the headset.
use crate::app::{AppCore, Page};
use crate::facts::{Facts, Live};
use crate::findings::{self, CatalogueRun};
use crate::headsets::{self, HeadsetModule};
use crate::tools::ToolId;
use pimax_device::onboarding::{Progress, State, Step};
use pimax_device::Severity;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// The progress file in the state directory.
pub const FILE: &str = "onboarding.state";

/// The onboarding page's state.
#[derive(Debug, Clone, Default)]
pub struct OnboardingView {
    /// What is done, skipped or failed (and the chosen module).
    pub progress: Progress,
    /// The file was read.
    pub loaded: bool,
    /// The step on screen when the user went back or picked one (None: the next one of the progress).
    pub view: Option<Step>,
    /// A step whose job runs (its job label): a failed job marks the step failed.
    pub running: Option<(Step, String)>,
    /// A step waiting for its pkexec confirmation.
    pub root_for: Option<Step>,
    /// A pairing ended "paired" in this session (left, right).
    pub paired: [bool; 2],
    /// The install of `./install.sh`: its one root job (System setup) and whether it ran.
    pub install: crate::install::InstallInfo,
    /// The automatic first-run setup (see [`crate::autosetup`]).
    pub setup: crate::autosetup::AutoSetup,
    /// The setup asked for fresh checks at this unix time and waits for facts collected after it.
    pub setup_wait_facts: Option<u64>,
    /// The job the setup started (a user fix or the root job).
    pub setup_job: Option<crate::autosetup::SetupJob>,
    /// The live guidance for a Dream Air whose HID chip is off ([`crate::wake`]).
    pub wake: crate::wake::WakeGuide,
    /// When the guide last asked for fresh USB facts (seconds, wall clock).
    pub wake_asked: f64,
    /// Screenshots and the demo: show this guidance step with the animation at this time.
    pub wake_demo: Option<(crate::wake::Wake, f64)>,
    /// The guide opened on its own (a start or a headset restart found the headset asleep or its cameras gone).
    pub wake_only: Option<WakeOnly>,
    /// What the guide waits for now (`Some(need_camera)`): a change starts it over.
    pub wake_for: Option<bool>,
    /// The setup's problem list was checked again after the guide saw the headset's parts back.
    pub wake_rechecked: bool,
    /// The check after an update (`post-install-check`) was started in this run of the app.
    pub post_install_started: bool,
    /// Its problems were shown (the setup screen opened once).
    pub post_install_shown: bool,
    /// The latest progress to write (the writer task takes it).
    slot: Arc<Mutex<Option<Progress>>>,
}

/// "Get the headset ready" on its own, outside the setup.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeOnly {
    /// The page Back returns to.
    pub back: Page,
    /// Why it opened, in plain words.
    pub reason: String,
    /// The cameras are needed too (not only the control chip).
    pub need_camera: bool,
}

/// What a step does, for people.
pub fn intro(s: Step) -> &'static str {
    match s {
        Step::Detect => "The app looks at the USB devices and matches them against the headset modules it knows. Pick the headset if it is not plugged in yet.",
        Step::Checks => "Driver, GPU, CPU, USB access and runtime checks, read-only: nothing is changed.",
        Step::Fixes => "What the checks found, each with its fix. Fixes as your user run here with their output shown; fixes that need root show the exact lines first and run only when you confirm them (pkexec asks for your password).",
        Step::Udev => "One password sets up this PC: the udev rule that lets your user open the headset, its cameras and the eye tracker, and (on a tested NVIDIA driver with the headset plugged in; an untested driver only if you turn on its switch below) the display patches and the boot-time display mode, plus the realtime permission for VR. The exact list is shown before anything runs; nothing else changes.",
        Step::Ipd => "Measure the distance between your pupils with the eye tracker and set the lenses once (pimax-eye ipd in a terminal: wear the headset, press a to apply, q to save and quit).",
        Step::Pair => "Pair each controller once: press Pair, then hold the controller's pairing buttons until it connects (at most one try per hand every 30 s).",
        Step::EyeCal => "Calibrate eye tracking: pimax-eyecal starts its own VR session; keep your head still and follow the dots with your eyes, then save.",
        Step::Room => "Map your room once so the windows come back to the same real spot (the Room page: walk slowly, look around, come back past where you started).",
        Step::Done => "Everything is set up. Skipped steps stay on this page; Home starts desk mode or VR.",
    }
}

/// The label of a step's Run button (None: the step has no single action).
pub fn run_label(s: Step) -> Option<&'static str> {
    match s {
        Step::Detect => Some("Look again"),
        Step::Checks => Some("Run the checks"),
        Step::Fixes => Some("Check again"),
        Step::Udev => Some("Set up this PC..."),
        Step::Ipd => Some("Measure and set IPD"),
        Step::Pair => None,
        Step::EyeCal => Some("Calibrate eye tracking"),
        Step::Room => Some("Open Room setup"),
        Step::Done => None,
    }
}

/// The steps of a module (the descriptor lists them; unknown names are skipped).
pub fn module_steps(m: &dyn HeadsetModule) -> Vec<Step> {
    // find / check / fix / system setup are the automatic setup now: only the steps with the person in the headset stay
    let v: Vec<Step> = m
        .descriptor()
        .onboarding
        .iter()
        .filter_map(|s| Step::from_id(s))
        .filter(|s| !matches!(s, Step::Detect | Step::Checks | Step::Fixes | Step::Udev))
        .collect();
    if v.is_empty() {
        vec![Step::Done]
    } else {
        v
    }
}

/// The machine summary of the detect step: (label, value).
pub fn machine_rows(facts: &Facts, live: &Live) -> Vec<(String, String)> {
    let d = |id: &str| facts.doctor(id).map(|c| c.detail.clone()).unwrap_or_else(|| "not checked yet".into());
    let mut v = vec![
        ("GPU / driver".to_string(), d("gpu")),
        ("Kernel".to_string(), facts.kernel_release.clone().unwrap_or_else(|| "?".into())),
        ("Desktop".to_string(), facts.compositor.clone().unwrap_or_else(|| "unknown (needs wp_drm_lease_device_v1)".into())),
        ("DRM lease".to_string(), d("lease")),
        ("Headset".to_string(), d("headset")),
        ("Eye tracker".to_string(), d("tobii")),
        ("Display connector".to_string(), d("connector")),
    ];
    for b in facts.builds.iter().filter(|b| b.built()) {
        v.push((format!("Monado {}", b.build.key), format!("{} (branch {})", b.build.dir.display(), b.branch.as_deref().unwrap_or("?"))));
    }
    if let Some(s) = &live.service {
        v.push(("Running".to_string(), format!("monado-service pid {}", s.pid)));
    }
    v
}

impl AppCore {
    /// The progress file.
    pub fn onboarding_path(&self) -> PathBuf {
        self.paths.state_dir.join(FILE)
    }

    /// Read the progress file once (in the background).
    pub fn want_onboarding(&mut self) {
        if !self.onboarding.install.loaded {
            if self.demo {
                self.onboarding.install = crate::install::demo_info();
            } else {
                self.refresh_install();
            }
        }
        if self.onboarding.loaded || self.task_busy("onboarding-load") {
            return;
        }
        let path = self.onboarding_path();
        self.task(Some("onboarding-load"), move || {
            let p = Progress::load(&path);
            Box::new(move |c: &mut AppCore| {
                if !c.onboarding.loaded {
                    c.onboarding.progress = p;
                    c.onboarding.loaded = true;
                }
            })
        });
    }

    /// The module the onboarding is for: the one picked at the first step, else the connected one, else the default.
    pub fn onboarding_module(&self) -> &'static dyn HeadsetModule {
        self.onboarding
            .progress
            .module
            .as_deref()
            .and_then(headsets::by_id)
            .or_else(|| headsets::connected(&self.facts))
            .unwrap_or_else(headsets::default_module)
    }

    /// The module's steps, in order.
    pub fn onboarding_steps(&self) -> Vec<Step> {
        module_steps(self.onboarding_module())
    }

    /// The step on screen.
    pub fn onboarding_step(&self) -> Step {
        let steps = self.onboarding_steps();
        self.onboarding.view.filter(|s| steps.contains(s)).or_else(|| self.onboarding.progress.next(&steps)).unwrap_or(Step::Done)
    }

    /// Failing checks that have a catalogue fix.
    pub fn onboarding_open_fixes(&self) -> usize {
        self.facts.findings.as_deref().unwrap_or(&[]).iter().filter(|f| f.severity == Severity::Fail && f.fix.is_some()).count()
    }

    /// Why Next is not possible yet (None: the step's condition holds).
    pub fn onboarding_blocker(&self, s: Step) -> Option<String> {
        let o = &self.onboarding;
        match s {
            Step::Detect => o.progress.module.is_none().then(|| "Pick the headset first (the Use button above).".to_string()),
            Step::Checks => self.facts.findings.is_none().then(|| "The checks have not run yet.".to_string()),
            Step::Fixes => {
                let n = self.onboarding_open_fixes();
                (n > 0).then(|| format!("{n} failing check(s) still have a fix to run (or Skip this step)."))
            }
            Step::Udev if self.onboarding.install.plan_text.is_some() => (!self.onboarding.install.root_done)
                .then(|| "The system setup has not run yet: Set up this PC (one password).".to_string()),
            Step::Udev => match (self.facts.udev_rule, self.facts.doctor_level("udev")) {
                (Some(true), Some(omarchyvr_doctor::Level::Ok) | None) => None,
                (Some(true), _) => Some("The rule is installed but the headset nodes are not writable yet: Restart headset (top bar), then Check again.".into()),
                (Some(false), _) => Some("The installed udev rule differs from the repository's.".into()),
                (None, _) => Some("The udev rule is not installed yet.".into()),
            },
            Step::Ipd => self.facts.ipd_mm.is_none().then(|| "No IPD saved yet (~/.config/omarchyvr/ipd.env).".to_string()),
            Step::Pair => {
                let c = &self.live.log.ctrl;
                let left = o.paired[0] || c.slots_seen.contains(&crate::devices::SLOT_LEFT);
                let right = o.paired[1] || c.slots_seen.contains(&crate::devices::SLOT_RIGHT);
                match (left, right) {
                    (true, true) => None,
                    (false, true) => Some("The left controller is not paired yet.".into()),
                    (true, false) => Some("The right controller is not paired yet.".into()),
                    (false, false) => Some("Neither controller is paired yet.".into()),
                }
            }
            Step::EyeCal => (!self.facts.eye_calibration).then(|| "No eye calibration saved yet (~/.config/omarchyvr/eye-calibration.toml).".to_string()),
            Step::Room => self.room.state.maps.is_empty().then(|| "No room map yet.".to_string()),
            Step::Done => None,
        }
    }

    /// Set a step's state and write the file (in the background).
    pub fn onboarding_set(&mut self, s: Step, st: State) {
        self.onboarding.progress.set(s, st);
        self.save_onboarding();
    }

    fn save_onboarding(&mut self) {
        if let Ok(mut s) = self.onboarding.slot.lock() {
            *s = Some(self.onboarding.progress.clone());
        }
        if self.task_busy("onboarding-save") {
            // the running writer takes it
            return;
        }
        let (slot, path) = (self.onboarding.slot.clone(), self.onboarding_path());
        self.task(Some("onboarding-save"), move || {
            let mut err = None;
            loop {
                let next = slot.lock().ok().and_then(|mut s| s.take());
                let Some(p) = next else { break };
                if let Err(e) = p.save(&path) {
                    err = Some(format!("cannot save {}: {e}", path.display()));
                }
            }
            Box::new(move |c: &mut AppCore| {
                if let Some(e) = err {
                    c.message = e;
                }
            })
        });
    }

    /// The headset of the first step.
    pub fn onboarding_use_module(&mut self, id: &str) {
        if headsets::by_id(id).is_none() {
            return;
        }
        self.onboarding.progress.module = Some(id.to_string());
        self.onboarding_set(Step::Detect, State::Done);
        self.onboarding.view = None;
    }

    /// Next: the step is done (when its condition holds).
    pub fn onboarding_next(&mut self) {
        let s = self.onboarding_step();
        if s == Step::Done {
            self.finish_onboarding();
            return;
        }
        if let Some(b) = self.onboarding_blocker(s) {
            self.message = b;
            return;
        }
        self.onboarding_set(s, State::Done);
        self.onboarding.view = None;
    }

    /// Skip the step on screen (it stays listed as skipped).
    pub fn onboarding_skip(&mut self) {
        let s = self.onboarding_step();
        if s == Step::Done {
            self.finish_onboarding();
            return;
        }
        self.onboarding_set(s, State::Skipped);
        self.onboarding.view = None;
    }

    /// Back to the previous step of the list (its state is kept).
    pub fn onboarding_back(&mut self) {
        let steps = self.onboarding_steps();
        let s = self.onboarding_step();
        if let Some(i) = steps.iter().position(|x| *x == s).filter(|i| *i > 0) {
            self.onboarding.view = Some(steps[i - 1]);
        }
    }

    /// Show a step (the stepper).
    pub fn onboarding_goto(&mut self, s: Step) {
        self.onboarding.view = Some(s);
    }

    /// Run the step's action.
    pub fn onboarding_run(&mut self, s: Step) {
        let before = self.job.as_ref().map(|j| j.label.clone());
        match s {
            Step::Detect | Step::Checks | Step::Fixes => {
                self.refresh(true);
                self.message = "Checking this PC...".into();
            }
            Step::Udev if self.ask_system_setup() => self.onboarding.root_for = Some(s),
            Step::Udev => match findings::udev_fix(&self.paths.repo) {
                Ok(p) => {
                    self.run_catalogue(CatalogueRun { planned: p, undo: false }, "Install the udev rule");
                    if self.pending.is_some() {
                        self.onboarding.root_for = Some(s);
                    }
                }
                Err(e) => self.message = e,
            },
            Step::Ipd => self.run_tool(ToolId::Ipd),
            Step::EyeCal => self.run_tool(ToolId::EyeCalibration),
            Step::Pair => {}
            Step::Room => {
                // the Room page; its maps complete this step
                self.page = Page::Room;
                self.room_requested = true;
            }
            Step::Done => self.finish_onboarding(),
        }
        let after = self.job.as_ref().map(|j| j.label.clone());
        if let Some(l) = after.filter(|a| Some(a) != before.as_ref()) {
            self.onboarding.running = Some((s, l));
        }
    }

    /// A job ended: a step's failed job marks the step failed (it comes back first).
    pub(crate) fn onboarding_job_done(&mut self, label: &str, ok: bool) {
        self.install_job_done(label, ok);
        if let Some((s, l)) = self.onboarding.running.clone() {
            if l == label {
                self.onboarding.running = None;
                if !ok {
                    self.onboarding_set(s, State::Failed);
                }
            }
        }
    }

    /// Start over: the progress file is deleted.
    pub fn onboarding_restart(&mut self) {
        self.onboarding.progress = Progress::default();
        self.onboarding.view = None;
        self.onboarding.paired = [false; 2];
        self.onboarding.running = None;
        if let Ok(mut s) = self.onboarding.slot.lock() {
            *s = None;
        }
        let path = self.onboarding_path();
        self.task(Some("onboarding-save"), move || {
            let r = match std::fs::remove_file(&path) {
                Err(e) if e.kind() != std::io::ErrorKind::NotFound => Err(format!("cannot delete {}: {e}", path.display())),
                _ => Ok(()),
            };
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = e;
                }
            })
        });
        self.state.onboarded = false;
        self.write_state();
    }

    /// Finish (or close) the onboarding: Home from now on; the page stays reachable from Help.
    pub fn finish_onboarding(&mut self) {
        if self.onboarding.progress.state(Step::Done) != State::Done {
            self.onboarding_set(Step::Done, State::Done);
        }
        self.state.onboarded = true;
        self.write_state();
        self.page = Page::Desk;
    }

    /// Seconds until a hand may be paired again (at most one StartPair per 30 s per hand).
    pub fn pair_wait(&self, side: &str) -> Option<u64> {
        let i = usize::from(side == "right");
        self.auto.pair_at[i].map(|t| t.elapsed()).filter(|e| *e < PAIR_EVERY).map(|e| (PAIR_EVERY - e).as_secs().max(1))
    }
}

/// At most one pairing request per hand in this time.
pub const PAIR_EVERY: Duration = Duration::from_secs(30);

#[cfg(test)]
mod tests {
    use super::*;
    use omarchyvr_config::Store;

    fn demo(tag: &str) -> AppCore {
        let dir = std::env::temp_dir().join(format!("pimax-onb-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let mut a = AppCore::demo(false, Store::new(dir.join("config"), dir.join("cache")));
        a.paths.state_dir = dir.join("state");
        a
    }

    #[test]
    fn a_demo_onboarding_run_over_a_temp_state_dir() {
        let mut a = demo("run");
        let file = a.onboarding_path();
        a.want_onboarding();
        assert!(a.onboarding.loaded);
        // the connected Dream Air lists only the steps with the person in the headset
        assert_eq!(a.onboarding_steps(), vec![Step::Ipd, Step::Pair, Step::EyeCal, Step::Room, Step::Done]);
        assert_eq!(a.onboarding_step(), Step::Ipd);
        // the demo PC has an IPD
        assert_eq!(a.onboarding_blocker(Step::Ipd), None);
        a.onboarding_next();
        // pairing: the demo only simulates it; one try per hand every 30 s
        assert_eq!(a.onboarding_step(), Step::Pair);
        assert!(a.onboarding_blocker(Step::Pair).is_some());
        a.pair("left");
        assert!(a.pair_wait("left").is_some_and(|s| s <= 30));
        assert!(a.pair_wait("right").is_none());
        a.onboarding.paired = [true, true];
        a.onboarding_next();
        assert_eq!(a.onboarding_step(), Step::EyeCal);
        a.onboarding_skip();
        assert_eq!(a.onboarding_step(), Step::Room);
        a.onboarding_back();
        assert_eq!(a.onboarding_step(), Step::EyeCal, "Back shows the previous step");
        a.onboarding.view = None;
        a.onboarding_skip();
        assert_eq!(a.onboarding_step(), Step::Done);
        a.onboarding_next();
        assert!(a.state.onboarded);
        assert_eq!(a.page, Page::Desk);
        // resumable: the file holds every decision
        let p = Progress::load(&file);
        assert_eq!(p.state(Step::Ipd), State::Done);
        assert_eq!(p.state(Step::EyeCal), State::Skipped);
        assert_eq!(p.state(Step::Room), State::Skipped);
        assert_eq!(p.state(Step::Done), State::Done);
        // a failed job brings its step back first
        let mut b = demo("run-again");
        b.paths.state_dir = file.parent().unwrap().to_path_buf();
        b.want_onboarding();
        assert_eq!(b.onboarding_step(), Step::Done);
        b.onboarding.running = Some((Step::Ipd, "x".into()));
        b.onboarding_job_done("x", false);
        assert_eq!(b.onboarding_step(), Step::Ipd);
        // start over deletes the file
        b.onboarding_restart();
        assert!(!file.exists());
        assert_eq!(b.onboarding_step(), Step::Ipd);
        let _ = std::fs::remove_dir_all(file.parent().unwrap().parent().unwrap());
        let _ = std::fs::remove_dir_all(std::env::temp_dir().join(format!("pimax-onb-run-again-{}", std::process::id())));
    }

    #[test]
    fn a_mock_headset_has_no_steps_in_the_headset() {
        let mut a = demo("mock");
        a.want_onboarding();
        a.onboarding_use_module("8kx");
        assert_eq!(a.onboarding_steps(), vec![Step::Done]);
        assert_eq!(a.onboarding_step(), Step::Done);
        let _ = std::fs::remove_dir_all(a.paths.state_dir.parent().unwrap());
    }
}
