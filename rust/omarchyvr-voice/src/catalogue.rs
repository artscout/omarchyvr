//! The voice command catalogue: `voice-intents.toml` (built in) merged with the user's
//! `~/.config/omarchyvr/voice-intents.toml`. See the comments at the top of the TOML file for the format.
//!
//! Safety gate (applies to both files): intents whose `source` is a research row the user did not approve
//! (2026-10-15) are dropped, and so is any intent whose command is one of those rows' programs or a `hyprctl` call
//! that reloads, closes, switches workspaces or touches displays. Only `enabled` intents are matched.

use serde::Deserialize;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The built-in catalogue text.
pub const BUILTIN: &str = include_str!("../voice-intents.toml");

/// Rows of analysis/research/omarchy-voice-commands.html that are not approved (careful rows except lock, never rows).
pub const BLOCKED_ROWS: &[u32] = &[12, 13, 14, 15, 23, 29, 30, 38, 39, 40, 41, 43, 44, 45, 46, 47, 48];

/// Programs of the blocked rows (and the umbrella `omarchy` CLI, which can reach all of them).
pub const BLOCKED_PROGRAMS: &[&str] = &[
    "omarchy",
    "omarchy-hyprland-workspace-layout-toggle",
    "omarchy-agent",
    "omarchy-agent-prompt",
    "omarchy-audio-input-mute",
    "omarchy-audio-output-switch",
    "omarchy-menu",
    "omarchy-theme-set",
    "omarchy-system-shutdown",
    "omarchy-system-reboot",
    "omarchy-toggle-suspend",
    "omarchy-system-logout",
    "omarchy-hyprland-window-close-all",
    "omarchy-hyprland-monitor-internal",
    "omarchy-hyprland-monitor-scaling",
    "omarchy-brightness-display",
    "omarchy-update",
    "omarchy-reinstall",
];

/// `hyprctl` arguments that are never allowed (rows 13-15, 46, 47).
const BLOCKED_HYPRCTL: &[&str] = &["reload", "dpms", "window.close", "workspace = \"e", "focus({ workspace", "keyword", "monitor"];

/// Slot types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotType {
    /// 1..=999.
    Number,
    /// One plain word (letters, digits, `-`, `_`).
    Word,
    /// Several plain words (letters, digits, spaces, basic punctuation), at most 120 characters.
    Text,
    /// A backdrop name (one or more words).
    Name,
    /// A VR screen side: left, right, up, main (middle, center, top, above, down, ...; `windows::slot_word`).
    Side,
    /// A VR screen by number (1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right; docs/SCREENS.md), by side, or
    /// `here` (the screen looked at / focused): `windows::screen_value`.
    Screen,
}

/// One intent.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Intent {
    pub id: String,
    #[serde(default)]
    pub phrases: Vec<String>,
    #[serde(default)]
    pub run: Vec<String>,
    #[serde(default)]
    pub slots: BTreeMap<String, SlotType>,
    #[serde(default = "safe")]
    pub safety: String,
    #[serde(default)]
    pub source: String,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default)]
    pub confirm: bool,
    /// Always asks for a spoken "yes", whatever `voice_confirm` says (stopping the session).
    #[serde(default)]
    pub ask_always: bool,
    #[serde(default)]
    pub say: String,
    #[serde(default)]
    pub output: bool,
    /// A few words for the voice AI's intent list (default: the first phrases).
    #[serde(default)]
    pub gloss: String,
}

fn safe() -> String {
    "safe".into()
}
fn yes() -> bool {
    true
}

/// `[wake]`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Wake {
    pub words: Vec<String>,
    #[serde(default = "followup")]
    pub followup_s: f64,
    #[serde(default = "confirm_s")]
    pub confirm_s: f64,
}
fn followup() -> f64 {
    6.0
}
fn confirm_s() -> f64 {
    8.0
}

/// `[paths]`.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct Paths {
    #[serde(default)]
    pub pimax: String,
    #[serde(default)]
    pub wayvrctl: String,
}

/// `[backdrop]`.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct Backdrop {
    #[serde(default)]
    pub wayvr_home: String,
    #[serde(default)]
    pub skybox_dir: String,
}

