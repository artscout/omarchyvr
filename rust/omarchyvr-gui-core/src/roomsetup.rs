//! Room setup from the app (docs/ROOM-SETUP.md): Fast setup and Full setup, guided IN THE HEADSET by
//! `omarchyvr-roommap setup fast|full` (the `omarchyvr-hud` panel over WayVR, triggers / pinch as input), and the Room switches
//! (`omarchyvr vr room on|off|steamvr on|off|name NAME`, saved in `~/.config/omarchyvr/room.toml`, shared with the CLI).
//!
//! The setup needs Work (desk mode) with 6DoF and the room anchor hook, and optical controller tracking for the full
//! setup (the boundary is traced with a controller): when the running session lacks them, the job restarts Work with
//! them first (like the hand test). Progress lines go to the Output panel; the result shows in the Room row.
use crate::app::AppCore;
use crate::cmd::Cmd;
use crate::guards;
use crate::job::{JobKind, JobSpec, Step};
use crate::launcher::{self, DeskToggles};
use crate::preflight::Gate;
use std::time::Duration;

/// The job kind (its end refreshes the Room state through `room_job_done`).
pub const KIND: &str = "room:setup";

/// `omarchyvr-roommap setup fast|full --name NAME --label NAME`.
pub fn setup_cmd(roommap: &std::path::Path, full: bool, name: &str, runtime_json: &std::path::Path) -> Cmd {
    Cmd::with_args(
        roommap.display().to_string(),
        vec!["setup".into(), if full { "full" } else { "fast" }.into(), "--name".into(), name.into(), "--label".into(), name.into()],
    )
    .env("XR_RUNTIME_JSON", runtime_json.display().to_string())
    .env("IPC_IGNORE_VERSION", "1")
}

/// One line for the Room page: "Room home: play area 3.0 x 2.0 m, floor set, in place in this session".
pub fn status_line(core: &AppCore) -> String {
    let st = &core.room.state;
    let name = &core.room.map_name;
    if core.demo && st.maps.is_empty() {
        return format!("Room \"{name}\": play area 2.6 x 2.0 m, floor set, in place after 4 s (floor, centre and forward).");
    }
    if !st.maps.iter().any(|m| m == name) {
        return format!("No room \"{name}\" yet: press Fast setup (30 seconds in the headset).");
    }
    let mut parts = Vec::new();
    match st.setups.iter().find(|(n, _)| n == name) {
        Some((_, s)) => parts.push(format!("play area {:.1} x {:.1} m", s.play_cm[0] as f64 / 100.0, s.play_cm[1] as f64 / 100.0)),
        None => parts.push("no play area yet".into()),
    }
    parts.push(if st.floors.iter().any(|(n, _)| n == name) { "floor set".into() } else { "floor not set".into() });
    let pid = core.live.service.as_ref().map(|s| s.pid).or(core.live.steamvr);
    let in_place = pid.is_some_and(|p| st.fixes.iter().any(|(n, fp)| n == name && *fp == p));
    match (&st.status, pid) {
        (Some(s), Some(p)) if s.pid == p && s.room == *name => parts.push(s.text.trim_end_matches('.').to_string()),
        _ if in_place => parts.push("in place in this session".into()),
        (_, Some(_)) => parts.push("not in place in this session".into()),
        _ => {}
    }
    format!("Room \"{name}\": {}", parts.join(", "))
}

impl AppCore {
    /// The switches of the setup session: Work with 6DoF and the anchor hook; optical controllers for the full one.
    pub fn room_setup_toggles(&self, full: bool) -> DeskToggles {
        DeskToggles { sixdof: true, anchor: true, ctrl_track: full || self.toggles.ctrl_track, ..self.toggles }
    }

    /// Does the running Work session already have what the setup needs?
    fn room_session_ready(&self, full: bool) -> bool {
        self.live.service.as_ref().is_some_and(|s| s.sixdof && s.anchor && (!full || s.ctrl_track)) && self.live.alive("broker") && self.live.desk_clients_alive()
    }

