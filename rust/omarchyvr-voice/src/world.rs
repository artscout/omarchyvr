//! The VR desk as STATE (docs/VOICE-STATE-TESTS.md): what the voice AI must know and what the user can change,
//! a simulator that applies the real actions to it, and a read-only snapshot of the live system.
//!
//! * [`World`]: screens 1-6 (present, shown, workspace, size, place), workspaces, apps (kind, class, title, cwd,
//!   workspace, fullscreen, browser tabs), focus, gaze, passthrough / backdrop, captions, volume, music, video,
//!   recording, the session. Serialisable (JSON), so a case or a planner can carry it.
//! * [`Sim`]: an [`act::Runner`] over a World. It answers the READS the voice code makes (`hyprctl -j clients |
//!   activewindow | monitors | workspaces`, `wayvrctl gaze`, `wl-paste`) from the World and applies the WRITES
//!   (`hyprctl dispatch hl.dsp...`, `wayvrctl screen-nudge | reset-screens | skybox | ...`, `omarchyvr desk screens
//!   show|more|add|hide`, `omarchyvr desk preset`, `omarchyvr desk stop`, `omarchyvr record`, the passthrough socket, the
//!   launchers, `wtype`, volume and media commands) to it. Nothing real runs. A command it does not know is recorded
//!   in [`Sim::unmodelled`] (the case then says so instead of passing by accident).
//! * [`apply`]: one tool (a catalogue intent id, or `focus_window` / `move_window` / `close_window`) with its inputs,
//!   run through the REAL [`act::Actor::execute`] against a [`Sim`]: the end state follows the real code by
//!   construction. A plan's dry run folds `apply` over its steps.
//! * [`snapshot`]: the World from the live desktop, read-only.
//!
//! Exactness: each primitive mirrors its implementation, cited at the function. Places where the real behaviour is
//! approximated are marked `APPROX:` (the screens watcher's re-sorting of apps by kind, Hyprland's directional focus
//! across monitors, screen placement after a reset).

use crate::act::{self, Runner};
use crate::catalogue::Catalogue;
use crate::command::Match;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// Screen slots by number (docs/SCREENS.md): 1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right.
pub const SLOTS: [&str; 6] = ["main", "left", "right", "up", "up-left", "up-right"];
/// The Hyprland outputs of screens 2-6 (`omarchyvr desk start --screens N`, omarchyvr-tools vr/screens.rs).
pub const OUTPUTS: [&str; 6] = ["", "PIMAX-LEFT", "PIMAX-RIGHT", "PIMAX-UP", "PIMAX-UPLEFT", "PIMAX-UPRIGHT"];
/// Monitor positions in the Hyprland layout (screens.rs `wanted`: left of, right of and above the main output).
const MON_XY: [(i64, i64); 6] = [(0, 0), (-1920, 0), (1920, 0), (0, -1080), (-1920, -1080), (1920, -1080)];
/// Where `wayvrctl desk-screens` puts each slot around the head (degrees yaw, pitch) for a 2 m wide screen at 2.5 m.
/// The angles WayVR's default layout computes for that size (windowing/viewlayout.rs `default_layout`, docs/SCREENS.md):
/// neighbours 1.5 deg apart as seen from the head, the upper row as a band above the lower one.
const SLOT_ANGLES: [(f32, f32); 6] = [(0.0, 0.0), (45.1, 0.0), (-45.1, 0.0), (0.0, 27.9), (56.3, 27.9), (-56.3, 27.9)];

/// The number of a slot name (1..6).
pub fn slot_number(slot: &str) -> Option<u8> {
    SLOTS.iter().position(|s| *s == slot).map(|i| i as u8 + 1)
}

/// What kind an app is (docs/VOICE-STATE-TESTS.md; the screens watcher sorts by terminal / browser / Pimax / other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default, PartialOrd, Ord)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Terminal,
    Browser,
    Editor,
    Video,
    Pimax,
    #[default]
    Other,
}

impl Kind {
    /// From a Hyprland class.
    pub fn of(class: &str) -> Kind {
        let c = class.to_lowercase();
        if crate::desk::is_terminal(&c) {
            Kind::Terminal
        } else if crate::windows::is_browser(&c) {
            Kind::Browser
        } else if c == "pimax" || c == "omarchyvr" || c.starts_with("pimax-") || c.starts_with("omarchyvr-") || c.starts_with("pimax.") || c.starts_with("omarchyvr.") {
            Kind::Pimax
        } else if c == "mpv" || c.contains("vlc") || c.contains("celluloid") {
            Kind::Video
        } else if c == "code" || c.contains("codium") || c.contains("zed") || c == "obsidian" || c.contains("neovide") {
            Kind::Editor
        } else {
            Kind::Other
        }
    }
    pub fn word(self) -> &'static str {
        match self {
            Kind::Terminal => "terminal",
            Kind::Browser => "browser",
            Kind::Editor => "editor",
            Kind::Video => "video",
            Kind::Pimax => "pimax",
            Kind::Other => "other",
        }
    }
}

/// The VR session (omarchyvrd / `omarchyvr desk`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub running: bool,
    /// work | play | off
    pub mode: String,
    /// cinema | work-3 | work-1 | assistant
    pub preset: String,
}

impl Default for Session {
    fn default() -> Self {
        Session { running: true, mode: "work".into(), preset: "work-3".into() }
    }
}

/// One VR screen.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Screen {
    /// 1..6 (docs/SCREENS.md).
    pub n: u8,
    pub slot: String,
    /// Its Hyprland output exists (the session's Screens setting).
    pub present: bool,
    /// The workspace it shows now.
    pub ws: i64,
    /// Shown in VR now (derived: [`World::refresh_shown`]).
    pub shown: bool,
    /// Width in metres, distance from the head in metres, place around the head (degrees), curvature.
    pub width: f32,
    pub distance: f32,
    pub yaw: f32,
    pub pitch: f32,
    pub curvature: f32,
}

impl Default for Screen {
    fn default() -> Self {
        Screen { n: 1, slot: "main".into(), present: true, ws: 1, shown: true, width: 1.6, distance: 2.0, yaw: 0.0, pitch: 0.0, curvature: 0.0 }
    }
}

/// `$XDG_RUNTIME_DIR/omarchyvr-screens.state` (omarchyvr-tools vr/screens.rs `ShowState`), screens by number.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShowRule {
    pub limit: u8,
    pub pinned: Vec<u8>,
    pub hidden: Vec<u8>,
    pub sort: bool,
}

impl Default for ShowRule {
    fn default() -> Self {
        ShowRule { limit: 6, pinned: Vec::new(), hidden: Vec::new(), sort: false }
    }
}

/// A Hyprland workspace and the screen (monitor) it lives on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Workspace {
    pub id: i64,
    pub screen: u8,
}

/// A browser tab.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Tab {
    pub title: String,
    pub url: String,
}

/// An app (a Hyprland client).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct App {
    /// The Hyprland address ("0x10").
    pub id: String,
    /// A short handle for people and cases ("t1", "c1").
    pub name: String,
    pub kind: Kind,
    pub class: String,
    /// The window title (a browser's: its active tab's title + " - Google Chrome").
    pub title: String,
    pub cwd: Option<String>,
    /// The workspace (negative: special, e.g. the scratchpad).
    pub ws: i64,
    pub fullscreen: bool,
    pub maximized: bool,
    pub floating: bool,
    pub tabs: Vec<Tab>,
    pub active_tab: usize,
    /// Focus order: higher = more recent.
    pub last_focus: u64,
}

impl App {
    /// The title Hyprland shows.
    pub fn shown_title(&self) -> String {
        match (self.kind, self.tabs.get(self.active_tab)) {
            (Kind::Browser, Some(t)) => format!("{} - Google Chrome", t.title),
            _ => self.title.clone(),
        }
    }
}

/// Where the eyes rest.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GazeAt {
    pub screen: u8,
    /// The app looked at (its id), when the eyes are on one.
    pub app: Option<String>,
}

/// The view around the screens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct View {
    /// normal | dim | off (the passthrough camera; act.rs `backdrop_plan`).
    pub passthrough: String,
    /// off | default | <skybox file>
    pub skybox: String,
    /// Passthrough brightness steps (brighter +1 / darker -1).
    pub brightness: i32,
    pub captions: bool,
    pub fps: bool,
    /// Screen number badges: ondemand | always | off.
    pub numbers: String,
    pub controllers: bool,
}

impl Default for View {
    fn default() -> Self {
        View { passthrough: "normal".into(), skybox: "off".into(), brightness: 0, captions: true, fps: false, numbers: "ondemand".into(), controllers: false }
    }
}

/// Sound and media.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Media {
    pub volume: u8,
    pub muted: bool,
    /// none | playing | paused
    pub music: String,
    pub track: i32,
}

impl Default for Media {
    fn default() -> Self {
        Media { volume: 40, muted: false, music: "none".into(), track: 0 }
    }
}

/// Text or keys sent to an app.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Typed {
    pub app: String,
    pub text: String,
}

/// The whole desk.
/// How slowly the simulated desktop answers (opt-in per world; the defaults are the old instant desk).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Timing {
    /// A launched app shows in `hyprctl -j clients` only after this many more client reads.
    pub launch_delay_reads: u32,
    /// A screen made shown or added shows in `hyprctl -j monitors` only after this many more monitor reads.
    pub screen_delay_reads: u32,
    /// Changes made once, right after the first read the voice code makes: "focus t2", "close s1", "gaze 3",
    /// "open <class> <screen>" (the desk changes while the AI thinks).
    pub drift: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct World {
    pub session: Session,
    /// The main desktop output's Hyprland name.
    pub main_output: String,
    /// Always 6 entries, n = 1..6.
    pub screens: Vec<Screen>,
    pub show: ShowRule,
    pub workspaces: Vec<Workspace>,
    pub apps: Vec<App>,
    /// The focused app (id).
    pub focus: Option<String>,
    pub focused_screen: u8,
    pub gaze: Option<GazeAt>,
    pub view: View,
    pub media: Media,
    pub recording: bool,
    pub locked: bool,
    pub typed: Vec<Typed>,
    /// Side effects that are not state: "reminder 10 stretch", "screenshot", "recenter", "wallpaper".
    pub events: Vec<String>,
    /// Videos in ~/Downloads (file names) "play X" can find.
    pub files: Vec<String>,
    pub skyboxes: Vec<String>,
    pub clipboard: String,
    /// Apps closed (kept for the report).
    pub closed: Vec<App>,
    /// Next address / focus counter.
    pub next_id: u64,
    pub clock: u64,
    pub timing: Timing,
}