/// `[llm]`.
#[derive(Debug, Clone, PartialEq, Deserialize, Default)]
pub struct Llm {
    #[serde(default)]
    pub backend: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_key_file: String,
    #[serde(default)]
    pub api_key_env: String,
    #[serde(default)]
    pub timeout_s: f64,
}

#[derive(Debug, Deserialize, Default)]
struct File {
    wake: Option<Wake>,
    paths: Option<Paths>,
    backdrop: Option<Backdrop>,
    llm: Option<Llm>,
    #[serde(default)]
    intent: Vec<Intent>,
}

/// The merged catalogue.
#[derive(Debug, Clone)]
pub struct Catalogue {
    pub wake: Wake,
    pub paths: Paths,
    pub backdrop: Backdrop,
    pub llm: Llm,
    /// All intents that passed the safety gate (enabled or not), in file order.
    pub intents: Vec<Intent>,
    /// What the loader dropped or fixed, for `omarchyvr-voice intents` and the log.
    pub notes: Vec<String>,
}

/// `~/.config/omarchyvr/voice-intents.toml`.
pub fn user_path() -> PathBuf {
    expand("~/.config/omarchyvr/voice-intents.toml")
}

/// `~/...` -> `$HOME/...`.
pub fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(rest),
        None => PathBuf::from(p),
    }
}

/// Why an intent may not exist, if so.
pub fn blocked(i: &Intent) -> Option<String> {
    if let Some(n) = i.source.strip_prefix("omarchy#").and_then(|n| n.parse::<u32>().ok()) {
        if BLOCKED_ROWS.contains(&n) {
            return Some(format!("{}: row {n} is not approved", i.id));
        }
    }
    let Some(prog) = i.run.first() else {
        return Some(format!("{}: no `run`", i.id));
    };
    let base = Path::new(prog).file_name().and_then(|s| s.to_str()).unwrap_or(prog);
    if BLOCKED_PROGRAMS.contains(&base) {
        return Some(format!("{}: {base} is not approved", i.id));
    }
    if base == "hyprctl" {
        let rest = i.run[1..].join(" ");
        if let Some(b) = BLOCKED_HYPRCTL.iter().find(|b| rest.contains(*b)) {
            return Some(format!("{}: hyprctl {b:?} is not approved", i.id));
        }
    }
    if prog.starts_with('@') && !matches!(prog.as_str(), "@preset" | "@backdrop" | "@passthrough" | "@captions" | "@help" | "@undo" | "@cancel" | "@say" | "@win" | "@debug" | "@desk") {
        return Some(format!("{}: unknown built-in {prog}", i.id));
    }
    if prog == "@win" {
        let action = i.run.get(1).map(String::as_str).unwrap_or("");
        if !crate::windows::ACTIONS.contains(&action) {
            return Some(format!("{}: unknown window action {action:?}", i.id));
        }
        if crate::windows::CAREFUL_ACTIONS.contains(&action) && !i.confirm {
            return Some(format!("{}: @win {action} closes something and needs `confirm = true`", i.id));
        }
    }
    for arg in &i.run {
        for slot in slot_names(arg) {
            if !i.slots.contains_key(&slot) && !matches!(slot.as_str(), "wayvrctl" | "pimax") {
                return Some(format!("{}: {{{slot}}} in run has no slot type", i.id));
            }
        }
    }
    None
}

/// `{name}` placeholders in a string.
pub fn slot_names(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(a) = rest.find('{') {
        let Some(b) = rest[a..].find('}') else { break };
        let name = &rest[a + 1..a + b];
        if !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            out.push(name.to_string());
            rest = &rest[a + b + 1..];
        } else {
            rest = &rest[a + 1..]; // `{ x = "{n}" }`: look again from the next character
        }
    }
    out
}

impl Catalogue {
    /// The built-in catalogue plus the user's file if it exists.
    /// Every literal word of the enabled intents' phrases (normalised), fillers out.
    pub fn vocabulary(&self) -> std::collections::BTreeSet<String> {
        self.enabled().flat_map(|i| i.phrases.iter()).flat_map(|p| crate::command::words(&p.replace(['{', '}'], " "))).filter(|w| !crate::command::FILLERS.contains(&w.as_str())).collect()
    }

