//! Omarchy's colours for the app (docs/GUI.md "Colours"). Omarchy keeps the current theme at
//! `~/.local/state/omarchy/current/theme/colors.toml` (its name in `theme.name` next to it) and every installed theme
//! under `~/.config/omarchy/themes/<name>/` and `/usr/share/omarchy/themes/<name>/`, each with a `colors.toml`
//! (`key = "#rrggbb"`, plus `mode = "dark" | "light"`). This module only reads and checks those files; the mapping onto
//! the app's own roles (background, cards, text, accent, status colours) is `omarchyvr-app/src/theme.rs`.
//! Nothing here can fail hard: a missing or broken file resolves to "no colours" and a plain-words note.
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// An sRGB colour.
pub type Rgb = [u8; 3];

/// The colours of one Omarchy theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OmarchyColors {
    /// `mode = "dark"`; taken from the background when the key is missing.
    pub dark: bool,
    /// Every valid colour by key (`accent`, `background`, `bright_red`, ...).
    pub colors: BTreeMap<String, Rgb>,
    /// Keys whose value was not a colour (skipped).
    pub bad: Vec<String>,
}

impl OmarchyColors {
    /// A colour by key.
    pub fn get(&self, key: &str) -> Option<Rgb> {
        self.colors.get(key).copied()
    }

    /// Parse the text of a `colors.toml`. `Err` when no colour at all can be used (not even a background or a
    /// foreground); otherwise bad values are skipped and listed in `bad`.
    pub fn parse(text: &str) -> Result<OmarchyColors, String> {
        let mut colors = BTreeMap::new();
        let mut bad = Vec::new();
        let mut mode: Option<bool> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') || line.starts_with('[') {
                continue;
            }
            let Some((k, v)) = line.split_once('=') else { continue };
            let (k, v) = (k.trim().trim_matches('"'), unquote(v));
            if k.is_empty() {
                continue;
            }
            if k == "mode" {
                mode = match v.to_ascii_lowercase().as_str() {
                    "dark" => Some(true),
                    "light" => Some(false),
                    _ => None,
                };
                continue;
            }
            match parse_hex(&v) {
                Some(c) => {
                    colors.insert(k.to_string(), c);
                }
                None => bad.push(k.to_string()),
            }
        }
        if !colors.contains_key("background") && !colors.contains_key("foreground") {
            return Err(if colors.is_empty() { "the file has no colours".into() } else { "the file has no background or foreground colour".into() });
        }
        let dark = mode.unwrap_or_else(|| colors.get("background").map(|c| luminance(*c) < 0.4).unwrap_or(true));
        Ok(OmarchyColors { dark, colors, bad })
    }

    /// Read and parse a file.
    pub fn load(path: &Path) -> Result<OmarchyColors, String> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("cannot read it: {}", e.kind()))?;
        OmarchyColors::parse(&text)
    }
}

/// The value of `key = value`: inside quotes, or up to a space or a comment.
fn unquote(v: &str) -> String {
    let v = v.trim();
    if let Some(rest) = v.strip_prefix('"').or_else(|| v.strip_prefix('\'')) {
        return rest.split(['"', '\'']).next().unwrap_or("").to_string();
    }
    v.split_whitespace().next().unwrap_or("").to_string()
}

/// `#rrggbb`, `rrggbb`, `0xrrggbb` or `#rgb`.
pub fn parse_hex(s: &str) -> Option<Rgb> {
    let s = s.trim();
    let s = s.strip_prefix('#').or_else(|| s.strip_prefix("0x")).or_else(|| s.strip_prefix("0X")).unwrap_or(s);
    if !s.is_ascii() || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let h = |i: usize, n: usize| u8::from_str_radix(&s[i..i + n], 16).ok();
    match s.len() {
        6 => Some([h(0, 2)?, h(2, 2)?, h(4, 2)?]),
        3 => Some([h(0, 1)? * 17, h(1, 1)? * 17, h(2, 1)? * 17]),
        _ => None,
    }
}

