//! WayVR in desk mode: where its files are, the settings the app owns (`~/.config/omarchyvr/wayvr-desk.conf`, applied by
//! `omarchyvr desk start` as `conf.d/50-pimax-app.yaml`), a check of the conf.d files for what keeps WayVR from starting,
//! the saved screen layout (`zz-saved-state.json5`), and the live commands over `wayvrctl`.
//!
//! The app never writes into WayVR's conf.d itself and never touches `~/.config/wayvr`. Key names, types and the
//! failure modes follow WayVR's `config.rs` (branch eye-gaze) as read for this page.
use crate::machine::Machine;
use crate::paths::Paths;
pub use omarchyvr_tools::vr::wayvr_conf::{ScreenNumbers, WayvrSettings, CONF_NAME};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where WayVR's desk-mode files are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WayvrPaths {
    /// The isolated home of the desk WayVR (`~/.local/share/omarchyvr/wayvr-home`).
    pub home: PathBuf,
    /// `<home>/config/wayvr/conf.d`.
    pub conf_d: PathBuf,
    /// Saved screen layout (`zz-saved-state.json5`).
    pub saved_state: PathBuf,
    /// Settings the in-VR dashboard saved (`zz-saved-config.json5`).
    pub saved_config: PathBuf,
    /// Backups of the layout, outside conf.d.
    pub backups: PathBuf,
    /// `wayvrctl`.
    pub ctl: PathBuf,
    /// The app's settings file.
    pub settings: PathBuf,
    /// docs/WAYVR-CONTROLS.md.
    pub guide: PathBuf,
}

impl WayvrPaths {
    /// From the app's paths (the launcher's defaults).
    pub fn new(p: &Paths) -> WayvrPaths {
        let home = omarchyvr_config::paths::wayvr_home(&p.home);
        let conf_d = home.join("config/wayvr/conf.d");
        WayvrPaths {
            saved_state: conf_d.join("zz-saved-state.json5"),
            saved_config: conf_d.join("zz-saved-config.json5"),
            backups: omarchyvr_config::paths::data_dir(&p.home).join("wayvr-state-backups"),
            ctl: p.wayvr_bin.with_file_name("wayvrctl"),
            settings: p.config.join("wayvr-desk.conf"),
            guide: p.doc("WAYVR-CONTROLS.md"),
            conf_d,
            home,
        }
    }
}

/// How bad a finding of the conf.d check is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LintLevel {
    /// Good to know.
    Info,
    /// Changes what the app's settings do.
    Warn,
    /// WayVR does not start.
    Error,
}

/// One finding of the conf.d check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lint {
    /// File name.
    pub file: String,
    /// Level.
    pub level: LintLevel,
    /// What.
    pub text: String,
}

/// Keys WayVR accepts under two names: both in the merged config is a "duplicate field" panic.
const ALIASES: [(&str, &str); 2] = [("ui_animation_speed", "animation_speed"), ("ui_round_multiplier", "round_multiplier")];

