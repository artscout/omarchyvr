//! The tracking settings of plan phase 1 (docs/TRACKING-STABILITY.md): the controller angle slider and the switches
//! "New camera exposure", "Smooth room corrections" and "Steadier controllers". All are on by default; the app passes
//! them at every start (`--ctrl-angle DEG`, and the flag of a switch that is off), so start-argv.txt keeps them. All
//! four are also saved in the active profile's setup.toml, which the launcher reads when a start has no flag (the CLI,
//! omarchyvrd); the angle is changed live in a running session. The status of a switch comes from the running
//! Monado's environment.
use crate::app::AppCore;
use crate::builds::Markers;
use crate::launcher::DeskToggles;

/// One of the three switches.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// The camera exposure law of 2026-10-17 (off = `--old-exposure`, PIMAX_CAM_AE_LAW=old).
    NewExposure,
    /// Room corrections glide (off = `--no-room-glide`, PIMAX_ANCHOR_GLIDE=0).
    RoomGlide,
    /// The controller fuser's 220 ms optical window (off = `--no-ctrl-steady`, PIMAX_CTRL_TRACK_HYST=0).
    CtrlSteady,
}

impl Switch {
    /// Every switch, in page order.
    pub const ALL: [Switch; 3] = [Switch::NewExposure, Switch::RoomGlide, Switch::CtrlSteady];

    /// The app's label.
    pub fn label(self) -> &'static str {
        match self {
            Switch::NewExposure => "New camera exposure",
            Switch::RoomGlide => "Smooth room corrections",
            Switch::CtrlSteady => "Steadier controllers",
        }
    }

    /// The hover help (what it does, what off does, when it applies).
    pub fn help(self) -> &'static str {
        match self {
            Switch::NewExposure => "The cameras meter only the parts of the view that are not blown out, so a bright window no longer makes the room pulse (at most 2 exposure changes a second). Off: the previous exposure law (PIMAX_CAM_AE_LAW=old). Applies at the next start.",
            Switch::RoomGlide => "After the first placement, every room correction (room map, AprilTag, floor) glides to its new place, faster while you turn your head, instead of jumping. Off: corrections over 25 cm snap at once, as before (PIMAX_ANCHOR_GLIDE=0). Applies at the next start.",
            Switch::CtrlSteady => "The controllers hold their camera fix for 220 ms instead of 80 ms and confirm before they switch back, so they shimmer and jump less at the edge of the view. Off: the old 80 ms window (PIMAX_CTRL_TRACK_HYST=0). Applies at the next start.",
        }
    }

    /// The launcher flag that turns it off.
    pub fn off_flag(self) -> &'static str {
        match self {
            Switch::NewExposure => "--old-exposure",
            Switch::RoomGlide => "--no-room-glide",
            Switch::CtrlSteady => "--no-ctrl-steady",
        }
    }

    /// The setting in the switches.
    pub fn get(self, t: &DeskToggles) -> bool {
        match self {
            Switch::NewExposure => t.new_exposure,
            Switch::RoomGlide => t.room_glide,
            Switch::CtrlSteady => t.ctrl_steady,
        }
    }

    /// The setting, mutable (the page's switch).
    pub fn get_mut(self, t: &mut DeskToggles) -> &mut bool {
        match self {
            Switch::NewExposure => &mut t.new_exposure,
            Switch::RoomGlide => &mut t.room_glide,
            Switch::CtrlSteady => &mut t.ctrl_steady,
        }
    }

    /// Does the installed Monado build carry it (its marker)?
    pub fn in_build(self, m: &Markers) -> bool {
        match self {
            Switch::NewExposure => m.ae_law,
            Switch::RoomGlide => m.anchor_glide,
            Switch::CtrlSteady => m.ctrl_hyst,
        }
    }

    /// Index into `omarchyvr_config::tracking::KEYS` and `Live::tracking_off`.
    pub fn index(self) -> usize {
        Switch::ALL.iter().position(|s| *s == self).unwrap_or(0)
    }
}

