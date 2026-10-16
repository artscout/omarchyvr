//! What the app remembers between runs (`~/.config/pimax/gui-state.toml`): the onboarding was finished, the last desk
//! switches and refresh rate. `key = value` lines, written atomically (temp file + rename). Missing or unreadable
//! state means defaults.
use pimax_config::ctrl_inhand::CtrlMode;
use crate::app::Page;
use crate::launcher::DeskToggles;
use std::path::Path;

/// Remembered state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GuiState {
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
    /// The last start was a game session (`pimax vr start`), not desk mode: Play starts the same kind.
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
}

impl Default for GuiState {
    fn default() -> Self {
        GuiState {
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
        }
    }
}

impl GuiState {
    /// Parse the file text.
    pub fn parse(text: &str) -> GuiState {
        let mut s = GuiState::default();
        let mut eye_v2 = false;
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            let on = matches!(v.trim(), "true" | "1" | "yes");
            match k.trim() {
                "onboarded" | "wizard_done" => s.onboarded = on,
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
                "passthrough" => s.toggles.passthrough = on,
                "voice" => s.toggles.voice = on,
                "hz" => s.toggles.hz90 = v.trim() == "90",
                "page" => s.page = Page::from_name(v.trim()),
                "perf" => s.toggles.perf = on,
                "anchor" => s.toggles.anchor = on,
                "preset" => s.toggles.preset = crate::launcher::Preset::parse(v),
                "controllers_work" => s.toggles.ctrl_work = CtrlMode::parse(v).unwrap_or(CtrlMode::WhenHeld),
                "controllers_play" => s.toggles.ctrl_play = CtrlMode::parse(v).unwrap_or(CtrlMode::Always),
                "screens" => s.toggles.screens = crate::screens::clamp(v.trim().parse().unwrap_or(0)),
                "scale_pct" => s.scale_pct = v.trim().parse::<u16>().ok().filter(|x| (50..=200).contains(x)).unwrap_or(100),
                _ => {}
            }
        }
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
        s
    }

    /// File text.
    pub fn render(&self) -> String {
        let t = &self.toggles;
        format!(
            "# Pimax app state (written by pimax-setup)\nonboarded = {}\neye_visual = true\neye = {}\nsixdof = {}\nctrl_track = {}\nhands = {}\nhands_roles = {}\nhands_chosen = {}\npassthrough = {}\nvoice = {}\nhz = {}\nperf = {}\nanchor = {}\npreset = {}\ncontrollers_work = {}\ncontrollers_play = {}\nscreens = {}\nscale_pct = {}\nautostart = {}\nlast_mode = {}\nlast_result = {}\nlast_at = {}\nargv_at = {}\ndev_mode = {}\n{}",
            self.onboarded,
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
    fn round_trip_and_defaults() {
        let s = GuiState {
            onboarded: true,
            toggles: DeskToggles { eye: false, sixdof: true, ctrl_track: true, hands: true, hands_roles: true, passthrough: true, voice: true, hz90: true, perf: false, anchor: true, preset: crate::launcher::Preset::Cinema, eye_default_on: false, screens: 2, ctrl_work: CtrlMode::Off, ctrl_play: CtrlMode::WhenHeld },
            page: Some(Page::Tools),
            scale_pct: 130,
            autostart: true,
            last_vr: true,
            hands_chosen: true,
            last_ok: Some(false),
            last_at: 1_791_968_155,
            argv_at: 1_791_968_100,
            dev_mode: true,
        };
        assert_eq!(GuiState::parse(&s.render()), s);
        assert_eq!(GuiState::parse(""), GuiState::default());
        assert!(!GuiState::default().autostart, "start at login is off by default");
        assert!(!GuiState::default().dev_mode, "developer mode is off by default");
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