/// Check WayVR's conf.d files (name, text) the way WayVR reads them (sorted by name, later wins).
pub fn lint(files: &[(String, String)]) -> Vec<Lint> {
    let mut v = Vec::new();
    let mut all_keys: Vec<(String, String)> = Vec::new();
    let mut add = |file: &str, level, text: String| v.push(Lint { file: file.to_string(), level, text });
    for (name, text) in files {
        let ext = name.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        match ext {
            "yaml" | "yml" => {}
            "json" | "json5" => {
                if text.trim().is_empty() {
                    add(name, LintLevel::Error, "empty JSON file: WayVR does not start".into());
                } else if serde_json::from_str::<serde_json::Value>(text).is_err() {
                    add(name, LintLevel::Error, "not valid JSON (cut off by a crash?): WayVR does not start".into());
                } else if let Ok(serde_json::Value::Object(m)) = serde_json::from_str::<serde_json::Value>(text) {
                    for k in m.keys() {
                        all_keys.push((name.clone(), k.clone()));
                    }
                }
                continue;
            }
            e if matches!(e.to_ascii_lowercase().as_str(), "yaml" | "yml" | "json" | "json5") => {
                add(name, LintLevel::Error, format!("extension .{e} in capitals: WayVR's config reader panics on it"));
                continue;
            }
            "" => continue,
            e => {
                add(name, LintLevel::Info, format!(".{e} files are ignored by WayVR (keep backups outside conf.d)"));
                continue;
            }
        }
        let mut seen: Vec<&str> = Vec::new();
        let mut docs = 0;
        for (i, l) in text.lines().enumerate() {
            if l.trim_start().starts_with('#') || l.trim().is_empty() {
                continue;
            }
            if l.trim_end() == "---" {
                docs += 1;
                if docs > 1 || !seen.is_empty() {
                    add(name, LintLevel::Error, format!("line {}: a second YAML document: WayVR does not start", i + 1));
                }
                continue;
            }
            // top-level keys start at column 0
            if l.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
                let Some((k, rest)) = l.split_once(':') else { continue };
                let k = k.trim();
                if seen.contains(&k) {
                    add(name, LintLevel::Error, format!("line {}: `{k}` twice in this file: WayVR does not start (duplicated key)", i + 1));
                } else {
                    seen.push(k);
                }
                let rest = rest.split('#').next().unwrap_or("").trim();
                let next_indented = text.lines().nth(i + 1).is_some_and(|n| n.starts_with(' ') || n.starts_with('-'));
                if rest.is_empty() && !next_indented {
                    add(name, LintLevel::Error, format!("line {}: `{k}:` has no value (null): WayVR does not start", i + 1));
                }
                all_keys.push((name.clone(), k.to_string()));
            }
        }
    }
    for (a, b) in ALIASES {
        let fa = all_keys.iter().find(|(_, k)| k == a);
        let fb = all_keys.iter().find(|(_, k)| k == b);
        if let (Some((f1, _)), Some((f2, _))) = (fa, fb) {
            let (x, y) = if f1 <= f2 { (f1, f2) } else { (f2, f1) };
            v.push(Lint {
                file: if x == y { x.clone() } else { format!("{x} + {y}") },
                level: LintLevel::Error,
                text: format!("`{a}` and its old name `{b}` both set: WayVR does not start (duplicate field)"),
            });
        }
    }
    if files.iter().any(|(n, _)| n == "zz-saved-config.json5") {
        v.push(Lint {
            file: "zz-saved-config.json5".into(),
            level: LintLevel::Warn,
            text: "the in-VR dashboard saved its settings here; they win over the app's for the keys it holds (enable_keyboard, hide_grab_help, ...)".into(),
        });
    }
    v.sort_by_key(|l| std::cmp::Reverse(l.level));
    v
}

/// Read conf.d (name, text), sorted like WayVR sorts them.
pub fn read_conf_d(m: &dyn Machine, dir: &Path) -> Vec<(String, String)> {
    let mut v: Vec<(String, String)> = m
        .list(dir)
        .into_iter()
        .filter(|n| !n.starts_with('.'))
        .filter_map(|n| {
            let t = m.read(&dir.join(&n)).map(|b| String::from_utf8_lossy(&b).into_owned())?;
            Some((n, t))
        })
        .collect();
    v.sort();
    v
}

/// One overlay of the saved layout.
#[derive(Debug, Clone, PartialEq)]
pub struct Overlay {
    /// Set name (`` or `global`).
    pub set: String,
    /// Overlay name (a screen: its Wayland output name).
    pub name: String,
    /// `Static`, `Floating`, `Anchored`, `FollowHand`, ...
    pub positioning: String,
    /// Can be grabbed.
    pub grabbable: bool,
    /// Position (m, the saved transform's translation).
    pub pos: Option<[f32; 3]>,
    /// Width (m, the length of its x axis).
    pub width: Option<f32>,
}

/// Overlays of WayVR itself and the app's panels (not screens).
pub const BUILTIN: [&str; 9] = ["watch", "anchor", "grab-help", "alttab-help", "kbd", "Dashboard", "toast", "captions", "perf"];

impl Overlay {
    /// A screen (not a built-in overlay or panel)?
    pub fn is_screen(&self) -> bool {
        !BUILTIN.contains(&self.name.as_str())
    }
}