/// WCAG relative luminance, 0 (black) to 1 (white).
pub fn luminance(c: Rgb) -> f32 {
    let f = |v: u8| {
        let v = f32::from(v) / 255.0;
        if v <= 0.03928 {
            v / 12.92
        } else {
            ((v + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2])
}

/// WCAG contrast ratio, 1 to 21.
pub fn contrast(a: Rgb, b: Rgb) -> f32 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

/// What the user chose for the app's colours.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Colours {
    /// Follow the Omarchy theme (the default; the omarchyvr colours when Omarchy has no colour file).
    #[default]
    Follow,
    /// The omarchyvr colours.
    Default,
    /// One installed Omarchy theme, by name.
    Theme(ThemeName),
}

impl Colours {
    /// The settings-file value (`follow`, `default`, `theme:<name>`).
    pub fn render(&self) -> String {
        match self {
            Colours::Follow => "follow".into(),
            Colours::Default => "default".into(),
            Colours::Theme(n) => format!("theme:{}", n.as_str()),
        }
    }

    /// Parse the settings-file value; anything unknown is `Follow`.
    pub fn parse(v: &str) -> Colours {
        let v = v.trim();
        match v {
            "default" => Colours::Default,
            _ => match v.strip_prefix("theme:").and_then(ThemeName::new) {
                Some(n) => Colours::Theme(n),
                None => Colours::Follow,
            },
        }
    }
}

/// A theme's folder name (letters, digits, `-`, `_`, `.`; at most 40 bytes). Fixed size so the settings stay `Copy`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ThemeName {
    len: u8,
    buf: [u8; 40],
}

impl ThemeName {
    /// A name; `None` when empty, too long, or with characters that could leave the themes folder.
    pub fn new(s: &str) -> Option<ThemeName> {
        let s = s.trim();
        let ok = !s.is_empty() && s.len() <= 40 && !s.starts_with('.') && s.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'));
        if !ok {
            return None;
        }
        let mut buf = [0u8; 40];
        buf[..s.len()].copy_from_slice(s.as_bytes());
        Some(ThemeName { len: s.len() as u8, buf })
    }

    /// The name.
    pub fn as_str(&self) -> &str {
        std::str::from_utf8(&self.buf[..self.len as usize]).unwrap_or("")
    }
}

/// Where Omarchy keeps its themes.
#[derive(Debug, Clone)]
pub struct OmarchyPaths {
    /// `~/.local/state/omarchy`.
    pub state: PathBuf,
    /// `~/.config/omarchy/themes` (the user's own themes win over the system ones of the same name).
    pub user_themes: PathBuf,
    /// `/usr/share/omarchy/themes`.
    pub system_themes: PathBuf,
}

/// An installed theme.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThemeEntry {
    /// Folder name.
    pub name: String,
    /// Its colours.
    pub colors: OmarchyColors,
}

/// What a choice resolves to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resolved {
    /// The colours to use; `None` = the omarchyvr colours.
    pub colors: Option<OmarchyColors>,
    /// The Omarchy theme name in use, when known.
    pub name: Option<String>,
    /// One plain sentence for the setting.
    pub note: String,
}

/// Cheap change marker of the files a choice depends on (their mtimes).
pub type Stamp = (Option<SystemTime>, Option<SystemTime>);

impl OmarchyPaths {
    /// The real places under a home directory.
    pub fn from_home(home: &Path) -> OmarchyPaths {
        OmarchyPaths { state: home.join(".local/state/omarchy"), user_themes: home.join(".config/omarchy/themes"), system_themes: PathBuf::from("/usr/share/omarchy/themes") }
    }

    /// The colour file of the current theme.
    pub fn current_colors(&self) -> PathBuf {
        self.state.join("current/theme/colors.toml")
    }

    fn current_name_file(&self) -> PathBuf {
        self.state.join("current/theme.name")
    }

    /// The current theme's name (`theme.name`).
    pub fn current_name(&self) -> Option<String> {
        let t = std::fs::read_to_string(self.current_name_file()).ok()?;
        let t = t.trim();
        (!t.is_empty()).then(|| t.to_string())
    }

    /// Does Omarchy keep a colour file for the current theme?
    pub fn omarchy_present(&self) -> bool {
        self.current_colors().is_file()
    }

