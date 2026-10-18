//! What the app remembers between runs (`~/.config/omarchyvr/gui-state.toml`): the onboarding was finished, the last desk
//! switches and refresh rate. `key = value` lines, written atomically (temp file + rename). Missing or unreadable
//! state means defaults.
use omarchyvr_config::ctrl_inhand::CtrlMode;
use crate::app::Page;
use crate::colors::Colours;
use crate::launcher::DeskToggles;
use std::path::Path;

/// Where the automatic first-run setup stands (`setup_stage` in `gui-state.toml`; shared with the CLI).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SetupStage {
    /// Not finished: the app opens on "Setting up your PC" and starts by itself.
    #[default]
    Todo,
    /// The system part ran and a restart is needed; after it the app continues with the post-restart checks.
    Restart,
    /// Finished (all checks green after the last run).
    Done,
}

impl SetupStage {
    /// The file value.
    pub fn name(self) -> &'static str {
        match self {
            SetupStage::Todo => "todo",
            SetupStage::Restart => "restart",
            SetupStage::Done => "done",
        }
    }

    /// Parse a file value.
    pub fn parse(s: &str) -> Option<SetupStage> {
        match s.trim() {
            "todo" => Some(SetupStage::Todo),
            "restart" => Some(SetupStage::Restart),
            "done" => Some(SetupStage::Done),
            _ => None,
        }
    }
}

/// Remembered state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiState {
    /// The automatic first-run setup (see [`SetupStage`]); files of older versions with `onboarded = true` count as done.
    pub setup: SetupStage,
    /// The onboarding was completed (or closed); `wizard_done` in files of older versions.
    pub onboarded: bool,
    /// Last desk switches.
    pub toggles: DeskToggles,
    /// The page shown when the app was closed.
    pub page: Option<Page>,
    /// Supersampling in percent (`XRT_COMPOSITOR_SCALE_PERCENTAGE`; 100 = off).
    pub scale_pct: u16,
    /// "Start VR when I log in": start the session when the app opens (headset there, cap present, gate clear).
    pub autostart: bool,
    /// The last start was a game session (`omarchyvr vr start`), not desk mode: Play starts the same kind.
    pub last_vr: bool,
    /// The user touched the camera hands switch: only then do game sessions (Play) get the hands.
    pub hands_chosen: bool,
    /// The last start from the app: Some(true) = started, Some(false) = failed, None = none yet.
    pub last_ok: Option<bool>,
    /// When that start ended (unix seconds; 0 = unknown).
    pub last_at: u64,
    /// mtime of the session's `start-argv.txt` whose switches were taken over last (any start path).
    pub argv_at: u64,
    /// Developer mode: shows the advanced pages and rows (tools, logs, tests, experimental switches). Off = the simple flow.
    pub dev_mode: bool,
    /// "Allow the patch on untested NVIDIA drivers" (off by default; handed to the root job, see `nvidia_patch`).
    pub nvidia_untested: bool,
    /// The headset the person chose on the first screen of the setup (`headset = dream-air | other`; shared with the CLI).
    pub headset: crate::headset_choice::HeadsetPick,
    /// The app's colours: follow Omarchy's theme (default), the omarchyvr colours, or one installed Omarchy theme.
    pub colours: Colours,
}

impl Default for GuiState {
    fn default() -> Self {
        GuiState {
            setup: SetupStage::Todo,
            onboarded: false,
            toggles: DeskToggles::default(),
            page: None,
            scale_pct: 100,
            autostart: false,
            last_vr: false,
            hands_chosen: false,
            last_ok: None,
            last_at: 0,
            argv_at: 0,
            dev_mode: false,
            nvidia_untested: false,
            headset: crate::headset_choice::HeadsetPick::Unset,
            colours: Colours::Follow,
        }
    }
}