/// Parse `zz-saved-state.json5` (written by serde_json).
pub fn parse_layout(text: &str) -> Result<Vec<Overlay>, String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("cannot read the saved layout: {e}"))?;
    let mut out = Vec::new();
    for set in v.get("sets").and_then(|s| s.as_array()).into_iter().flatten() {
        let set_name = set.get("name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        let Some(ov) = set.get("overlays").and_then(|o| o.as_object()) else { continue };
        for (name, o) in ov {
            let positioning = match o.get("positioning") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Object(m)) => m.keys().next().cloned().unwrap_or_default(),
                _ => String::new(),
            };
            let t: Option<Vec<f32>> = o.get("saved_transform").and_then(|t| t.as_array()).map(|a| a.iter().filter_map(|x| x.as_f64().map(|f| f as f32)).collect());
            let (pos, width) = match t.as_deref() {
                Some(t) if t.len() == 12 => (Some([t[9], t[10], t[11]]), Some((t[0] * t[0] + t[1] * t[1] + t[2] * t[2]).sqrt())),
                _ => (None, None),
            };
            out.push(Overlay {
                set: set_name.clone(),
                name: name.clone(),
                positioning,
                grabbable: o.get("grabbable").and_then(|g| g.as_bool()).unwrap_or(false),
                pos,
                width,
            });
        }
    }
    Ok(out)
}

/// The screens of a layout, in order, without duplicates.
pub fn screens(layout: &[Overlay]) -> Vec<String> {
    let mut v: Vec<String> = Vec::new();
    for o in layout.iter().filter(|o| o.is_screen()) {
        if !v.contains(&o.name) {
            v.push(o.name.clone());
        }
    }
    v
}

/// A command over `wayvrctl`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ctl {
    /// Every active screen back in front of you (and saved).
    ResetScreens,
    /// Show / hide all overlays (toggle).
    ShowHide,
    /// `switch-set N`: 0 hides everything, 1 shows the first set.
    SwitchSet(u32),
    /// Apps inside WayVR.
    ProcessList,
    /// Start an app inside WayVR (floating, 1920x1080).
    Launch {
        /// Name shown in WayVR.
        name: String,
        /// Program (and arguments, one string).
        exec: String,
    },
    /// Stop an app inside WayVR (WayVR sends it SIGTERM).
    Terminate(u64),
    /// Show / hide a panel element (captions `cap_box`, the wrist readout `perf_outer`).
    Panel {
        /// Panel.
        panel: &'static str,
        /// Element.
        element: &'static str,
        /// Visible.
        visible: bool,
    },
    /// The eye-tracking laser (where you look), on or off (`wayvrctl gaze-laser`).
    GazeLaser(bool),
    /// Controllers in Work in the running session (`wayvrctl controllers on|off`).
    Controllers(bool),
    /// Screen number badges mode in the running session (`wayvrctl screen-numbers off|ondemand|always`).
    ScreenNumbers(crate::wayvr::ScreenNumbers),
    /// Show the screen number badges now for a few seconds (`wayvrctl screen-numbers flash`).
    FlashScreenNumbers,
}

impl Ctl {
    /// Arguments of `wayvrctl`.
    pub fn args(&self) -> Vec<String> {
        let s = |x: &str| x.to_string();
        match self {
            Ctl::ResetScreens => vec![s("reset-screens")],
            Ctl::ShowHide => vec![s("show-hide")],
            Ctl::SwitchSet(n) => vec![s("switch-set"), n.to_string()],
            Ctl::ProcessList => vec![s("process-list")],
            Ctl::Launch { name, exec } => {
                let mut parts = exec.split_whitespace();
                let prog = parts.next().unwrap_or("").to_string();
                let rest = parts.collect::<Vec<_>>().join(" ");
                let mut v = vec![s("process-launch"), s("-n"), name.clone(), prog, s("1920x1080"), s("floating")];
                if !rest.is_empty() {
                    // positional: [ICON] comes before the arguments
                    v.push(String::new());
                    v.push(rest);
                }
                v
            }
            Ctl::Terminate(h) => vec![s("process-terminate"), format!("{{\"user\":{h}}}")],
            Ctl::Panel { panel, element, visible } => vec![s("panel-modify"), s(panel), s(element), s("set-visible"), if *visible { s("1") } else { s("0") }],
            Ctl::GazeLaser(on) => vec![s("gaze-laser"), if *on { s("on") } else { s("off") }],
            Ctl::Controllers(on) => vec![s("controllers"), if *on { s("on") } else { s("off") }],
            Ctl::ScreenNumbers(m) => vec![s("screen-numbers"), s(m.key())],
            Ctl::FlashScreenNumbers => vec![s("screen-numbers"), s("flash")],
        }
    }

