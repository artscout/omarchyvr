//! The VR home (docs/VR-HOME.md): a small panel in WayVR's own (blue) look, in the headset, that Play lands in.
//!
//! * **The home session** is a Monado game session (`omarchyvr vr start ... --home --runtime monado`): Monado, 6DoF, the
//!   controllers, the room anchor, passthrough (or WayVR's sky) and the *home overlay*: a minimal headless WayVR (no
//!   screens, no watch, no keyboard) that carries the panel [`PANEL`] and shows it at start (`menu_open_at_start`).
//!   No local AI, no voice, no desk: it is the Play side, so Play's rules hold (the whole graphics card for games).
//! * **The panel** ([`panel_xml`]) has big entries: Work, Games with SteamVR, the installed Steam VR games with the
//!   runtime each one uses (`game-runtime.json`, else "Games run with"), Room setup Quick / Full, a few settings
//!   (Recenter, Fix floor, Close) and Exit VR. In Work the Work menu (`work_menu`) is the same panel with Work's own
//!   buttons (Exit Work, Recenter, WayVR settings) and "VR home" in place of "Work".
//! * **The menu button** of either controller (the Pimax controllers have one menu button per hand, report byte 48
//!   bit 0; Monado presents the left one as `/input/menu/click` and the right one as `/input/system/click`; WayVR's
//!   `menu` action binds both) opens and closes it, in the home, in Work, and over an xrizer game when "Menu button
//!   opens the VR home in games" is on (the overlay then runs in every Monado game session). Under SteamVR the button
//!   belongs to SteamVR (its dashboard); nothing of ours runs there.
//! * **A button press** runs `omarchyvr home go TARGET` (WayVR `::ShellExec`). [`go`] returns at once: it refuses while
//!   another transition runs (one at a time, a flock on [`LOCK_FILE`]), asks for a second press for Exit, then starts
//!   `omarchyvr home run TARGET` detached (it must outlive WayVR, which the transition may stop).
//! * **A transition** ([`plan`] then [`execute`]) only uses the existing full stop and start paths: `desk stop --grace
//!   30` (Work's full stop: screens, voice, the AI), `vr stop --grace 30` (the home, a Monado game session or SteamVR),
//!   a wait until no monado-service is left, then the launcher's `vr start` / `desk start` with the app's own saved
//!   arguments ([`LaunchArgs`], written by the app at every start). Progress is plain text: a toast in the headset, the
//!   panel's status line and [`STATUS_FILE`] (the app shows it); a start that fails after the stop falls back to the
//!   home so the headset is not left dark.
//! * Settings ([`Settings`], `~/.config/omarchyvr/home.toml`, shared with the app): `start_in_home` (Play lands in the
//!   home; default on), `pi_menu` (the menu button opens the home over games; default on), `background`
//!   (passthrough, the default, or sky).
use super::config::StartOptions;
use super::gameruntime;
use super::session::{self, Deps};
use super::steamvr::Games;
use crate::args::Args;
use crate::{CliError, CliResult};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The home panel's name (a WayVR custom panel: `gui/vr_home.xml`).
pub const PANEL: &str = "vr_home";
/// The flag of `vr start` that makes the session the VR home (recorded in start-argv).
pub const HOME_FLAG: &str = "--home";
/// Games on the panel, at most.
pub const MAX_GAMES: usize = 6;
/// `<session dir>/home-transition.txt`: the last transition (state, target, text, when).
pub const STATUS_FILE: &str = "home-transition.txt";
/// `<session dir>/home-transition.lock`: held (flock) while a transition runs.
pub const LOCK_FILE: &str = "home-transition.lock";
/// `<session dir>/home-exit-armed`: the first Exit VR press (unix ms).
pub const EXIT_ARMED: &str = "home-exit-armed";
/// The second Exit VR press must come within this.
pub const EXIT_CONFIRM_MS: u64 = 6000;
/// How long a transition waits for Monado (or SteamVR) to be gone after a stop.
pub const GONE_WAIT: Duration = Duration::from_secs(45);
/// SteamVR's own app id (never listed as a game).
const STEAMVR_APPID: &str = super::steamvr::STEAMVR_APPID;

pub const USAGE: &str = "usage: omarchyvr home <command>
  status               what runs now (home, Work, a game session, SteamVR), the settings, the games, the last transition
  games                the installed Steam VR games the home lists, with the runtime each one uses
  plan TARGET [APPID]  print the steps a transition would take now (nothing is done)
  go TARGET [APPID]    from the headset (the home panel's buttons): start the transition detached and return at once;
                       refused while another one runs; `exit` asks for a second press within 6 s
  run TARGET [APPID]   the transition itself (what `go` starts): one at a time, progress in the headset and in
                       <session dir>/home-transition.txt
  panel [--work]       print the panel's WayVR layout (the home, or the Work menu)
  set start-in-home on|off | pi-menu on|off | background passthrough|sky
TARGET: home | work | steamvr | game APPID | room-quick | room-full | exit
Settings: ~/.config/omarchyvr/home.toml (shared with the app). Start arguments: ~/.config/omarchyvr/home-launch.json
(written by the app at every Play and Work). docs/VR-HOME.md.";

// ---- settings ---------------------------------------------------------------------------------------------------

/// What is behind the home panel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Background {
    /// The room (camera passthrough), the default.
    Passthrough,
    /// WayVR's built-in sky.
    Sky,
}

impl Background {
    pub fn name(self) -> &'static str {
        match self {
            Background::Passthrough => "passthrough",
            Background::Sky => "sky",
        }
    }
    pub fn parse(s: &str) -> Option<Background> {
        match s.trim() {
            "passthrough" => Some(Background::Passthrough),
            "sky" => Some(Background::Sky),
            _ => None,
        }
    }
}

/// `~/.config/omarchyvr/home.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settings {
    /// Play lands in the VR home.
    pub start_in_home: bool,
    /// The controller's menu button opens the VR home in every Monado session (the home overlay runs over games too).
    pub pi_menu: bool,
    /// Behind the home panel.
    pub background: Background,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { start_in_home: true, pi_menu: true, background: Background::Passthrough }
    }
}

pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/home.toml")
}

fn value<'a>(t: &'a str, key: &str) -> Option<&'a str> {
    t.lines().find_map(|l| {
        let l = l.split('#').next()?.trim();
        let rest = l.strip_prefix(key)?.trim_start();
        Some(rest.strip_prefix('=')?.trim().trim_matches('"'))
    })
}

/// Parse `home.toml` (unknown or bad values keep the default).
pub fn parse_settings(t: &str) -> Settings {
    let d = Settings::default();
    let b = |k: &str, def: bool| match value(t, k) {
        Some("true") => true,
        Some("false") => false,
        _ => def,
    };
    Settings {
        start_in_home: b("start_in_home", d.start_in_home),
        pi_menu: b("pi_menu", d.pi_menu),
        background: value(t, "background").and_then(Background::parse).unwrap_or(d.background),
    }
}

pub fn settings_text(s: &Settings) -> String {
    format!(
        "# The VR home (docs/VR-HOME.md); the Pimax app and `omarchyvr home set` write this file.\nstart_in_home = {}\npi_menu = {}\nbackground = \"{}\"\n",
        s.start_in_home,
        s.pi_menu,
        s.background.name()
    )
}

pub fn load(home: &Path) -> Settings {
    std::fs::read_to_string(settings_path(home)).map(|t| parse_settings(&t)).unwrap_or_default()
}

pub fn save(home: &Path, s: &Settings) -> io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("toml.tmp");
    std::fs::write(&tmp, settings_text(s))?;
    std::fs::rename(tmp, p)
}

// ---- the app's start arguments -------------------------------------------------------------------------------------

/// The launcher arguments the app uses for Work and Play (its switches), so a transition from the headset starts
/// exactly what the app's buttons would. Written by the app at every start; `home-launch.json`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchArgs {
    /// `desk start ...`
    pub work: Vec<String>,
    /// `vr start ...`
    pub play: Vec<String>,
}

impl Default for LaunchArgs {
    /// Without the app's file: the fresh defaults (6DoF, optical controllers, the room anchor for Work, 90 Hz).
    fn default() -> Self {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        LaunchArgs { work: v("desk start --fps 90 --6dof --ctrl-track --anchor"), play: v("vr start --fps 90 --6dof --ctrl-track") }
    }
}

pub fn launch_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/home-launch.json")
}

/// Parse `home-launch.json` (`{"work": [...], "play": [...]}`); a list that does not start with `desk start` /
/// `vr start` is replaced by the default.
pub fn parse_launch(text: &str) -> LaunchArgs {
    let d = LaunchArgs::default();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return d };
    let list = |k: &str, first: &str| -> Option<Vec<String>> {
        let a: Vec<String> = v.get(k)?.as_array()?.iter().map(|x| x.as_str().map(String::from)).collect::<Option<_>>()?;
        (a.len() >= 2 && a[0] == first && a[1] == "start").then_some(a)
    };
    LaunchArgs { work: list("work", "desk").unwrap_or(d.work), play: list("play", "vr").unwrap_or(d.play) }
}

pub fn launch_text(l: &LaunchArgs) -> String {
    format!("{}\n", serde_json::to_string_pretty(&serde_json::json!({ "work": l.work, "play": l.play })).unwrap())
}

pub fn load_launch(home: &Path) -> LaunchArgs {
    std::fs::read_to_string(launch_path(home)).map(|t| parse_launch(&t)).unwrap_or_default()
}

pub fn save_launch(home: &Path, l: &LaunchArgs) -> io::Result<()> {
    let p = launch_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_extension("json.tmp");
    std::fs::write(&tmp, launch_text(l))?;
    std::fs::rename(tmp, p)
}

// ---- games ------------------------------------------------------------------------------------------------------

/// A game on the home panel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Game {
    pub appid: String,
    pub name: String,
    /// The runtime it starts with: its `game-runtime.json` entry, else "Games run with".
    pub runtime: Games,
}

impl Game {
    /// "xrizer" or "SteamVR", for the panel.
    pub fn runtime_words(&self) -> &'static str {
        match self.runtime {
            Games::SteamVr => "SteamVR",
            Games::Monado => "xrizer",
        }
    }
}