/// The status line of a switch, in plain words, and whether it is a warning.
/// `markers`: the installed build's (None = not known yet); `running`: whether it is on in the running Monado (from
/// its environment; None = no session).
pub fn status(sw: Switch, want: bool, markers: Option<&Markers>, running: Option<bool>) -> (String, bool) {
    if markers.is_some_and(|m| !sw.in_build(m)) {
        return ("The installed Monado build does not have it yet: install the new build (Setup > Install).".into(), true);
    }
    let word = |on: bool| if on { "on" } else { "off" };
    match running {
        Some(now) => {
            if now == want {
                (format!("{} in this session", if now { "On" } else { "Off" }), false)
            } else {
                (format!("{} from the next start (this session: {})", if want { "On" } else { "Off" }, word(now)), false)
            }
        }
        None => (format!("{} from the next start", if want { "On" } else { "Off" }), false),
    }
}

/// The status line of the controller angle.
pub fn angle_status(deg: i32, markers: Option<&Markers>, running: bool) -> (String, bool) {
    if markers.is_some_and(|m| !m.ctrl_angle) {
        return ("The installed Monado build does not have it yet: install the new build (Setup > Install).".into(), true);
    }
    let what = if deg == omarchyvr_config::ctrl_angle::DEFAULT_DEG {
        format!("{deg} deg (default)")
    } else {
        format!("{deg} deg")
    };
    if running {
        (format!("{what}: applies to the running session within a second"), false)
    } else {
        (format!("{what}: from the next start"), false)
    }
}

impl AppCore {
    /// The installed (desk) Monado build's markers, if it was scanned.
    pub fn build_markers(&self) -> Option<Markers> {
        self.facts.builds.iter().find(|b| b.build.key == "desk" && b.built()).and_then(|b| b.markers)
    }

    /// The status line of one switch.
    pub fn tracking_status(&self, sw: Switch) -> (String, bool) {
        let m = self.build_markers();
        let run = self.live.service.as_ref().map(|_| !self.live.tracking_off[sw.index()]);
        status(sw, sw.get(&self.toggles), m.as_ref(), run)
    }

    /// The status line of the controller angle.
    pub fn ctrl_angle_status(&self) -> (String, bool) {
        let m = self.build_markers();
        angle_status(self.toggles.ctrl_angle, m.as_ref(), self.live.service.is_some())
    }

    /// A switch changed on the page: saved for the next start (the app state and the active profile's setup.toml,
    /// which the launcher reads too).
    pub fn set_tracking_switch(&mut self, sw: Switch, on: bool) {
        *sw.get_mut(&mut self.toggles) = on;
        self.save_state();
        self.message = format!("{}: {} from the next start", sw.label(), if on { "on" } else { "off" });
        if self.demo {
            return;
        }
        let store = self.profiles.store.clone();
        self.task(None, move || {
            let r = omarchyvr_config::tracking::save(&store, sw.index(), on);
            Box::new(move |c: &mut AppCore| {
                if let Err(e) = r {
                    c.message = format!("{}: saved for this app only; {e}", sw.label());
                }
            })
        });
    }