impl Default for World {
    fn default() -> Self {
        let screens = (1..=6u8)
            .map(|n| {
                let (yaw, pitch) = SLOT_ANGLES[n as usize - 1];
                Screen { n, slot: SLOTS[n as usize - 1].into(), present: n <= 4, ws: n as i64, shown: n == 1, yaw, pitch, ..Screen::default() }
            })
            .collect();
        let mut w = World {
            session: Session::default(),
            main_output: "DP-1".into(),
            screens,
            show: ShowRule::default(),
            workspaces: Vec::new(),
            apps: Vec::new(),
            focus: None,
            focused_screen: 1,
            gaze: None,
            view: View::default(),
            media: Media::default(),
            recording: false,
            locked: false,
            typed: Vec::new(),
            events: Vec::new(),
            files: vec!["Dune.2021.1080p.mkv".into(), "Arrival.2016.mkv".into()],
            skyboxes: vec!["mountain_lake".into()],
            clipboard: String::new(),
            closed: Vec::new(),
            next_id: 0x100,
            clock: 1,
            timing: Timing::default(),
        };
        w.normalize();
        w
    }
}

impl World {
    pub fn screen(&self, n: u8) -> Option<&Screen> {
        self.screens.iter().find(|s| s.n == n)
    }
    fn screen_mut(&mut self, n: u8) -> Option<&mut Screen> {
        self.screens.iter_mut().find(|s| s.n == n)
    }
    pub fn app(&self, id: &str) -> Option<&App> {
        self.apps.iter().find(|a| a.id == id || a.name == id)
    }
    /// The screen an app is on (its workspace is the one a present screen shows), else None (off-screen).
    pub fn screen_of(&self, a: &App) -> Option<u8> {
        self.screens.iter().find(|s| s.present && s.ws == a.ws).map(|s| s.n)
    }
    /// The screen of an output name.
    pub fn screen_by_output(&self, name: &str) -> Option<u8> {
        if name == self.main_output {
            return Some(1);
        }
        OUTPUTS.iter().position(|o| !o.is_empty() && *o == name).map(|i| i as u8 + 1)
    }
    pub fn output(&self, n: u8) -> String {
        if n == 1 {
            self.main_output.clone()
        } else {
            OUTPUTS[n as usize - 1].to_string()
        }
    }
    /// The home screen of a workspace (created on the focused screen when new).
    fn ws_home(&self, ws: i64) -> Option<u8> {
        self.workspaces.iter().find(|w| w.id == ws).map(|w| w.screen)
    }

    /// Make the derived parts consistent: workspaces of every app and screen exist, screens 1-6 present in order,
    /// focus valid, empty workspaces no screen shows are gone (Hyprland drops them), shown recomputed.
    pub fn normalize(&mut self) {
        self.screens.sort_by_key(|s| s.n);
        let main_ws = self.screens.first().map(|s| s.ws).unwrap_or(1);
        for s in self.screens.clone() {
            if s.present && self.ws_home(s.ws).is_none() {
                self.workspaces.push(Workspace { id: s.ws, screen: s.n });
            }
        }
        for a in self.apps.clone() {
            if a.ws > 0 && self.ws_home(a.ws).is_none() {
                self.workspaces.push(Workspace { id: a.ws, screen: 1 });
            }
        }
        let _ = main_ws;
        let shown_ws: Vec<i64> = self.screens.iter().filter(|s| s.present).map(|s| s.ws).collect();
        let used: Vec<i64> = self.apps.iter().map(|a| a.ws).collect();
        self.workspaces.retain(|w| shown_ws.contains(&w.id) || used.contains(&w.id));
        self.workspaces.sort_by_key(|w| w.id);
        if self.focus.as_ref().is_some_and(|f| !self.apps.iter().any(|a| &a.id == f)) {
            self.focus = None;
        }
        if !self.screen(self.focused_screen).is_some_and(|s| s.present) {
            self.focused_screen = 1;
        }
        for a in self.apps.iter_mut() {
            if a.kind == Kind::Other && !a.class.is_empty() {
                a.kind = Kind::of(&a.class);
            }
            if a.kind == Kind::Browser && a.tabs.is_empty() {
                a.tabs.push(Tab { title: a.title.trim_end_matches(" - Google Chrome").to_string(), url: String::new() });
            }
            if a.active_tab >= a.tabs.len().max(1) {
                a.active_tab = 0;
            }
        }
        self.refresh_shown();
    }

    /// Which screens VR shows: omarchyvr-tools vr/screens.rs `visibility` (the main one always; an extra one when
    /// pinned, busy (an app on its workspace) or focused, at most `limit` in all in slot order; the focused one even
    /// over the limit; a hidden one never). No session: nothing shows.
    /// APPROX: the screens watcher also re-sorts apps of different kinds onto their own screens (`layout_plan`) when
    /// `sort` is on; that move is not simulated.
    pub fn refresh_shown(&mut self) {
        let running = self.session.running;
        let busy: Vec<i64> = self.apps.iter().filter(|a| a.ws > 0).map(|a| a.ws).collect();
        let focused = self.focused_screen;
        let st = self.show.clone();
        let mut shown = 1u8;
        for s in self.screens.iter_mut() {
            if s.n == 1 {
                s.shown = running;
                continue;
            }
            if !s.present {
                s.shown = false;
                continue;
            }
            let want = st.pinned.contains(&s.n) || busy.contains(&s.ws) || s.n == focused;
            let on = !st.hidden.contains(&s.n) && ((want && shown < st.limit) || s.n == focused);
            if on {
                shown += 1;
            }
            s.shown = on && running;
        }
    }

    fn tick(&mut self) -> u64 {
        self.clock += 1;
        self.clock
    }

    fn new_id(&mut self) -> String {
        self.next_id += 1;
        format!("0x{:x}", self.next_id)
    }

    /// Focus an app: Hyprland makes its workspace the active one on that workspace's screen.
    fn focus_app(&mut self, id: &str) -> Result<(), String> {
        let Some(a) = self.apps.iter().find(|a| a.id == id).cloned() else { return Err(format!("no window {id}")) };
        if a.ws > 0 {
            let home = self.ws_home(a.ws).unwrap_or(self.focused_screen);
            if let Some(s) = self.screen_mut(home) {
                s.ws = a.ws;
            }
            self.focused_screen = home;
        }
        let t = self.tick();
        if let Some(x) = self.apps.iter_mut().find(|x| x.id == id) {
            x.last_focus = t;
        }
        self.focus = Some(id.to_string());
        Ok(())
    }

    /// The most recently focused app on a workspace.
    fn top_on(&self, ws: i64) -> Option<String> {
        self.apps.iter().filter(|a| a.ws == ws).max_by_key(|a| a.last_focus).map(|a| a.id.clone())
    }

    fn focus_screen(&mut self, n: u8) {
        self.focused_screen = n;
        let ws = self.screen(n).map(|s| s.ws).unwrap_or(1);
        self.focus = self.top_on(ws);
        if let Some(f) = self.focus.clone() {
            let t = self.tick();
            if let Some(x) = self.apps.iter_mut().find(|x| x.id == f) {
                x.last_focus = t;
            }
        }
    }

    /// Show workspace `ws` (create it on the focused screen when new).
    fn show_ws(&mut self, ws: i64) {
        let home = match self.ws_home(ws) {
            Some(h) => h,
            None => {
                self.workspaces.push(Workspace { id: ws, screen: self.focused_screen });
                self.focused_screen
            }
        };
        if let Some(s) = self.screen_mut(home) {
            s.ws = ws;
        }
        self.focus_screen(home);
    }

    /// Apply drift verbs (`Timing::drift`). Unknown verbs and missing apps are skipped (the desk moved on).
    pub fn drift(&mut self, verbs: &[String]) {
        for v in verbs {
            let t: Vec<&str> = v.split_whitespace().collect();
            match t.as_slice() {
                ["focus", name] => {
                    if let Some(id) = self.app(name).map(|a| a.id.clone()) {
                        let _ = self.focus_app(&id);
                    }
                }
                ["close", name] => {
                    if let Some(i) = self.apps.iter().position(|a| a.id == *name || a.name == *name) {
                        let a = self.apps.remove(i);
                        if self.focus.as_deref() == Some(a.id.as_str()) {
                            let ws_now = self.screen(self.focused_screen).map(|s| s.ws).unwrap_or(1);
                            self.focus = self.top_on(ws_now);
                        }
                        self.closed.push(a);
                    }
                }
                ["gaze", n] => {
                    if let Ok(n) = n.parse::<u8>() {
                        self.gaze = Some(GazeAt { screen: n, app: None });
                    }
                }
                ["open", class, n] => {
                    if let (Ok(n), true) = (n.parse::<u8>(), true) {
                        let ws = self.screen(n).map(|s| s.ws);
                        let id = self.launch(class, class, Vec::new());
                        if let (Some(ws), Some(a)) = (ws, self.apps.iter_mut().find(|a| a.id == id)) {
                            a.ws = ws;
                        }
                    }
                }
                _ => {}
            }
            self.normalize();
        }
    }

    /// A new app on the focused screen's workspace, focused.
    fn launch(&mut self, class: &str, title: &str, tabs: Vec<Tab>) -> String {
        let id = self.new_id();
        let ws = self.screen(self.focused_screen).map(|s| s.ws).unwrap_or(1);
        let kind = Kind::of(class);
        let n = self.apps.iter().filter(|a| a.kind == kind).count() + self.closed.iter().filter(|a| a.kind == kind).count() + 1;
        let prefix = match kind {
            Kind::Terminal => "t",
            Kind::Browser => "c",
            Kind::Video => "v",
            Kind::Pimax => "p",
            Kind::Editor => "e",
            Kind::Other => "o",
        };
        let fullscreen = kind == Kind::Video;
        self.apps.push(App { id: id.clone(), name: format!("{prefix}{n}new"), kind, class: class.into(), title: title.into(), cwd: (kind == Kind::Terminal).then(|| "~".to_string()), ws, fullscreen, tabs, ..App::default() });
        let _ = self.focus_app(&id);
        id
    }

    fn focused_app_mut(&mut self) -> Option<&mut App> {
        let f = self.focus.clone()?;
        self.apps.iter_mut().find(|a| a.id == f)
    }

    // ------------------------------------------------------------------------------------- Hyprland JSON (reads)

    /// `hyprctl -j clients`: apps tiled side by side on their monitor (the geometry `gaze` resolves against).
    pub fn clients_json(&self) -> String {
        let mut v = Vec::new();
        let mut order: Vec<&App> = self.apps.iter().collect();
        order.sort_by(|a, b| b.last_focus.cmp(&a.last_focus));
        for a in &self.apps {
            let hist = order.iter().position(|x| x.id == a.id).unwrap_or(0);
            let home = self.ws_home(a.ws).unwrap_or(1);
            let (mx, my) = MON_XY[home as usize - 1];
            let peers: Vec<&App> = self.apps.iter().filter(|x| x.ws == a.ws && !x.floating).collect();
            let i = peers.iter().position(|x| x.id == a.id).unwrap_or(0) as i64;
            let n = peers.len().max(1) as i64;
            let (at, size) = if a.fullscreen { ([mx, my], [1920, 1080]) } else { ([mx + i * 1920 / n, my + 28], [1920 / n, 1052]) };
            v.push(json!({"address": a.id, "mapped": true, "hidden": false, "at": at, "size": size,
                "workspace": {"id": a.ws, "name": if a.ws < 0 { "special:scratchpad".to_string() } else { a.ws.to_string() }},
                "floating": a.floating, "monitor": home as i64 - 1, "class": a.class, "initialClass": a.class, "title": a.shown_title(),
                "pid": 1000 + i, "pinned": false, "fullscreen": if a.fullscreen { 2 } else if a.maximized { 1 } else { 0 },
                "grouped": [], "focusHistoryID": hist}));
        }
        Value::Array(v).to_string()
    }