    pub fn load_default() -> Result<Catalogue, String> {
        let user = user_path();
        let text = match std::fs::read_to_string(&user) {
            Ok(t) => Some(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => return Err(format!("{}: {e}", user.display())),
        };
        Catalogue::from_texts(BUILTIN, text.as_deref())
    }

    /// Parse and merge (`over` = the user's file).
    pub fn from_texts(base: &str, over: Option<&str>) -> Result<Catalogue, String> {
        let b: File = toml::from_str(base).map_err(|e| format!("built-in catalogue: {e}"))?;
        let mut c = Catalogue {
            wake: b.wake.ok_or("built-in catalogue: no [wake]")?,
            paths: b.paths.unwrap_or_default(),
            backdrop: b.backdrop.unwrap_or_default(),
            llm: b.llm.unwrap_or_default(),
            intents: Vec::new(),
            notes: Vec::new(),
        };
        let mut all = b.intent;
        if let Some(o) = over {
            let u: File = toml::from_str(o).map_err(|e| format!("{}: {e}", user_path().display()))?;
            if let Some(w) = u.wake {
                c.wake = w;
            }
            if let Some(p) = u.paths {
                if !p.pimax.is_empty() {
                    c.paths.pimax = p.pimax;
                }
                if !p.wayvrctl.is_empty() {
                    c.paths.wayvrctl = p.wayvrctl;
                }
            }
            if let Some(bd) = u.backdrop {
                if !bd.wayvr_home.is_empty() {
                    c.backdrop.wayvr_home = bd.wayvr_home;
                }
                if !bd.skybox_dir.is_empty() {
                    c.backdrop.skybox_dir = bd.skybox_dir;
                }
            }
            if let Some(l) = u.llm {
                c.llm = l;
            }
            for ui in u.intent {
                match all.iter_mut().find(|i| i.id == ui.id) {
                    // an existing id: enabled follows the user, phrases are added, run/slots only if given
                    Some(i) => {
                        i.enabled = ui.enabled;
                        i.phrases.extend(ui.phrases);
                        if !ui.run.is_empty() {
                            i.run = ui.run;
                            i.slots = ui.slots;
                        }
                        if !ui.say.is_empty() {
                            i.say = ui.say;
                        }
                        if !ui.gloss.is_empty() {
                            i.gloss = ui.gloss;
                        }
                    }
                    None => all.push(ui),
                }
            }
        }
        let mut seen = std::collections::BTreeSet::new();
        for i in all {
            if !seen.insert(i.id.clone()) {
                c.notes.push(format!("{}: duplicate id, later one dropped", i.id));
                continue;
            }
            if let Some(why) = blocked(&i) {
                c.notes.push(format!("dropped {why}"));
                continue;
            }
            if i.phrases.is_empty() {
                c.notes.push(format!("{}: no phrases", i.id));
            }
            c.intents.push(i);
        }
        if c.paths.pimax.is_empty() {
            c.paths.pimax = "omarchyvr".into();
        }
        if c.paths.wayvrctl.is_empty() {
            c.paths.wayvrctl = "~/.local/lib/omarchyvr/wayvr/wayvrctl".into();
        }
        Ok(c)
    }

    /// The enabled intents.
    pub fn enabled(&self) -> impl Iterator<Item = &Intent> {
        self.intents.iter().filter(|i| i.enabled)
    }

    pub fn get(&self, id: &str) -> Option<&Intent> {
        self.intents.iter().find(|i| i.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_parses_and_every_intent_passes_the_gate() {
        let c = Catalogue::from_texts(BUILTIN, None).unwrap();
        assert!(c.notes.is_empty(), "{:?}", c.notes);
        assert!(c.wake.words.iter().any(|w| w == "pimax"));
        assert!(c.wake.words.iter().any(|w| w == "max"));
        // every approved Omarchy row is present: the 30 safe rows + lock (42)
        let rows: std::collections::BTreeSet<u32> = c
            .intents
            .iter()
            .filter_map(|i| i.source.strip_prefix("omarchy#")?.parse().ok())
            .collect();
        let safe = [1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 16, 17, 18, 19, 20, 21, 22, 24, 25, 26, 27, 28, 31, 32, 33, 34, 35, 36, 37];
        assert_eq!(safe.len(), 30);
        let mut want: std::collections::BTreeSet<u32> = safe.into_iter().collect();
        want.insert(42);
        assert_eq!(rows, want);
        assert!(c.get("omarchy.lock").unwrap().confirm);
        assert_eq!(c.enabled().count(), c.intents.len());
        assert_eq!(c.intents.len(), 143); // + win.screen.place/here (2026-10-17) // + vr.input.show_controllers/hide_controllers (PIMAX_CTRL_INHAND) // + win.app.open/here // + win.close.named (no app closes unless named) // + vr.screens.hide/hide_others, win.screen.scale, vr.session.stop (live 2026-10-16 b) // + vr.screens.numbers/numbers_hide/numbers_always (WayVR badges) // + screen numbers: win.screen.put_from/move_all/move_n/goto_n/swap/adjust/list_n/open_n, win.layout.save(_as)/restore(_as), omarchy.volume.set // + win.screen.put/put_n, vr.preset.exit_cinema, media.play (agent mode) // + vr.screens.count/more/one_more/add, win.screen.open/gather (2026-10-16) // + vr.input.controllers/hands (prefer-hands) // + win.screen.goto/move/nudge/closer/farther/bigger/smaller, win.organize/together (VR screens) // + voice.debug.on/off, typing, keys, browser, + win.ws.new/ws.close/focus.gaze/move.here/merge.here
        // window control: user request 2026-10-15; the destructive ones ask for a spoken yes
        for id in ["win.close", "win.ws.close", "win.tab.close"] {
            assert!(c.get(id).unwrap().confirm && c.get(id).unwrap().safety == "careful", "{id}");
        }
        assert!(c.intents.iter().filter(|i| i.id.starts_with("win.")).all(|i| i.source.starts_with("user request 2026-10-1")));
    }

    #[test]
    fn override_merges_and_cannot_enable_blocked_rows() {
        let over = r#"
[[intent]]
id = "omarchy.screenshot"
enabled = false

[[intent]]
id = "vr.preset.cinema"
phrases = ["popcorn time"]

[[intent]]
id = "my.shutdown"
phrases = ["shut down"]
run = ["omarchy-system-shutdown"]

[[intent]]
id = "my.row40"
phrases = ["power off"]
run = ["systemctl", "poweroff"]
source = "omarchy#40"

[[intent]]
id = "my.reload"
phrases = ["reload"]
run = ["hyprctl", "reload"]

[[intent]]
id = "my.hello"
phrases = ["hello"]
run = ["notify-send", "hi"]

[llm]
backend = "off"
"#;
        let c = Catalogue::from_texts(BUILTIN, Some(over)).unwrap();
        assert!(!c.get("omarchy.screenshot").unwrap().enabled);
        assert!(c.get("vr.preset.cinema").unwrap().phrases.iter().any(|p| p == "popcorn time"));
        assert!(c.get("my.shutdown").is_none() && c.get("my.row40").is_none() && c.get("my.reload").is_none());
        assert_eq!(c.notes.len(), 3, "{:?}", c.notes);
        assert!(c.get("my.hello").is_some());
        assert_eq!(c.llm.backend, "off");
        // a placeholder without a slot type is refused
        let bad = "[[intent]]\nid = \"x\"\nphrases = [\"x\"]\nrun = [\"echo\", \"{what}\"]\n";
        let c = Catalogue::from_texts(BUILTIN, Some(bad)).unwrap();
        assert!(c.get("x").is_none());
        assert!(Catalogue::from_texts(BUILTIN, Some("[[intent]]\nid = 3\n")).is_err());
    }

    #[test]
    fn placeholders() {
        assert_eq!(slot_names(r#"hl.dsp.window.move({ workspace = "{n}", follow = false })"#), ["n"]);
        assert_eq!(slot_names("{wayvrctl}"), ["wayvrctl"]);
        assert!(slot_names(r#"hl.dsp.focus({ direction = "l" })"#).is_empty());
    }
}