    /// The controller angle slider moved. Every move goes to a running session at once (the driver's live file; the
    /// recorded start too, so a restart keeps it); `done` (the slider was let go) also saves it: the app state and the
    /// active profile's setup.toml (shared with the CLI).
    pub fn set_ctrl_angle(&mut self, deg: i32, done: bool) {
        use omarchyvr_config::ctrl_angle;
        let deg = deg.clamp(ctrl_angle::MIN_DEG, ctrl_angle::MAX_DEG);
        let changed = deg != self.toggles.ctrl_angle;
        self.toggles.ctrl_angle = deg;
        if self.demo {
            if done {
                self.save_state();
            }
            return;
        }
        let live = self.live.service.is_some() && self.build_markers().is_none_or(|m| m.ctrl_angle);
        if live && changed {
            let (rt, sess) = (self.paths.runtime_dir.clone(), self.paths.session_dir.clone());
            if let Err(e) = omarchyvr_tools::vr::sixdof::apply_ctrl_angle_live(&rt, &sess, deg) {
                self.message = format!("Controller angle: {e}");
            }
        }
        if done {
            self.save_state();
            let store = self.profiles.store.clone();
            self.task(None, move || {
                let r = ctrl_angle::save(&store, deg);
                Box::new(move |c: &mut AppCore| {
                    c.message = match r {
                        Ok(_) if live => format!("Controller angle: {deg} deg (now and from the next start)"),
                        Ok(_) => format!("Controller angle: {deg} deg (from the next start)"),
                        Err(e) => format!("Controller angle: {deg} deg; {e}"),
                    }
                })
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all() -> Markers {
        Markers { ctrl_angle: true, anchor_glide: true, ae_law: true, ctrl_hyst: true, ..Markers::default() }
    }

    #[test]
    fn switch_status_in_plain_words() {
        let m = all();
        assert_eq!(status(Switch::RoomGlide, true, Some(&m), None), ("On from the next start".into(), false));
        assert_eq!(status(Switch::RoomGlide, false, Some(&m), Some(false)), ("Off in this session".into(), false));
        assert_eq!(
            status(Switch::RoomGlide, true, Some(&m), Some(false)),
            ("On from the next start (this session: off)".into(), false)
        );
        assert_eq!(status(Switch::CtrlSteady, true, Some(&m), Some(true)), ("On in this session".into(), false));
        let old = Markers { ctrl_hyst: false, ..m };
        assert!(status(Switch::CtrlSteady, true, Some(&old), None).1, "a build without it is a warning");
        assert!(!status(Switch::CtrlSteady, true, None, None).1, "an unknown build is not");
    }

    #[test]
    fn angle_status_says_when_it_applies() {
        assert_eq!(angle_status(30, Some(&all()), false).0, "30 deg (default): from the next start");
        assert_eq!(angle_status(45, Some(&all()), true).0, "45 deg: applies to the running session within a second");
        assert!(angle_status(45, Some(&Markers::default()), true).1);
    }

    #[test]
    fn flags_and_argv_round_trip() {
        use crate::launcher::{desk_flags, toggles_from_argv, vr_game_args};
        let t = DeskToggles { tracking_flags: true, ctrl_angle: 45, room_glide: false, ..DeskToggles::default() };
        let f = desk_flags(&t);
        assert!(f.windows(2).any(|w| w == ["--ctrl-angle", "45"]), "{f:?}");
        assert!(f.contains(&"--no-room-glide".to_string()) && !f.contains(&"--old-exposure".to_string()));
        let g = vr_game_args(&t, false, false, None);
        assert!(g.windows(2).any(|w| w == ["--ctrl-angle", "45"]) && g.contains(&"--no-room-glide".to_string()), "{g:?}");
        let text = format!("desk\n{}\n", f.join("\n"));
        let (back, vr) = toggles_from_argv(&text, DeskToggles { tracking_flags: true, ..DeskToggles::default() }).unwrap();
        assert!(!vr);
        assert_eq!((back.ctrl_angle, back.new_exposure, back.room_glide, back.ctrl_steady), (45, true, false, true));
        // no flag = the saved setting (the launcher reads the same setup.toml): the app's value stays
        let base = DeskToggles { new_exposure: false, ..DeskToggles::default() };
        assert!(!toggles_from_argv("desk\n--6dof\n", base).unwrap().0.new_exposure);
        // a launcher without the flags gets none of them
        let old = DeskToggles { tracking_flags: false, ..t };
        assert!(!desk_flags(&old).iter().any(|a| a == "--ctrl-angle" || a == "--no-room-glide"));
        let caps = crate::launcher::parse_caps("omarchyvr desk start [--ctrl-angle DEG] [--old-exposure] [--no-room-glide] [--no-ctrl-steady]");
        assert!(caps.tracking);
        assert!(!t.supported(&crate::launcher::LauncherCaps::default()).tracking_flags);
    }
}
