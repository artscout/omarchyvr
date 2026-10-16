//! `~/.config/pimax/auto.toml`: a flat `key = value` file (comments with `#`, section headers ignored).
//! No toml crate: the file is short and every value is a bool, a number or a word. Unknown keys are reported.
use std::path::{Path, PathBuf};

/// What starts when the headset is put on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// `pimax desk start` (WayVR desktop over passthrough).
    Desk,
    /// The game runtime (see [`Runtime`]); nothing desk-like on top.
    Game,
}

/// Runtime for games.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runtime {
    /// Our Monado + xrizer (`pimax vr start ...`).
    Monado,
    /// SteamVR (phase 1: static, the user starts it from Steam; auto mode only keeps out of its way).
    SteamVr,
}

impl Mode {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "desk" => Some(Self::Desk),
            "game" => Some(Self::Game),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Desk => "desk",
            Self::Game => "game",
        }
    }
}

impl Runtime {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "monado" => Some(Self::Monado),
            "steamvr" => Some(Self::SteamVr),
            _ => None,
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Monado => "monado",
            Self::SteamVr => "steamvr",
        }
    }
}

/// Desk presets (implemented by `pimax desk start --preset`).
pub const PRESETS: [&str; 6] = ["none", "cinema", "work", "work-3", "work-1", "assistant"];

/// All settings. Times in seconds.
#[derive(Debug, Clone, PartialEq)]
pub struct Config {
    /// Log what would be done, do nothing. Default true until the user flips it.
    pub dry_run: bool,
    /// Start a session when the headset is put on.
    pub auto_start: bool,
    pub mode: Mode,
    pub game_runtime: Runtime,
    /// `none`, `cinema`, `work`, `assistant`.
    pub preset: String,
    /// Desk/vr start flags.
    pub sixdof: bool,
    pub ctrl_track: bool,
    pub perf: bool,
    /// Relocalise against the room map `room_map` after a start, when that map exists.
    pub relocalize: bool,
    pub room_map: String,
    /// Stop a game session for idleness too (default: never touch a game).
    pub sleep_games: bool,
    /// Stop sessions that auto mode did not start (default: only its own).
    pub sleep_foreign: bool,
    /// Signal poll period.
    pub poll_s: f64,
    pub plug_debounce_s: f64,
    pub unplug_debounce_s: f64,
    /// Continuous wear evidence before a start.
    pub wear_on_s: f64,
    /// Not worn this long in a session -> idle-dim.
    pub dim_after_s: f64,
    /// Idle-dim this long -> sleep (session stopped).
    pub sleep_after_s: f64,
    /// A session that vanished for this long is gone.
    pub session_lost_s: f64,
    /// After a failed start, wait this long (and for a new put-on) before trying again.
    pub retry_after_s: f64,
    /// IMU gyro std (deg/s) below which the headset lies still; above `worn_dps` it is worn (hysteresis between).
    pub still_dps: f64,
    pub worn_dps: f64,
    /// Read the IMU (hidraw IF2, read-only, sends nothing) for the wear detection.
    pub imu: bool,
    /// Binaries.
    pub pimax_bin: PathBuf,
    pub roommap_bin: PathBuf,
}

impl Config {
    pub fn defaults(home: &Path) -> Self {
        Self {
            dry_run: true,
            auto_start: true,
            mode: Mode::Desk,
            game_runtime: Runtime::Monado,
            preset: "none".into(),
            sixdof: true,
            ctrl_track: true,
            perf: true,
            relocalize: true,
            room_map: "home".into(),
            sleep_games: false,
            sleep_foreign: false,
            poll_s: 2.0,
            plug_debounce_s: 4.0,
            unplug_debounce_s: 6.0,
            wear_on_s: 4.0,
            dim_after_s: 60.0,
            sleep_after_s: 600.0,
            session_lost_s: 10.0,
            retry_after_s: 120.0,
            still_dps: 0.2,
            worn_dps: 0.3,
            imu: true,
            pimax_bin: home.join(".local/bin/pimax"),
            roommap_bin: home.join("pimax-re/rust/target/release/pimax-roommap"),
        }
    }

