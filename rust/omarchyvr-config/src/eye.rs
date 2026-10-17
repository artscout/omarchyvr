//! Eye tracking settings shared by the Pimax app and `omarchyvr desk start`: `~/.config/omarchyvr/eye.conf`.
//!
//! ```text
//! dot=on          # the small gaze dot in Work (WayVR): visual only. on (default) | off
//! gaze_mouse=off  # the ONE gaze input switch: on = your eyes move the pointer and can click (and voice "this"
//!                 # may use the window you look at). off (default) = the gaze is only a visual point.
//! calibration=auto  # which eye calibration: auto (default: yours once you calibrated, else the built-in one) |
//!                   # builtin (the developer's test calibration) | own (your own; none until you calibrate)
//! ```
//! Eye tracking itself (running or not) is the app's "Eye tracking" switch. By default it is never an input: no
//! pointer, no click, no focus, no cursor movement (user decision 2026-10-17). Unknown lines are ignored.
//!
//! Migration: the old developer key `targeting=` (and WayVR's `gaze_pointer` in wayvr-desk.conf, whose code default
//! was `true`) are NOT read: a saved value cannot be told apart from the old default, so every old file reads as
//! `gaze_mouse=off`. Only the new key, written by the new switch, turns the gaze mouse on.
use std::path::{Path, PathBuf};

/// Which eye calibration the setting asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CalMode {
    /// Your own once it exists, else the built-in one (default).
    #[default]
    Auto,
    /// Always the built-in test calibration (the developer's eyes).
    Builtin,
    /// Always your own calibration.
    Own,
}

/// The calibration that is really used.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CalSource {
    /// The built-in test calibration (someone else's eyes): sent to the chip with the start list.
    Builtin,
    /// The user's own calibration (`eye-calibration.toml`, applied on the host).
    Own,
}

impl CalMode {
    /// The file word.
    pub fn word(self) -> &'static str {
        match self {
            CalMode::Auto => "auto",
            CalMode::Builtin => "builtin",
            CalMode::Own => "own",
        }
    }

    /// What is used, given whether the user's own calibration file exists. `Own` without a file still means "own":
    /// the eye then does not start (calibrate first), as before.
    pub fn source(self, own_exists: bool) -> CalSource {
        match self {
            CalMode::Builtin => CalSource::Builtin,
            CalMode::Own => CalSource::Own,
            CalMode::Auto if own_exists => CalSource::Own,
            CalMode::Auto => CalSource::Builtin,
        }
    }
}

/// The settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EyeSettings {
    /// Show the dot where you look (visual effect).
    pub dot: bool,
    /// The one gaze input switch (default off): the eyes move the pointer and can click; voice "this" may use the
    /// window looked at. Off: the gaze is only a visual point.
    pub mouse: bool,
    /// Which eye calibration to use.
    pub calibration: CalMode,
}

impl Default for EyeSettings {
    fn default() -> Self {
        EyeSettings { dot: true, mouse: false, calibration: CalMode::Auto }
    }
}

impl EyeSettings {
    /// Parse the file text.
    pub fn parse(text: &str) -> EyeSettings {
        let mut s = EyeSettings::default();
        for l in text.lines() {
            let Some((k, v)) = l.split_once('=') else { continue };
            let v = v.split('#').next().unwrap_or("").trim();
            let on = matches!(v, "on" | "true" | "1" | "yes");
            match k.trim() {
                "dot" => s.dot = on || !matches!(v, "off" | "false" | "0" | "no"),
                "gaze_mouse" => s.mouse = on,
                // "targeting" (the old developer key) is ignored on purpose: migrated to off
                "calibration" => {
                    s.calibration = match v {
                        "builtin" => CalMode::Builtin,
                        "own" => CalMode::Own,
                        _ => CalMode::Auto,
                    }
                }
                _ => {}
            }
        }
        s
    }

    /// The file text.
    pub fn render(&self) -> String {
        let f = |b: bool| if b { "on" } else { "off" };
        format!(
            "# written by the Pimax app (Eye tracking); gaze_mouse=off: the gaze is only a visual point\ndot={}\ngaze_mouse={}\ncalibration={}\n",
            f(self.dot),
            f(self.mouse),
            self.calibration.word()
        )
    }

    /// Load (defaults when the file is missing or unreadable).
    pub fn load(path: &Path) -> EyeSettings {
        std::fs::read_to_string(path).map(|t| EyeSettings::parse(&t)).unwrap_or_default()
    }

    /// Save atomically.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(d) = path.parent() {
            std::fs::create_dir_all(d).map_err(|e| e.to_string())?;
        }
        let tmp = path.with_extension("conf.tmp");
        std::fs::write(&tmp, self.render()).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    }
}

/// `$XDG_CONFIG_HOME/omarchyvr/eye.conf`, else `$HOME/.config/omarchyvr/eye.conf`.
pub fn path_from(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into())).join(".config"));
    base.join("omarchyvr/eye.conf")
}

/// [`path_from`] with this process's environment.
pub fn default_path() -> PathBuf {
    path_from(&|k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_dot_on_and_no_gaze_mouse() {
        assert_eq!(EyeSettings::parse(""), EyeSettings { dot: true, mouse: false, calibration: CalMode::Auto });
        assert_eq!(EyeSettings::parse("junk\ndot=off\n"), EyeSettings { dot: false, ..EyeSettings::default() });
        assert_eq!(EyeSettings::parse("gaze_mouse = on # explicit\n"), EyeSettings { mouse: true, ..EyeSettings::default() });
    }

    #[test]
    fn old_targeting_key_migrates_to_off() {
        // written by the old app (developer "Gaze targets windows"): never turns gaze input on any more
        let old = "# written by the Pimax app (Eye tracking); visual only\ndot=on\ntargeting=on\ncalibration=auto\n";
        let s = EyeSettings::parse(old);
        assert!(!s.mouse);
        assert!(s.render().contains("gaze_mouse=off\n") && !s.render().contains("targeting"));
    }

    #[test]
    fn render_round_trips_and_saves() {
        let s = EyeSettings { dot: false, mouse: true, calibration: CalMode::Builtin };
        assert_eq!(EyeSettings::parse(&s.render()), s);
        let d = std::env::temp_dir().join(format!("pimax-eye-conf-{}", std::process::id()));
        let p = d.join("omarchyvr/eye.conf");
        s.save(&p).unwrap();
        assert_eq!(EyeSettings::load(&p), s);
        assert_eq!(EyeSettings::load(&d.join("none")), EyeSettings::default());
        let _ = std::fs::remove_dir_all(d);
        assert_eq!(path_from(&|k| (k == "HOME").then(|| "/h".to_string())), PathBuf::from("/h/.config/omarchyvr/eye.conf"));
    }

    #[test]
    fn calibration_setting_round_trips_and_picks_the_source() {
        for m in [CalMode::Auto, CalMode::Builtin, CalMode::Own] {
            let s = EyeSettings { calibration: m, ..EyeSettings::default() };
            assert_eq!(EyeSettings::parse(&s.render()).calibration, m);
        }
        assert_eq!(EyeSettings::parse("calibration=nonsense\n").calibration, CalMode::Auto);
        assert_eq!(CalMode::Auto.source(false), CalSource::Builtin, "default: the built-in test calibration");
        assert_eq!(CalMode::Auto.source(true), CalSource::Own, "after the user's own calibration");
        assert_eq!(CalMode::Builtin.source(true), CalSource::Builtin);
        assert_eq!(CalMode::Own.source(false), CalSource::Own);
    }
}