    pub fn active_json(&self) -> String {
        match self.focus.as_ref().and_then(|f| self.apps.iter().find(|a| &a.id == f)) {
            Some(a) => {
                let all: Value = serde_json::from_str(&self.clients_json()).unwrap_or_default();
                all.as_array().and_then(|v| v.iter().find(|c| c["address"] == a.id.as_str()).cloned()).unwrap_or_default().to_string()
            }
            None => "{}".into(),
        }
    }

    pub fn monitors_json(&self) -> String {
        let v: Vec<Value> = self
            .screens
            .iter()
            .filter(|s| s.present)
            .map(|s| {
                let (x, y) = MON_XY[s.n as usize - 1];
                json!({"id": s.n as i64 - 1, "name": self.output(s.n), "width": 1920, "height": 1080, "x": x, "y": y, "scale": 1.0,
                    "transform": 0, "focused": s.n == self.focused_screen, "disabled": false, "mirrorOf": "none",
                    "activeWorkspace": {"id": s.ws, "name": s.ws.to_string()}, "specialWorkspace": {"id": 0, "name": ""}})
            })
            .collect();
        Value::Array(v).to_string()
    }

    pub fn workspaces_json(&self) -> String {
        let v: Vec<Value> = self
            .workspaces
            .iter()
            .map(|w| json!({"id": w.id, "name": w.id.to_string(), "monitor": self.output(w.screen), "windows": self.apps.iter().filter(|a| a.ws == w.id).count()}))
            .collect();
        Value::Array(v).to_string()
    }

    /// `wayvrctl gaze` (WayVR eye-gaze branch): a stable fixation on the gazed screen, on the gazed app's tile.
    pub fn gaze_json(&self, now_ms: u64) -> Option<String> {
        let g = self.gaze.as_ref()?;
        let s = self.screen(g.screen).filter(|s| s.shown)?;
        let mut uv = [0.5f32, 0.5];
        if let Some(a) = g.app.as_ref().and_then(|id| self.apps.iter().find(|a| &a.id == id || &a.name == id)) {
            let peers: Vec<&App> = self.apps.iter().filter(|x| x.ws == a.ws && !x.floating).collect();
            let i = peers.iter().position(|x| x.id == a.id).unwrap_or(0) as f32;
            let n = peers.len().max(1) as f32;
            uv = [(i + 0.5) / n, 0.5];
        }
        let f = json!({"overlay": self.output(s.n), "uv": uv, "px": null, "start_ms": now_ms - 900, "end_ms": now_ms - 100,
            "duration_ms": 800, "confidence": 0.9, "dispersion_deg": 0.4, "dist_m": s.distance, "samples": 60});
        Some(json!({"ts_ms": now_ms, "tracking": true, "laser": false, "valid": true, "min_fixation_ms": 200,
            "current": f, "last_stable": f, "history": [f]}).to_string())
    }
}

// ------------------------------------------------------------------------------------------------ the simulator

/// A Runner over a World. Every write the voice code makes changes [`Sim::world`]; reads come from it.
pub struct Sim {
    pub world: World,
    /// Every command that would have run (writes and reads), in order.
    pub commands: Vec<String>,
    pub toasts: Vec<String>,
    /// Commands the simulator does not know (the state may be incomplete).
    pub unmodelled: Vec<String>,
    /// The Unix ms the gaze report is stamped with.
    pub now_ms: u64,
    /// The world after every write (not reads), with the command that made it. The steps and the in-between
    /// checks of statetest.rs look at these.
    pub history: Vec<(String, World)>,
    /// Apps launched but not yet in `hyprctl -j clients`: id -> client reads still to wait.
    unseen_apps: BTreeMap<String, u32>,
    /// Screens made shown or added but not yet in `hyprctl -j monitors`: n -> monitor reads still to wait.
    unseen_screens: BTreeMap<u8, u32>,
    drift_done: bool,
}

impl Sim {
    pub fn new(world: World) -> Sim {
        let now_ms = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(1_000_000);
        Sim { world, commands: Vec::new(), toasts: Vec::new(), unmodelled: Vec::new(), now_ms, history: Vec::new(), unseen_apps: BTreeMap::new(), unseen_screens: BTreeMap::new(), drift_done: false }
    }

    /// The commands that change something (reads left out).
    pub fn writes(&self) -> Vec<String> {
        self.commands.iter().filter(|c| !is_read(c)).cloned().collect()
    }

    /// Record the world after a write.
    pub fn snap(&mut self, cmd: &str) {
        self.history.push((cmd.to_string(), self.world.clone()));
    }

    /// The first read the voice code makes: the desk changes once (Timing::drift).
    fn drift_after_read(&mut self) {
        if self.drift_done || self.world.timing.drift.is_empty() {
            return;
        }
        self.drift_done = true;
        let verbs = self.world.timing.drift.clone();
        self.world.drift(&verbs);
    }

    /// `hyprctl -j clients` / `activewindow` with the launch delay applied (apps not there yet are left out).
    fn clients_read(&mut self, active: bool) -> String {
        if self.unseen_apps.is_empty() {
            return if active { self.world.active_json() } else { self.world.clients_json() };
        }
        let mut w = self.world.clone();
        {
            let gone: Vec<String> = self.unseen_apps.keys().cloned().collect();
            w.apps.retain(|a| !gone.contains(&a.id));
            if w.focus.as_ref().is_some_and(|f| gone.contains(f)) {
                w.focus = None;
            }
            if !active {
                for n in self.unseen_apps.values_mut() {
                    *n -= 1;
                }
                self.unseen_apps.retain(|_, n| *n > 0);
            }
        }
        if active {
            w.active_json()
        } else {
            w.clients_json()
        }
    }

    /// `hyprctl -j monitors` with the screen delay applied.
    fn monitors_read(&mut self) -> String {
        if self.unseen_screens.is_empty() {
            return self.world.monitors_json();
        }
        let mut w = self.world.clone();
        for (n, _) in self.unseen_screens.iter() {
            if let Some(s) = w.screens.iter_mut().find(|s| s.n == *n) {
                s.present = false;
            }
        }
        for n in self.unseen_screens.values_mut() {
            *n -= 1;
        }
        self.unseen_screens.retain(|_, n| *n > 0);
        w.monitors_json()
    }

    /// What the delays hide: the apps and the (present, shown) flags before a write.
    fn timing_before(&self) -> Option<(Vec<String>, Vec<(bool, bool)>)> {
        let t = &self.world.timing;
        (t.launch_delay_reads > 0 || t.screen_delay_reads > 0).then(|| (self.world.apps.iter().map(|a| a.id.clone()).collect(), self.world.screens.iter().map(|s| (s.present, s.shown)).collect()))
    }

    fn timing_after(&mut self, before: (Vec<String>, Vec<(bool, bool)>)) {
        let (ids, flags) = before;
        let (ld, sd) = (self.world.timing.launch_delay_reads, self.world.timing.screen_delay_reads);
        if ld > 0 {
            for a in self.world.apps.iter().filter(|a| !ids.contains(&a.id)) {
                self.unseen_apps.insert(a.id.clone(), ld);
            }
        }
        if sd > 0 {
            for (s, (p, sh)) in self.world.screens.iter().zip(flags) {
                if (s.present && !p) || (s.shown && !sh) {
                    self.unseen_screens.insert(s.n, sd);
                }
            }
        }
    }

    fn unknown(&mut self, what: String) -> Result<String, String> {
        self.unmodelled.push(what);
        Ok(String::new())
    }

    /// `hyprctl dispatch <lua>` (Hyprland 0.56 Lua dispatchers, windows.rs and the catalogue's run lines).
    fn dispatch(&mut self, lua: &str) -> Result<String, String> {
        let w = &mut self.world;
        let arg = |k: &str| lua_arg(lua, k);
        if lua.starts_with("hl.dsp.focus(") {
            if let Some(win) = arg("window") {
                let id = win.trim_start_matches("address:").to_string();
                w.focus_app(&id)?;
            } else if let Some(m) = arg("monitor") {
                let n = w.screen_by_output(&m).ok_or_else(|| format!("no monitor {m}"))?;
                w.focus_screen(n);
            } else if let Some(ws) = arg("workspace") {
                let cur = w.screen(w.focused_screen).map(|s| s.ws).unwrap_or(1);
                let target = match ws.as_str() {
                    "e+1" | "e-1" => {
                        // the next / previous existing workspace (Hyprland "e+1": among the existing ones)
                        let mut ids: Vec<i64> = w.workspaces.iter().map(|x| x.id).filter(|x| *x > 0).collect();
                        ids.sort();
                        let i = ids.iter().position(|x| *x == cur).unwrap_or(0) as i64;
                        let j = if ws == "e+1" { (i + 1).rem_euclid(ids.len().max(1) as i64) } else { (i - 1).rem_euclid(ids.len().max(1) as i64) };
                        ids.get(j as usize).copied().unwrap_or(cur)
                    }
                    "emptym" => {
                        // the first empty workspace on this monitor, else a new one
                        let fs = w.focused_screen;
                        let empty = w.workspaces.iter().filter(|x| x.screen == fs && x.id > 0).map(|x| x.id).find(|id| !w.apps.iter().any(|a| a.ws == *id));
                        match empty {
                            Some(e) => e,
                            None => (1..100).find(|id| !w.workspaces.iter().any(|x| x.id == *id)).unwrap_or(99),
                        }
                    }
                    n => n.parse::<i64>().map_err(|_| format!("bad workspace {n}"))?,
                };
                w.show_ws(target);
            } else if let Some(d) = arg("direction") {
                // APPROX: within the workspace by tile order; past the edge to the screen on that side
                let ws = w.screen(w.focused_screen).map(|s| s.ws).unwrap_or(1);
                let peers: Vec<String> = w.apps.iter().filter(|a| a.ws == ws).map(|a| a.id.clone()).collect();
                let i = w.focus.as_ref().and_then(|f| peers.iter().position(|p| p == f));
                let next = match (d.as_str(), i) {
                    ("l", Some(i)) if i > 0 => Some(peers[i - 1].clone()),
                    ("r", Some(i)) if i + 1 < peers.len() => Some(peers[i + 1].clone()),
                    _ => None,
                };
                match next {
                    Some(id) => w.focus_app(&id)?,
                    None => {
                        let from = SLOTS[w.focused_screen as usize - 1];
                        let side = match d.as_str() {
                            "l" => "left",
                            "r" => "right",
                            "u" => "up",
                            _ => "down",
                        };
                        if let Some(to) = crate::windows::neighbour(from, side).and_then(slot_number).filter(|n| w.screen(*n).is_some_and(|s| s.present)) {
                            w.focus_screen(to);
                        }
                    }
                }
            } else {
                return self.unknown(format!("hyprctl dispatch {lua}"));
            }
        } else if lua.starts_with("hl.dsp.window.move(") {
            let ws = arg("workspace").ok_or("move: no workspace")?;
            let follow = lua.contains("follow = true");
            let f = w.focus.clone().ok_or("no focused window")?;
            let target: i64 = if ws.starts_with("special") { -99 } else { ws.parse().map_err(|_| format!("bad workspace {ws}"))? };
            if target > 0 && w.ws_home(target).is_none() {
                let fs = w.focused_screen;
                w.workspaces.push(Workspace { id: target, screen: fs });
            }
            if let Some(a) = w.apps.iter_mut().find(|a| a.id == f) {
                a.ws = target;
            }
            if follow && target > 0 {
                w.show_ws(target);
                w.focus_app(&f)?;
            } else {
                // the focus moves to what is left on this screen
                let ws_now = w.screen(w.focused_screen).map(|s| s.ws).unwrap_or(1);
                w.focus = w.top_on(ws_now);
            }
        } else if lua.starts_with("hl.dsp.window.close(") {
            let id = arg("window").map(|x| x.trim_start_matches("address:").to_string()).or(w.focus.clone()).ok_or("no window")?;
            let Some(i) = w.apps.iter().position(|a| a.id == id) else { return Err(format!("no window {id}")) };
            let a = w.apps.remove(i);
            w.closed.push(a);
            if w.focus.as_deref() == Some(id.as_str()) {
                let ws_now = w.screen(w.focused_screen).map(|s| s.ws).unwrap_or(1);
                w.focus = w.top_on(ws_now);
            }
        } else if lua.starts_with("hl.dsp.window.fullscreen(") {
            let max = arg("mode").is_some_and(|m| m == "maximized");
            let a = w.focused_app_mut().ok_or("no focused window")?;
            if max {
                a.maximized = !a.maximized;
            } else {
                a.fullscreen = !a.fullscreen;
            }
        } else if lua.starts_with("hl.dsp.window.float(") {
            let a = w.focused_app_mut().ok_or("no focused window")?;
            a.floating = !a.floating;
        } else if lua.starts_with("hl.dsp.window.resize(") || lua.starts_with("hl.dsp.layout(") {
            w.events.push(format!("layout {lua}"));
        } else if lua.starts_with("hl.dsp.window.cycle_next(") {
            let ws = w.screen(w.focused_screen).map(|s| s.ws).unwrap_or(1);
            let peers: Vec<String> = w.apps.iter().filter(|a| a.ws == ws).map(|a| a.id.clone()).collect();
            if !peers.is_empty() {
                let i = w.focus.as_ref().and_then(|f| peers.iter().position(|p| p == f)).map(|i| (i + 1) % peers.len()).unwrap_or(0);
                let id = peers[i].clone();
                w.focus_app(&id)?;
            }
        } else if lua.starts_with("hl.dsp.workspace.toggle_special(") {
            w.events.push("scratchpad toggled".into());
        } else if lua.starts_with("hl.dsp.send_key_state(") {
            if arg("state").as_deref() == Some("down") {
                let mods = arg("mods").unwrap_or_default().to_lowercase();
                let key = arg("key").unwrap_or_default();
                self.key(&mods, &key);
            }
        } else if lua.starts_with("hl.dsp.group.") {
            // no app in a World is a Hyprland group
            return Err("not grouped".into());
        } else {
            return self.unknown(format!("hyprctl dispatch {lua}"));
        }
        Ok("ok".into())
    }