    /// Why a setup cannot start now (empty = it can).
    pub fn room_setup_blockers(&self, full: bool) -> Vec<String> {
        let mut v = Vec::new();
        if self.busy() {
            v.push("another action is running".to_string());
        }
        if !self.room.state.tools.0 && !self.demo {
            v.push("omarchyvr-roommap is not built (Setup > Install).".into());
        }
        if !crate::room::valid_name(&self.room.map_name) {
            v.push("Room names are letters, digits, - and _.".into());
        }
        if !self.facts.headset && !self.demo {
            v.push("the headset is not connected".into());
        }
        if !self.facts.caps.anchor && !self.demo {
            v.push("the installed pimax launcher has no --anchor (Setup > Install)".into());
        }
        if !self.live.foreign_monado.is_empty() {
            v.push("a monado-service the app did not start is running".into());
        }
        if !self.room_session_ready(full) {
            match self.room_setup_gate(full) {
                Gate::Ready => {}
                Gate::NeedsConsent(r) if !self.consent_force => v.extend(r.into_iter().map(|x| format!("needs your OK (tick Start anyway): {x}"))),
                Gate::NeedsConsent(_) => {}
                Gate::Blocked(r) => v.extend(r),
            }
        }
        v
    }

    /// The preflight gate of the setup session, judged as if our session were stopped (the setup restarts it).
    fn room_setup_gate(&self, full: bool) -> Gate {
        let t = self.room_setup_toggles(full);
        let mut live = self.live.clone();
        let session: Vec<u32> = live.session.iter().filter_map(|s| s.pid).collect();
        live.service = None;
        live.steamvr = None;
        for s in live.session.iter_mut() {
            s.alive = false;
        }
        live.monado_pids.retain(|p| !session.contains(p));
        let mut facts = self.facts.clone();
        facts.camera_holders.retain(|h| !session.contains(&h.pid));
        facts.eye_holders.retain(|h| !session.contains(&h.pid));
        self.preflight_for(&live, &facts, &t).gate
    }

    /// The setup job.
    pub fn room_setup_job(&self, full: bool) -> Result<JobSpec, String> {
        let b = self.room_setup_blockers(full);
        if !b.is_empty() {
            return Err(format!("Room setup not started: {}", b.join("; ")));
        }
        let t = self.room_setup_toggles(full);
        let name = self.room.map_name.clone();
        let launcher_bin = self.launcher_bin().display().to_string();
        let mut steps = Vec::new();
        let build = if self.room_session_ready(full) {
            self.live.service.as_ref().and_then(|s| s.build.clone()).unwrap_or_else(|| self.paths.desk_build().dir.clone())
        } else {
            if self.live.service.is_some() || self.live.session_helpers_alive() || self.live.steamvr.is_some() {
                let args = if self.live.desk_side() { launcher::desk_stop_args() } else { launcher::vr_stop_args(false) };
                steps.push(Step::RunAllowFail(Cmd::with_args(launcher_bin.clone(), args)));
            }
            steps.push(Step::WaitNoMonado(Duration::from_secs(45)));
            steps.push(Step::Guard("camera and eye chip still free", guards::devices_free(true, t.eye)));
            steps.push(Step::Run(self.own_scope(self.desk_start_cmd(&t, self.consent_force && matches!(self.room_setup_gate(full), Gate::NeedsConsent(_))), "desk")));
            steps.push(Step::Note("Work is starting with 6DoF and the room anchor; put the headset on.".into()));
            match self.plan_for_toggles(&t) {
                Some(Ok(p)) => p.monado_build,
                _ => self.paths.desk_build().dir.clone(),
            }
        };
        steps.push(Step::Note(if full {
            "Full room setup in the headset: walk the room (60 s), lay a controller on the floor, trace the boundary holding a trigger, then stand in the middle and look forward.".into()
        } else {
            "Fast room setup in the headset: stand in the middle of your play space and look around, then look forward and pull a trigger (or hold still).".into()
        }));
        steps.push(Step::Run(setup_cmd(&self.room_paths.roommap_bin, full, &name, &build.join("openxr_monado-dev.json"))));
        Ok(JobSpec {
            kind: JobKind::Other(KIND.into()),
            label: format!("{} room setup \"{name}\" (in the headset)", if full { "Full" } else { "Fast" }),
            steps,
        })
    }