impl GuiState {
    /// Parse the file text.
    pub fn parse(text: &str) -> GuiState {
        let mut s = GuiState::default();
        let mut eye_v2 = false;
        let mut stage = None;
        let mut old_assistant = false;
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            let on = matches!(v.trim(), "true" | "1" | "yes");
            match k.trim() {
                "onboarded" | "wizard_done" => s.onboarded = on,
                "setup_stage" => stage = SetupStage::parse(v),
                "eye" => s.toggles.eye = on,
                "eye_visual" => eye_v2 = true,
                "sixdof" => s.toggles.sixdof = on,
                "ctrl_track" => s.toggles.ctrl_track = on,
                "hands" => s.toggles.hands = on,
                "hands_roles" => s.toggles.hands_roles = on,
                "autostart" => s.autostart = on,
                "hands_chosen" => s.hands_chosen = on,
                "last_mode" => s.last_vr = v.trim() == "vr",
                "last_result" => s.last_ok = match v.trim() { "ok" => Some(true), "failed" => Some(false), _ => None },
                "last_at" => s.last_at = v.trim().parse().unwrap_or(0),
                "argv_at" => s.argv_at = v.trim().parse().unwrap_or(0),
                "dev_mode" => s.dev_mode = on,
                "nvidia_allow_untested" => s.nvidia_untested = on,
                "headset" => s.headset = crate::headset_choice::HeadsetPick::parse(v),
                "colours" => s.colours = Colours::parse(v),
                "passthrough" => s.toggles.passthrough = on,
                "voice" => s.toggles.voice = on,
                "hz" => s.toggles.hz90 = v.trim() != "72",
                "page" => s.page = Page::from_name(v.trim()),
                "perf" => s.toggles.perf = on,
                "anchor" => s.toggles.anchor = on,
                "preset" => {
                    s.toggles.preset = crate::launcher::Preset::parse(v);
                    old_assistant = crate::launcher::Preset::captions_alias(v);
                }
                "controllers_work" => s.toggles.ctrl_work = CtrlMode::parse(v).unwrap_or(CtrlMode::WhenHeld),
                "controllers_play" => s.toggles.ctrl_play = CtrlMode::parse(v).unwrap_or(CtrlMode::Always),
                "ctrl_angle" => {
                    if let Ok(d) = omarchyvr_config::ctrl_angle::parse(v) {
                        s.toggles.ctrl_angle = d;
                    }
                }
                "new_exposure" => s.toggles.new_exposure = on,
                "room_glide" => s.toggles.room_glide = on,
                "ctrl_steady" => s.toggles.ctrl_steady = on,
                "drift_hold" => s.toggles.drift_hold = on,
                "screens" => s.toggles.screens = crate::screens::clamp(v.trim().parse().unwrap_or(0)),
                "scale_pct" => s.scale_pct = v.trim().parse::<u16>().ok().filter(|x| (50..=200).contains(x)).unwrap_or(100),
                _ => {}
            }
        }
        // a file from before the automatic setup: a finished onboarding was a finished setup
        s.setup = stage.unwrap_or(if s.onboarded { SetupStage::Done } else { SetupStage::Todo });
        // 2026-10-16: camera hands are off by default (they still need work); a saved `hands = true` the user never
        // chose themselves (the old default) is dropped, a switch the user turned on stays
        if !s.hands_chosen {
            s.toggles.hands = false;
            s.toggles.hands_roles = false;
        }
        // 2026-10-16: eye tracking is on by default (visual only); a state file from before that decision had the old
        // dev-only value, so it is switched on once (the user can switch it off afterwards: the key is then written)
        if !eye_v2 && !text.trim().is_empty() {
            s.toggles.eye = true;
        }
        // the old "assistant" preset was Work 6 with the voice (and its captions) on at start; the launcher now gets
        // `--preset work-6`, so the voice switch carries the captions for a user upgrading from it (saved from now on)
        if old_assistant {
            s.toggles.voice = true;
        }
        s
    }

    /// File text.
    pub fn render(&self) -> String {
        let t = &self.toggles;
        format!(
            "# Pimax app state (written by omarchyvr-app)\nonboarded = {}\nsetup_stage = {}\neye_visual = true\neye = {}\nsixdof = {}\nctrl_track = {}\nhands = {}\nhands_roles = {}\nhands_chosen = {}\npassthrough = {}\nvoice = {}\nhz = {}\nperf = {}\nanchor = {}\npreset = {}\ncontrollers_work = {}\ncontrollers_play = {}\nctrl_angle = {}\nnew_exposure = {}\nroom_glide = {}\nctrl_steady = {}\ndrift_hold = {}\nscreens = {}\nscale_pct = {}\nautostart = {}\nlast_mode = {}\nlast_result = {}\nlast_at = {}\nargv_at = {}\ndev_mode = {}\nnvidia_allow_untested = {}\nheadset = {}\ncolours = {}\n{}",
            self.onboarded,
            self.setup.name(),
            t.eye,
            t.sixdof,
            t.ctrl_track,
            t.hands,
            t.hands_roles,
            self.hands_chosen,
            t.passthrough,
            t.voice,
            t.hz(),
            t.perf,
            t.anchor,
            t.preset.name(),
            t.ctrl_work.name(),
            t.ctrl_play.name(),
            t.ctrl_angle,
            t.new_exposure,
            t.room_glide,
            t.ctrl_steady,
            t.drift_hold,
            t.screens,
            self.scale_pct,
            self.autostart,
            if self.last_vr { "vr" } else { "desk" },
            match self.last_ok {
                Some(true) => "ok",
                Some(false) => "failed",
                None => "none",
            },
            self.last_at,
            self.argv_at,
            self.dev_mode,
            self.nvidia_untested,
            self.headset.name(),
            self.colours.render(),
            self.page.map(|p| format!("page = {}\n", p.name())).unwrap_or_default()
        )
    }

    /// Load (defaults when missing).
    pub fn load(path: &Path) -> GuiState {
        std::fs::read_to_string(path).map(|t| GuiState::parse(&t)).unwrap_or_default()
    }

    /// Save atomically.
    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d)?;
        }
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, self.render())?;
        std::fs::rename(&tmp, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refresh_rate_defaults_to_90_and_keeps_a_saved_72() {
        assert!(GuiState::default().toggles.hz90, "fresh default is 90 Hz");
        assert!(GuiState::parse("onboarded = true\n").toggles.hz90, "absent = 90 Hz");
        assert!(GuiState::parse("hz = auto\n").toggles.hz90, "auto = 90 Hz");
        assert!(!GuiState::parse("hz = 72\n").toggles.hz90, "an explicit 72 is kept");
        let mut s = GuiState::default();
        s.toggles.hz90 = false;
        assert!(!GuiState::parse(&s.render()).toggles.hz90, "72 survives a save and load");
    }

    #[test]
    fn the_old_assistant_preset_keeps_its_captions() {
        // a gui-state.toml from before the locked layouts: preset = assistant (Work 6 + voice captions at start)
        let s = GuiState::parse("onboarded = true\nvoice = false\npreset = assistant\n");
        assert_eq!(s.toggles.preset, crate::launcher::Preset::Work6);
        assert!(s.toggles.voice, "the voice switch carries the captions the old preset turned on");
        let flags = crate::launcher::desk_flags(&DeskToggles { preset_flag: true, ..s.toggles });
        assert!(flags.iter().any(|f| f == "--voice") && flags.windows(2).any(|w| w[0] == "--preset" && w[1] == "work-6"), "{flags:?}");
        // saved again it is a plain work-6 with voice on; a plain work-6 does not switch the voice on
        let again = GuiState::parse(&s.render());
        assert!(again.toggles.voice && again.toggles.preset == crate::launcher::Preset::Work6);
        assert!(!GuiState::parse("onboarded = true\nvoice = false\npreset = work-6\n").toggles.voice);
    }

    #[test]
    fn round_trip_and_defaults() {
        let s = GuiState {
            setup: SetupStage::Restart,
            onboarded: true,
            toggles: DeskToggles { eye: false, sixdof: true, ctrl_track: true, hands: true, hands_roles: true, passthrough: true, voice: true, hz90: true, perf: false, anchor: true, preset: crate::launcher::Preset::Cinema, eye_default_on: false, screens: 2, ctrl_work: CtrlMode::Off, ctrl_play: CtrlMode::WhenHeld, ctrl_angle: -12, new_exposure: false, room_glide: false, ctrl_steady: false, drift_hold: false, tracking_flags: false, preset_flag: false },
            page: Some(Page::Tools),
            scale_pct: 130,
            autostart: true,
            last_vr: true,
            hands_chosen: true,
            last_ok: Some(false),
            last_at: 1_791_968_155,
            argv_at: 1_791_968_100,
            dev_mode: true,
            nvidia_untested: true,
            headset: crate::headset_choice::HeadsetPick::Model(0),
            colours: Colours::Theme(crate::colors::ThemeName::new("tokyo-night").unwrap()),
        };
        assert_eq!(GuiState::parse(&s.render()), s);
        assert_eq!(GuiState::parse(""), GuiState::default());
        assert!(!GuiState::default().autostart, "start at login is off by default");
        assert_eq!(GuiState::default().colours, Colours::Follow, "follow Omarchy by default");
        assert_eq!(GuiState::parse("colours = default\n").colours, Colours::Default);
        assert!(!GuiState::default().dev_mode, "developer mode is off by default");
        assert!(!GuiState::default().nvidia_untested, "the patch on untested NVIDIA drivers is off by default");
        assert!(!GuiState::parse("onboarded = true\n").nvidia_untested, "an older file: off");
        assert!(GuiState::parse("nvidia_allow_untested = true\n").nvidia_untested);
        assert_eq!(GuiState::default().headset, crate::headset_choice::HeadsetPick::Unset, "nothing is chosen on a fresh install");
        assert_eq!(GuiState::parse("headset = other\n").headset, crate::headset_choice::HeadsetPick::Other);
        assert_eq!(GuiState::parse("headset = dream-air\n").headset, crate::headset_choice::HeadsetPick::Model(0));
        assert_eq!(GuiState::default().setup, SetupStage::Todo, "a fresh install starts the automatic setup");
        assert_eq!(GuiState::parse("onboarded = true\n").setup, SetupStage::Done, "an older finished onboarding is a finished setup");
        assert_eq!(GuiState::parse("onboarded = true\nsetup_stage = restart\n").setup, SetupStage::Restart);
        assert_eq!(GuiState::parse("setup_stage = nonsense\n").setup, SetupStage::Todo);
        assert!(GuiState::default().toggles.eye, "eye tracking (visual only) is on by default");
        assert!(GuiState::parse("onboarded = true\neye = false\n").toggles.eye, "an older file with the dev-only value is switched on once");
        assert!(!GuiState::parse("eye_visual = true\neye = false\n").toggles.eye, "a later off is kept");
        assert!(GuiState::parse("wizard_done = true\npage = wizard\n").onboarded, "older files");
        assert_eq!(GuiState::parse("page = wizard\n").page, Some(Page::Onboarding));
        let d = std::env::temp_dir().join(format!("pimax-guistate-{}", std::process::id()));
        let f = d.join("gui-state.toml");
        assert_eq!(GuiState::load(&f), GuiState::default());
        s.save(&f).unwrap();
        assert_eq!(GuiState::load(&f), s);
        let _ = std::fs::remove_dir_all(d);
    }
}