    /// A key chord to the focused app (wtype / send_key_state): browser tab keys change its tabs, the rest is typed.
    fn key(&mut self, mods: &str, key: &str) {
        let w = &mut self.world;
        let Some(f) = w.focus.clone() else { return };
        let ctrl = mods.contains("ctrl");
        let alt = mods.contains("alt");
        let Some(a) = w.apps.iter_mut().find(|a| a.id == f) else { return };
        if a.kind == Kind::Browser {
            let n = a.tabs.len().max(1);
            let k = key.to_lowercase();
            match (ctrl, alt, k.as_str()) {
                (true, _, "page_down") | (true, _, "tab") => return a.active_tab = (a.active_tab + 1) % n,
                (true, _, "page_up") => return a.active_tab = (a.active_tab + n - 1) % n,
                (true, _, d) if d.len() == 1 && d.chars().all(|c| c.is_ascii_digit()) => {
                    let i: usize = d.parse().unwrap_or(1);
                    a.active_tab = if i == 9 { n - 1 } else { (i.max(1) - 1).min(n - 1) };
                    return;
                }
                (true, _, "t") => {
                    a.tabs.push(Tab { title: "New Tab".into(), url: "chrome://newtab".into() });
                    a.active_tab = a.tabs.len() - 1;
                    return;
                }
                (true, _, "w") => {
                    if a.tabs.len() > 1 {
                        a.tabs.remove(a.active_tab);
                        a.active_tab = a.active_tab.min(a.tabs.len() - 1);
                    } else {
                        let gone = a.clone();
                        w.apps.retain(|x| x.id != f);
                        w.closed.push(gone);
                        w.focus = None;
                    }
                    return;
                }
                (true, _, "c") => {
                    // Ctrl+L then Ctrl+C copies the active tab's URL (windows.rs tab_url)
                    w.clipboard = a.tabs.get(a.active_tab).map(|t| t.url.clone()).unwrap_or_default();
                    return;
                }
                (true, _, "l") => return,
                (_, true, "left") => {
                    w.typed.push(Typed { app: f, text: "<Back>".into() });
                    return;
                }
                (_, _, "f5") => {
                    w.typed.push(Typed { app: f, text: "<Reload>".into() });
                    return;
                }
                (_, _, "escape") if w.clipboard.starts_with("http") => return,
                _ => {}
            }
        }
        let chord = if mods.is_empty() { format!("<{key}>") } else { format!("<{}+{key}>", mods.replace(' ', "+")) };
        w.typed.push(Typed { app: f, text: chord });
    }

    /// `wtype ...` (desk.rs): `-M mod -k key -m mod` chords and `-- text`.
    fn wtype(&mut self, argv: &[String]) -> Result<String, String> {
        if let Some(i) = argv.iter().position(|a| a == "--") {
            let text = argv[i + 1..].join(" ");
            let f = self.world.focus.clone().ok_or("No window is focused")?;
            self.world.typed.push(Typed { app: f, text });
            return Ok(String::new());
        }
        let mut mods: Vec<String> = Vec::new();
        let mut i = 1;
        while i < argv.len() {
            match argv[i].as_str() {
                "-M" => {
                    mods.push(argv.get(i + 1).cloned().unwrap_or_default());
                    i += 2;
                }
                "-m" => {
                    i += 2;
                }
                "-k" => {
                    let k = argv.get(i + 1).cloned().unwrap_or_default();
                    self.key(&mods.join("+"), &k);
                    i += 2;
                }
                "-s" | "-d" => i += 2,
                _ => i += 1,
            }
        }
        Ok(String::new())
    }

    /// `omarchy-launch-browser [--new-window] [URL]`: with a URL and a browser open (no --new-window) the URL opens
    /// as a new tab of the most recent browser window (Chromium's default), else a new browser window.
    fn browser(&mut self, argv: &[String]) -> Result<String, String> {
        let new_window = argv.iter().any(|a| a == "--new-window");
        let url = argv.iter().skip(1).find(|a| !a.starts_with("--")).cloned();
        let title = url.as_deref().map(title_for_url).unwrap_or_else(|| "New Tab".into());
        let tab = Tab { title: title.clone(), url: url.clone().unwrap_or_else(|| "chrome://newtab".into()) };
        let recent = self.world.apps.iter().filter(|a| a.kind == Kind::Browser).max_by_key(|a| a.last_focus).map(|a| a.id.clone());
        match (new_window, url.is_some(), recent) {
            (false, true, Some(id)) => {
                if let Some(a) = self.world.apps.iter_mut().find(|a| a.id == id) {
                    a.tabs.push(tab);
                    a.active_tab = a.tabs.len() - 1;
                }
                self.world.focus_app(&id)?;
            }
            (false, false, Some(id)) => self.world.focus_app(&id)?,
            _ => {
                self.world.launch("google-chrome", &format!("{title} - Google Chrome"), vec![tab]);
            }
        }
        Ok(String::new())
    }