    /// Start Fast setup (`full = false`) or Full setup.
    pub fn start_room_setup(&mut self, full: bool) -> bool {
        match self.room_setup_job(full) {
            Ok(j) => self.spawn(j),
            Err(e) => {
                self.message = e;
                false
            }
        }
    }

    /// A Room switch: `omarchyvr vr room on|off`, `steamvr on|off`, `name NAME` (saved, shared with the CLI).
    pub fn room_pref(&mut self, args: &[&str]) {
        let ok = matches!(args, ["on"] | ["off"] | ["steamvr", "on"] | ["steamvr", "off"]) || matches!(args, ["name", n] if crate::room::valid_name(n));
        if !ok {
            return;
        }
        if self.demo {
            match args {
                ["on"] | ["off"] => self.room.state.prefs.auto = args[0] == "on",
                ["steamvr", x] => self.room.state.prefs.steamvr = *x == "on",
                ["name", n] => self.room.state.prefs.room = n.to_string(),
                _ => {}
            }
            return;
        }
        let mut a = vec!["vr".to_string(), "room".to_string()];
        a.extend(args.iter().map(|s| s.to_string()));
        let label = match args {
            ["on"] => "Room at every start: on".to_string(),
            ["off"] => "Room at every start: off".to_string(),
            ["steamvr", x] => format!("Tell SteamVR the room: {x}"),
            ["name", n] => format!("Room for every start: {n}"),
            _ => "Room".to_string(),
        };
        let j = JobSpec { kind: JobKind::Other("room:prefs".into()), label, steps: vec![Step::Run(Cmd::with_args(self.launcher_bin().display().to_string(), a))] };
        self.spawn(j);
    }
}

struct CoreEnv<'a>(&'a AppCore);

impl crate::roomflow::FlowEnv for CoreEnv<'_> {
    fn calibration_present(&self) -> bool {
        self.0.calibration_ok()
    }
    fn chip(&self) -> crate::headset_choice::HeadsetUsb {
        crate::headset_choice::headset_state_from_usb(&self.0.facts.usb)
    }
}

/// `2026-10-17` from Unix seconds (UTC; the tile only needs the day).
pub fn ymd(unix: u64) -> String {
    let z = (unix / 86_400) as i64 + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + i64::from(m <= 2);
    format!("{y:04}-{m:02}-{d:02}")
}

impl AppCore {
    /// The headset's factory calibration is in `~/.config/omarchyvr/headset-calibration.xml`.
    pub fn calibration_ok(&self) -> bool {
        if let (true, Some(v)) = (self.demo, self.demo_calibration) {
            return v;
        }
        crate::calibration::present(&self.paths.home)
    }

    /// The third big button: open the guided Room setup at its first undone step.
    pub fn open_room_flow(&mut self) {
        if !self.demo {
            let _ = crate::calibration::migrate(&self.paths.home);
        }
        self.want_room();
        self.page = crate::app::Page::RoomSetup;
        self.onboarding.wake.reset();
        self.onboarding.wake_asked = 0.0;
        let mut f = self.room_flow.clone();
        f.open(&CoreEnv(self));
        self.room_flow = f;
    }

    /// "Read it from the headset" (also "Try again").
    pub fn room_flow_read(&mut self) {
        let mut f = self.room_flow.clone();
        let fx = f.read_pressed(&CoreEnv(self));
        self.room_flow = f;
        self.room_flow_apply(fx);
    }

    /// "Fast room setup" / "Full room setup".
    pub fn room_flow_choose(&mut self, full: bool) {
        let mut f = self.room_flow.clone();
        let fx = f.choose(full, &CoreEnv(self));
        self.room_flow = f;
        self.room_flow_apply(fx);
    }

    /// Back to the first step.
    pub fn room_flow_restart(&mut self) {
        let mut f = self.room_flow.clone();
        f.restart(&CoreEnv(self));
        self.room_flow = f;
    }

