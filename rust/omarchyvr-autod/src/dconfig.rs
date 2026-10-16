//! `~/.config/omarchyvr/omarchyvrd.toml`: what "everything on" means, the restart budgets and switches, and (user decision
//! 2026-10-15) auto mode's keys in `[auto]`. Without an `[auto]` section, auto mode still reads `auto.toml`.
//! No toml crate: sections and `key = value` lines (bools, numbers, words, quoted strings). Unknown keys warn.
//!
//! ```toml
//! [session]
//! mode = "desk"            # desk | vr
//! [components]             # wanted while a session runs (used when omarchyvrd itself starts a session)
//! passthrough = true
//! sixdof = true
//! ctrltrack = true
//! eye = "silent"           # off | silent | pointer
//! eye_laser = true
//! perf = true
//! voice = false
//! voicehud = false
//! ai_server = false
//! anchor = true
//! floor_follow = false
//! hands = false            # camera hands (--hands; needs sixdof and the hands Monado build, docs/HANDS.md)
//! preset = "cinema"
//! [restart]
//! enabled = false          # false = observe only (the default); the switches below apply once it is true
//! leaves = true            # voice, perf, ctrltrack, ai, passthrough, wayvr (P5)
//! core = true              # the Monado subtree, unplug/replug, resume (P7)
//! device_recover = true    # one automatic reset-headset after a camera-only drop (then ask)
//! inhibit_sleep = true     # systemd-inhibit while a session runs
//! leaf = "5/10m"
//! xr_client = "3/10m"
//! monado = "2/15m"
//! device_recover_budget = "1/30m"
//! global = "8/10m"
//! [notify]
//! desktop = true           # notify-send
//! headset = true           # WayVR toast while WayVR runs
//! [auto]
//! dry_run = true           # the auto.toml keys
//! ```
use omarchyvr_components::model::{Desired, Eye, Mode};
use omarchyvr_components::policy::{parse_budget, Budgets};
use omarchyvr_components::state::Switches;
use std::path::{Path, PathBuf};

/// Everything omarchyvrd.toml says.
#[derive(Debug, Clone, PartialEq)]
pub struct DaemonConfig {
    pub desired: Desired,
    pub budgets: Budgets,
    pub switches: Switches,
    pub notify_desktop: bool,
    pub notify_headset: bool,
    /// The `[auto]` section's lines (auto.toml syntax), if the section exists.
    pub auto_text: Option<String>,
}

impl Default for DaemonConfig {
    fn default() -> Self {
        DaemonConfig {
            desired: Desired::default(),
            budgets: Budgets::default(),
            switches: Switches::default(),
            notify_desktop: true,
            notify_headset: true,
            auto_text: None,
        }
    }
}

pub fn path(home: &Path) -> PathBuf {
    std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| home.join(".config")).join("omarchyvr/omarchyvrd.toml")
}

fn strip(s: &str) -> &str {
    let mut q = false;
    for (i, c) in s.char_indices() {
        match c {
            '"' => q = !q,
            '#' if !q => return s[..i].trim(),
            _ => {}
        }
    }
    s.trim()
}

fn unq(s: &str) -> &str {
    s.trim().trim_matches('"')
}

fn b(v: &str) -> Option<bool> {
    match unq(v) {
        "true" | "on" | "yes" | "1" => Some(true),
        "false" | "off" | "no" | "0" => Some(false),
        _ => None,
    }
}

impl DaemonConfig {
    pub fn parse(text: &str) -> (DaemonConfig, Vec<String>) {
        let mut c = DaemonConfig::default();
        let mut w = Vec::new();
        let mut sect = String::new();
        let mut auto: Option<String> = None;
        for (n, raw) in text.lines().enumerate() {
            let l = strip(raw);
            if l.is_empty() {
                continue;
            }
            if l.starts_with('[') && l.ends_with(']') {
                sect = l[1..l.len() - 1].trim().to_string();
                if sect == "auto" {
                    auto.get_or_insert_with(String::new);
                }
                continue;
            }
            if sect == "auto" {
                let a = auto.get_or_insert_with(String::new);
                a.push_str(raw);
                a.push('\n');
                continue;
            }
            let Some((k, v)) = l.split_once('=') else {
                w.push(format!("line {}: not key = value: {l}", n + 1));
                continue;
            };
            let (k, v) = (k.trim(), v.trim());
            if let Err(e) = c.set(&sect, k, v) {
                w.push(format!("line {}: {e}", n + 1));
            }
        }
        c.auto_text = auto;
        (c, w)
    }