    /// `omarchyvr desk screens show|more|add|hide ARG` (omarchyvr-tools vr/screens.rs `parse_show` + `plan_show` +
    /// `run_show`): a new show state; never moves or closes an app.
    fn desk_screens(&mut self, sub: &str, arg: &str) -> Result<String, String> {
        let w = &mut self.world;
        if !w.session.running {
            return Err("No Work session is running".into());
        }
        let present: Vec<u8> = w.screens.iter().filter(|s| s.present && s.n > 1).map(|s| s.n).collect();
        let shown_now: Vec<u8> = w.screens.iter().filter(|s| s.shown && s.n > 1).map(|s| s.n).collect();
        let plural = |n: usize| format!("{n} screen{}", if n == 1 { "" } else { "s" });
        let slot_of = |x: &str| -> Option<u8> {
            match x.trim().parse::<u8>() {
                Ok(n) if (1..=6).contains(&n) => Some(n),
                Ok(_) => None,
                Err(_) => match x.trim().to_lowercase().as_str() {
                    "main" | "middle" | "center" | "centre" => Some(1),
                    "left" => Some(2),
                    "right" => Some(3),
                    "up" | "top" | "upper" | "above" => Some(4),
                    "up-left" | "upleft" | "up_left" | "top-left" => Some(5),
                    "up-right" | "upright" | "up_right" | "top-right" => Some(6),
                    _ => None,
                },
            }
        };
        let msg = match sub {
            "show" | "more" => {
                let k: u32 = arg.trim().parse().map_err(|_| format!("desk screens {sub}: a number is needed"))?;
                let asked = if sub == "show" { k } else { 1 + shown_now.len() as u32 + k };
                if asked == 0 {
                    return Err("Screens go from 1 to 6".into());
                }
                let n = asked.min(6) as u8;
                let needed: Vec<u8> = (2..=n).collect();
                let pinned: Vec<u8> = needed.iter().copied().filter(|s| present.contains(s)).collect();
                let missing = needed.iter().any(|s| !present.contains(s));
                let mut m = if asked > 6 { "That's the most: 6 screens".to_string() } else { format!("Showing {}", plural(n as usize)) };
                if missing {
                    m = format!("{n} screens need Screens = {n} in the Pimax app and a Work restart. Showing {}", plural(1 + pinned.len()));
                }
                w.show = ShowRule { limit: n, pinned, hidden: Vec::new(), sort: w.show.sort };
                m
            }
            "add" => {
                let s = slot_of(arg).ok_or_else(|| format!("No screen called {arg}"))?;
                if s == 1 {
                    return Err("The main screen is always shown".into());
                }
                if !present.contains(&s) {
                    return Ok(format!("The {} screen needs Screens = {s} in the Pimax app and a Work restart", SLOTS[s as usize - 1]));
                }
                if !w.show.pinned.contains(&s) {
                    w.show.pinned.push(s);
                }
                w.show.hidden.retain(|x| *x != s);
                let room = if shown_now.contains(&s) { 1 + shown_now.len() } else { 2 + shown_now.len() };
                w.show.limit = (w.show.limit as usize).max(room).max(1 + w.show.pinned.len()).min(6) as u8;
                format!("Showing the {} screen", SLOTS[s as usize - 1].replace('-', " "))
            }
            "hide" => {
                let slots: Vec<u8> = if matches!(arg, "others" | "all" | "everything") {
                    (2..=6).collect()
                } else {
                    let mut v = Vec::new();
                    for x in arg.split([',', ' ']).filter(|x| !x.trim().is_empty()) {
                        match slot_of(x) {
                            Some(1) => return Err("The main screen is always shown".into()),
                            Some(s) if !v.contains(&s) => v.push(s),
                            Some(_) => {}
                            None => return Err(format!("No screen {x}")),
                        }
                    }
                    v
                };
                if slots.is_empty() {
                    return Err("desk screens hide: which screens?".into());
                }
                w.show.pinned.retain(|s| !slots.contains(s));
                for s in &slots {
                    if !w.show.hidden.contains(s) {
                        w.show.hidden.push(*s);
                    }
                }
                let all = (2..=6).all(|s| slots.contains(&s));
                if all {
                    w.show.limit = 1;
                }
                let was: Vec<String> = slots.iter().filter(|s| shown_now.contains(s)).map(|s| s.to_string()).collect();
                if all {
                    "Only the main screen now; the apps stay open".to_string()
                } else if was.is_empty() {
                    format!("Screen {} is not shown", slots.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" and "))
                } else {
                    format!("Hid screen {}; its apps stay open", was.join(" and "))
                }
            }
            other => return self.unknown(format!("omarchyvr desk screens {other} {arg}")),
        };
        self.world.refresh_shown();
        Ok(msg)
    }

    /// `omarchyvr desk preset P` (omarchyvr-tools vr/preset.rs `live_steps` + screens.rs `ShowState::switch_to`): the
    /// passthrough line, the show state, and `desk-screens --place` with the preset's width, distance, curvature.
    fn preset(&mut self, p: &str) -> Result<String, String> {
        let w = &mut self.world;
        let p = match p {
            "work" | "work3" => "work-3",
            "work1" => "work-1",
            x => x,
        };
        let (limit, pinned, width, distance, curv, pt): (u8, Vec<u8>, f32, f32, f32, &str) = match p {
            "cinema" => (1, vec![], 3.2, 3.5, 0.35, "dim"),
            "cinema-huge" => (1, vec![], 11.0, 8.0, 0.2, "off"),
            "work-3" => (3, vec![2, 3], 1.6, 2.0, 0.0, "normal"),
            "work-1" => (1, vec![], 2.0, 2.2, 0.0, "normal"),
            "assistant" => (6, vec![], 2.0, 2.5, 0.15, "normal"),
            other => return Err(format!("unknown preset {other}")),
        };
        if !w.session.running {
            return Err("No Work session is running".into());
        }
        w.show = ShowRule { limit, pinned, hidden: Vec::new(), sort: w.show.sort };
        w.view.passthrough = pt.into();
        w.view.skybox = "off".into();
        if p == "assistant" {
            w.view.captions = true;
        }
        for s in w.screens.iter_mut() {
            let (yaw, pitch) = SLOT_ANGLES[s.n as usize - 1];
            s.width = width;
            s.distance = distance;
            s.curvature = curv;
            s.yaw = yaw;
            s.pitch = pitch;
        }
        w.session.preset = p.into();
        w.refresh_shown();
        let hidden: Vec<String> = w
            .screens
            .iter()
            .filter(|s| s.present && !s.shown && w.apps.iter().any(|a| a.ws == s.ws))
            .map(|s| format!("{} ({})", s.slot, w.apps.iter().filter(|a| a.ws == s.ws).map(|a| a.class.clone()).collect::<Vec<_>>().join(", ")))
            .collect();
        let mut out = String::new();
        if !hidden.is_empty() {
            out += &format!("omarchyvr desk: still open on hidden screens: {}\n", hidden.join(", "));
        }
        out += &format!("omarchyvr desk: preset {p}\n");
        Ok(out)
    }

    /// `wayvrctl screen-nudge [--step S] NAME DIR` (WayVR grabmath.rs `nudge`, step 0.02..1, default 0.25):
    /// left/right turn the screen around the head by step x width / distance (radians), up/down by step x height,
    /// closer/farther scale the distance by 1 -/+ step (0.4..8 m), bigger/smaller scale the size by 1 + step or
    /// 1 / (1 + step) (0.3..6 m). A screen that is not shown does not move.
    fn nudge(&mut self, argv: &[String]) -> Result<String, String> {
        let mut step = 0.25f32;
        let mut rest: Vec<&String> = Vec::new();
        let mut i = 2;
        while i < argv.len() {
            if argv[i] == "--step" || argv[i] == "-s" {
                step = argv.get(i + 1).and_then(|x| x.parse().ok()).unwrap_or(0.25);
                i += 2;
            } else {
                rest.push(&argv[i]);
                i += 1;
            }
        }
        let step = step.clamp(0.02, 1.0);
        let (Some(name), Some(dir)) = (rest.first(), rest.get(1)) else { return Err("screen-nudge NAME DIR".into()) };
        let Some(n) = self.world.screen_by_output(name) else { return Ok(String::new()) };
        let Some(s) = self.world.screen_mut(n) else { return Ok(String::new()) };
        if !s.shown {
            return Ok(String::new());
        }
        let deg = |x: f32| x.to_degrees();
        match dir.as_str() {
            "left" => s.yaw += deg(step * s.width / s.distance),
            "right" => s.yaw -= deg(step * s.width / s.distance),
            "up" => s.pitch += deg(step * s.width * 9.0 / 16.0 / s.distance),
            "down" => s.pitch -= deg(step * s.width * 9.0 / 16.0 / s.distance),
            "closer" => s.distance = (s.distance * (1.0 - step)).clamp(0.4, 8.0),
            "farther" => s.distance = (s.distance * (1.0 + step)).clamp(0.4, 8.0),
            "bigger" => s.width = (s.width * (1.0 + step)).clamp(0.3, 6.0),
            "smaller" => s.width = (s.width / (1.0 + step)).clamp(0.3, 6.0),
            other => return Err(format!("bad direction {other}")),
        }
        Ok(String::new())
    }

    /// `wayvrctl screen-place NAME left|right|up|down|front|swap [--of OTHER]` (WayVR viewlayout.rs `place`).
    /// APPROX: beside the reference (or one place over) at the neighbour angle of a 2 m screen at 2.5 m (45 deg yaw,
    /// 27 deg pitch); front = straight ahead; swap = the two change angles. WayVR's minimal shift of the other screens
    /// is not modelled (no two screens share an angle here unless the test makes them).
    fn place(&mut self, argv: &[String]) -> Result<String, String> {
        let mut of: Option<String> = None;
        let mut rest: Vec<&String> = Vec::new();
        let mut i = 2;
        while i < argv.len() {
            if argv[i] == "--of" {
                of = argv.get(i + 1).cloned();
                i += 2;
            } else {
                rest.push(&argv[i]);
                i += 1;
            }
        }
        let (Some(name), Some(rel)) = (rest.first(), rest.get(1)) else { return Err("screen-place NAME REL".into()) };
        let Some(n) = self.world.screen_by_output(name) else { return Ok(String::new()) };
        let r = of.as_deref().and_then(|o| self.world.screen_by_output(o));
        let (ry, rp) = match r.and_then(|r| self.world.screen_mut(r).map(|s| (s.yaw, s.pitch))) {
            Some(x) => x,
            None => match self.world.screen_mut(n) {
                Some(s) => (s.yaw, s.pitch),
                None => return Ok(String::new()),
            },
        };
        let (y, p) = match rel.as_str() {
            "left" => (ry + 45.0, rp),
            "right" => (ry - 45.0, rp),
            "up" | "above" => (ry, rp + 27.0),
            "down" | "below" => (ry, rp - 27.0),
            "front" | "here" => (0.0, 0.0),
            "swap" => {
                let Some(r) = r else { return Err("swap needs --of".into()) };
                let mine = self.world.screen_mut(n).map(|s| (s.yaw, s.pitch)).unwrap_or((0.0, 0.0));
                if let Some(o) = self.world.screen_mut(r) {
                    (o.yaw, o.pitch) = mine;
                }
                (ry, rp)
            }
            other => return Err(format!("bad place {other}")),
        };
        if let Some(s) = self.world.screen_mut(n).filter(|s| s.shown) {
            s.yaw = y;
            s.pitch = p;
        }
        Ok(String::new())
    }

    fn pimax(&mut self, argv: &[String]) -> Result<String, String> {
        let a: Vec<&str> = argv.iter().map(String::as_str).collect();
        match a.get(1..).unwrap_or(&[]) {
            ["desk", "screens", sub, arg @ ..] => {
                let arg = arg.join(" ");
                self.desk_screens(sub, &arg)
            }
            ["desk", "preset", p, ..] => self.preset(p),
            ["desk", "stop", ..] => {
                self.world.session.running = false;
                self.world.session.mode = "off".into();
                self.world.refresh_shown();
                Ok("omarchyvr desk: stopped".into())
            }
            ["record", "start", ..] => {
                self.world.recording = true;
                Ok("recording".into())
            }
            ["record", "stop", ..] => {
                self.world.recording = false;
                Ok("saved".into())
            }
            ["record", "status", ..] => Ok(if self.world.recording { "recording" } else { "not recording" }.into()),
            _ => self.unknown(argv.join(" ")),
        }
    }

    fn wayvrctl(&mut self, argv: &[String]) -> Result<String, String> {
        let w = &mut self.world;
        match argv.get(1).map(String::as_str) {
            Some("gaze") => w.gaze_json(self.now_ms).ok_or_else(|| "no gaze".to_string()),
            Some("screen-nudge") => self.nudge(argv),
            Some("screen-place") => self.place(argv),
            Some("reset-screens") => {
                // APPROX: every screen back to its place around the head at its distance (size kept)
                for s in w.screens.iter_mut() {
                    let (yaw, pitch) = SLOT_ANGLES[s.n as usize - 1];
                    s.yaw = yaw;
                    s.pitch = pitch;
                }
                Ok(String::new())
            }
            Some("recenter") => {
                w.events.push("recenter".into());
                Ok(String::new())
            }
            Some("controllers") => {
                w.view.controllers = argv.get(2).is_some_and(|x| x == "on");
                Ok(String::new())
            }
            Some("panel-modify") if argv.get(2).is_some_and(|x| x == "perf") => {
                w.view.fps = argv.last().is_some_and(|x| x == "1");
                Ok(String::new())
            }
            Some("screen-numbers") => {
                match argv.get(2).map(String::as_str) {
                    Some("always") => w.view.numbers = "always".into(),
                    Some("hide") | Some("off") => w.view.numbers = "ondemand".into(),
                    _ => {}
                }
                Ok(String::new())
            }
            Some("skybox") => {
                w.view.skybox = argv.get(2).cloned().unwrap_or_default();
                Ok(String::new())
            }
            _ => self.unknown(argv.join(" ")),
        }
    }
}