/// The VR apps of Steam's `config/steamapps.vrmanifest` (Steam writes it for the VR games it knows): (app id, name),
/// in file order, each once.
pub fn parse_vrmanifest(text: &str) -> Vec<(String, String)> {
    let Ok(v) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let mut out: Vec<(String, String)> = Vec::new();
    for a in v.get("applications").and_then(|a| a.as_array()).into_iter().flatten() {
        let Some(id) = a.get("app_key").and_then(|k| k.as_str()).and_then(|k| k.strip_prefix("steam.app.")) else { continue };
        if id.is_empty() || !id.bytes().all(|b| b.is_ascii_digit()) || id == STEAMVR_APPID || out.iter().any(|(i, _)| i == id) {
            continue;
        }
        let name = a.pointer("/strings/en_us/name").and_then(|n| n.as_str()).unwrap_or("").to_string();
        out.push((id.to_string(), name));
    }
    out
}

/// Steam library folders (`steamapps/libraryfolders.vdf`, `"path"  "..."` lines), the default one first.
pub fn library_dirs(vdf: Option<&str>, steam_root: &Path) -> Vec<PathBuf> {
    let mut v = vec![steam_root.to_path_buf()];
    for l in vdf.unwrap_or("").lines() {
        let mut q = l.split('"').filter(|s| !s.trim().is_empty());
        if q.next() == Some("path") {
            if let Some(p) = q.next() {
                let p = PathBuf::from(p);
                if !v.contains(&p) {
                    v.push(p);
                }
            }
        }
    }
    v
}

/// The panel's games: Steam's VR apps plus the games `game-runtime.json` knows, installed ones only, with the runtime
/// each starts with, sorted by name, at most `max`.
pub fn games_from(vr_apps: &[(String, String)], store: &gameruntime::Store, global: Games, installed: &dyn Fn(&str) -> bool, max: usize) -> Vec<Game> {
    let mut v: Vec<Game> = Vec::new();
    let listed = store.list();
    let ids = vr_apps.iter().map(|(i, n)| (i.clone(), n.clone())).chain(listed.iter().map(|e| (e.appid.clone(), e.name.clone())));
    for (id, name) in ids {
        if v.iter().any(|g| g.appid == id) || !installed(&id) {
            continue;
        }
        let name = if name.trim().is_empty() { listed.iter().find(|e| e.appid == id).map(|e| e.name.clone()).unwrap_or_else(|| format!("Steam app {id}")) } else { name };
        v.push(Game { runtime: gameruntime::resolve(store, Some(&id), global), appid: id, name });
    }
    v.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()).then(a.appid.cmp(&b.appid)));
    v.truncate(max);
    v
}

/// The real list (Steam under `~/.local/share/Steam`).
pub fn games(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> Vec<Game> {
    let steam = home.join(".local/share/Steam");
    let vr = std::fs::read_to_string(steam.join("config/steamapps.vrmanifest")).map(|t| parse_vrmanifest(&t)).unwrap_or_default();
    let libs = library_dirs(std::fs::read_to_string(steam.join("steamapps/libraryfolders.vdf")).ok().as_deref(), &steam);
    let installed = |id: &str| libs.iter().any(|l| l.join("steamapps").join(format!("appmanifest_{id}.acf")).is_file());
    games_from(&vr, &gameruntime::Store::load(home), super::steamvr::games_runtime(env), &installed, MAX_GAMES)
}

// ---- the panel --------------------------------------------------------------------------------------------------

/// Where the panel is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctx {
    /// The VR home and Monado game sessions (panel [`PANEL`]).
    Home,
    /// Work (the Work menu, panel `work_menu`).
    Work,
}

impl Ctx {
    pub fn panel(self) -> &'static str {
        match self {
            Ctx::Home => PANEL,
            Ctx::Work => super::desk::WORK_MENU_NAME,
        }
    }
}

/// Text for an XML attribute.
pub fn xml_attr(s: &str) -> String {
    s.replace('&', "&amp;").replace('"', "&quot;").replace('<', "&lt;").replace('>', "&gt;")
}

fn button(id: &str, mac: &str, text: &str, press: &str, close: Option<&str>, tip: &str, extra: &str) -> String {
    let close = close.map(|p| format!(" _press2=\"::OverlayToggle {p}\"")).unwrap_or_default();
    format!(
        "          <Button id=\"{id}\" macro=\"{mac}\" text=\"{}\"{extra} _press=\"{}\"{close} tooltip_str=\"{}\" tooltip_side=\"top\" />\n",
        xml_attr(text),
        xml_attr(press),
        xml_attr(tip)
    )
}

const DANGER: &str = " text_color=\"on_danger\" color=\"danger\" border_color=\"danger(rgb-mult-0.5)\" hover_color=\"danger(rgb-add-0.2)\" hover_border_color=\"danger\"";

/// The panel's WayVR layout. WayVR's own palette (the blue dashboard and watch look): `background`, `outline`,
/// `on_background`, the default button colours, `danger` for Exit. `status` is the line under the title (the last
/// transition, in plain words).
pub fn panel_xml(ctx: Ctx, games: &[Game], status: &str) -> String {
    let me = ctx.panel();
    let go = |t: &str| format!("::ShellExec omarchyvr home go {t}");
    let mut x = String::new();
    x.push_str("<layout>\n");
    x.push_str("  <macro name=\"menu_button\" padding=\"14\" border=\"2\" round=\"10\" align_items=\"center\" justify_content=\"center\" min_width=\"170\" />\n");
    x.push_str("  <macro name=\"big_button\" padding=\"18\" border=\"2\" round=\"12\" align_items=\"center\" justify_content=\"center\" min_width=\"300\" text_size=\"26\" text_weight=\"bold\" />\n");
    x.push_str("  <macro name=\"game_button\" padding=\"12\" border=\"2\" round=\"10\" align_items=\"center\" justify_content=\"center\" min_width=\"300\" max_width=\"300\" />\n");
    x.push_str("  <elements>\n    <div padding=\"24\" interactable=\"0\" flex_direction=\"column\" gap=\"12\">\n");
    x.push_str("      <rectangle id=\"home_root\" padding=\"20\" color=\"background\" border=\"2\" border_color=\"outline\" round=\"14\" flex_direction=\"column\" gap=\"14\">\n");
    let title = match ctx {
        Ctx::Home => "VR home",
        Ctx::Work => "Work",
    };
    x.push_str(&format!("        <label text=\"{title}\" size=\"34\" weight=\"bold\" color=\"on_background\" />\n"));
    x.push_str(&format!("        <label id=\"home_status\" text=\"{}\" size=\"18\" color=\"on_background_variant\" />\n", xml_attr(status)));
    // the big choices
    x.push_str("        <div flex_direction=\"row\" gap=\"12\">\n");
    match ctx {
        Ctx::Home => x.push_str(&button("home_work", "big_button", "Work", &go("work"), None, "The desk: screens, voice, the AI. The VR home closes first; the headset goes dark for a moment.", "")),
        Ctx::Work => x.push_str(&button("home_home", "big_button", "VR home", &go("home"), None, "Work closes completely (screens, voice, the AI), then the VR home starts.", "")),
    }
    x.push_str(&button("home_steamvr", "big_button", "Games with SteamVR", &go("steamvr"), None, "SteamVR with the Pimax driver: its own home and dashboard. What runs now closes first.", ""));
    x.push_str("        </div>\n");
    // the games
    x.push_str("        <label text=\"Games\" size=\"22\" weight=\"bold\" color=\"on_background\" />\n");
    if games.is_empty() {
        x.push_str("        <label id=\"home_no_games\" text=\"No VR games installed in Steam yet.\" size=\"18\" color=\"on_background_variant\" />\n");
    } else {
        x.push_str("        <div flex_direction=\"row\" flex_wrap=\"wrap\" gap=\"10\">\n");
        for (i, g) in games.iter().enumerate() {
            let tip = match g.runtime {
                Games::Monado => format!("Starts {} with Monado (xrizer) right here.", g.name),
                Games::SteamVr => format!("{} runs with SteamVR: what runs now closes first, then SteamVR starts it.", g.name),
            };
            x.push_str(&button(&format!("home_game_{i}"), "game_button", &format!("{} ({})", g.name, g.runtime_words()), &go(&format!("game {}", g.appid)), Some(me), &tip, ""));
        }
        x.push_str("        </div>\n");
    }
    // room setup
    x.push_str("        <label text=\"Room setup\" size=\"22\" weight=\"bold\" color=\"on_background\" />\n");
    x.push_str("        <div flex_direction=\"row\" gap=\"12\">\n");
    x.push_str(&button("home_room_quick", "menu_button", "Quick (30 s)", &go("room-quick"), Some(me), "Stand in the middle and look around, then look forward and pull a trigger.", ""));
    x.push_str(&button("home_room_full", "menu_button", "Full (3 min)", &go("room-full"), Some(me), "Walk the room, lay a controller on the floor, trace your play area with a trigger.", ""));
    x.push_str("        </div>\n");
    // settings and exit
    x.push_str("        <div flex_direction=\"row\" gap=\"12\">\n");
    match ctx {
        Ctx::Home => {
            x.push_str(&button("home_recenter", "menu_button", "Recenter", "::PlayspaceRecenter", None, "Forward is where you look now", ""));
            x.push_str(&button("home_floor", "menu_button", "Fix floor", "::PlayspaceFixFloor", None, "Hold a controller on the floor, then press", ""));
            x.push_str(&button("home_close", "menu_button", "Close", &format!("::OverlayToggle {me}"), None, "Hide this panel (the menu button opens it again)", ""));
        }
        Ctx::Work => {
            // Work's own buttons, with the ids and commands of the Work menu (docs/WAYVR-CONTROLS.md)
            x.push_str(&button("btn_close", "menu_button", "Close menu", &format!("::OverlayToggle {me}"), None, "Hide this menu (the menu button opens it again)", ""));
            x.push_str(&button("btn_recenter", "menu_button", "Recenter", "::PlayspaceRecenter", None, "Put the screens in front of you again", ""));
            x.push_str(&button("btn_dashboard", "menu_button", "WayVR settings", "::DashToggle", None, "WayVR's own dashboard", ""));
        }
    }
    x.push_str("        </div>\n");
    x.push_str("        <div flex_direction=\"row\" gap=\"12\">\n");
    match ctx {
        Ctx::Home => x.push_str(&button("home_exit", "menu_button", "Exit VR", &go("exit"), None, "Stops everything of VR and goes back to the desktop. Press it twice.", DANGER)),
        Ctx::Work => x.push_str(&button("btn_exit", "menu_button", "Exit Work", "::ShellExec omarchyvr desk exit", None, "Stops everything (screens, voice, AI) and goes back to the desktop. Press it twice.", DANGER)),
    }
    x.push_str("        </div>\n");
    let hint = match ctx {
        Ctx::Home => "The menu button on either controller opens and closes this panel.",
        Ctx::Work => "Exit Work: press it, then press it again.",
    };
    x.push_str(&format!("        <label id=\"exit_hint\" text=\"{hint}\" size=\"18\" color=\"on_background_variant\" />\n"));
    x.push_str("      </rectangle>\n    </div>\n  </elements>\n</layout>\n");
    x
}