    /// Set one key of a section.
    pub fn set(&mut self, sect: &str, k: &str, v: &str) -> Result<(), String> {
        let bad = || format!("[{sect}] {k}: bad value {v}");
        let bool_ = |v: &str| b(v).ok_or_else(bad);
        let d = &mut self.desired;
        match (sect, k) {
            ("session", "mode") => d.mode = Mode::parse(unq(v)).ok_or_else(bad)?,
            ("components", "passthrough") => d.passthrough = bool_(v)?,
            ("components", "sixdof") => d.sixdof = bool_(v)?,
            ("components", "ctrltrack") => d.ctrltrack = bool_(v)?,
            ("components", "eye") => d.eye = Eye::parse(unq(v)).ok_or_else(bad)?,
            ("components", "eye_laser") => d.eye_laser = bool_(v)?,
            ("components", "perf") => d.perf = bool_(v)?,
            ("components", "voice") => d.voice = bool_(v)?,
            ("components", "voicehud") => d.voicehud = bool_(v)?,
            ("components", "ai_server") => d.ai_server = bool_(v)?,
            ("components", "anchor") => d.anchor = bool_(v)?,
            ("components", "floor_follow") => d.floor_follow = bool_(v)?,
            ("components", "hands") => d.hands = bool_(v)?,
            ("components", "preset") => d.preset = unq(v).to_string(),
            ("restart", "enabled") => self.switches.enabled = bool_(v)?,
            ("restart", "leaves") => self.switches.leaves = bool_(v)?,
            ("restart", "core") => self.switches.core = bool_(v)?,
            ("restart", "device_recover") => self.switches.device_recover = bool_(v)?,
            ("restart", "inhibit_sleep") => self.switches.inhibit_sleep = bool_(v)?,
            ("restart", "leaf") => self.budgets.leaf = parse_budget(v).ok_or_else(bad)?,
            ("restart", "xr_client") => self.budgets.xr_client = parse_budget(v).ok_or_else(bad)?,
            ("restart", "monado") | ("restart", "core_budget") => self.budgets.core = parse_budget(v).ok_or_else(bad)?,
            ("restart", "device_recover_budget") => self.budgets.device = parse_budget(v).ok_or_else(bad)?,
            ("restart", "global") => self.budgets.global = parse_budget(v).ok_or_else(bad)?,
            ("notify", "desktop") => self.notify_desktop = bool_(v)?,
            ("notify", "headset") => self.notify_headset = bool_(v)?,
            _ => return Err(format!("unknown key [{sect}] {k}")),
        }
        Ok(())
    }

    /// Load omarchyvrd.toml (missing: defaults).
    pub fn load(home: &Path) -> (DaemonConfig, Vec<String>, PathBuf) {
        let p = path(home);
        let text = std::fs::read_to_string(&p).unwrap_or_default();
        let (c, w) = DaemonConfig::parse(&text);
        (c, w, p)
    }

    /// The auto-mode settings: omarchyvrd.toml `[auto]` if present, else auto.toml (or `explicit`, `--config`).
    pub fn auto_config(&self, home: &Path, explicit: Option<&Path>) -> (crate::config::Config, Vec<String>, String) {
        if let Some(p) = explicit {
            let (c, w) = crate::config::Config::parse(&std::fs::read_to_string(p).unwrap_or_default(), home);
            return (c, w, p.display().to_string());
        }
        match &self.auto_text {
            Some(t) => {
                let (c, w) = crate::config::Config::parse(t, home);
                (c, w, format!("{} [auto]", path(home).display()))
            }
            None => {
                let p = crate::config::default_path(home);
                let (c, w) = crate::config::Config::parse(&std::fs::read_to_string(&p).unwrap_or_default(), home);
                (c, w, p.display().to_string())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_documented_example_parses_cleanly() {
        let doc: String = include_str!("dconfig.rs")
            .lines()
            .skip_while(|l| !l.starts_with("//! ```toml"))
            .skip(1)
            .take_while(|l| !l.starts_with("//! ```"))
            .map(|l| l.trim_start_matches("//!").trim_start().to_string() + "\n")
            .collect();
        let (c, w) = DaemonConfig::parse(&doc);
        assert!(w.is_empty(), "{w:?}");
        let d = DaemonConfig::default();
        assert_eq!(c.desired, d.desired);
        assert_eq!(c.budgets, d.budgets);
        assert_eq!(c.switches, d.switches);
        assert!(!c.switches.enabled, "observe only by default");
        assert_eq!(c.auto_text.as_deref().map(str::trim), Some("dry_run = true           # the auto.toml keys"));
        let (a, aw, from) = c.auto_config(Path::new("/h"), None);
        assert!(a.dry_run && aw.is_empty() && from.ends_with("[auto]"));
    }

    #[test]
    fn values_and_warnings() {
        let (c, w) = DaemonConfig::parse("[restart]\nenabled = true\nleaf = \"2/5m\"\nbogus = 1\n[components]\neye = \"pointer\" # x\nvoice = maybe\n[session]\nmode = vr\n");
        assert!(c.switches.enabled);
        assert_eq!(c.budgets.leaf, (2, 300.0));
        assert_eq!(c.desired.eye, Eye::Pointer);
        assert_eq!(c.desired.mode, Mode::Vr);
        assert_eq!(w.len(), 2, "{w:?}");
        assert!(c.auto_text.is_none());
    }
}