/// Reads change nothing.
fn is_read(cmd: &str) -> bool {
    cmd.starts_with("hyprctl -j ") || cmd.ends_with(" gaze") || cmd.starts_with("wl-paste") || cmd.contains("record status") || cmd.contains("screen-numbers flash")
}

/// `key = "value"` or `key = value` inside a Lua dispatcher string.
fn lua_arg(lua: &str, key: &str) -> Option<String> {
    let pat = format!("{key} = ");
    let i = lua.find(&pat)? + pat.len();
    let rest = &lua[i..];
    if let Some(r) = rest.strip_prefix('"') {
        r.find('"').map(|j| r[..j].to_string())
    } else {
        Some(rest.split([',', ' ', '}', ')']).next().unwrap_or("").to_string())
    }
}

/// The tab title a site gets.
fn title_for_url(url: &str) -> String {
    if let Some(q) = url.split("search?q=").nth(1) {
        return format!("{} - Google Search", q.replace('+', " "));
    }
    let host = url.trim_start_matches("https://").trim_start_matches("http://").trim_start_matches("www.");
    let host = host.split('/').next().unwrap_or(host);
    for (h, t) in [("mail.google.com", "Gmail - Inbox"), ("youtube.com", "YouTube"), ("github.com", "GitHub"), ("wikipedia.org", "Wikipedia")] {
        if host.ends_with(h) {
            return t.to_string();
        }
    }
    let name = host.split('.').next().unwrap_or(host);
    let mut c = name.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

impl Runner for Sim {
    fn run(&mut self, argv: &[String], _capture: bool) -> Result<String, String> {
        let line = argv.join(" ");
        self.commands.push(line.clone());
        let prog = argv.first().map(String::as_str).unwrap_or("");
        let base = prog.rsplit('/').next().unwrap_or(prog);
        let before = if is_read(&line) { None } else { self.timing_before() };
        let r = match base {
            "hyprctl" => match (argv.get(1).map(String::as_str), argv.get(2).map(String::as_str)) {
                (Some("-j"), Some("clients")) => Ok(self.clients_read(false)),
                (Some("-j"), Some("activewindow")) => Ok(self.clients_read(true)),
                (Some("-j"), Some("monitors")) => Ok(self.monitors_read()),
                (Some("-j"), Some("workspaces")) => Ok(self.world.workspaces_json()),
                (Some("dispatch"), Some(lua)) => {
                    let lua = lua.to_string();
                    self.dispatch(&lua)
                }
                _ => self.unknown(line.clone()),
            },
            "wayvrctl" => self.wayvrctl(argv),
            "pimax" | "omarchyvr" => self.pimax(argv),
            "setsid" => {
                let rest = argv[1..].to_vec();
                self.commands.pop();
                return self.run(&rest, _capture);
            }
            "wtype" => self.wtype(argv),
            "wl-paste" => Ok(self.world.clipboard.clone()),
            "wl-copy" => {
                self.world.clipboard = argv.last().cloned().unwrap_or_default();
                Ok(String::new())
            }
            "omarchy-launch-terminal" => {
                self.world.launch("foot", "user@host:~", Vec::new());
                Ok(String::new())
            }
            "omarchy-launch-browser" => self.browser(argv),
            "omarchy-launch-nautilus" => {
                self.world.launch("org.gnome.Nautilus", "Home", Vec::new());
                Ok(String::new())
            }
            "omarchy-launch-or-focus" => {
                let pat = argv.get(1).cloned().unwrap_or_default().trim_matches(['^', '$']).to_lowercase();
                match self.world.apps.iter().filter(|a| a.class.to_lowercase().contains(&pat)).max_by_key(|a| a.last_focus).map(|a| a.id.clone()) {
                    Some(id) => self.world.focus_app(&id).map(|_| String::new()),
                    None => {
                        let class = argv.get(2).and_then(|c| c.split_whitespace().last()).unwrap_or(&pat).to_string();
                        self.world.launch(&class, &class, Vec::new());
                        Ok(String::new())
                    }
                }
            }
            "mpv" | "xdg-open" => {
                let path = argv.last().cloned().unwrap_or_default();
                let name = path.rsplit('/').next().unwrap_or(&path).to_string();
                self.world.launch("mpv", &format!("{name} - mpv"), Vec::new());
                Ok(String::new())
            }
            "omarchy-audio-output-volume" => {
                let m = &mut self.world.media;
                match argv.get(1).map(String::as_str) {
                    Some("raise") => m.volume = (m.volume + 5).min(100),
                    Some("lower") => m.volume = m.volume.saturating_sub(5),
                    Some("mute-toggle") => m.muted = !m.muted,
                    _ => {}
                }
                Ok(String::new())
            }
            "wpctl" => {
                let v = argv.last().and_then(|x| x.trim_end_matches('%').parse::<u8>().ok());
                match v {
                    Some(v) => {
                        self.world.media.volume = v.min(100);
                        Ok(String::new())
                    }
                    None => self.unknown(line.clone()),
                }
            }
            "omarchy-shell" => {
                let m = &mut self.world.media;
                match (argv.get(1).map(String::as_str), argv.get(2).map(String::as_str)) {
                    (Some("media"), Some("next")) => m.track += 1,
                    (Some("media"), Some("previous")) => m.track -= 1,
                    (Some("media"), Some("playPause")) => m.music = if m.music == "playing" { "paused".into() } else { "playing".into() },
                    (Some("notifications"), _) => self.world.events.push("notifications dismissed".into()),
                    _ => return self.unknown(line.clone()),
                }
                Ok(String::new())
            }
            "omarchy-system-stats" => Ok("CPU 12% | RAM 9.1 GB".into()),
            "omarchy-battery-status" => Ok("No battery".into()),
            "omarchy-network-status" => Ok("Online (ethernet)".into()),
            "omarchy-reminder" => {
                self.world.events.push(format!("reminder {}", argv[1..].join(" ")));
                Ok(String::new())
            }
            "omarchy-capture-screenshot" => {
                self.world.events.push("screenshot".into());
                Ok(String::new())
            }
            "omarchy-theme-bg-next" => {
                self.world.events.push("wallpaper".into());
                Ok(String::new())
            }
            "omarchy-system-lock" => {
                self.world.locked = true;
                Ok(String::new())
            }
            "omarchy-hyprland-window-tiled-fullscreen-toggle" | "omarchy-hyprland-window-pop" | "omarchy-hyprland-window-width" => {
                self.world.events.push(format!("layout {base}"));
                Ok(String::new())
            }
            _ => self.unknown(line.clone()),
        };
        if r.is_ok() && !is_read(&line) {
            self.world.normalize();
            if let Some(b) = before {
                self.timing_after(b);
            }
            self.snap(&line);
        }
        if is_read(&line) {
            self.drift_after_read();
        }
        r
    }
    fn passthrough(&mut self, line: &str) -> Result<String, String> {
        self.commands.push(format!("passthrough {line}"));
        let v = &mut self.world.view;
        match line {
            "normal" | "on" => v.passthrough = "normal".into(),
            "dim" => v.passthrough = "dim".into(),
            "off" => v.passthrough = "off".into(),
            "brighter" => v.brightness += 1,
            "darker" => v.brightness -= 1,
            "status" => return Ok(format!("ok {}", v.passthrough)),
            other => {
                self.unmodelled.push(format!("passthrough {other}"));
            }
        }
        self.snap(&format!("passthrough {line}"));
        Ok(format!("ok {line}"))
    }
    fn toast(&mut self, text: &str) {
        self.toasts.push(text.to_string());
    }
    fn captions(&mut self, on: bool) -> Result<(), String> {
        self.commands.push(format!("captions {}", if on { "on" } else { "off" }));
        self.world.view.captions = on;
        self.snap(&format!("captions {}", if on { "on" } else { "off" }));
        Ok(())
    }
    fn skyboxes(&mut self) -> Vec<String> {
        self.world.skyboxes.clone()
    }
    /// act.rs `Real::screens_on_demand`: `limit = 4` in the state file, the rest kept.
    fn screens_on_demand(&mut self) {
        if self.world.show.limit < 4 {
            self.commands.push("screens limit = 4".into());
            self.world.show.limit = 4;
            self.world.refresh_shown();
            self.snap("screens limit = 4");
        }
    }
    /// act.rs `Real::screen_unhide`: screen `n` out of the hidden list.
    fn screen_unhide(&mut self, n: usize) {
        if self.world.show.hidden.contains(&(n as u8)) {
            self.commands.push(format!("screens unhide {n}"));
            self.world.show.hidden.retain(|h| *h as usize != n);
            self.world.refresh_shown();
            self.snap(&format!("screens unhide {n}"));
        }
    }
    fn screens_sort(&mut self, on: bool) {
        self.commands.push(format!("screens sort = {}", if on { "on" } else { "off" }));
        self.world.show.sort = on;
        self.snap(&format!("screens sort = {}", if on { "on" } else { "off" }));
    }
}

// ----------------------------------------------------------------------------------------------- apply (tools)

/// What one tool did.
#[derive(Debug, Clone)]
pub struct Applied {
    pub world: World,
    pub toasts: Vec<String>,
    /// The commands that changed something.
    pub commands: Vec<String>,
    pub unmodelled: Vec<String>,
}

/// The voice state an [`act::Actor`] keeps (preset and backdrop) from a World.
pub fn actor_state(w: &World) -> act::State {
    let backdrop = match (w.view.passthrough.as_str(), w.view.skybox.as_str()) {
        (_, "default") => "sky".to_string(),
        ("dim", _) => "dimmed".into(),
        ("off", "off") => "black".into(),
        ("off", f) => f.rsplit('/').next().unwrap_or(f).trim_end_matches(".dds").to_string(),
        _ => "passthrough".into(),
    };
    act::State { preset: Some(w.session.preset.clone()), backdrop: Some(backdrop), history: Vec::new(), before_huge: None }
}

/// Run one tool against a World: a catalogue intent id with its slot values, or a window tool (`focus_window
/// window=0x..`, `move_window window=0x.. workspace=N`, `close_window window=0x..`). The intent runs through the real
/// [`act::Actor::execute`] (the same code a voice command runs), its commands land on a [`Sim`]. Err: unknown tool,
/// a missing input, or a command the simulator does not model.
pub fn apply(w: &World, cat: &Catalogue, tool: &str, inputs: &BTreeMap<String, String>) -> Result<Applied, String> {
    let mut sim = Sim::new(w.clone());
    match tool {
        "focus_window" | "move_window" | "close_window" => {
            let win = inputs.get("window").ok_or("window missing")?.clone();
            let id = sim.world.app(&win).map(|a| a.id.clone()).ok_or_else(|| format!("window {win} is not in the list"))?;
            sim.run(&["hyprctl".into(), "dispatch".into(), format!("hl.dsp.focus({{ window = \"address:{id}\" }})")], true)?;
            if tool == "move_window" {
                let n = inputs.get("workspace").ok_or("workspace missing")?;
                sim.run(&["hyprctl".into(), "dispatch".into(), format!("hl.dsp.window.move({{ workspace = \"{n}\", follow = false }})")], true)?;
            } else if tool == "close_window" {
                sim.run(&["hyprctl".into(), "dispatch".into(), format!("hl.dsp.window.close({{ window = \"address:{id}\" }})")], true)?;
            }
        }
        _ => {
            let intent = cat.get(tool).ok_or_else(|| format!("no tool {tool}"))?;
            for (k, _) in &intent.slots {
                if !inputs.contains_key(k) {
                    return Err(format!("{tool}: input {k} missing"));
                }
            }
            let mut actor = act::Actor::new(cat.clone(), Some("wayvrctl".into()), actor_state(w));
            actor.confirm = omarchyvr_config::ai::VoiceConfirm::None;
            actor.last_words = inputs.values().cloned().collect::<Vec<_>>().join(" ");
            actor.execute(&Match { id: tool.to_string(), slots: inputs.clone(), score: 0 }, &mut sim);
            if let Some(p) = actor.state.preset.clone() {
                sim.world.session.preset = p;
            }
        }
    }
    if !sim.unmodelled.is_empty() {
        return Err(format!("not simulated: {}", sim.unmodelled.join("; ")));
    }
    let commands = sim.writes();
    Ok(Applied { world: sim.world, toasts: sim.toasts, commands, unmodelled: sim.unmodelled })
}

// --------------------------------------------------------------------------------------------- describe / diff

/// The compact state text for a prompt. `blocks`: any of screens, apps, focus, gaze, tabs, session, view, media
/// (empty = all). Apps are named by their handle (t1, c1, ...), screens by number.
pub fn describe(w: &World, blocks: &[&str]) -> String {
    let on = |b: &str| blocks.is_empty() || blocks.contains(&b);
    let mut out = Vec::new();
    let app_line = |a: &App| -> String {
        let mut s = format!("{} {} \"{}\"", a.name, a.kind.word(), a.shown_title().chars().take(48).collect::<String>());
        if let Some(c) = &a.cwd {
            s += &format!(" cwd {c}");
        }
        if a.fullscreen {
            s += " fullscreen";
        }
        if w.focus.as_deref() == Some(a.id.as_str()) {
            s += " *focused";
        }
        s
    };
    if on("session") {
        out.push(format!(
            "session: {}, preset {}{}",
            if w.session.running { w.session.mode.as_str() } else { "stopped" },
            w.session.preset,
            if w.recording { ", recording" } else { "" }
        ));
    }
    if on("screens") || on("apps") {
        out.push("screens (1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right):".into());
        for s in w.screens.iter().filter(|s| s.present) {
            let apps: Vec<String> = if on("apps") { w.apps.iter().filter(|a| a.ws == s.ws).map(app_line).collect() } else { Vec::new() };
            let mut l = format!("  {} {}: {}", s.n, s.slot, if s.shown { "shown" } else { "hidden" });
            if on("screens") {
                l += &format!(", {:.1} m wide at {:.1} m", s.width, s.distance);
            }
            l += &if apps.is_empty() { ", empty".to_string() } else { format!(", apps: {}", apps.join("; ")) };
            out.push(l);
        }
        let absent: Vec<String> = w.screens.iter().filter(|s| !s.present).map(|s| s.n.to_string()).collect();
        if !absent.is_empty() {
            out.push(format!("  not available: {} (Screens setting)", absent.join(",")));
        }
        if on("apps") {
            let off: Vec<String> = w.apps.iter().filter(|a| w.screen_of(a).is_none()).map(|a| format!("{} (workspace {})", app_line(a), a.ws)).collect();
            if !off.is_empty() {
                out.push(format!("  not on a screen: {}", off.join("; ")));
            }
        }
    }
    if on("focus") {
        out.push(format!("focus: screen {}{}", w.focused_screen, w.focus.as_ref().and_then(|f| w.app(f)).map(|a| format!(", {}", a.name)).unwrap_or_default()));
    }
    if on("gaze") {
        out.push(match &w.gaze {
            Some(g) => format!("eyes on: screen {}{}", g.screen, g.app.as_ref().and_then(|x| w.app(x)).map(|a| format!(", {}", a.name)).unwrap_or_default()),
            None => "eyes on: unknown".into(),
        });
    }
    if on("tabs") {
        for a in w.apps.iter().filter(|a| a.kind == Kind::Browser) {
            let t: Vec<String> = a.tabs.iter().enumerate().map(|(i, t)| format!("{}{} {}", if i == a.active_tab { "*" } else { "" }, i + 1, t.title)).collect();
            out.push(format!("tabs of {}: {}", a.name, t.join(" | ")));
        }
    }
    if on("view") {
        out.push(format!("view: passthrough {}, background {}, captions {}", w.view.passthrough, w.view.skybox, if w.view.captions { "on" } else { "off" }));
    }
    if on("media") {
        out.push(format!("sound: volume {}{}, music {}", w.media.volume, if w.media.muted { " (muted)" } else { "" }, w.media.music));
    }
    out.join("\n")
}

/// One observable change.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Change {
    /// What changed, as a key the expectations cover: `app:t1:ws`, `app:t1:closed`, `app:new`, `screen:2:shown`,
    /// `screen:2:size`, `screen:2:place`, `view:passthrough`, `session`, `media:volume`, `typed`, `event`, `focus`, ...
    pub key: String,
    pub text: String,
}

