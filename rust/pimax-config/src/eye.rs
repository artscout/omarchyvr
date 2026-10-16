//! Eye tracking settings shared by the Pimax app and `pimax desk start`: `~/.config/pimax/eye.conf`.
//!
//! ```text
//! dot=on          # the small gaze dot in Work (WayVR): visual only. on (default) | off
//! targeting=off   # developer: gaze may target windows for voice ("close this"). off (default) | on
//! ```
//! Eye tracking itself (running or not) is the app's "Eye tracking" switch. It is never an input: no pointer, no
//! selection, no scrolling. Unknown lines are ignored.
use std::path::{Path, PathBuf};

/// The settings.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EyeSettings {
    /// Show the dot where you look (visual effect).
    pub dot: bool,
    /// Developer: let the gaze target windows for voice commands.
    pub targeting: bool,
}

impl Default for EyeSettings {
    fn default() -> Self {
        EyeSettings { dot: true, targeting: false }
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
                "targeting" => s.targeting = on,
                _ => {}
            }
        }
        s
    }

    /// The file text.
    pub fn render(&self) -> String {
        let f = |b: bool| if b { "on" } else { "off" };
        format!("# written by the Pimax app (Eye tracking); visual only\ndot={}\ntargeting={}\n", f(self.dot), f(self.targeting))
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

/// `$XDG_CONFIG_HOME/pimax/eye.conf`, else `$HOME/.config/pimax/eye.conf`.
pub fn path_from(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    let base = env("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into())).join(".config"));
    base.join("pimax/eye.conf")
}

/// [`path_from`] with this process's environment.
pub fn default_path() -> PathBuf {
    path_from(&|k| std::env::var(k).ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_dot_on_and_no_targeting() {
        assert_eq!(EyeSettings::parse(""), EyeSettings { dot: true, targeting: false });
        assert_eq!(EyeSettings::parse("junk\ndot=off\n"), EyeSettings { dot: false, targeting: false });
        assert_eq!(EyeSettings::parse("targeting = on # dev\n"), EyeSettings { dot: true, targeting: true });
    }

    #[test]
    fn render_round_trips_and_saves() {
        let s = EyeSettings { dot: false, targeting: true };
        assert_eq!(EyeSettings::parse(&s.render()), s);
        let d = std::env::temp_dir().join(format!("pimax-eye-conf-{}", std::process::id()));
        let p = d.join("pimax/eye.conf");
        s.save(&p).unwrap();
        assert_eq!(EyeSettings::load(&p), s);
        assert_eq!(EyeSettings::load(&d.join("none")), EyeSettings::default());
        let _ = std::fs::remove_dir_all(d);
        assert_eq!(path_from(&|k| (k == "HOME").then(|| "/h".to_string())), PathBuf::from("/h/.config/pimax/eye.conf"));
    }
}