// ---- what runs now ----------------------------------------------------------------------------------------------

/// The session that runs now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Nothing,
    /// A Monado game session started as the VR home (`vr start --home`), maybe with an xrizer game on top.
    Home,
    /// A Monado game session (Play without the home).
    Play,
    /// SteamVR (Games run with: SteamVR, or a game that needs it).
    SteamVr,
    /// Work (the desk).
    Work,
}

impl Kind {
    pub fn words(self) -> &'static str {
        match self {
            Kind::Nothing => "nothing",
            Kind::Home => "the VR home",
            Kind::Play => "the game session",
            Kind::SteamVr => "SteamVR",
            Kind::Work => "Work",
        }
    }
}

/// The running session and what it has for the room setup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Current {
    pub kind: Kind,
    pub sixdof: bool,
    pub anchor: bool,
    pub ctrl_track: bool,
}

/// From the recorded start (`start-argv.txt`) and what is alive.
pub fn classify(argv: Option<&(String, Vec<String>)>, monado_up: bool, steamvr_up: bool) -> Current {
    let has = |f: &str| argv.is_some_and(|(_, a)| a.iter().any(|x| x == f));
    let kind = if steamvr_up {
        Kind::SteamVr
    } else if !monado_up {
        Kind::Nothing
    } else {
        match argv.map(|(k, _)| k.as_str()) {
            Some("desk") => Kind::Work,
            Some("vr") if has(HOME_FLAG) => Kind::Home,
            _ => Kind::Play,
        }
    };
    let sixdof = has("--6dof");
    Current { kind, sixdof, anchor: sixdof && (has("--anchor") || has(HOME_FLAG)), ctrl_track: sixdof && has("--ctrl-track") }
}

// ---- targets and plans ------------------------------------------------------------------------------------------

/// Where a button press goes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Home,
    Work,
    SteamVr,
    Game(String),
    Room { full: bool },
    Exit,
}

impl Target {
    pub fn parse(words: &[String]) -> Result<Target, String> {
        let w: Vec<&str> = words.iter().map(String::as_str).collect();
        Ok(match w.as_slice() {
            ["home"] => Target::Home,
            ["work"] => Target::Work,
            ["steamvr"] => Target::SteamVr,
            ["game", id] if !id.is_empty() && id.len() <= 12 && id.bytes().all(|b| b.is_ascii_digit()) => Target::Game(id.to_string()),
            ["game", ..] => return Err("game wants a Steam app id (digits)".into()),
            ["room-quick" | "room-fast"] => Target::Room { full: false },
            ["room-full"] => Target::Room { full: true },
            ["exit"] => Target::Exit,
            _ => return Err(format!("unknown target {:?}: home | work | steamvr | game APPID | room-quick | room-full | exit", w.join(" "))),
        })
    }

    pub fn words(&self) -> String {
        match self {
            Target::Home => "home".into(),
            Target::Work => "work".into(),
            Target::SteamVr => "steamvr".into(),
            Target::Game(id) => format!("game {id}"),
            Target::Room { full: false } => "room-quick".into(),
            Target::Room { full: true } => "room-full".into(),
            Target::Exit => "exit".into(),
        }
    }
}

/// One step of a transition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    /// Plain words in the headset (a toast and the panel's status line) and in the status file.
    Say(String),
    /// Work's full stop (`desk stop --grace 30`), then the wait until Monado is gone.
    StopWork,
    /// The home / a game session / SteamVR (`vr stop --grace 30`), then the wait until Monado and SteamVR are gone.
    StopVr,
    /// The home's passthrough (by pid): an xrizer game becomes the main app.
    StopPassthrough,
    /// The home's passthrough again (back in the home after a game), when the background is passthrough.
    StartPassthrough,
    /// The launcher with these arguments (`vr start ...` / `desk start ...`).
    Start(Vec<String>),
    /// `steam steam://rungameid/APPID` (Steam starts the game; its runtime is the session's).
    LaunchGame(String),
    /// `omarchyvr-roommap setup fast|full` in this session (guided in the headset); waits for it.
    RoomSetup { full: bool },
}

fn drop_opts(args: &[String], flags: &[&str], with_value: &[&str]) -> Vec<String> {
    let mut v = Vec::new();
    let mut skip = false;
    for a in args {
        if skip {
            skip = false;
            continue;
        }
        if with_value.contains(&a.as_str()) {
            skip = true;
            continue;
        }
        if with_value.iter().any(|f| a.starts_with(&format!("{f}="))) || flags.contains(&a.as_str()) {
            continue;
        }
        v.push(a.clone());
    }
    v
}

fn ensure(v: &mut Vec<String>, f: &str) {
    if !v.iter().any(|x| x == f) {
        v.push(f.into());
    }
}

/// The app's Play arguments without what a home or a given game decides itself (`--force` is never passed from the
/// headset: consent lives in the app).
fn base_play(la: &LaunchArgs) -> Vec<String> {
    drop_opts(&la.play, &["--perf", "--light", "--no-light", "--force", HOME_FLAG], &["--game", "--runtime"])
}

/// `vr start ... --home --runtime monado`: the home is always Monado (a SteamVR choice is a game's, not the home's),
/// with the room anchor when 6DoF is on; `room` = the room setup needs 6DoF, the anchor (and for the full one the
/// optical controllers).
pub fn home_start(la: &LaunchArgs, room: Option<bool>) -> Vec<String> {
    let mut v = base_play(la);
    if let Some(full) = room {
        ensure(&mut v, "--6dof");
        if full {
            ensure(&mut v, "--ctrl-track");
        }
    }
    if v.iter().any(|x| x == "--6dof") {
        ensure(&mut v, "--anchor");
    }
    v.extend([HOME_FLAG.into(), "--runtime".into(), "monado".into()]);
    v
}

/// `desk start ...` as the app would (never `--force`).
pub fn work_start(la: &LaunchArgs) -> Vec<String> {
    drop_opts(&la.work, &["--force"], &[])
}

/// `vr start ... --runtime R [--game APPID]` (a game session for SteamVR or an xrizer game).
pub fn play_start(la: &LaunchArgs, runtime: Games, game: Option<&str>) -> Vec<String> {
    let mut v = base_play(la);
    v.extend(["--runtime".into(), runtime.name().into()]);
    if let Some(g) = game {
        v.extend(["--game".into(), g.into()]);
    }
    v
}

fn stop_of(k: Kind) -> Option<Step> {
    match k {
        Kind::Nothing => None,
        Kind::Work => Some(Step::StopWork),
        Kind::Home | Kind::Play | Kind::SteamVr => Some(Step::StopVr),
    }
}