/// What changed from `a` to `b` (focus included; callers decide whether it matters).
pub fn diff(a: &World, b: &World) -> Vec<Change> {
    let mut v = Vec::new();
    let mut add = |key: String, text: String| v.push(Change { key, text });
    for x in &a.apps {
        match b.apps.iter().find(|y| y.id == x.id) {
            None => add(format!("app:{}:closed", x.name), format!("{} ({}) closed", x.name, x.class)),
            Some(y) => {
                if x.ws != y.ws {
                    let sa = a.screen_of(x).map(|n| format!("screen {n}")).unwrap_or(format!("ws {}", x.ws));
                    let sb = b.screen_of(y).map(|n| format!("screen {n}")).unwrap_or(format!("ws {}", y.ws));
                    add(format!("app:{}:ws", x.name), format!("{} moved {sa} -> {sb}", x.name));
                }
                if x.fullscreen != y.fullscreen || x.maximized != y.maximized || x.floating != y.floating {
                    add(format!("app:{}:mode", x.name), format!("{} fullscreen {} -> {}", x.name, x.fullscreen, y.fullscreen));
                }
                if x.tabs != y.tabs || x.active_tab != y.active_tab {
                    add(format!("app:{}:tabs", x.name), format!("{} tab {} -> {} ({} tabs -> {})", x.name, x.tabs.get(x.active_tab).map(|t| t.title.as_str()).unwrap_or(""), y.tabs.get(y.active_tab).map(|t| t.title.as_str()).unwrap_or(""), x.tabs.len(), y.tabs.len()));
                }
            }
        }
    }
    for y in b.apps.iter().filter(|y| !a.apps.iter().any(|x| x.id == y.id)) {
        add(format!("app:new:{}", y.kind.word()), format!("opened {} \"{}\" on {}", y.class, y.shown_title(), b.screen_of(y).map(|n| format!("screen {n}")).unwrap_or(format!("ws {}", y.ws))));
    }
    for s in &a.screens {
        let Some(t) = b.screen(s.n) else { continue };
        if s.shown != t.shown {
            add(format!("screen:{}:shown", s.n), format!("screen {} {}", s.n, if t.shown { "shown" } else { "hidden" }));
        }
        if (s.width - t.width).abs() > 0.01 || (s.curvature - t.curvature).abs() > 0.01 {
            add(format!("screen:{}:size", s.n), format!("screen {} width {:.2} -> {:.2} m, curvature {:.2} -> {:.2}", s.n, s.width, t.width, s.curvature, t.curvature));
        }
        if (s.distance - t.distance).abs() > 0.01 {
            add(format!("screen:{}:distance", s.n), format!("screen {} distance {:.2} -> {:.2} m", s.n, s.distance, t.distance));
        }
        if (s.yaw - t.yaw).abs() > 0.5 || (s.pitch - t.pitch).abs() > 0.5 {
            add(format!("screen:{}:place", s.n), format!("screen {} at {:.0},{:.0} -> {:.0},{:.0} deg", s.n, s.yaw, s.pitch, t.yaw, t.pitch));
        }
    }
    if a.session.running != b.session.running {
        add("session".into(), format!("session {}", if b.session.running { "started" } else { "stopped" }));
    }
    if a.session.preset != b.session.preset {
        add("preset".into(), format!("preset {} -> {}", a.session.preset, b.session.preset));
    }
    if a.view.passthrough != b.view.passthrough || a.view.skybox != b.view.skybox || a.view.brightness != b.view.brightness {
        add("view:backdrop".into(), format!("passthrough {} / {} -> {} / {} (brightness {} -> {})", a.view.passthrough, a.view.skybox, b.view.passthrough, b.view.skybox, a.view.brightness, b.view.brightness));
    }
    if a.view.captions != b.view.captions {
        add("view:captions".into(), format!("captions {}", if b.view.captions { "on" } else { "off" }));
    }
    if a.view.fps != b.view.fps || a.view.numbers != b.view.numbers || a.view.controllers != b.view.controllers {
        add("view:other".into(), format!("fps {} numbers {} controllers {}", b.view.fps, b.view.numbers, b.view.controllers));
    }
    if a.media.volume != b.media.volume || a.media.muted != b.media.muted {
        add("media:volume".into(), format!("volume {}{} -> {}{}", a.media.volume, if a.media.muted { " muted" } else { "" }, b.media.volume, if b.media.muted { " muted" } else { "" }));
    }
    if a.media.music != b.media.music || a.media.track != b.media.track {
        add("media:music".into(), format!("music {} track {} -> {} track {}", a.media.music, a.media.track, b.media.music, b.media.track));
    }
    if a.recording != b.recording {
        add("recording".into(), format!("recording {}", if b.recording { "on" } else { "off" }));
    }
    if a.locked != b.locked {
        add("locked".into(), "screen locked".into());
    }
    for t in b.typed.iter().skip(a.typed.len()) {
        add("typed".into(), format!("typed {:?} into {}", t.text, b.app(&t.app).or_else(|| a.app(&t.app)).map(|x| x.name.as_str()).unwrap_or("?")));
    }
    for e in b.events.iter().skip(a.events.len()) {
        add("event".into(), e.clone());
    }
    if a.focus != b.focus {
        let n = |w: &World| w.focus.as_ref().and_then(|f| w.app(f)).map(|x| x.name.clone()).unwrap_or("-".into());
        add("focus".into(), format!("focus {} -> {}", n(a), n(b)));
    }
    v
}

// ------------------------------------------------------------------------------------------------ live snapshot