    fn room_flow_apply(&mut self, fx: crate::roomflow::Effect) {
        use crate::roomflow::Effect;
        match fx {
            Effect::None => {}
            Effect::ReadCalibration => {
                let spec = JobSpec {
                    kind: JobKind::Other(crate::calibration::READ_KIND.into()),
                    label: "Reading the headset calibration".into(),
                    steps: vec![
                        Step::Note("Reading the factory calibration from the headset (about 10 seconds). It only reads; nothing is changed on the headset.".into()),
                        Step::Run(crate::calibration::read_cmd(&self.launcher_bin())),
                    ],
                };
                if !self.spawn(spec) {
                    self.room_flow_restart();
                }
            }
            Effect::StartRoomSetup { full } => {
                if !self.start_room_setup(full) {
                    // the plain reason is in `message`; stay on the choice
                    self.room_flow.stage = crate::roomflow::Stage::Choose;
                }
            }
        }
    }

    pub(crate) fn calib_read_done(&mut self, ok: bool) {
        let mut f = self.room_flow.clone();
        f.read_done(ok, &CoreEnv(self));
        self.room_flow = f;
    }

    pub(crate) fn room_flow_room_done(&mut self, ok: bool) {
        self.room_flow.room_done(ok);
    }

    /// Feeds the flow from the live USB facts while the headset sleeps (the wake guide), and asks for fresh facts.
    pub(crate) fn room_flow_poll(&mut self) {
        let mut f = self.room_flow.clone();
        let fx = f.tick(&CoreEnv(self));
        self.room_flow = f;
        self.room_flow_apply(fx);
    }

    /// The tile under Play and Work: (done, words). Done = the calibration is in place and a room setup is saved.
    pub fn room_tile(&self) -> (bool, String) {
        if self.demo_reference || (!self.demo && !self.calibration_ok() && crate::calibration::reference_in_use(&self.paths.home)) {
            return (false, "Using the reference calibration".into());
        }
        let cal = self.calibration_ok();
        let when = self.room.state.setups.iter().map(|(_, s)| s.when).max().filter(|w| *w > 0);
        match (cal, when) {
            (true, Some(w)) => (true, format!("Done {}", ymd(w))),
            (true, None) if !self.room.state.setups.is_empty() => (true, "Done".into()),
            (true, None) => (false, "Calibrated; room not mapped".into()),
            (false, _) => (false, "Not done".into()),
        }
    }