    /// Parse `text` over the defaults. Returns the config and warnings (unknown keys, bad values).
    pub fn parse(text: &str, home: &Path) -> (Self, Vec<String>) {
        let mut c = Self::defaults(home);
        let mut warn = Vec::new();
        for (n, raw) in text.lines().enumerate() {
            let line = strip_comment(raw).trim();
            if line.is_empty() || line.starts_with('[') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else {
                warn.push(format!("line {}: not key = value", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), unquote(v.trim()));
            if let Err(e) = c.set(k, v, home) {
                warn.push(format!("line {}: {e}", n + 1));
            }
        }
        if c.worn_dps < c.still_dps {
            warn.push("worn_dps < still_dps: swapped".into());
            std::mem::swap(&mut c.worn_dps, &mut c.still_dps);
        }
        (c, warn)
    }

    /// Set one key (also used by the IPC `set` command).
    pub fn set(&mut self, k: &str, v: &str, home: &Path) -> Result<(), String> {
        let b = || parse_bool(v).ok_or_else(|| format!("{k}: want true/false, not {v:?}"));
        let f = |lo: f64, hi: f64| -> Result<f64, String> {
            v.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite() && (lo..=hi).contains(x))
                .ok_or_else(|| format!("{k}: want a number in {lo}..{hi}, not {v:?}"))
        };
        let path = || {
            if let Some(rest) = v.strip_prefix("~/") {
                home.join(rest)
            } else {
                PathBuf::from(v)
            }
        };
        match k {
            "dry_run" => self.dry_run = b()?,
            "auto_start" => self.auto_start = b()?,
            "mode" => self.mode = Mode::parse(v).ok_or(format!("mode: desk|game, not {v:?}"))?,
            "game_runtime" => {
                self.game_runtime =
                    Runtime::parse(v).ok_or(format!("game_runtime: monado|steamvr, not {v:?}"))?
            }
            "preset" => {
                if !PRESETS.contains(&v) {
                    return Err(format!("preset: one of {PRESETS:?}, not {v:?}"));
                }
                self.preset = v.into()
            }
            "sixdof" => self.sixdof = b()?,
            "ctrl_track" => self.ctrl_track = b()?,
            "perf" => self.perf = b()?,
            "relocalize" => self.relocalize = b()?,
            "room_map" => {
                if v.is_empty() || v.contains('/') {
                    return Err("room_map: a map name".into());
                }
                self.room_map = v.into()
            }
            "sleep_games" => self.sleep_games = b()?,
            "sleep_foreign" => self.sleep_foreign = b()?,
            "poll_s" => self.poll_s = f(0.2, 30.0)?,
            "plug_debounce_s" => self.plug_debounce_s = f(0.0, 120.0)?,
            "unplug_debounce_s" => self.unplug_debounce_s = f(0.0, 120.0)?,
            "wear_on_s" => self.wear_on_s = f(0.0, 120.0)?,
            "dim_after_s" => self.dim_after_s = f(5.0, 86400.0)?,
            "sleep_after_s" => self.sleep_after_s = f(0.0, 86400.0)?,
            "session_lost_s" => self.session_lost_s = f(1.0, 600.0)?,
            "retry_after_s" => self.retry_after_s = f(0.0, 3600.0)?,
            "still_dps" => self.still_dps = f(0.0, 50.0)?,
            "worn_dps" => self.worn_dps = f(0.0, 50.0)?,
            "imu" => self.imu = b()?,
            "pimax_bin" => self.pimax_bin = path(),
            "roommap_bin" => self.roommap_bin = path(),
            _ => return Err(format!("unknown key {k:?}")),
        }
        Ok(())
    }
}

fn strip_comment(s: &str) -> &str {
    let mut inq = false;
    for (i, ch) in s.char_indices() {
        match ch {
            '"' => inq = !inq,
            '#' if !inq => return &s[..i],
            _ => {}
        }
    }
    s
}

fn unquote(s: &str) -> &str {
    s.strip_prefix('"')
        .and_then(|x| x.strip_suffix('"'))
        .unwrap_or(s)
}

fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "true" | "1" | "yes" | "on" => Some(true),
        "false" | "0" | "no" | "off" => Some(false),
        _ => None,
    }
}

/// `$PIMAX_AUTO_CONFIG` or `~/.config/pimax/auto.toml`.
pub fn default_path(home: &Path) -> PathBuf {
    std::env::var_os("PIMAX_AUTO_CONFIG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config/pimax/auto.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_safe() {
        let c = Config::defaults(Path::new("/h"));
        assert!(c.dry_run, "dry-run until the user flips it");
        assert!(!c.sleep_games && !c.sleep_foreign);
        assert_eq!(c.mode, Mode::Desk);
        assert_eq!(c.game_runtime, Runtime::Monado);
        assert!(c.sixdof && c.ctrl_track && c.perf && c.relocalize);
    }

    #[test]
    fn example_file_parses_without_warnings_and_matches_the_defaults() {
        let text = include_str!("../../../packaging/auto.toml.example");
        let (c, w) = Config::parse(text, Path::new("/h"));
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c, Config::defaults(Path::new("/h")));
    }

    #[test]
    fn values_comments_and_errors() {
        let (c, w) = Config::parse(
            "# hi\n[auto]\ndry_run = false # live\nmode = \"game\"\ngame_runtime = steamvr\npreset = cinema\n\
             dim_after_s = 30\npimax_bin = ~/bin/pimax\nbogus = 1\nwear_on_s = -1\nnoequals\n",
            Path::new("/h"),
        );
        assert!(!c.dry_run);
        assert_eq!(c.mode, Mode::Game);
        assert_eq!(c.game_runtime, Runtime::SteamVr);
        assert_eq!(c.preset, "cinema");
        assert_eq!(c.dim_after_s, 30.0);
        assert_eq!(c.pimax_bin, PathBuf::from("/h/bin/pimax"));
        assert_eq!(c.wear_on_s, 4.0, "bad value keeps the default");
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn thresholds_are_ordered() {
        let (c, w) = Config::parse("still_dps = 1\nworn_dps = 0.5\n", Path::new("/h"));
        assert!(c.still_dps < c.worn_dps);
        assert_eq!(w.len(), 1);
    }
}