/// The World from the live desktop, read-only: `hyprctl -j monitors | clients | activewindow`, `wayvrctl gaze`,
/// `$XDG_RUNTIME_DIR/omarchyvr-screens.state`. Not readable live (left at defaults, said in `events`): screen size and
/// place (no wayvrctl command reports them), hidden browser tabs (the voice code only sees the active tab's title),
/// terminal cwd (taken from the title when it looks like `user@host:path`), volume.
pub fn snapshot(r: &mut dyn Runner, wayvrctl: &str) -> World {
    let mut w = World { apps: Vec::new(), workspaces: Vec::new(), ..World::default() };
    let mons: Vec<Value> = r.run(&["hyprctl".into(), "-j".into(), "monitors".into()], true).ok().and_then(|j| serde_json::from_str::<Value>(&j).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    for s in w.screens.iter_mut() {
        s.present = false;
    }
    for m in &mons {
        let name = m["name"].as_str().unwrap_or("").to_string();
        let ws = m.pointer("/activeWorkspace/id").and_then(Value::as_i64).unwrap_or(0);
        let n = if name.starts_with(crate::windows::VR_PREFIX) {
            OUTPUTS.iter().position(|o| *o == name).map(|i| i as u8 + 1)
        } else if m["x"].as_i64() == Some(0) && m["y"].as_i64() == Some(0) {
            w.main_output = name.clone();
            Some(1)
        } else {
            None
        };
        let Some(n) = n else { continue };
        if let Some(s) = w.screens.iter_mut().find(|s| s.n == n) {
            s.present = true;
            s.ws = ws;
        }
        if m["focused"].as_bool() == Some(true) {
            w.focused_screen = n;
        }
    }
    let clients: Vec<Value> = r.run(&["hyprctl".into(), "-j".into(), "clients".into()], true).ok().and_then(|j| serde_json::from_str::<Value>(&j).ok()).and_then(|v| v.as_array().cloned()).unwrap_or_default();
    let mut counts: BTreeMap<&'static str, usize> = BTreeMap::new();
    for c in &clients {
        let class = c["class"].as_str().unwrap_or("").to_string();
        let title = c["title"].as_str().unwrap_or("").to_string();
        let kind = Kind::of(&class);
        let p = match kind {
            Kind::Terminal => "t",
            Kind::Browser => "c",
            Kind::Video => "v",
            Kind::Pimax => "p",
            Kind::Editor => "e",
            Kind::Other => "o",
        };
        let k = counts.entry(p).or_insert(0);
        *k += 1;
        let cwd = (kind == Kind::Terminal).then(|| title.split_once(':').map(|(_, p)| p.trim().to_string())).flatten();
        let tabs = if kind == Kind::Browser { vec![Tab { title: title.trim_end_matches(" - Google Chrome").to_string(), url: String::new() }] } else { Vec::new() };
        w.apps.push(App {
            id: c["address"].as_str().unwrap_or("").to_string(),
            name: format!("{p}{k}"),
            kind,
            class,
            title,
            cwd,
            ws: c.pointer("/workspace/id").and_then(Value::as_i64).unwrap_or(0),
            fullscreen: c["fullscreen"].as_i64().unwrap_or(0) == 2,
            maximized: c["fullscreen"].as_i64().unwrap_or(0) == 1,
            floating: c["floating"].as_bool().unwrap_or(false),
            tabs,
            active_tab: 0,
            last_focus: 1000 - c["focusHistoryID"].as_i64().unwrap_or(0).clamp(0, 999) as u64,
        });
    }
    if let Ok(a) = r.run(&["hyprctl".into(), "-j".into(), "activewindow".into()], true) {
        w.focus = serde_json::from_str::<Value>(&a).ok().and_then(|v| v["address"].as_str().map(str::to_string));
    }
    let st = std::fs::read_to_string(act::screens_state_path()).unwrap_or_default();
    for l in st.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        let nums = |v: &str| -> Vec<u8> { v.split(',').filter_map(|x| slot_number(x.trim())).collect() };
        match k.trim() {
            "limit" => w.show.limit = v.trim().parse().unwrap_or(6),
            "pinned" => w.show.pinned = nums(v),
            "hidden" => w.show.hidden = nums(v),
            "sort" => w.show.sort = matches!(v.trim(), "on" | "true" | "1" | "yes"),
            _ => {}
        }
    }
    if let Ok(g) = r.run(&[wayvrctl.to_string(), "gaze".into()], true) {
        if let Ok(rep) = crate::gaze::parse_report(&g) {
            if let Some(f) = rep.last_stable {
                if let Some(n) = w.screen_by_output(&f.overlay) {
                    w.gaze = Some(GazeAt { screen: n, app: None });
                }
            }
        }
    }
    w.events.push("snapshot: screen sizes/places, hidden tabs and volume are not readable live".into());
    w.normalize();
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalogue::BUILTIN;

    fn desk() -> World {
        let mut w = World::default();
        w.screens[1].present = true;
        w.screens[2].present = true;
        w.apps = vec![
            App { id: "0x1".into(), name: "t1".into(), class: "foot".into(), title: "user@host:~/pimax-re".into(), cwd: Some("~/pimax-re".into()), ws: 1, last_focus: 5, ..App::default() },
            App { id: "0x2".into(), name: "t2".into(), class: "foot".into(), title: "btop".into(), ws: 2, last_focus: 3, ..App::default() },
            App { id: "0x3".into(), name: "c1".into(), class: "google-chrome".into(), tabs: vec![Tab { title: "Gmail".into(), url: "https://mail.google.com".into() }, Tab { title: "YouTube".into(), url: "https://www.youtube.com".into() }], ws: 3, last_focus: 4, ..App::default() },
        ];
        w.focus = Some("0x1".into());
        w.normalize();
        w
    }

    #[test]
    fn shown_follows_the_screens_rule() {
        let w = desk();
        let shown: Vec<u8> = w.screens.iter().filter(|s| s.shown).map(|s| s.n).collect();
        assert_eq!(shown, [1, 2, 3], "busy screens show; 4 is empty");
    }

    #[test]
    fn put_terminals_on_screen_3_moves_only_terminals() {
        let cat = Catalogue::from_texts(BUILTIN, None).unwrap();
        let w = desk();
        let inputs: BTreeMap<String, String> = [("screen".to_string(), "3".to_string()), ("app".to_string(), "terminals".to_string())].into();
        let out = apply(&w, &cat, "win.screen.put_n", &inputs).unwrap();
        let ws3 = out.world.screen(3).unwrap().ws;
        assert!(out.world.apps.iter().filter(|a| a.kind == Kind::Terminal).all(|a| a.ws == ws3), "{:?}", out.commands);
        assert_eq!(out.world.app("c1").unwrap().ws, ws3);
        assert_eq!(out.world.apps.len(), 3, "nothing closed");
    }

    #[test]
    fn nudge_and_scale_follow_wayvr() {
        let mut s = Sim::new(desk());
        s.run(&["wayvrctl".into(), "screen-nudge".into(), "--step".into(), "1".into(), "PIMAX-LEFT".into(), "smaller".into()], true).unwrap();
        assert!((s.world.screen(2).unwrap().width - 0.8).abs() < 1e-3);
        s.run(&["wayvrctl".into(), "screen-nudge".into(), "PIMAX-UP".into(), "bigger".into()], true).unwrap();
        assert!((s.world.screen(4).unwrap().width - 1.6).abs() < 1e-3, "a hidden screen does not move");
    }

    #[test]
    fn hide_keeps_apps_and_close_window_closes_them() {
        let mut s = Sim::new(desk());
        let out = s.run(&["omarchyvr".into(), "desk".into(), "screens".into(), "hide".into(), "2".into()], true).unwrap();
        assert!(out.contains("Hid screen 2"));
        assert!(!s.world.screen(2).unwrap().shown);
        assert_eq!(s.world.apps.len(), 3);
        s.run(&["hyprctl".into(), "dispatch".into(), "hl.dsp.window.close({ window = \"address:0x2\" })".into()], true).unwrap();
        assert_eq!(s.world.apps.len(), 2);
        let d = diff(&desk(), &s.world);
        assert!(d.iter().any(|c| c.key == "app:t2:closed"));
    }

    fn argv(s: &str) -> Vec<String> {
        s.split_whitespace().map(str::to_string).collect()
    }

    #[test]
    fn every_write_is_kept_in_the_history_and_reads_are_not() {
        let mut s = Sim::new(desk());
        s.run(&argv("hyprctl -j clients"), true).unwrap();
        assert!(s.history.is_empty());
        s.run(&argv("omarchy-launch-terminal"), true).unwrap();
        s.run(&argv("wl-paste"), true).unwrap();
        s.run(&argv("omarchy-audio-output-volume raise"), true).unwrap();
        assert_eq!(s.history.len(), 2);
        assert_eq!(s.history[0].1.apps.len(), 4);
        assert_eq!(s.history[1].1.media.volume, s.history[0].1.media.volume + 5);
    }

    #[test]
    fn a_launched_app_shows_in_clients_after_the_delay() {
        let mut w = desk();
        w.timing.launch_delay_reads = 2;
        let mut s = Sim::new(w);
        s.run(&argv("omarchy-launch-terminal"), true).unwrap();
        let n = |s: &mut Sim| serde_json::from_str::<Value>(&s.run(&argv("hyprctl -j clients"), true).unwrap()).unwrap().as_array().unwrap().len();
        assert_eq!((n(&mut s), n(&mut s), n(&mut s)), (3, 3, 4), "two reads without it, the third has it");
        assert_eq!(s.world.apps.len(), 4, "it was there all along in the world");
    }

    #[test]
    fn a_new_screen_shows_in_monitors_after_the_delay() {
        let mut w = desk();
        w.timing.screen_delay_reads = 1;
        let mut s = Sim::new(w);
        let mons = |s: &mut Sim| serde_json::from_str::<Value>(&s.run(&argv("hyprctl -j monitors"), true).unwrap()).unwrap().as_array().unwrap().len();
        assert_eq!(mons(&mut s), 4, "screens 1-4 are there; 4 is empty and hidden");
        s.run(&argv("omarchyvr desk screens more 1"), true).unwrap();
        assert!(s.world.screens[3].shown);
        assert_eq!((mons(&mut s), mons(&mut s)), (3, 4), "the newly shown screen is missing for one read");
    }

    #[test]
    fn drift_happens_once_after_the_first_read() {
        let mut w = desk();
        w.timing.drift = vec!["focus t2".into(), "close c1".into(), "gaze 3".into()];
        let mut s = Sim::new(w);
        let first = s.run(&argv("hyprctl -j clients"), true).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&first).unwrap().as_array().unwrap().len(), 3, "the first read still shows the old desk");
        assert!(s.world.app("c1").is_none() && s.world.focus.as_deref() == Some("0x2") && s.world.gaze.as_ref().map(|g| g.screen) == Some(3));
        assert!(s.history.is_empty(), "a drift is not a write of the voice code");
        s.world.drift(&["open foot 2".to_string()]);
        let again = s.run(&argv("hyprctl -j clients"), true).unwrap();
        assert_eq!(serde_json::from_str::<Value>(&again).unwrap().as_array().unwrap().len(), 3);
        assert_eq!(s.world.apps.len(), 3, "once: the second read changed nothing");
    }

    #[test]
    fn the_world_round_trips_as_json() {
        let w = desk();
        let j = serde_json::to_string(&w).unwrap();
        let back: World = serde_json::from_str(&j).unwrap();
        assert_eq!(back, w);
        assert!(describe(&w, &[]).contains("1 main: shown"));
    }
}
