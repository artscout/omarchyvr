//! Room setup from the app (docs/ROOM-SETUP.md): Fast setup and Full setup, guided IN THE HEADSET by
//! `pimax-roommap setup fast|full` (the `pimax-hud` panel over WayVR, triggers / pinch as input), and the Room switches
//! (`pimax vr room on|off|steamvr on|off|name NAME`, saved in `~/.config/pimax/room.toml`, shared with the CLI).
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

/// `pimax-roommap setup fast|full --name NAME --label NAME`.
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
            v.push("pimax-roommap is not built (Setup > Install).".into());
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
            if self.live.service.is_some() || self.live.desk_clients_alive() || self.live.steamvr.is_some() {
                let args = if self.live.desk_clients_alive() { launcher::desk_stop_args() } else { launcher::vr_stop_args(false) };
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
    pub fn start_room_setup(&mut self, full: bool) {
        match self.room_setup_job(full) {
            Ok(j) => {
                self.spawn(j);
            }
            Err(e) => self.message = e,
        }
    }

    /// A Room switch: `pimax vr room on|off`, `steamvr on|off`, `name NAME` (saved, shared with the CLI).
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn setup_command_is_safe_and_has_the_runtime() {
        let c = setup_cmd(Path::new("/r/pimax-roommap"), true, "home", Path::new("/b/openxr_monado-dev.json"));
        assert_eq!(c.display(), "XR_RUNTIME_JSON=/b/openxr_monado-dev.json IPC_IGNORE_VERSION=1 /r/pimax-roommap setup full --name home --label home");
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
}