    /// Work and Play need the headset's calibration (Basalt/SLAM). Without it the start says so in plain words and
    /// opens the Room setup, which reads it first. Returns true when the start must not go on.
    pub(crate) fn calibration_blocks_start(&mut self) -> bool {
        if (self.demo && self.demo_calibration.is_none()) || !self.facts.headset {
            return false;
        }
        if !self.demo {
            let _ = crate::calibration::migrate(&self.paths.home);
        }
        if self.calibration_ok() {
            return false;
        }
        // the shipped reference stands in (user decision 2026-10-17): the start goes on, the Room setup tile says
        // "Using the reference calibration" until the headset's own is read
        if self.demo_reference || (!self.demo && crate::calibration::reference_in_use(&self.paths.home)) {
            return false;
        }
        self.message = crate::calibration::MISSING.into();
        self.open_room_flow();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn setup_command_is_safe_and_has_the_runtime() {
        let c = setup_cmd(Path::new("/r/omarchyvr-roommap"), true, "home", Path::new("/b/openxr_monado-dev.json"));
        assert_eq!(c.display(), "XR_RUNTIME_JSON=/b/openxr_monado-dev.json IPC_IGNORE_VERSION=1 /r/omarchyvr-roommap setup full --name home --label home");
        assert!(c.env.contains(&("XR_RUNTIME_JSON".into(), "/b/openxr_monado-dev.json".into())));
        assert_eq!(crate::safety::check(&c), Ok(()));
        assert!(setup_cmd(Path::new("/r/x"), false, "a", Path::new("/j")).display().contains(" fast "));
    }

    #[test]
    fn setup_file_and_status_line() {
        let t = "label = \"home\"\nmode = \"fast\"\ncentre = [0.5, -1.0]\nyaw_deg = 12.5\nplay_area = [2.0, 1.5]\nhead_m = 1.6\nwhen = 9\nboundary = [[-0.5, -2.0], [1.5, -2.0], [1.5, 0.0], [-0.5, 0.0]]\n";
        let s = crate::room::parse_setup(t).unwrap();
        assert_eq!(s.play_cm, [200, 150]);
        assert_eq!(s.centre_cm, [50, -100]);
        assert_eq!(s.yaw_decideg, 125);
        assert!((s.area_m2() - 4.0).abs() < 1e-9);
        assert!(s.describe().contains("2.0 x 1.5 m"));
        assert!(crate::room::parse_setup("centre = [0, 0]\n").is_none());
    }

    fn core(tag: &str) -> AppCore {
        let d = std::env::temp_dir().join(format!("omarchyvr-roomflow-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        AppCore::demo(false, omarchyvr_config::Store::new(d.join("config"), d.join("cache")))
    }

    #[test]
    fn dates_are_civil_days() {
        assert_eq!(ymd(0), "1970-01-01");
        assert_eq!(ymd(1_791_968_000), "2026-10-14");
        assert_eq!(ymd(1_792_000_000), "2026-10-14");
        assert_eq!(ymd(1_792_281_600), "2026-10-18");
    }

    #[test]
    fn start_without_calibration_says_so_in_plain_words_and_opens_room_setup() {
        for vr in [false, true] {
            let mut a = core(if vr { "play" } else { "work" });
            a.facts.headset = true;
            a.demo_calibration = Some(false);
            a.work_setup.record.done = true;
            if vr { a.play() } else { a.work() }
            assert_eq!(a.message, crate::calibration::MISSING);
            assert!(!a.message.contains("omarchyvr-slam") && !a.message.contains("no calibration file found"));
            assert_eq!(a.page, crate::app::Page::RoomSetup);
            assert!(a.job.is_none(), "nothing started");
            assert_eq!(a.room_flow.stage, crate::roomflow::Stage::NeedCalibration);
        }
    }

    #[test]
    fn start_with_calibration_is_not_blocked() {
        let mut a = core("ok");
        a.facts.headset = true;
        a.demo_calibration = Some(true);
        assert!(!a.calibration_blocks_start());
    }

    #[test]
    fn the_shipped_reference_lets_work_and_play_start_and_the_tile_says_so() {
        let mut a = core("reference");
        a.facts.headset = true;
        a.demo_calibration = Some(false);
        a.demo_reference = true;
        assert!(!a.calibration_blocks_start(), "the reference stands in for the missing own calibration");
        assert!(a.message.is_empty());
        assert_eq!(a.room_tile(), (false, "Using the reference calibration".to_string()));
    }

    #[test]
    fn reference_in_use_means_no_own_file_and_the_reference_allowed() {
        let d = std::env::temp_dir().join(format!("omarchyvr-calref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        assert!(crate::calibration::reference_in_use_with(&d, &|_| None));
        assert!(!crate::calibration::reference_in_use_with(&d, &|k| (k == "PIMAX_NO_REFERENCE").then(|| "1".to_string())));
        std::fs::create_dir_all(d.join(".config/omarchyvr")).unwrap();
        std::fs::write(d.join(".config/omarchyvr/headset-calibration.xml"), format!("<?xml version=\"1.0\"?><DeviceConfiguration>{}</DeviceConfiguration>", " ".repeat(300))).unwrap();
        assert!(!crate::calibration::reference_in_use_with(&d, &|_| None), "the headset's own file wins");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_tile_shows_not_done_then_the_date() {
        let mut a = core("tile");
        a.demo_calibration = Some(false);
        assert_eq!(a.room_tile(), (false, "Not done".to_string()));
        a.demo_calibration = Some(true);
        assert_eq!(a.room_tile().1, "Calibrated; room not mapped");
        a.room.state.setups = vec![("home".into(), crate::room::SetupInfo { when: 1_791_968_000, ..Default::default() })];
        assert_eq!(a.room_tile(), (true, "Done 2026-10-14".to_string()));
    }

    #[test]
    fn the_read_job_is_the_allowlisted_calib_command() {
        let mut a = core("job");
        a.demo_calibration = Some(false);
        a.room_flow.stage = crate::roomflow::Stage::NeedCalibration;
        let c = crate::calibration::read_cmd(&a.launcher_bin());
        assert!(c.display().ends_with(" calib --save-config"));
        assert_eq!(crate::safety::check(&c), Ok(()));
    }
}