/// What a transition from `cur` to `t` does. `game` is the game's name and runtime (for [`Target::Game`]). Errors are
/// plain words for the headset ("Work is already running").
pub fn plan(cur: &Current, t: &Target, game: Option<&Game>, la: &LaunchArgs, bg: Background) -> Result<Vec<Step>, String> {
    let k = cur.kind;
    let mut v = Vec::new();
    let leave = |what: &str| format!("{what}: {} closes first; the headset goes dark for a moment.", capital(k.words()));
    match t {
        Target::Exit => match k {
            Kind::Nothing => return Err("Nothing of VR is running.".into()),
            Kind::Work => v.extend([Step::Say("Exiting Work: stopping everything (screens, voice, AI). The headset goes dark in a few seconds.".into()), Step::StopWork]),
            _ => v.extend([Step::Say(format!("Exiting VR: stopping {}. The headset goes dark in a few seconds.", k.words())), Step::StopVr]),
        },
        Target::Home => match k {
            Kind::Home => {
                v.push(Step::Say("You are in the VR home.".into()));
                if bg == Background::Passthrough {
                    v.push(Step::StartPassthrough);
                }
            }
            Kind::Nothing => v.push(Step::Start(home_start(la, None))),
            _ => v.extend([Step::Say(leave("Going to the VR home")), stop_of(k).unwrap(), Step::Start(home_start(la, None))]),
        },
        Target::Work => match k {
            Kind::Work => return Err("Work is already running.".into()),
            Kind::Nothing => v.push(Step::Start(work_start(la))),
            _ => v.extend([Step::Say(leave("Starting Work")), stop_of(k).unwrap(), Step::Start(work_start(la))]),
        },
        Target::SteamVr => match k {
            Kind::SteamVr => return Err("SteamVR is already running.".into()),
            Kind::Nothing => v.push(Step::Start(play_start(la, Games::SteamVr, None))),
            _ => v.extend([Step::Say(leave("Starting SteamVR")), stop_of(k).unwrap(), Step::Start(play_start(la, Games::SteamVr, None))]),
        },
        Target::Game(id) => {
            let (name, runtime) = game.map(|g| (g.name.clone(), g.runtime)).unwrap_or_else(|| (format!("Steam app {id}"), Games::Monado));
            match (runtime, k) {
                (Games::SteamVr, Kind::SteamVr) => v.extend([Step::Say(format!("Starting {name} in SteamVR.")), Step::LaunchGame(id.clone())]),
                (Games::SteamVr, Kind::Nothing) => v.extend([Step::Start(play_start(la, Games::SteamVr, Some(id))), Step::LaunchGame(id.clone())]),
                (Games::SteamVr, _) => v.extend([
                    Step::Say(leave(&format!("{name} runs with SteamVR"))),
                    stop_of(k).unwrap(),
                    Step::Start(play_start(la, Games::SteamVr, Some(id))),
                    Step::LaunchGame(id.clone()),
                ]),
                (Games::Monado, Kind::Home) => v.extend([Step::Say(format!("Starting {name} (xrizer). The menu button brings this panel back.")), Step::StopPassthrough, Step::LaunchGame(id.clone())]),
                (Games::Monado, Kind::Play) => v.extend([Step::Say(format!("Starting {name} (xrizer).")), Step::LaunchGame(id.clone())]),
                (Games::Monado, Kind::Nothing) => v.extend([Step::Start(play_start(la, Games::Monado, Some(id))), Step::LaunchGame(id.clone())]),
                (Games::Monado, _) => v.extend([
                    Step::Say(leave(&format!("Starting {name} (xrizer)"))),
                    stop_of(k).unwrap(),
                    Step::Start(play_start(la, Games::Monado, Some(id))),
                    Step::LaunchGame(id.clone()),
                ]),
            }
        }
        Target::Room { full } => {
            let what = if *full { "Full room setup (about 3 minutes)" } else { "Quick room setup (30 seconds)" };
            let ready = cur.sixdof && cur.anchor && (!*full || cur.ctrl_track);
            match k {
                Kind::Home | Kind::Work if ready => v.extend([Step::Say(format!("{what}: follow the steps in front of you.")), Step::RoomSetup { full: *full }]),
                Kind::Nothing => v.extend([Step::Start(home_start(la, Some(*full))), Step::RoomSetup { full: *full }]),
                _ => v.extend([
                    Step::Say(format!(
                        "{what} needs 6DoF and the room anchor{}: {} closes, then the VR home starts with them.",
                        if *full { " and controller tracking" } else { "" },
                        capital(k.words())
                    )),
                    stop_of(k).unwrap(),
                    Step::Start(home_start(la, Some(*full))),
                    Step::RoomSetup { full: *full },
                ]),
            }
        }
    }
    Ok(v)
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

// ---- running a plan ---------------------------------------------------------------------------------------------

/// What a transition needs from the outside world (the real one runs the launcher; tests use a fake).
pub trait Exec {
    /// Plain words in the headset and in the status file.
    fn say(&mut self, text: &str);
    /// The launcher (`omarchyvr <args>`); Err = its last words.
    fn launcher(&mut self, args: &[String]) -> Result<(), String>;
    /// Wait until no monado-service and no SteamVR of ours runs (after a stop).
    fn wait_gone(&mut self) -> Result<(), String>;
    fn stop_passthrough(&mut self) -> Result<(), String>;
    fn start_passthrough(&mut self) -> Result<(), String>;
    fn launch_game(&mut self, appid: &str) -> Result<(), String>;
    fn room_setup(&mut self, full: bool) -> Result<(), String>;
}

fn stop_args(work: bool) -> Vec<String> {
    let k = if work { "desk" } else { "vr" };
    [k, "stop", "--grace", "30"].map(String::from).to_vec()
}

/// Run the steps in order; the first failure ends the transition. When a start fails after a stop (the headset is
/// dark), `fallback` (the home's start) runs once so the user is back in the home with the reason on the panel.
pub fn execute(steps: &[Step], x: &mut dyn Exec, fallback: Option<&[String]>) -> Result<(), String> {
    let mut stopped = false;
    for s in steps {
        let r = match s {
            Step::Say(t) => {
                x.say(t);
                Ok(())
            }
            Step::StopWork | Step::StopVr => {
                stopped = true;
                x.launcher(&stop_args(matches!(s, Step::StopWork))).and_then(|_| x.wait_gone())
            }
            Step::StopPassthrough => x.stop_passthrough(),
            Step::StartPassthrough => x.start_passthrough(),
            Step::Start(a) => x.launcher(a),
            Step::LaunchGame(id) => x.launch_game(id),
            Step::RoomSetup { full } => x.room_setup(*full),
        };
        if let Err(e) = r {
            let failed = format!("{}: {e}", step_words(s));
            // after a stop the headset is dark: back to the home (once; never a home that itself failed to start)
            if let (true, Step::Start(a), Some(fb)) = (stopped, s, fallback) {
                if a.as_slice() != fb {
                    x.say(&format!("{failed}. Back to the VR home."));
                    let _ = x.launcher(fb);
                    return Err(failed);
                }
            }
            x.say(&failed);
            return Err(failed);
        }
    }
    Ok(())
}

/// A step in plain words (for failures).
pub fn step_words(s: &Step) -> String {
    match s {
        Step::Say(t) => t.clone(),
        Step::StopWork => "Stopping Work failed".into(),
        Step::StopVr => "Stopping VR failed".into(),
        Step::StopPassthrough => "Stopping the passthrough failed".into(),
        Step::StartPassthrough => "The passthrough did not start".into(),
        Step::Start(a) if a.first().map(String::as_str) == Some("desk") => "Work did not start".into(),
        Step::Start(a) if a.iter().any(|x| x == HOME_FLAG) => "The VR home did not start".into(),
        Step::Start(a) if a.windows(2).any(|w| w == ["--runtime", "steamvr"]) => "SteamVR did not start".into(),
        Step::Start(_) => "VR did not start".into(),
        Step::LaunchGame(id) => format!("Steam did not start game {id}"),
        Step::RoomSetup { .. } => "The room setup did not finish".into(),
    }
}

/// The plan as lines (`home plan`, the app's dry run).
pub fn plan_lines(steps: &[Step]) -> Vec<String> {
    steps
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let t = match s {
                Step::Say(t) => format!("say: {t}"),
                Step::StopWork => format!("omarchyvr {} (Work's full stop), then wait until Monado is gone", stop_args(true).join(" ")),
                Step::StopVr => format!("omarchyvr {} (the session, SteamVR), then wait until Monado and SteamVR are gone", stop_args(false).join(" ")),
                Step::StopPassthrough => "stop the home's passthrough (by pid)".into(),
                Step::StartPassthrough => "start the home's passthrough again when it is not running".into(),
                Step::Start(a) => format!("omarchyvr {}", a.join(" ")),
                Step::LaunchGame(id) => format!("steam steam://rungameid/{id}"),
                Step::RoomSetup { full } => format!("omarchyvr-roommap setup {} (in the headset), wait for it", if *full { "full" } else { "fast" }),
            };
            format!("{}. {t}", i + 1)
        })
        .collect()
}

// ---- status and the one-at-a-time lock ----------------------------------------------------------------------------

/// The last transition.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Status {
    /// `running`, `done`, `failed`.
    pub state: String,
    pub target: String,
    pub text: String,
    /// Unix seconds.
    pub when: u64,
}

pub fn status_text(s: &Status) -> String {
    format!("state = {}\ntarget = {}\ntext = \"{}\"\nwhen = {}\n", s.state, s.target, s.text.replace('"', "'").replace('\n', " "), s.when)
}

pub fn parse_status(t: &str) -> Option<Status> {
    Some(Status {
        state: value(t, "state")?.to_string(),
        target: value(t, "target").unwrap_or("").to_string(),
        text: value(t, "text").unwrap_or("").to_string(),
        when: value(t, "when").and_then(|w| w.parse().ok()).unwrap_or(0),
    })
}

pub fn read_status(dir: &Path) -> Option<Status> {
    parse_status(&std::fs::read_to_string(dir.join(STATUS_FILE)).ok()?)
}

pub fn write_status(dir: &Path, s: &Status) {
    let _ = std::fs::create_dir_all(dir);
    let tmp = dir.join(format!("{STATUS_FILE}.tmp"));
    if std::fs::write(&tmp, status_text(s)).is_ok() {
        let _ = std::fs::rename(tmp, dir.join(STATUS_FILE));
    }
}

/// The transition lock (an exclusive flock while the returned file is open). None = another transition holds it.
pub fn try_lock(dir: &Path) -> Option<std::fs::File> {
    use std::os::unix::io::AsRawFd;
    std::fs::create_dir_all(dir).ok()?;
    let f = std::fs::OpenOptions::new().create(true).truncate(false).write(true).open(dir.join(LOCK_FILE)).ok()?;
    // SAFETY: flock on a file descriptor we own.
    (unsafe { libc::flock(f.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0).then_some(f)
}

/// Is a transition running now?
pub fn busy(dir: &Path) -> bool {
    try_lock(dir).is_none()
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn unix_ms() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// What a button press does now ([`go`]): refuse while busy, arm Exit, or start.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Press {
    /// Another transition runs: its text.
    Busy(String),
    /// The first Exit press: wait for the second one.
    ArmExit,
    /// Start the transition.
    Start,
}

/// The decision of a press: `busy` = the lock is held (with the running transition's text), `armed_at` = the first
/// Exit press (unix ms), `now` (unix ms).
pub fn press(t: &Target, busy: Option<String>, armed_at: Option<u64>, now: u64) -> Press {
    if let Some(b) = busy {
        return Press::Busy(b);
    }
    if *t == Target::Exit && !armed_at.is_some_and(|a| now.saturating_sub(a) <= EXIT_CONFIRM_MS) {
        return Press::ArmExit;
    }
    Press::Start
}

// ---- the overlay WayVR (the home panel in Monado sessions) -------------------------------------------------------

/// The home overlay's WayVR config (`conf.d/45-home.yaml`): the panel is the menu; shown at start in the home.
pub fn overlay_conf(open_at_start: bool) -> String {
    format!("menu_panel: {PANEL}\nmenu_open_at_start: {open_at_start}\n")
}

/// The panel's status line when it is created: the last transition's words, else a greeting.
pub fn start_status(dir: &Path) -> String {
    match read_status(dir) {
        Some(s) if s.state == "failed" => s.text,
        Some(s) if s.state == "running" => s.text,
        _ => "Pick what to do. The menu button opens and closes this panel.".into(),
    }
}

/// Write the home panel into a WayVR config home (the overlay's or the desk's).
pub fn write_panel(wayvr_home: &Path, ctx: Ctx, games: &[Game], status: &str) -> io::Result<()> {
    let gui = wayvr_home.join("config/wayvr/theme/gui");
    std::fs::create_dir_all(&gui)?;
    std::fs::write(gui.join(format!("{}.xml", ctx.panel())), panel_xml(ctx, games, status))
}

// ---- the CLI ----------------------------------------------------------------------------------------------------

fn session_opts() -> Result<StartOptions, CliError> {
    let mut o = super::parse_options(&mut Args(Vec::new()), &super::real_env)?;
    super::auto_connector(&mut o);
    Ok(o)
}

/// What runs now (real process table).
pub fn current(o: &StartOptions, d: &Deps) -> Current {
    let monado_up = !session::stop_targets(o, d.procs).ours.is_empty() || !d.procs.find_by_comm(&o.service_comm()).is_empty();
    let steamvr_up = super::steamvr::running_pid(&o.session_dir, d).is_some();
    classify(super::supervise::read_argv(&o.session_dir).as_ref(), monado_up, steamvr_up)
}

/// The real executor: the launcher, the headset words, the room setup, Steam.
pub struct RealExec<'a> {
    pub o: &'a StartOptions,
    pub d: &'a Deps<'a>,
    pub home: PathBuf,
    pub target: String,
    pub bg: Background,
}

/// The user's environment for what a transition starts: WayVR's runtime and library variables out, the user's own
/// XDG directories back (a WayVR button gives WayVR's isolated ones), no watchdog flag.
fn clean(c: &mut std::process::Command, home: &Path) {
    for k in ["XR_RUNTIME_JSON", "IPC_IGNORE_VERSION", "LD_LIBRARY_PATH", "PIMAX_WATCHDOG", "RUST_LOG"] {
        c.env_remove(k);
    }
    for (k, v) in omarchyvr_config::paths::user_xdg_fix(&|k| std::env::var(k).ok(), home) {
        match v {
            Some(v) => {
                c.env(k, v);
            }
            None => {
                c.env_remove(k);
            }
        }
    }
}

impl RealExec<'_> {
    fn status(&self, state: &str, text: &str) {
        write_status(&self.o.session_dir, &Status { state: state.into(), target: self.target.clone(), text: text.into(), when: unix_now() });
    }

    fn wayvrctl(&self) -> PathBuf {
        omarchyvr_config::paths::wayvr_bin(&self.home).with_file_name("wayvrctl")
    }

    /// The build of the running monado-service (its `/proc/PID/exe`), else the default one.
    fn runtime_json(&self) -> PathBuf {
        let pid = self.d.procs.find_by_comm(&self.o.service_comm()).into_iter().next();
        pid.and_then(|p| std::fs::read_link(format!("/proc/{p}/exe")).ok())
            .and_then(|exe| exe.ancestors().nth(5).map(Path::to_path_buf))
            .map(|b| b.join("openxr_monado-dev.json"))
            .filter(|j| j.exists())
            .unwrap_or_else(|| self.o.runtime_json())
    }
}

