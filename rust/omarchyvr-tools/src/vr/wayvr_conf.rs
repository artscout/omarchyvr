//! WayVR settings the Pimax app lets the user change for desk mode: written by the app to
//! `~/.config/omarchyvr/wayvr-desk.conf` (`key = value`), turned by `omarchyvr desk start` into
//! `<wayvr home>/config/wayvr/conf.d/50-pimax-app.yaml`. That file sorts after the launcher's own `30-worldlock.yaml`
//! and `35-desk.yaml` (rewritten every start), so the app's values win; WayVR reads its config once, at start.
//!
//! The YAML is generated whole, every key once, from validated values: WayVR panics at start on a duplicated key,
//! a null value, `min > max` of the grab limits or a non-finite number (wayvr `config.rs`, `grabmath.rs`). An invalid
//! settings file is not applied at all.
use std::io;
use std::path::{Path, PathBuf};

/// The conf.d file the launcher writes.
pub const CONF_NAME: &str = "50-pimax-app.yaml";

/// Number badges on the VR screens (`screen_numbers` of WayVR; numbering in docs/SCREENS.md: 1 main, 2 left, 3 right,
/// 4 up, 5 up-left, 6 up-right).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScreenNumbers {
    /// Never shown.
    Off,
    /// Shown for about 5 s when a screen is added or moved, or when voice hears "Max" or a screen command.
    #[default]
    OnDemand,
    /// Always shown.
    Always,
}

impl ScreenNumbers {
    /// The word in the settings file and in `wayvrctl screen-numbers`.
    pub fn key(self) -> &'static str {
        match self {
            ScreenNumbers::Off => "off",
            ScreenNumbers::OnDemand => "ondemand",
            ScreenNumbers::Always => "always",
        }
    }
    /// The word in WayVR's YAML.
    pub fn yaml_key(self) -> &'static str {
        match self {
            ScreenNumbers::OnDemand => "on_demand",
            o => o.key(),
        }
    }
    /// Parse a settings word (`off`, `ondemand` / `on_demand` / `on-demand` / `on demand`, `always`).
    pub fn parse(v: &str) -> Option<ScreenNumbers> {
        match v.trim().to_lowercase().replace(['_', '-', ' '], "").as_str() {
            "off" | "false" | "no" | "never" => Some(ScreenNumbers::Off),
            "ondemand" | "demand" | "auto" => Some(ScreenNumbers::OnDemand),
            "always" | "on" | "true" | "yes" => Some(ScreenNumbers::Always),
            _ => None,
        }
    }
}

/// The settings (defaults = what the launcher writes without them).
#[derive(Debug, Clone, PartialEq)]
pub struct WayvrSettings {
    /// Grip moves a screen; release leaves it world-locked (`world_lock_grab`).
    pub world_lock_grab: bool,
    /// A/X click pins a screen (`world_lock_pin`, needs `world_lock_grab`).
    pub world_lock_pin: bool,
    /// Push/pull also scales, so the screen keeps its apparent size (`grab_keep_apparent_size`).
    pub grab_keep_apparent_size: bool,
    /// Push/pull speed (`grab_dist_rate`, 0 = off).
    pub grab_dist_rate: f32,
    /// Smallest screen width in metres (`grab_size_min`).
    pub grab_size_min: f32,
    /// Largest screen width in metres (`grab_size_max`).
    pub grab_size_max: f32,
    /// Nearest distance in metres (`grab_dist_min`).
    pub grab_dist_min: f32,
    /// Farthest distance in metres (`grab_dist_max`).
    pub grab_dist_max: f32,
    /// The WayVR keyboard overlay (`enable_keyboard`; off in desk mode).
    pub enable_keyboard: bool,
    /// The eye gaze pointer (`gaze_pointer`; needs `desk start --eye`).
    pub gaze_pointer: bool,
    /// No grab hint popup (`hide_grab_help`).
    pub hide_grab_help: bool,
    /// Controllers work in Work (`controllers_in_work`, default false = hands only: controller rays, buttons, sticks
    /// and grabs are ignored entirely; `wayvrctl controllers on|off` / voice "use controllers" / "hands only"
    /// change it in a running session).
    pub controllers_in_work: bool,
    /// 3D controller models at the controllers while they are the pointer (`show_controller_models`, Touch v3 /
    /// Quest 2 model like SteamVR's, trigger/grip/stick/buttons move; hidden with hands only).
    pub controller_models: bool,
    /// Debug: axes at each controller's aim (ray origin) and grip pose (`controller_axes`; red X, green Y, blue =
    /// ray direction), to check in the headset that the ray leaves the controller tip.
    pub controller_axes: bool,
    /// Number badges (1-6) at the top centre of each VR screen: off, on demand (default) or always
    /// (`screen_numbers`; `wayvrctl screen-numbers ...` changes it in a running session).
    pub screen_numbers: ScreenNumbers,
    /// The screen the voice captions sit under (`PIMAX_CAPTIONS_SCREEN` for WayVR; None = the first screen).
    pub captions_screen: Option<String>,
}