    /// For people.
    pub fn label(&self) -> String {
        match self {
            Ctl::ResetScreens => "Reset screens".into(),
            Ctl::ShowHide => "Show / hide all".into(),
            Ctl::SwitchSet(0) => "Hide all".into(),
            Ctl::SwitchSet(_) => "Show all".into(),
            Ctl::ProcessList => "List apps".into(),
            Ctl::Launch { name, .. } => format!("Start {name}"),
            Ctl::Terminate(h) => format!("Stop app {h}"),
            Ctl::Panel { panel, visible, .. } => format!("{} {panel}", if *visible { "Show" } else { "Hide" }),
            Ctl::GazeLaser(on) => format!("Eye laser {}", if *on { "on" } else { "off" }),
            Ctl::Controllers(on) => (if *on { "Controllers on" } else { "Hands only" }).into(),
            Ctl::ScreenNumbers(m) => format!("Screen numbers: {}", match m {
                ScreenNumbers::Off => "off",
                ScreenNumbers::OnDemand => "on demand",
                ScreenNumbers::Always => "always",
            }),
            Ctl::FlashScreenNumbers => "Show screen numbers".into(),
        }
    }
}

/// Apps of a `process-list` answer: (name, handle).
pub fn parse_processes(out: &str) -> Vec<(String, u64)> {
    let Ok(serde_json::Value::Array(a)) = serde_json::from_str::<serde_json::Value>(out.trim()) else { return Vec::new() };
    a.iter()
        .filter_map(|p| {
            let name = p.get("name")?.as_str()?.to_string();
            let h = p.get("handle")?.get("user")?.as_u64()?;
            Some((name, h))
        })
        .collect()
}

/// How long a `wayvrctl` call may take (its request/response commands have no timeout of their own).
pub const CTL_TIMEOUT: Duration = Duration::from_secs(3);

/// Run `wayvrctl` (background tasks only): its stdout, or why it failed. Refused in dry-run mode.
pub fn run_ctl(ctl: &Path, c: &Ctl) -> Result<String, String> {
    crate::cmd::allow_start("wayvrctl")?;
    let mut child = std::process::Command::new(ctl)
        .args(c.args())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot start {}: {e}", ctl.display()))?;
    let t0 = std::time::Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(st)) => {
                let out = child.wait_with_output().map_err(|e| e.to_string())?;
                let so = String::from_utf8_lossy(&out.stdout).into_owned();
                let se = String::from_utf8_lossy(&out.stderr).into_owned();
                return if st.success() {
                    Ok(so)
                } else {
                    Err(format!("wayvrctl: {}", se.lines().last().unwrap_or("failed (is WayVR running?)")))
                };
            }
            Ok(None) if t0.elapsed() < CTL_TIMEOUT => std::thread::sleep(Duration::from_millis(20)),
            Ok(None) => {
                // our own child; WayVR did not answer
                // SAFETY: kill(2) with our child's positive pid.
                unsafe {
                    libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                }
                std::thread::spawn(move || {
                    let _ = child.wait();
                });
                return Err("WayVR did not answer within 3 s".into());
            }
            Err(e) => return Err(e.to_string()),
        }
    }
}

/// Write the app's settings file atomically.
pub fn save_settings(path: &Path, s: &WayvrSettings) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("conf.tmp");
    std::fs::write(&tmp, s.render())?;
    std::fs::rename(&tmp, path)
}

/// Copy the saved layout into the backups directory; returns the copy.
pub fn backup_layout(w: &WayvrPaths, stamp: &str) -> std::io::Result<PathBuf> {
    std::fs::create_dir_all(&w.backups)?;
    let to = w.backups.join(format!("{stamp}-zz-saved-state.json5"));
    std::fs::copy(&w.saved_state, &to)?;
    Ok(to)
}