impl Exec for RealExec<'_> {
    fn say(&mut self, text: &str) {
        vout!("omarchyvr home: {text}");
        self.status("running", text);
        super::work::toast("VR home", text);
        for panel in [PANEL, super::desk::WORK_MENU_NAME] {
            let _ = std::process::Command::new(self.wayvrctl())
                .args(["panel-modify", panel, "home_status", "set-text", text])
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
    }

    fn launcher(&mut self, args: &[String]) -> Result<(), String> {
        vout!("omarchyvr home: omarchyvr {}", args.join(" "));
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut c = std::process::Command::new(exe);
        c.args(args).stdin(std::process::Stdio::null());
        clean(&mut c, &self.home);
        let out = c.output().map_err(|e| format!("cannot run the launcher: {e}"))?;
        let text = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
        for l in text.lines() {
            vout!("  {l}");
        }
        if out.status.success() {
            Ok(())
        } else {
            Err(text.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or("it failed").trim().to_string())
        }
    }

    fn wait_gone(&mut self) -> Result<(), String> {
        let t0 = self.d.clock.now();
        loop {
            let monado = !self.d.procs.find_by_comm(&self.o.service_comm()).is_empty();
            let steamvr = super::steamvr::running_pid(&self.o.session_dir, self.d).is_some();
            if !monado && !steamvr {
                return Ok(());
            }
            if self.d.clock.now().saturating_sub(t0) > GONE_WAIT {
                return Err(format!("{} still runs {} s after the stop", if monado { "monado-service" } else { "SteamVR" }, GONE_WAIT.as_secs()));
            }
            self.d.clock.sleep(Duration::from_millis(500));
        }
    }

    fn stop_passthrough(&mut self) -> Result<(), String> {
        let pf = self.o.session_dir.join("passthrough.pid");
        let bin = super::config::tools_dir(&self.home).join("omarchyvr-passthrough");
        let mut info = |m: &str| vout!("omarchyvr home: {m}");
        match super::desk::stop_client("passthrough", &pf, &super::config::comm_of(&bin), self.d.procs, self.d.clock, Duration::from_secs(10), &mut info) {
            super::desk::ClientStop::StillAlive => Err("the passthrough did not exit".into()),
            super::desk::ClientStop::CannotSignal(m) => Err(m),
            _ => {
                let _ = std::fs::remove_file(&pf);
                Ok(())
            }
        }
    }

    fn start_passthrough(&mut self) -> Result<(), String> {
        if self.bg != Background::Passthrough {
            return Ok(());
        }
        let pf = self.o.session_dir.join("passthrough.pid");
        let bin = super::config::tools_dir(&self.home).join("omarchyvr-passthrough");
        if session::read_pid(&pf).is_some_and(|p| self.d.procs.is_alive(p)) {
            return Ok(());
        }
        let spec = home_passthrough_spec(&bin, &self.runtime_json(), self.o.session_dir.join("broker.pid").exists(), &self.o.session_dir);
        let pid = super::desk::Spawner::spawn(&super::desk::RealSpawner, &spec).map_err(|e| format!("cannot start {}: {e}", bin.display()))?;
        let _ = std::fs::write(&pf, format!("{pid}\n"));
        Ok(())
    }

    fn launch_game(&mut self, appid: &str) -> Result<(), String> {
        use std::os::unix::process::CommandExt;
        let url = format!("steam://rungameid/{appid}");
        vout!("omarchyvr home: steam {url}");
        let mut c = std::process::Command::new("steam");
        c.arg(&url).stdin(std::process::Stdio::null()).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
        clean(&mut c, &self.home);
        // SAFETY: setsid in the child before exec (Steam must not belong to this transition).
        unsafe {
            c.pre_exec(|| {
                libc::setsid();
                Ok(())
            });
        }
        c.spawn().map(|_| ()).map_err(|e| format!("cannot run steam: {e}"))
    }

    fn room_setup(&mut self, full: bool) -> Result<(), String> {
        let room = super::room::load(&self.home).room;
        let bin = super::room::roommap_bin(&self.home);
        let log = std::fs::OpenOptions::new().create(true).append(true).open(self.o.session_dir.join("room-setup.log")).map_err(|e| e.to_string())?;
        let mut c = std::process::Command::new(&bin);
        c.args(["setup", if full { "full" } else { "fast" }, "--name", &room, "--label", &room]);
        clean(&mut c, &self.home);
        c.env("XR_RUNTIME_JSON", self.runtime_json()).env("IPC_IGNORE_VERSION", "1");
        c.stdin(std::process::Stdio::null()).stdout(log.try_clone().map_err(|e| e.to_string())?).stderr(log);
        let st = c.status().map_err(|e| format!("cannot run {}: {e}", bin.display()))?;
        if st.success() {
            self.say(&format!("Room \"{room}\" is set up. The menu button opens the VR home."));
            Ok(())
        } else {
            Err(format!("see {}", self.o.session_dir.join("room-setup.log").display()))
        }
    }
}

/// The home's passthrough (the main OpenXR app behind the panel), on the broker with 6DoF.
pub fn home_passthrough_spec(bin: &Path, runtime_json: &Path, broker: bool, dir: &Path) -> super::desk::ProcSpec {
    omarchyvr_components::components::passthrough::spec(bin, &[], runtime_json, broker, dir)
}

/// The minimal WayVR of a Monado game session (`vr start`): the VR home panel when this is the home (`--home`, shown
/// at start) or when the menu button opens the home over games (`pi_menu`, hidden until pressed), plus the wrist
/// readout when `--perf` asked for it. None = neither.
pub fn overlay_options(home: &Path, home_flag: bool, perf: Option<&super::perf::PerfOptions>, o: &StartOptions) -> Option<super::perf::OverlayOptions> {
    let set = load(home);
    let panel_wanted = home_flag || set.pi_menu;
    if !panel_wanted && perf.is_none() {
        return None;
    }
    let wayvr_bin = perf.map(|p| p.wayvr_bin.clone()).unwrap_or_else(|| omarchyvr_config::paths::wayvr_bin(home));
    let tools = super::config::tools_dir(home);
    let broker = o.extra_env.iter().any(|(k, v)| k == "PIMAX_CAM_SOURCE" && v == "broker");
    Some(super::perf::OverlayOptions {
        wayvrctl_bin: perf.map(|p| p.wayvrctl_bin.clone()).unwrap_or_else(|| wayvr_bin.with_file_name("wayvrctl")),
        wayvr_home: omarchyvr_config::paths::wayvr_home_perf(home),
        panel: panel_wanted.then(|| (panel_xml(Ctx::Home, &games(home, &super::real_env), &start_status(&o.session_dir)), home_flag)),
        tools_dir: panel_wanted.then(|| tools.clone()),
        passthrough: (home_flag && set.background == Background::Passthrough)
            .then(|| home_passthrough_spec(&tools.join("omarchyvr-passthrough"), &o.runtime_json(), broker, &o.session_dir)),
        sky: home_flag && set.background == Background::Sky,
        wayvr_bin,
    })
}

/// `vr stop`: the home's passthrough (when its pid file exists), by pid, SIGINT, 10 s.
pub fn stop_home_passthrough(dir: &Path, home: &Path, d: &Deps) {
    let pf = dir.join("passthrough.pid");
    if !pf.exists() {
        return;
    }
    let bin = super::config::tools_dir(home).join("omarchyvr-passthrough");
    let mut info = |m: &str| vout!("omarchyvr vr: {m}");
    match super::desk::stop_client("passthrough", &pf, &super::config::comm_of(&bin), d.procs, d.clock, Duration::from_secs(10), &mut info) {
        super::desk::ClientStop::StillAlive => verr!("omarchyvr vr: WARNING: passthrough is still running after SIGINT"),
        super::desk::ClientStop::CannotSignal(m) => verr!("omarchyvr vr: {m}"),
        _ => {
            let _ = std::fs::remove_file(&pf);
        }
    }
}