impl Default for WayvrSettings {
    fn default() -> Self {
        WayvrSettings {
            world_lock_grab: true,
            world_lock_pin: false,
            grab_keep_apparent_size: true,
            grab_dist_rate: 0.8,
            grab_size_min: 0.3,
            grab_size_max: 6.0,
            grab_dist_min: 0.4,
            grab_dist_max: 8.0,
            enable_keyboard: false,
            gaze_pointer: true,
            hide_grab_help: false,
            controllers_in_work: false,
            controller_models: true,
            controller_axes: false,
            screen_numbers: ScreenNumbers::OnDemand,
            captions_screen: None,
        }
    }
}

/// `~/.config/omarchyvr/wayvr-desk.conf`.
pub fn settings_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let config = env("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into())).join(".config")
    });
    config.join("omarchyvr/wayvr-desk.conf")
}

fn parse_bool(v: &str) -> Option<bool> {
    match v {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

impl WayvrSettings {
    /// Read a settings file text: unknown keys and bad values are reported and left at their defaults.
    pub fn parse(text: &str) -> (WayvrSettings, Vec<String>) {
        let mut s = WayvrSettings::default();
        let mut warn = Vec::new();
        for (n, l) in text.lines().enumerate() {
            let l = l.trim();
            if l.is_empty() || l.starts_with('#') {
                continue;
            }
            let Some((k, v)) = l.split_once('=') else {
                warn.push(format!("line {}: no `key = value`", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim().trim_matches('"'));
            let b = |w: &mut Vec<String>| {
                let r = parse_bool(v);
                if r.is_none() {
                    w.push(format!("line {}: {k} wants true or false, not {v:?}", n + 1));
                }
                r
            };
            let f = |w: &mut Vec<String>| {
                let r = v.parse::<f32>().ok().filter(|x| x.is_finite());
                if r.is_none() {
                    w.push(format!("line {}: {k} wants a number, not {v:?}", n + 1));
                }
                r
            };
            match k {
                "world_lock_grab" => s.world_lock_grab = b(&mut warn).unwrap_or(s.world_lock_grab),
                "world_lock_pin" => s.world_lock_pin = b(&mut warn).unwrap_or(s.world_lock_pin),
                "grab_keep_apparent_size" => s.grab_keep_apparent_size = b(&mut warn).unwrap_or(s.grab_keep_apparent_size),
                "enable_keyboard" => s.enable_keyboard = b(&mut warn).unwrap_or(s.enable_keyboard),
                "gaze_pointer" => s.gaze_pointer = b(&mut warn).unwrap_or(s.gaze_pointer),
                "hide_grab_help" => s.hide_grab_help = b(&mut warn).unwrap_or(s.hide_grab_help),
                "controllers_in_work" => s.controllers_in_work = b(&mut warn).unwrap_or(s.controllers_in_work),
                "controller_models" => s.controller_models = b(&mut warn).unwrap_or(s.controller_models),
                "controller_axes" => s.controller_axes = b(&mut warn).unwrap_or(s.controller_axes),
                // the old name (before 2026-10-15): hands only = the inverse
                "prefer_hands" => s.controllers_in_work = b(&mut warn).map_or(s.controllers_in_work, |v| !v),
                "grab_dist_rate" => s.grab_dist_rate = f(&mut warn).unwrap_or(s.grab_dist_rate),
                "grab_size_min" => s.grab_size_min = f(&mut warn).unwrap_or(s.grab_size_min),
                "grab_size_max" => s.grab_size_max = f(&mut warn).unwrap_or(s.grab_size_max),
                "grab_dist_min" => s.grab_dist_min = f(&mut warn).unwrap_or(s.grab_dist_min),
                "grab_dist_max" => s.grab_dist_max = f(&mut warn).unwrap_or(s.grab_dist_max),
                "screen_numbers" => match ScreenNumbers::parse(v) {
                    Some(m) => s.screen_numbers = m,
                    None => warn.push(format!("line {}: screen_numbers wants off, ondemand or always, not {v:?}", n + 1)),
                },
                "captions_screen" => s.captions_screen = Some(v.to_string()).filter(|x| !x.is_empty()),
                _ => warn.push(format!("line {}: unknown key {k}", n + 1)),
            }
        }
        (s, warn)
    }

    /// The settings file text.
    pub fn render(&self) -> String {
        let mut o = String::from("# WayVR in desk mode, written by the Pimax app; `omarchyvr desk start` turns it into conf.d/50-pimax-app.yaml\n");
        o += &format!("world_lock_grab = {}\n", self.world_lock_grab);
        o += &format!("world_lock_pin = {}\n", self.world_lock_pin);
        o += &format!("grab_keep_apparent_size = {}\n", self.grab_keep_apparent_size);
        o += &format!("grab_dist_rate = {:.2}\n", self.grab_dist_rate);
        o += &format!("grab_size_min = {:.2}\n", self.grab_size_min);
        o += &format!("grab_size_max = {:.2}\n", self.grab_size_max);
        o += &format!("grab_dist_min = {:.2}\n", self.grab_dist_min);
        o += &format!("grab_dist_max = {:.2}\n", self.grab_dist_max);
        o += &format!("enable_keyboard = {}\n", self.enable_keyboard);
        o += &format!("gaze_pointer = {}\n", self.gaze_pointer);
        o += &format!("hide_grab_help = {}\n", self.hide_grab_help);
        o += &format!("controllers_in_work = {}\n", self.controllers_in_work);
        o += &format!("controller_models = {}\n", self.controller_models);
        o += &format!("controller_axes = {}\n", self.controller_axes);
        o += &format!("screen_numbers = {}\n", self.screen_numbers.key());
        if let Some(s) = &self.captions_screen {
            o += &format!("captions_screen = {s}\n");
        }
        o
    }

    /// What WayVR would refuse or panic on.
    pub fn validate(&self) -> Vec<String> {
        let mut v = Vec::new();
        let pos = |name: &str, x: f32, v: &mut Vec<String>| {
            if !x.is_finite() || x <= 0.0 {
                v.push(format!("{name} must be a positive number"));
            }
        };
        pos("grab_size_min", self.grab_size_min, &mut v);
        pos("grab_size_max", self.grab_size_max, &mut v);
        pos("grab_dist_min", self.grab_dist_min, &mut v);
        pos("grab_dist_max", self.grab_dist_max, &mut v);
        if self.grab_size_min > self.grab_size_max {
            v.push("the smallest screen size is above the largest".into());
        }
        if self.grab_dist_min > self.grab_dist_max {
            v.push("the nearest distance is above the farthest".into());
        }
        if !self.grab_dist_rate.is_finite() || !(0.0..=5.0).contains(&self.grab_dist_rate) {
            v.push("grab_dist_rate must be 0-5".into());
        }
        if self.grab_size_max > 50.0 || self.grab_dist_max > 100.0 {
            v.push("the size or distance limit is not a room any more (max 50 m wide, 100 m away)".into());
        }
        if let Some(s) = &self.captions_screen {
            if s.chars().any(|c| c.is_control() || c == '"') || s.len() > 64 {
                v.push("the captions screen name is not an output name".into());
            }
        }
        v
    }

    /// The conf.d YAML (each key once, lowercase booleans, plain numbers).
    pub fn yaml(&self) -> String {
        format!(
            "# written by `omarchyvr desk start` from ~/.config/omarchyvr/wayvr-desk.conf (the Pimax app); do not edit\n\
             world_lock_grab: {}\nworld_lock_pin: {}\ngrab_keep_apparent_size: {}\ngrab_dist_rate: {:.3}\n\
             grab_size_min: {:.3}\ngrab_size_max: {:.3}\ngrab_dist_min: {:.3}\ngrab_dist_max: {:.3}\n\
             enable_keyboard: {}\ngaze_pointer: {}\nhide_grab_help: {}\ncontrollers_in_work: {}\n\
             show_controller_models: {}\ncontroller_axes: {}\nscreen_numbers: {}\n",
            self.world_lock_grab,
            self.world_lock_pin,
            self.grab_keep_apparent_size,
            self.grab_dist_rate,
            self.grab_size_min,
            self.grab_size_max,
            self.grab_dist_min,
            self.grab_dist_max,
            self.enable_keyboard,
            self.gaze_pointer,
            self.hide_grab_help,
            self.controllers_in_work,
            self.controller_models,
            self.controller_axes,
            self.screen_numbers.yaml_key()
        )
    }
}

/// Read the settings file (None when there is none); problems in it come back as notes.
pub fn load(path: &Path) -> (Option<WayvrSettings>, Vec<String>) {
    match std::fs::read_to_string(path) {
        Ok(t) => {
            let (s, w) = WayvrSettings::parse(&t);
            (Some(s), w)
        }
        Err(_) => (None, Vec::new()),
    }
}

/// Write (or remove) `conf.d/50-pimax-app.yaml` of a WayVR home. Returns a note for the start output.
pub fn apply(wayvr_home: &Path, s: Option<&WayvrSettings>) -> io::Result<Option<String>> {
    let conf_d = wayvr_home.join("config/wayvr/conf.d");
    let f = conf_d.join(CONF_NAME);
    let Some(s) = s else {
        if f.exists() {
            std::fs::remove_file(&f)?;
            return Ok(Some(format!("removed {CONF_NAME} (no app settings)")));
        }
        return Ok(None);
    };
    let bad = s.validate();
    if !bad.is_empty() {
        let _ = std::fs::remove_file(&f);
        return Ok(Some(format!("the app's WayVR settings are not applied: {}", bad.join("; "))));
    }
    std::fs::create_dir_all(&conf_d)?;
    let tmp = conf_d.join(".50-pimax-app.yaml.tmp");
    std::fs::write(&tmp, s.yaml())?;
    std::fs::rename(&tmp, &f)?;
    Ok(Some(format!("WayVR settings of the Pimax app: {}", f.display())))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_validation_and_yaml() {
        let d = WayvrSettings::default();
        let (back, warn) = WayvrSettings::parse(&d.render());
        assert!(warn.is_empty(), "{warn:?}");
        assert_eq!(back, d);
        let mut s = WayvrSettings { captions_screen: Some("SUNSHINE".into()), grab_size_min: 2.0, ..d.clone() };
        assert!(s.validate().is_empty());
        s.grab_size_max = 1.0;
        assert_eq!(s.validate(), vec!["the smallest screen size is above the largest".to_string()]);
        let y = d.yaml();
        let keys: Vec<&str> = y.lines().filter(|l| !l.starts_with('#')).map(|l| l.split(':').next().unwrap()).collect();
        let mut uniq = keys.clone();
        uniq.sort();
        uniq.dedup();
        assert_eq!(keys.len(), uniq.len(), "every key once: {y}");
        assert!(y.contains("world_lock_grab: true\n") && y.contains("grab_dist_rate: 0.800\n"));
        assert!(y.contains("show_controller_models: true\n") && y.contains("controller_axes: false\n"));
        let (m, w) = WayvrSettings::parse("controller_models = off\ncontroller_axes = on\n");
        assert!(w.is_empty() && !m.controller_models && m.controller_axes);
        assert!(y.contains("screen_numbers: on_demand\n"), "default: on demand");
        let (m, w) = WayvrSettings::parse("screen_numbers = always\n");
        assert!(w.is_empty() && m.screen_numbers == ScreenNumbers::Always && m.yaml().contains("screen_numbers: always\n"));
        assert_eq!(WayvrSettings::parse(&m.render()).0, m);
        assert_eq!(ScreenNumbers::parse("On demand"), Some(ScreenNumbers::OnDemand));
        assert_eq!(WayvrSettings::parse("screen_numbers = maybe\n").1.len(), 1);
        let (_, w) = WayvrSettings::parse("bogus = 1\ngaze_pointer = maybe\ngrab_dist_max = nan\nnoequals\n");
        assert_eq!(w.len(), 4, "{w:?}");
        assert_eq!(settings_path(&|k| (k == "HOME").then(|| "/home/u".to_string())), PathBuf::from("/home/u/.config/omarchyvr/wayvr-desk.conf"));
    }

    #[test]
    fn apply_writes_or_removes_the_conf_file() {
        let h = crate::vr::test_dir("wayvr-conf");
        let f = h.join("config/wayvr/conf.d").join(CONF_NAME);
        assert_eq!(apply(&h, None).unwrap(), None);
        let s = WayvrSettings::default();
        assert!(apply(&h, Some(&s)).unwrap().unwrap().contains(CONF_NAME));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), s.yaml());
        let bad = WayvrSettings { grab_dist_min: 9.0, ..s };
        assert!(apply(&h, Some(&bad)).unwrap().unwrap().contains("not applied"));
        assert!(!f.exists(), "an invalid set never reaches WayVR");
        std::fs::write(&f, "x: 1\n").unwrap();
        assert!(apply(&h, None).unwrap().unwrap().contains("removed"));
        assert!(!f.exists());
        let _ = std::fs::remove_dir_all(h);
    }
}