/// Back up and remove the saved layout (WayVR must not run: it writes the file when it exits). The next start puts
/// every screen at its default place.
pub fn reset_layout(w: &WayvrPaths, stamp: &str) -> std::io::Result<PathBuf> {
    let b = backup_layout(w, stamp)?;
    std::fs::remove_file(&w.saved_state)?;
    Ok(b)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(n: &str, t: &str) -> (String, String) {
        (n.to_string(), t.to_string())
    }

    #[test]
    fn conf_d_check_finds_what_keeps_wayvr_from_starting() {
        let files = vec![
            f("10-am.yaml", "capture_method: screencopy\nblock_game_input: true\n"),
            f("20-gaze.yaml", "gaze_pointer: true\ngaze_pointer: false\n"),
            f("30-worldlock.yaml", "world_lock_grab: true\ngrab_size_min:\n"),
            f("40-panels.yaml", "custom_panels:\n  - perf\n"),
            f("41-ui.yaml", "animation_speed: 1.0\n---\nx: 1\n"),
            f("42-ui.yaml", "ui_animation_speed: 2.0\n"),
            f("50-app.YAML", "a: 1\n"),
            f("zz-saved-state.json5", "{\"sets\": ["),
            f("zz-saved-config.json5", "{\"enable_keyboard\": true}"),
            f("old.bak", "x"),
        ];
        let l = lint(&files);
        let has = |file: &str, text: &str| l.iter().any(|x| x.file.contains(file) && x.text.contains(text));
        assert!(has("20-gaze.yaml", "`gaze_pointer` twice"), "{l:#?}");
        assert!(has("30-worldlock.yaml", "`grab_size_min:` has no value"), "{l:#?}");
        assert!(!has("40-panels.yaml", "no value"), "a list under the key is a value");
        assert!(has("41-ui.yaml", "second YAML document"));
        assert!(has("41-ui.yaml + 42-ui.yaml", "both set"));
        assert!(has("50-app.YAML", "capitals"));
        assert!(has("zz-saved-state.json5", "not valid JSON"));
        assert!(has("zz-saved-config.json5", "dashboard saved"));
        assert!(has("old.bak", "ignored"));
        assert_eq!(l[0].level, LintLevel::Error);
        assert!(lint(&[f("10-am.yaml", "capture_method: screencopy\n")]).is_empty());
    }

    #[test]
    fn layout_and_processes() {
        let t = r#"{"sets":[{"name":"","overlays":{"SUNSHINE":{"alpha":1.0,"grabbable":true,"positioning":"Static","saved_transform":[1.0,0.0,0.0,0.0,1.0,0.0,0.0,0.0,1.0,0.5,1.1,-2.0]},"captions":{"grabbable":false,"positioning":"Floating","saved_transform":null}},"hidden_overlays":{}},{"name":"global","overlays":{"watch":{"positioning":{"FollowHand":{"hand":"Left","lerp":1.0}},"saved_transform":null}}}],"last_set":0}"#;
        let l = parse_layout(t).unwrap();
        assert_eq!(l.len(), 3);
        let s = l.iter().find(|o| o.name == "SUNSHINE").unwrap();
        assert_eq!(s.pos, Some([0.5, 1.1, -2.0]));
        assert!((s.width.unwrap() - 1.0).abs() < 1e-6);
        assert_eq!(l.iter().find(|o| o.name == "watch").unwrap().positioning, "FollowHand");
        assert_eq!(screens(&l), vec!["SUNSHINE".to_string()]);
        assert!(parse_layout("{").is_err());
        assert_eq!(parse_processes(r#"[{"name":"foot","handle":{"user":3},"userdata":{}}]"#), vec![("foot".to_string(), 3)]);
        assert!(parse_processes("null").is_empty());
    }

    #[test]
    fn wayvrctl_command_lines() {
        assert_eq!(Ctl::ResetScreens.args(), vec!["reset-screens"]);
        assert_eq!(Ctl::SwitchSet(0).args(), vec!["switch-set", "0"]);
        assert_eq!(Ctl::Terminate(7).args(), vec!["process-terminate", "{\"user\":7}"]);
        assert_eq!(
            Ctl::Panel { panel: "captions", element: "cap_box", visible: false }.args(),
            vec!["panel-modify", "captions", "cap_box", "set-visible", "0"]
        );
        let l = Ctl::Launch { name: "Terminal".into(), exec: "foot".into() }.args();
        assert_eq!(l, vec!["process-launch", "-n", "Terminal", "foot", "1920x1080", "floating"]);
        let l = Ctl::Launch { name: "Web".into(), exec: "firefox --new-window".into() }.args();
        assert_eq!(l, vec!["process-launch", "-n", "Web", "firefox", "1920x1080", "floating", "", "--new-window"]);
    }
}