    /// The mtimes of the current colour file and of `theme.name`: when either changes, the theme was switched.
    pub fn stamp(&self) -> Stamp {
        let m = |p: PathBuf| std::fs::metadata(p).and_then(|m| m.modified()).ok();
        (m(self.current_colors()), m(self.current_name_file()))
    }

    /// Every installed theme that has a readable `colors.toml`, by name (a user theme replaces the system one).
    pub fn themes(&self) -> Vec<ThemeEntry> {
        let mut by_name: BTreeMap<String, ThemeEntry> = BTreeMap::new();
        for dir in [&self.system_themes, &self.user_themes] {
            let Ok(rd) = std::fs::read_dir(dir) else { continue };
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().to_string();
                if ThemeName::new(&name).is_none() {
                    continue;
                }
                if let Ok(colors) = OmarchyColors::load(&e.path().join("colors.toml")) {
                    by_name.insert(name.clone(), ThemeEntry { name, colors });
                }
            }
        }
        by_name.into_values().collect()
    }

    /// Resolve the user's choice to colours.
    pub fn resolve(&self, choice: Colours) -> Resolved {
        let default = |note: String| Resolved { colors: None, name: None, note };
        match choice {
            Colours::Default => default("The omarchyvr colours.".into()),
            Colours::Follow => {
                let name = self.current_name();
                match OmarchyColors::load(&self.current_colors()) {
                    Ok(c) => {
                        let shown = name.as_deref().map(|n| format!(" \"{n}\"")).unwrap_or_default();
                        let skipped = if c.bad.is_empty() { String::new() } else { format!(" {} colours in the file were not valid; those use the omarchyvr colours.", c.bad.len()) };
                        Resolved { name, note: format!("Following the Omarchy theme{shown}; it changes here when you change it in Omarchy.{skipped}"), colors: Some(c) }
                    }
                    Err(_) if !self.current_colors().exists() => default("Omarchy's colour file was not found, so the omarchyvr colours are used.".into()),
                    Err(e) => default(format!("Omarchy's colour file could not be used ({e}), so the omarchyvr colours are used.")),
                }
            }
            Colours::Theme(n) => match self.themes().into_iter().find(|t| t.name == n.as_str()) {
                Some(t) => {
                    let skipped = if t.colors.bad.is_empty() { String::new() } else { format!(" {} colours in its file were not valid; those use the omarchyvr colours.", t.colors.bad.len()) };
                    Resolved { note: format!("The Omarchy theme \"{}\".{skipped}", t.name), name: Some(t.name), colors: Some(t.colors) }
                }
                None => default(format!("The theme \"{}\" is not installed (or its colour file is unreadable), so the omarchyvr colours are used.", n.as_str())),
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOKYO: &str = include_str!("../../fixtures/omarchy-themes/tokyo-night/colors.toml");
    const LATTE: &str = include_str!("../../fixtures/omarchy-themes/catppuccin-latte/colors.toml");
    const FLEXOKI: &str = include_str!("../../fixtures/omarchy-themes/flexoki-light/colors.toml");

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("omarchyvr-colors-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    fn put(dir: &Path, rel: &str, text: &str) {
        let p = dir.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, text).unwrap();
    }

    #[test]
    fn parses_every_key_of_a_real_theme() {
        let c = OmarchyColors::parse(TOKYO).unwrap();
        assert!(c.dark && c.bad.is_empty());
        assert_eq!(c.colors.len(), 25);
        assert_eq!(c.get("accent"), Some([0x7a, 0xa2, 0xf7]));
        assert_eq!(c.get("background"), Some([0x1a, 0x1b, 0x26]));
        assert_eq!(c.get("bright_magenta"), Some([0xbb, 0x9a, 0xf7]));
        assert_eq!(c.get("brown"), Some([0x75, 0x49, 0x3d]));
        let l = OmarchyColors::parse(LATTE).unwrap();
        assert!(!l.dark);
        assert_eq!(OmarchyColors::parse(FLEXOKI).unwrap().get("accent"), Some([0x20, 0x5e, 0xa6]), "upper-case hex");
    }

    #[test]
    fn missing_keys_bad_hex_and_junk() {
        let c = OmarchyColors::parse("background = \"#ffffff\"\naccent = \"#12345\"\nred = \"nothex\"\ngreen = \"0x00ff00\"\ncyan = \"#0f8\" # c\nmode = \"light\"\n").unwrap();
        assert_eq!(c.bad, vec!["accent".to_string(), "red".to_string()]);
        assert_eq!(c.get("green"), Some([0, 255, 0]));
        assert_eq!(c.get("cyan"), Some([0, 255, 136]));
        assert_eq!(c.get("foreground"), None, "missing stays missing");
        assert!(!c.dark);
        // no mode: inferred from the background
        assert!(!OmarchyColors::parse("background = \"#fafafa\"").unwrap().dark);
        assert!(OmarchyColors::parse("background = \"#101010\"").unwrap().dark);
        assert!(OmarchyColors::parse("").is_err());
        assert!(OmarchyColors::parse("red = \"#ff0000\"").is_err());
        assert!(OmarchyColors::parse("\u{0}\u{1}= = =\n[x]\n\"\"").is_err());
    }

    #[test]
    fn contrast_math() {
        assert!((contrast([0, 0, 0], [255, 255, 255]) - 21.0).abs() < 0.01);
        assert!((contrast([10, 10, 10], [10, 10, 10]) - 1.0).abs() < 0.001);
        assert!(contrast([0xa9, 0xb1, 0xd6], [0x1a, 0x1b, 0x26]) > 7.0);
    }

    #[test]
    fn colours_setting_round_trip() {
        for c in [Colours::Follow, Colours::Default, Colours::Theme(ThemeName::new("tokyo-night").unwrap())] {
            assert_eq!(Colours::parse(&c.render()), c);
        }
        assert_eq!(Colours::parse(""), Colours::Follow);
        assert_eq!(Colours::parse("theme:../../etc"), Colours::Follow, "no path escapes");
        assert_eq!(Colours::parse("theme:"), Colours::Follow);
        assert!(ThemeName::new(&"x".repeat(41)).is_none());
    }

    #[test]
    fn resolve_follow_theme_missing_and_broken() {
        let d = tmp("resolve");
        let paths = OmarchyPaths { state: d.join("state"), user_themes: d.join("user"), system_themes: d.join("sys") };
        // nothing there
        let r = paths.resolve(Colours::Follow);
        assert!(r.colors.is_none() && r.note.contains("not found"), "{}", r.note);
        assert!(!paths.omarchy_present());
        // a current theme
        put(&d, "state/current/theme/colors.toml", TOKYO);
        put(&d, "state/current/theme.name", "tokyo-night\n");
        let r = paths.resolve(Colours::Follow);
        assert_eq!(r.name.as_deref(), Some("tokyo-night"));
        assert!(r.note.contains("tokyo-night"));
        assert!(r.colors.unwrap().dark);
        let s1 = paths.stamp();
        assert!(s1.0.is_some() && s1.1.is_some());
        // installed themes: system + user, the user one wins, a folder without colours is skipped
        put(&d, "sys/tokyo-night/colors.toml", TOKYO);
        put(&d, "sys/catppuccin-latte/colors.toml", LATTE);
        put(&d, "sys/nocolors/other.txt", "x");
        put(&d, "user/tokyo-night/colors.toml", FLEXOKI);
        put(&d, "user/broken/colors.toml", "garbage");
        let names: Vec<_> = paths.themes().into_iter().map(|t| (t.name, t.colors.dark)).collect();
        assert_eq!(names, vec![("catppuccin-latte".to_string(), false), ("tokyo-night".to_string(), false)]);
        let r = paths.resolve(Colours::Theme(ThemeName::new("catppuccin-latte").unwrap()));
        assert!(r.colors.is_some_and(|c| !c.dark));
        let r = paths.resolve(Colours::Theme(ThemeName::new("gone").unwrap()));
        assert!(r.colors.is_none() && r.note.contains("not installed"));
        // a broken current file
        put(&d, "state/current/theme/colors.toml", "not = toml at all");
        let r = paths.resolve(Colours::Follow);
        assert!(r.colors.is_none() && r.note.contains("could not be used"), "{}", r.note);
        assert!(paths.resolve(Colours::Default).colors.is_none());
        let _ = std::fs::remove_dir_all(d);
    }
}