/// `home go`: start the transition detached (`omarchyvr home run ...` in a session of its own, log
/// `<session dir>/home.log`).
fn spawn_run(dir: &Path, t: &Target, home: &Path) -> io::Result<u32> {
    use std::os::unix::process::CommandExt;
    let exe = std::env::current_exe()?;
    std::fs::create_dir_all(dir)?;
    let log = std::fs::OpenOptions::new().create(true).append(true).open(dir.join("home.log"))?;
    let log2 = log.try_clone()?;
    let mut c = std::process::Command::new(exe);
    c.arg("home").arg("run").args(t.words().split_whitespace());
    c.stdin(std::process::Stdio::null()).stdout(log).stderr(log2);
    clean(&mut c, home);
    // SAFETY: setsid in the child before exec (the transition must outlive WayVR, which it may stop).
    unsafe {
        c.pre_exec(|| {
            libc::setsid();
            Ok(())
        });
    }
    Ok(c.spawn()?.id())
}

fn on_off(v: Option<&str>) -> Result<bool, CliError> {
    match v {
        Some("on") => Ok(true),
        Some("off") => Ok(false),
        _ => Err(CliError::usage("on or off")),
    }
}

/// `omarchyvr home ...`
pub fn run(mut a: Args) -> CliResult {
    // a WayVR button runs us with WayVR's isolated XDG directories: the user's own first (config, session dir)
    omarchyvr_config::paths::restore_user_xdg();
    let Some(first) = a.0.first().cloned() else { return Err(CliError::usage(USAGE)) };
    a.0.remove(0);
    let home = PathBuf::from(super::real_env("HOME").unwrap_or_default());
    match first.as_str() {
        "-h" | "--help" | "help" => {
            vout!("{USAGE}");
            Ok(())
        }
        "set" => {
            let mut s = load(&home);
            let w: Vec<&str> = a.0.iter().map(String::as_str).collect();
            match w.as_slice() {
                ["start-in-home", v] => s.start_in_home = on_off(Some(v))?,
                ["pi-menu", v] => s.pi_menu = on_off(Some(v))?,
                ["background", v] => s.background = Background::parse(v).ok_or_else(|| CliError::usage("background passthrough|sky"))?,
                _ => return Err(CliError::usage(USAGE)),
            }
            save(&home, &s).map_err(|e| CliError::new(format!("cannot save {}: {e}", settings_path(&home).display())))?;
            vout!("{}", settings_text(&s).lines().skip(1).collect::<Vec<_>>().join("\n"));
            Ok(())
        }
        "games" => {
            let g = games(&home, &super::real_env);
            if g.is_empty() {
                vout!("No installed Steam VR games found (Steam's config/steamapps.vrmanifest and game-runtime.json).");
            }
            for g in g {
                vout!("{}  {}  ({})", g.appid, g.name, g.runtime_words());
            }
            Ok(())
        }
        "panel" => {
            let ctx = if a.flag("--work") { Ctx::Work } else { Ctx::Home };
            vout!("{}", panel_xml(ctx, &games(&home, &super::real_env), "Pick what to do."));
            Ok(())
        }
        "status" | "plan" | "go" | "run" => {
            let o = session_opts()?;
            let s = load(&home);
            let la = load_launch(&home);
            let target = if first == "status" { None } else { Some(Target::parse(&a.0).map_err(CliError::usage)?) };
            let gl = games(&home, &super::real_env);
            let game_of = |t: &Option<Target>| -> Option<Game> {
                let Some(Target::Game(id)) = t else { return None };
                Some(gl.iter().find(|g| &g.appid == id).cloned().unwrap_or_else(|| Game {
                    appid: id.clone(),
                    name: gameruntime::steam_app_name(&home, id).unwrap_or_else(|| format!("Steam app {id}")),
                    runtime: gameruntime::resolve(&gameruntime::Store::load(&home), Some(id), super::steamvr::games_runtime(&super::real_env)),
                }))
            };
            let game = game_of(&target);
            super::with_deps(&o, &|| false, |d| -> CliResult {
                let cur = current(&o, d);
                match (first.as_str(), &target) {
                    ("status", _) => {
                        vout!("now: {}", cur.kind.words());
                        vout!("settings: start in the VR home {}, menu button opens it in games {}, background {}", s.start_in_home, s.pi_menu, s.background.name());
                        vout!("transition: {}", if busy(&o.session_dir) { "running" } else { "none running" });
                        if let Some(st) = read_status(&o.session_dir) {
                            vout!("last: {} {} ({}): {}", st.target, st.state, st.when, st.text);
                        }
                        for g in &gl {
                            vout!("game: {} {} ({})", g.appid, g.name, g.runtime_words());
                        }
                        Ok(())
                    }
                    ("plan", Some(t)) => {
                        vout!("now: {}", cur.kind.words());
                        for l in plan_lines(&plan(&cur, t, game.as_ref(), &la, s.background).map_err(CliError::new)?) {
                            vout!("{l}");
                        }
                        Ok(())
                    }
                    ("go", Some(t)) => {
                        let lock_busy = busy(&o.session_dir).then(|| read_status(&o.session_dir).map(|s| s.text).unwrap_or_else(|| "another change".into()));
                        let armed = std::fs::read_to_string(o.session_dir.join(EXIT_ARMED)).ok().and_then(|x| x.trim().parse::<u64>().ok());
                        match press(t, lock_busy, armed, unix_ms()) {
                            Press::Busy(b) => {
                                super::work::toast("One thing at a time", &format!("Still busy: {b}"));
                                Err(CliError::new(format!("a transition is still running: {b}")))
                            }
                            Press::ArmExit => {
                                let _ = std::fs::write(o.session_dir.join(EXIT_ARMED), format!("{}\n", unix_ms()));
                                super::work::toast("Exit VR?", "Press Exit VR again to stop everything.");
                                vout!("omarchyvr home: Exit VR: press it again within 6 s");
                                Ok(())
                            }
                            Press::Start => {
                                let _ = std::fs::remove_file(o.session_dir.join(EXIT_ARMED));
                                // a plan that cannot run is said at once, in the headset, without starting anything
                                if let Err(e) = plan(&cur, t, game.as_ref(), &la, s.background) {
                                    super::work::toast("VR home", &e);
                                    return Err(CliError::new(e));
                                }
                                let pid = spawn_run(&o.session_dir, t, &home).map_err(|e| CliError::new(format!("could not start the transition: {e}")))?;
                                vout!("omarchyvr home: {} (pid {pid}, log {})", t.words(), o.session_dir.join("home.log").display());
                                Ok(())
                            }
                        }
                    }
                    ("run", Some(t)) => {
                        let Some(_lock) = try_lock(&o.session_dir) else {
                            return Err(CliError::new("another transition is running"));
                        };
                        let steps = plan(&cur, t, game.as_ref(), &la, s.background).map_err(CliError::new)?;
                        let mut x = RealExec { o: &o, d, home: home.clone(), target: t.words(), bg: s.background };
                        x.status("running", &format!("{} ...", t.words()));
                        let fb = home_start(&la, None);
                        let fallback = (!matches!(t, Target::Exit | Target::Home)).then_some(fb.as_slice());
                        match execute(&steps, &mut x, fallback) {
                            Ok(()) => {
                                x.status("done", &format!("{}: done", t.words()));
                                Ok(())
                            }
                            Err(e) => {
                                x.status("failed", &e);
                                Err(CliError::new(e))
                            }
                        }
                    }
                    _ => Err(CliError::usage(USAGE)),
                }
            })
        }
        other => Err(CliError::usage(format!("unknown home command {other}\n{USAGE}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn la() -> LaunchArgs {
        let v = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        LaunchArgs {
            work: v("desk start --fps 90 --eye --6dof --ctrl-track --voice --anchor --force"),
            play: v("vr start --fps 90 --6dof --ctrl-track --perf --no-light --game 546560 --force"),
        }
    }

    fn cur(kind: Kind, room: bool) -> Current {
        Current { kind, sixdof: room, anchor: room, ctrl_track: room }
    }

    fn game(id: &str, rt: Games) -> Game {
        Game { appid: id.into(), name: format!("G{id}"), runtime: rt }
    }

    fn starts(steps: &[Step]) -> Vec<String> {
        steps.iter().filter_map(|s| if let Step::Start(a) = s { Some(a.join(" ")) } else { None }).collect()
    }

    // ---- settings and files

    #[test]
    fn settings_default_on_and_round_trip() {
        let d = Settings::default();
        assert!(d.start_in_home && d.pi_menu && d.background == Background::Passthrough, "VR home and the menu button on by default");
        assert_eq!(parse_settings(""), d);
        let s = Settings { start_in_home: false, pi_menu: false, background: Background::Sky };
        assert_eq!(parse_settings(&settings_text(&s)), s);
        assert_eq!(parse_settings("start_in_home = maybe\nbackground = \"moon\"\n"), d, "bad values keep the default");
        let h = crate::vr::test_dir("home-settings");
        assert_eq!(load(&h), d, "no file: defaults");
        save(&h, &s).unwrap();
        assert_eq!(load(&h), s);
    }

    #[test]
    fn launch_args_round_trip_and_bad_lists_fall_back() {
        let l = la();
        assert_eq!(parse_launch(&launch_text(&l)), l);
        let d = LaunchArgs::default();
        assert_eq!(parse_launch("not json"), d);
        assert_eq!(parse_launch(r#"{"work": ["rm", "-rf"], "play": ["vr", "start", "--6dof"]}"#), LaunchArgs { work: d.work.clone(), play: vec!["vr".into(), "start".into(), "--6dof".into()] });
        assert_eq!(&d.work[..2], ["desk", "start"]);
        assert_eq!(&d.play[..2], ["vr", "start"]);
    }

    // ---- games

    #[test]
    fn vrmanifest_lists_steam_vr_apps_once_without_steamvr_itself() {
        let t = r#"{"applications": [
            {"app_key": "steam.app.546560", "strings": {"en_us": {"name": "Half-Life: Alyx"}}},
            {"app_key": "steam.app.3189340", "strings": {"en_us": {"name": "Dungeons of Eternity"}}},
            {"app_key": "steam.app.3189340", "strings": {"en_us": {"name": "Dungeons of Eternity"}}},
            {"app_key": "steam.app.250820", "strings": {"en_us": {"name": "SteamVR"}}},
            {"app_key": "system.generated.foo"}
        ]}"#;
        assert_eq!(parse_vrmanifest(t), [("546560".to_string(), "Half-Life: Alyx".to_string()), ("3189340".into(), "Dungeons of Eternity".into())]);
        assert!(parse_vrmanifest("garbage").is_empty());
    }

    #[test]
    fn library_folders_come_from_the_vdf_default_first() {
        let vdf = "\"libraryfolders\"\n{\n\t\"0\"\n\t{\n\t\t\"path\"\t\t\"/home/u/.local/share/Steam\"\n\t}\n\t\"1\"\n\t{\n\t\t\"path\"\t\t\"/mnt/games/SteamLibrary\"\n\t}\n}\n";
        let root = Path::new("/home/u/.local/share/Steam");
        assert_eq!(library_dirs(Some(vdf), root), [root.to_path_buf(), PathBuf::from("/mnt/games/SteamLibrary")]);
        assert_eq!(library_dirs(None, root), [root.to_path_buf()]);
    }

    #[test]
    fn games_are_installed_ones_with_their_own_runtime_sorted_and_capped() {
        let vr = vec![("3189340".to_string(), "Dungeons of Eternity".to_string()), ("546560".into(), "Half-Life: Alyx".into()), ("9".into(), "Not installed".into())];
        let mut store = gameruntime::Store::default();
        store.set("658920", "Half-Life 2: VR Mod", Games::SteamVr, "user", gameruntime::Source::User);
        let installed = |id: &str| id != "9";
        let g = games_from(&vr, &store, Games::Monado, &installed, 6);
        let names: Vec<(&str, Games)> = g.iter().map(|g| (g.name.as_str(), g.runtime)).collect();
        // Alyx is in the built-in list (xrizer crashes) -> SteamVR; a user entry is listed even without a vrmanifest line
        assert_eq!(names, [("Dungeons of Eternity", Games::Monado), ("Half-Life 2: VR Mod", Games::SteamVr), ("Half-Life: Alyx", Games::SteamVr)]);
        assert_eq!(g[0].runtime_words(), "xrizer");
        // the global choice applies to an unlisted game
        assert_eq!(games_from(&vr, &store, Games::SteamVr, &installed, 6)[0].runtime, Games::SteamVr);
        assert_eq!(games_from(&vr, &store, Games::Monado, &installed, 2).len(), 2);
    }

    // ---- the panel

    #[test]
    fn the_home_panel_has_every_entry_with_its_command() {
        let x = panel_xml(Ctx::Home, &[game("1", Games::Monado), game("2", Games::SteamVr)], "Pick \"what\" <to> do");
        for want in [
            "text=\"VR home\"",
            "_press=\"::ShellExec omarchyvr home go work\"",
            "_press=\"::ShellExec omarchyvr home go steamvr\"",
            "text=\"G1 (xrizer)\" _press=\"::ShellExec omarchyvr home go game 1\" _press2=\"::OverlayToggle vr_home\"",
            "text=\"G2 (SteamVR)\" _press=\"::ShellExec omarchyvr home go game 2\"",
            "_press=\"::ShellExec omarchyvr home go room-quick\" _press2=\"::OverlayToggle vr_home\"",
            "_press=\"::ShellExec omarchyvr home go room-full\"",
            "_press=\"::PlayspaceRecenter\"",
            "_press=\"::OverlayToggle vr_home\"",
            "text=\"Exit VR\" text_color=\"on_danger\" color=\"danger\"",
            "_press=\"::ShellExec omarchyvr home go exit\"",
            "id=\"home_status\" text=\"Pick &quot;what&quot; &lt;to&gt; do\"",
        ] {
            assert!(x.contains(want), "missing {want}\n{x}");
        }
        assert_eq!(x.matches("<Button").count(), 10, "{x}");
        assert!(panel_xml(Ctx::Home, &[], "").contains("No VR games installed"));
    }

    #[test]
    fn the_work_menu_is_the_same_panel_with_works_own_buttons() {
        let x = panel_xml(Ctx::Work, &[game("1", Games::Monado)], "");
        for want in [
            "text=\"Work\"",
            "_press=\"::ShellExec omarchyvr home go home\"",
            "_press=\"::ShellExec omarchyvr desk exit\"",
            "_press=\"::OverlayToggle work_menu\"",
            "id=\"btn_recenter\"",
            "_press=\"::DashToggle\"",
            "_press2=\"::OverlayToggle work_menu\"",
        ] {
            assert!(x.contains(want), "missing {want}\n{x}");
        }
        assert!(!x.contains("home go work"), "no Work button inside Work");
        assert!(!x.contains("home go exit"), "Work's exit is Exit Work");
    }

    #[test]
    fn the_overlay_conf_names_the_panel_and_opens_it_only_in_the_home() {
        assert_eq!(overlay_conf(true), "menu_panel: vr_home\nmenu_open_at_start: true\n");
        assert_eq!(overlay_conf(false), "menu_panel: vr_home\nmenu_open_at_start: false\n");
    }

    // ---- what runs

    #[test]
    fn the_running_session_is_read_from_the_record_and_the_processes() {
        let rec = |k: &str, a: &str| (k.to_string(), a.split_whitespace().map(String::from).collect::<Vec<_>>());
        assert_eq!(classify(None, false, false).kind, Kind::Nothing);
        assert_eq!(classify(Some(&rec("vr", "--6dof --home")), false, false).kind, Kind::Nothing, "a record without Monado is history");
        let h = classify(Some(&rec("vr", "--fps 90 --6dof --ctrl-track --anchor --home --runtime monado")), true, false);
        assert_eq!(h, Current { kind: Kind::Home, sixdof: true, anchor: true, ctrl_track: true });
        assert_eq!(classify(Some(&rec("vr", "--6dof")), true, false), Current { kind: Kind::Play, sixdof: true, anchor: false, ctrl_track: false });
        assert_eq!(classify(Some(&rec("desk", "--6dof --anchor")), true, false).kind, Kind::Work);
        assert_eq!(classify(Some(&rec("vr", "--6dof --runtime steamvr")), false, true).kind, Kind::SteamVr);
        assert!(!classify(Some(&rec("vr", "--anchor")), true, false).anchor, "the anchor needs 6DoF");
    }

    // ---- plans

    #[test]
    fn home_is_always_monado_with_the_anchor_and_without_what_the_app_decides_per_start() {
        let h = home_start(&la(), None).join(" ");
        assert_eq!(h, "vr start --fps 90 --6dof --ctrl-track --anchor --home --runtime monado");
        let no6 = LaunchArgs { play: vec!["vr".into(), "start".into(), "--fps".into(), "72".into()], ..la() };
        assert_eq!(home_start(&no6, None).join(" "), "vr start --fps 72 --home --runtime monado", "no 6DoF: no anchor");
        assert_eq!(home_start(&no6, Some(true)).join(" "), "vr start --fps 72 --6dof --ctrl-track --anchor --home --runtime monado", "the full room setup brings what it needs");
        assert_eq!(work_start(&la()).join(" "), "desk start --fps 90 --eye --6dof --ctrl-track --voice --anchor", "never --force from the headset");
        assert_eq!(play_start(&la(), Games::SteamVr, Some("7")).join(" "), "vr start --fps 90 --6dof --ctrl-track --runtime steamvr --game 7");
    }

    #[test]
    fn every_switch_is_the_full_stop_of_what_runs_then_one_start() {
        let l = la();
        let bg = Background::Passthrough;
        // home -> Work: the home's full stop (vr stop), then Work with the app's switches
        let p = plan(&cur(Kind::Home, true), &Target::Work, None, &l, bg).unwrap();
        assert!(matches!(&p[0], Step::Say(t) if t.contains("The VR home closes first")), "{p:?}");
        assert_eq!(p[1], Step::StopVr);
        assert_eq!(starts(&p), [work_start(&l).join(" ")]);
        // Work -> home: Work's full stop (screens, voice, AI), then the home
        let p = plan(&cur(Kind::Work, true), &Target::Home, None, &l, bg).unwrap();
        assert_eq!(p[1], Step::StopWork);
        assert_eq!(starts(&p), [home_start(&l, None).join(" ")]);
        // home -> SteamVR
        let p = plan(&cur(Kind::Home, true), &Target::SteamVr, None, &l, bg).unwrap();
        assert_eq!(p[1], Step::StopVr);
        assert_eq!(starts(&p), [play_start(&l, Games::SteamVr, None).join(" ")]);
        // SteamVR -> home and -> Work: SteamVR's stop (vr stop)
        assert_eq!(plan(&cur(Kind::SteamVr, true), &Target::Home, None, &l, bg).unwrap()[1], Step::StopVr);
        assert_eq!(plan(&cur(Kind::SteamVr, true), &Target::Work, None, &l, bg).unwrap()[1], Step::StopVr);
        // nothing runs: just the start
        assert_eq!(plan(&cur(Kind::Nothing, false), &Target::Home, None, &l, bg).unwrap(), [Step::Start(home_start(&l, None))]);
        // exactly one stop and at most one start per switch
        for k in [Kind::Home, Kind::Play, Kind::SteamVr, Kind::Work] {
            for t in [Target::Home, Target::Work, Target::SteamVr, Target::Exit] {
                if let Ok(p) = plan(&cur(k, true), &t, None, &l, bg) {
                    let stops = p.iter().filter(|s| matches!(s, Step::StopVr | Step::StopWork)).count();
                    assert!(stops <= 1 && starts(&p).len() <= 1, "{k:?} -> {t:?}: {p:?}");
                    if stops == 1 {
                        assert!(matches!(p[0], Step::Say(_)), "{k:?} -> {t:?}: the headset hears it before it goes dark");
                    }
                }
            }
        }
    }

    #[test]
    fn already_there_is_said_not_restarted() {
        let l = la();
        let bg = Background::Passthrough;
        assert_eq!(plan(&cur(Kind::Work, true), &Target::Work, None, &l, bg).unwrap_err(), "Work is already running.");
        assert_eq!(plan(&cur(Kind::SteamVr, true), &Target::SteamVr, None, &l, bg).unwrap_err(), "SteamVR is already running.");
        assert_eq!(plan(&cur(Kind::Nothing, true), &Target::Exit, None, &l, bg).unwrap_err(), "Nothing of VR is running.");
        assert_eq!(plan(&cur(Kind::Home, true), &Target::Home, None, &l, bg).unwrap(), [Step::Say("You are in the VR home.".into()), Step::StartPassthrough]);
        assert_eq!(plan(&cur(Kind::Home, true), &Target::Home, None, &l, Background::Sky).unwrap().len(), 1);
    }

    #[test]
    fn games_start_with_their_own_runtime() {
        let l = la();
        let bg = Background::Passthrough;
        let x = game("10", Games::Monado);
        let s = game("20", Games::SteamVr);
        // an xrizer game in the home: no restart, the passthrough makes way, Steam starts it
        assert_eq!(&plan(&cur(Kind::Home, true), &Target::Game("10".into()), Some(&x), &l, bg).unwrap()[1..], [Step::StopPassthrough, Step::LaunchGame("10".into())]);
        // a SteamVR game from the home: the home stops, SteamVR starts for this game, then the game
        let p = plan(&cur(Kind::Home, true), &Target::Game("20".into()), Some(&s), &l, bg).unwrap();
        assert_eq!(p[1], Step::StopVr);
        assert_eq!(starts(&p), [play_start(&l, Games::SteamVr, Some("20")).join(" ")]);
        assert_eq!(p.last(), Some(&Step::LaunchGame("20".into())));
        // a SteamVR game while SteamVR runs: just the game
        assert_eq!(plan(&cur(Kind::SteamVr, true), &Target::Game("20".into()), Some(&s), &l, bg).unwrap()[1..], [Step::LaunchGame("20".into())]);
        // an xrizer game from Work: Work's full stop first, then a Monado game session for it
        let p = plan(&cur(Kind::Work, true), &Target::Game("10".into()), Some(&x), &l, bg).unwrap();
        assert_eq!(p[1], Step::StopWork);
        assert_eq!(starts(&p), [play_start(&l, Games::Monado, Some("10")).join(" ")]);
        // an xrizer game from SteamVR: SteamVR stops
        assert_eq!(plan(&cur(Kind::SteamVr, true), &Target::Game("10".into()), Some(&x), &l, bg).unwrap()[1], Step::StopVr);
    }

    #[test]
    fn room_setup_runs_in_place_when_the_session_has_what_it_needs() {
        let l = la();
        let bg = Background::Passthrough;
        let q = Target::Room { full: false };
        let f = Target::Room { full: true };
        // the home and Work with 6DoF and the anchor: right here
        for k in [Kind::Home, Kind::Work] {
            assert_eq!(&plan(&cur(k, true), &q, None, &l, bg).unwrap()[1..], [Step::RoomSetup { full: false }], "{k:?}");
        }
        // the full one also needs controller tracking: a home without it restarts into one with it
        let no_ct = Current { kind: Kind::Home, sixdof: true, anchor: true, ctrl_track: false };
        assert_eq!(&plan(&no_ct, &q, None, &l, bg).unwrap()[1..], [Step::RoomSetup { full: false }]);
        let p = plan(&no_ct, &f, None, &l, bg).unwrap();
        assert_eq!(p[1], Step::StopVr);
        assert_eq!(starts(&p), [home_start(&l, Some(true)).join(" ")]);
        assert!(matches!(&p[0], Step::Say(t) if t.contains("controller tracking")));
        // SteamVR or a plain game session: the home with what the setup needs, then the setup
        for k in [Kind::SteamVr, Kind::Play] {
            let p = plan(&cur(k, false), &q, None, &l, bg).unwrap();
            assert_eq!(p[1], Step::StopVr);
            assert_eq!(p.last(), Some(&Step::RoomSetup { full: false }));
        }
        // nothing runs: the home starts with it
        assert_eq!(plan(&cur(Kind::Nothing, false), &f, None, &l, bg).unwrap(), [Step::Start(home_start(&l, Some(true))), Step::RoomSetup { full: true }]);
    }

    #[test]
    fn exit_stops_what_runs() {
        let l = la();
        assert_eq!(plan(&cur(Kind::Work, true), &Target::Exit, None, &l, Background::Sky).unwrap()[1], Step::StopWork);
        for k in [Kind::Home, Kind::Play, Kind::SteamVr] {
            let p = plan(&cur(k, true), &Target::Exit, None, &l, Background::Sky).unwrap();
            assert_eq!(p.len(), 2);
            assert_eq!(p[1], Step::StopVr);
        }
    }

    #[test]
    fn targets_parse_from_the_button_words() {
        let w = |s: &str| s.split_whitespace().map(String::from).collect::<Vec<_>>();
        assert_eq!(Target::parse(&w("game 546560")).unwrap(), Target::Game("546560".into()));
        assert_eq!(Target::parse(&w("room-fast")).unwrap(), Target::Room { full: false });
        assert!(Target::parse(&w("game 5;rm")).is_err());
        assert!(Target::parse(&w("reboot")).is_err());
        for t in [Target::Home, Target::Work, Target::SteamVr, Target::Game("1".into()), Target::Room { full: false }, Target::Room { full: true }, Target::Exit] {
            assert_eq!(Target::parse(&w(&t.words())).unwrap(), t);
        }
    }

    // ---- running a plan (fakes)

    #[derive(Default)]
    struct Fake {
        log: Vec<String>,
        fail: Option<&'static str>,
    }

    impl Exec for Fake {
        fn say(&mut self, t: &str) {
            self.log.push(format!("say {t}"));
        }
        fn launcher(&mut self, a: &[String]) -> Result<(), String> {
            let l = a.join(" ");
            self.log.push(format!("run {l}"));
            match self.fail {
                Some(f) if l.contains(f) => Err("Steam must restart for VR".into()),
                _ => Ok(()),
            }
        }
        fn wait_gone(&mut self) -> Result<(), String> {
            self.log.push("wait".into());
            Ok(())
        }
        fn stop_passthrough(&mut self) -> Result<(), String> {
            self.log.push("stop passthrough".into());
            Ok(())
        }
        fn start_passthrough(&mut self) -> Result<(), String> {
            self.log.push("start passthrough".into());
            Ok(())
        }
        fn launch_game(&mut self, id: &str) -> Result<(), String> {
            self.log.push(format!("game {id}"));
            Ok(())
        }
        fn room_setup(&mut self, full: bool) -> Result<(), String> {
            self.log.push(format!("room {full}"));
            Ok(())
        }
    }

    #[test]
    fn a_switch_runs_the_full_stop_waits_then_starts() {
        let l = la();
        let p = plan(&cur(Kind::Home, true), &Target::Work, None, &l, Background::Passthrough).unwrap();
        let mut f = Fake::default();
        execute(&p, &mut f, Some(&home_start(&l, None))).unwrap();
        assert_eq!(f.log[1..], ["run vr stop --grace 30", "wait", "run desk start --fps 90 --eye --6dof --ctrl-track --voice --anchor"]);
    }

    #[test]
    fn a_start_that_fails_after_the_stop_goes_back_to_the_home_with_the_reason() {
        let l = la();
        let p = plan(&cur(Kind::Home, true), &Target::SteamVr, None, &l, Background::Passthrough).unwrap();
        let mut f = Fake { fail: Some("--runtime steamvr"), ..Fake::default() };
        let e = execute(&p, &mut f, Some(&home_start(&l, None))).unwrap_err();
        assert_eq!(e, "SteamVR did not start: Steam must restart for VR");
        assert_eq!(f.log.iter().filter(|x| x.starts_with("run vr start") && x.contains("--home")).count(), 1, "{:?}", f.log);
        assert!(f.log.iter().any(|x| x == "say SteamVR did not start: Steam must restart for VR. Back to the VR home."), "{:?}", f.log);
        // a failing home start is not retried in a loop
        let p = plan(&cur(Kind::Work, true), &Target::Home, None, &l, Background::Passthrough).unwrap();
        let mut f = Fake { fail: Some("--home"), ..Fake::default() };
        assert!(execute(&p, &mut f, Some(&home_start(&l, None))).is_err());
        assert_eq!(f.log.iter().filter(|x| x.contains("--home")).count(), 1);
        // nothing stopped yet (an xrizer game in the home): no fallback start
        let x = game("10", Games::Monado);
        let p = plan(&cur(Kind::Home, true), &Target::Game("10".into()), Some(&x), &l, Background::Passthrough).unwrap();
        let mut f = Fake::default();
        execute(&p, &mut f, Some(&home_start(&l, None))).unwrap();
        assert_eq!(f.log[1..], ["stop passthrough", "game 10"]);
    }

    #[test]
    fn one_transition_at_a_time_and_exit_asks_twice() {
        let d = crate::vr::test_dir("home-lock");
        let held = try_lock(&d).expect("free");
        assert!(busy(&d), "held by the running transition");
        assert!(try_lock(&d).is_none());
        drop(held);
        assert!(!busy(&d), "released when the transition ends");
        // a press while busy is refused with the running transition's words
        assert_eq!(press(&Target::Work, Some("Starting Work".into()), None, 1000), Press::Busy("Starting Work".into()));
        // Exit: the first press arms, a second within 6 s starts, a late one arms again
        assert_eq!(press(&Target::Exit, None, None, 1000), Press::ArmExit);
        assert_eq!(press(&Target::Exit, None, Some(1000), 6000), Press::Start);
        assert_eq!(press(&Target::Exit, None, Some(1000), 8000), Press::ArmExit);
        assert_eq!(press(&Target::Home, None, None, 1000), Press::Start);
    }

    #[test]
    fn status_round_trips_and_feeds_the_next_panel() {
        let d = crate::vr::test_dir("home-status");
        assert!(start_status(&d).contains("menu button"));
        let s = Status { state: "failed".into(), target: "steamvr".into(), text: "SteamVR did not start: \"x\"".into(), when: 5 };
        write_status(&d, &s);
        let r = read_status(&d).unwrap();
        assert_eq!((r.state.as_str(), r.target.as_str(), r.when), ("failed", "steamvr", 5));
        assert_eq!(start_status(&d), "SteamVR did not start: 'x'");
    }
}
