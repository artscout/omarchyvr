//! The simple screen state the voice decides window moves on (docs/SCREENS.md "What voice knows").
//!
//! The state is only: per screen NUMBER (1 main, 2 left, 3 right, 4 up, 5 up-left, 6 up-right, found by OUTPUT
//! NAME, never by order) whether it exists and is shown, the workspace it shows and the windows on it (address,
//! class, title); the windows no screen shows; and the VR background and preset. It is read FRESH for every
//! command from one `hyprctl -j monitors` + one `hyprctl -j clients` (plus the screens state file), never cached.
//!
//! A move request becomes a DESIRED state in the same shape (window address -> screen number). The moves are
//! exactly the diff. After the moves the state is read again and compared; the windows still wrong are moved ONE
//! more time, read again, and whatever is still wrong is reported in plain words ([`Report::say`]). Nothing is
//! claimed that the second read did not show (live 2026-10-17: "Terminals already on screen 3" while the user saw
//! one on the main screen; "google-chrome on screen 4" on a screen that showed nothing).

use crate::act::Runner;
use crate::windows::{self, Client, SCREEN_ORDER};

/// A window, as much as a move needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Win {
    pub address: String,
    pub class: String,
    pub title: String,
    /// Hyprland's `focusHistoryID` (0 = focused now).
    pub focus: i64,
}

impl Win {
    fn of(c: &Client) -> Win {
        Win { address: c.address.clone(), class: c.class.clone(), title: c.title.clone(), focus: c.focus }
    }

    /// "foot 'update-repo-docs'" (a terminal: its title tells it from the others) / "google-chrome".
    pub fn name(&self) -> String {
        let class = self.class.rsplit('.').next().unwrap_or(&self.class).to_string();
        if !windows::is_terminal(&self.class) {
            return class;
        }
        let t: String = windows::clean_title(&self.title).trim_start_matches(|c: char| !c.is_alphanumeric()).chars().take(28).collect();
        if t.is_empty() || t.eq_ignore_ascii_case(&class) {
            class
        } else {
            format!("{class} '{t}'")
        }
    }
}

/// One screen number.
#[derive(Debug, Clone, PartialEq)]
pub struct ScreenState {
    /// 1..6 (docs/SCREENS.md).
    pub n: u8,
    /// main, left, right, up, up-left, up-right.
    pub slot: &'static str,
    /// The Hyprland output; empty when the screen does not exist.
    pub output: String,
    /// The output is there (the session made it).
    pub exists: bool,
    /// VR draws it now (the screens watcher's rule: main always; an extra one when pinned, with apps or focused,
    /// within the preset's limit, never when hidden by voice).
    pub shown: bool,
    /// The workspace it shows (0 = none).
    pub ws: i64,
    pub wins: Vec<Win>,
}

/// The whole state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct State {
    /// Always 6 entries, screen 1 first.
    pub screens: Vec<ScreenState>,
    /// Windows on a workspace no screen shows: (window, workspace).
    pub off: Vec<(Win, i64)>,
    /// The VR background (`passthrough`, `dimmed`, `black`, `sky`, a picture, `ocean:high`), when known.
    pub backdrop: Option<String>,
    /// The VR preset (assistant, cinema, work1, work3), when known.
    pub preset: Option<String>,
    /// The screen with the keyboard focus (its output is the focused monitor).
    pub focused: Option<u8>,
}

/// The screens state file's fields that decide what VR shows (`$XDG_RUNTIME_DIR/omarchyvr-screens.state`,
/// omarchyvr-tools vr/screens.rs `ShowState`).
#[derive(Debug, Clone, PartialEq)]
struct Show {
    limit: u8,
    pinned: Vec<String>,
    hidden: Vec<String>,
}

fn show_of(text: &str) -> Show {
    let mut s = Show { limit: 6, pinned: Vec::new(), hidden: Vec::new() };
    let list = |v: &str| v.split(',').map(|x| x.trim().to_string()).filter(|x| !x.is_empty() && x != "main").collect::<Vec<_>>();
    for l in text.lines() {
        let Some((k, v)) = l.split_once('=') else { continue };
        match k.trim() {
            "limit" => s.limit = v.trim().parse::<u8>().ok().filter(|n| (1..=6).contains(n)).unwrap_or(6),
            "pinned" => s.pinned = list(v),
            "hidden" => s.hidden = list(v),
            _ => {}
        }
    }
    s
}

impl State {
    /// From the JSON of `hyprctl -j monitors` and `hyprctl -j clients` and the screens state text.
    pub fn from_json(monitors: &str, clients: &str, show: &str) -> Result<State, String> {
        let scr = windows::screens_from_monitors(monitors)?;
        let clients = windows::parse_clients(clients)?;
        let focused_out: Option<String> = scr.iter().find(|s| s.focused).map(|s| s.output.clone());
        let st = show_of(show);
        let mut screens = Vec::with_capacity(6);
        let mut count = 1u8;
        for (i, slot) in SCREEN_ORDER.iter().enumerate() {
            let s = scr.iter().find(|s| s.slot == *slot);
            let ws = s.map(|s| s.ws).unwrap_or(0);
            let mut wins: Vec<Win> = clients.iter().filter(|c| s.is_some() && c.workspace == ws && ws != 0).map(Win::of).collect();
            wins.sort_by_key(|w| w.focus);
            let exists = s.is_some();
            let shown = if i == 0 {
                exists
            } else if !exists || st.hidden.iter().any(|h| h == slot) {
                false
            } else {
                let focused = s.is_some_and(|s| Some(&s.output) == focused_out.as_ref());
                let want = st.pinned.iter().any(|p| p == slot) || !wins.is_empty() || focused;
                let on = (want && count < st.limit) || focused;
                count += u8::from(on);
                on
            };
            screens.push(ScreenState { n: i as u8 + 1, slot, output: s.map(|s| s.output.clone()).unwrap_or_default(), exists, shown, ws, wins });
        }
        let on_screen = |ws: i64| screens.iter().any(|s: &ScreenState| s.exists && s.ws == ws);
        let off = clients.iter().filter(|c| c.workspace > 0 && !on_screen(c.workspace)).map(|c| (Win::of(c), c.workspace)).collect();
        let focused = screens.iter().find(|s| s.exists && Some(&s.output) == focused_out.as_ref()).map(|s| s.n);
        Ok(State { screens, off, backdrop: None, preset: None, focused })
    }

    /// Read it now: one `hyprctl -j monitors`, one `hyprctl -j clients`, the screens state file.
    pub fn read(r: &mut dyn Runner) -> Result<State, String> {
        let m = windows::hyprctl_json(r, "monitors")?;
        let c = windows::hyprctl_json(r, "clients")?;
        let show = r.screens_state();
        State::from_json(&m, &c, &show)
    }

    pub fn screen(&self, n: u8) -> Option<&ScreenState> {
        self.screens.iter().find(|s| s.n == n)
    }

    /// The screens that exist, by number.
    pub fn existing(&self) -> Vec<u8> {
        self.screens.iter().filter(|s| s.exists).map(|s| s.n).collect()
    }

    /// The screen a window is on (None: on no screen, or not open).
    pub fn screen_of(&self, address: &str) -> Option<u8> {
        self.screens.iter().find(|s| s.exists && s.wins.iter().any(|w| w.address == address)).map(|s| s.n)
    }

    /// Every open window (on a screen or not).
    pub fn wins(&self) -> Vec<&Win> {
        self.screens.iter().flat_map(|s| s.wins.iter()).chain(self.off.iter().map(|(w, _)| w)).collect()
    }

    pub fn win(&self, address: &str) -> Option<&Win> {
        self.wins().into_iter().find(|w| w.address == address)
    }

    /// Where a window is, in words: "screen 1 (main)", "workspace 7 (no screen shows it)", "closed".
    pub fn where_is(&self, address: &str) -> String {
        if let Some(n) = self.screen_of(address) {
            return label(n);
        }
        match self.off.iter().find(|(w, _)| w.address == address) {
            Some((_, ws)) => format!("workspace {ws} (no screen shows it)"),
            None => "closed".into(),
        }
    }

    /// The state for the model, one line: `1 main (SUNSHINE) shown [w2 foot] ; 2 left shown [w1 google-chrome] ;
    /// 3 right shown [] ; 4 up hidden [] ; 5 up-left missing ; 6 up-right missing`. `name` gives a window's label
    /// (its `wN` id), else its class is used.
    pub fn screens_line(&self, name: &dyn Fn(&Win) -> String) -> String {
        self.screens
            .iter()
            .map(|s| {
                if !s.exists {
                    return format!("{} {} missing", s.n, s.slot);
                }
                let wins: Vec<String> = s.wins.iter().take(6).map(name).collect();
                format!("{} {} {} [{}]", s.n, s.slot, if s.shown { "shown" } else { "hidden" }, wins.join(", "))
            })
            .collect::<Vec<_>>()
            .join(" ; ")
    }
}

/// "screen 3 (right)".
pub fn label(n: u8) -> String {
    match SCREEN_ORDER.get(usize::from(n).wrapping_sub(1)) {
        Some(s) => format!("screen {n} ({s})"),
        None => format!("screen {n}"),
    }
}

/// A screen that does not exist in this session (the Screens setting can say 6 while the session made 4 outputs):
/// nothing moves, and the words say how to get it.
pub fn missing(n: u8, have: usize) -> String {
    format!(
        "There is no {} now: this Work session has {have} screen{}. Set Screens to {n} or more in the Pimax app and restart Work",
        label(n),
        if have == 1 { "" } else { "s" }
    )
}

/// The desired state: window address -> screen number.
pub type Want = Vec<(String, u8)>;

/// The moves from `s` to `want`: the windows not on their screen (a window that is not open is left out).
pub fn diff(s: &State, want: &[(String, u8)]) -> Want {
    want.iter().filter(|(a, n)| s.win(a).is_some() && s.screen_of(a) != Some(*n)).cloned().collect()
}

/// What happened to a desired state.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Report {
    /// Windows that were already on their screen.
    pub already: Vec<(Win, u8)>,
    /// Windows that are now on their screen after a move (verified by a fresh read).
    pub moved: Vec<(Win, u8)>,
    /// Windows still not there after the retry: (window, wanted screen, where it is, the error if any).
    pub failed: Vec<(Win, u8, String, Option<String>)>,
    /// A second round of moves was needed.
    pub retried: bool,
    /// Screens shown first (they were hidden).
    pub revealed: Vec<u8>,
}

impl Report {
    /// The toast: honest, from the verified end state. `what`: "terminals", "google-chrome", "apps".
    pub fn say(&self, what: &str) -> String {
        let n_moved = self.moved.len();
        let target = |v: &[(Win, u8)]| {
            let mut ns: Vec<u8> = v.iter().map(|x| x.1).collect();
            ns.dedup();
            if ns.len() == 1 { label(ns[0]) } else { "their screens".into() }
        };
        let mut parts = Vec::new();
        if n_moved > 0 {
            let names = if n_moved == 1 { self.moved[0].0.name() } else { format!("{n_moved} {what}") };
            parts.push(format!("Moved {names} to {}", target(&self.moved)));
        }
        for (w, n, at, e) in &self.failed {
            let why = e.as_ref().map(|e| format!(": {e}")).unwrap_or_default();
            parts.push(format!("Could not move {} to {}, it is still on {at}{why}", w.name(), label(*n)));
        }
        if parts.is_empty() && !self.already.is_empty() {
            let all = self.already.len();
            let names = if all == 1 { self.already[0].0.name() } else { format!("all {all} {what}") };
            return format!("{} already on {}", capital(&names), target(&self.already));
        }
        if !self.already.is_empty() && n_moved > 0 {
            parts.push(format!("{} already there", self.already.len()));
        }
        parts.join("; ")
    }

    /// Everything wanted is there.
    pub fn ok(&self) -> bool {
        self.failed.is_empty()
    }
}

fn capital(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
}

/// Reach a desired state: check the target screens exist (else nothing moves and the error says so), show a hidden
/// target screen first, move each window that is not there BY ADDRESS, read the state again, move what is still
/// wrong ONE more time, read again, report. `Err`: nothing was tried (a screen that does not exist, a window that
/// is not open, hyprctl not answering).
pub fn reach(r: &mut dyn Runner, want: &[(String, u8)]) -> Result<Report, String> {
    let s0 = State::read(r)?;
    for (a, n) in want {
        let Some(sc) = s0.screen(*n) else { return Err(format!("Screens are numbered 1 to 6, not {n}")) };
        if !sc.exists {
            return Err(missing(*n, s0.existing().len()));
        }
        if s0.win(a).is_none() {
            return Err(format!("That window ({a}) is not open any more"));
        }
    }
    let mut report = Report::default();
    for (a, n) in want {
        if s0.screen_of(a) == Some(*n) {
            report.already.push((s0.win(a).cloned().unwrap_or_else(|| Win { address: a.clone(), class: "?".into(), title: String::new(), focus: 0 }), *n));
        }
    }
    let todo = diff(&s0, want);
    if todo.is_empty() {
        return Ok(report);
    }
    // a hidden target screen shows first (the screens watcher reads the state file)
    let mut targets: Vec<u8> = todo.iter().map(|x| x.1).collect();
    targets.sort_unstable();
    targets.dedup();
    r.screens_on_demand();
    for n in &targets {
        if s0.screen(*n).is_some_and(|s| !s.shown) {
            report.revealed.push(*n);
        }
        r.screen_unhide(usize::from(*n));
    }
    let focused = windows::hyprctl_json(r, "activewindow").ok().and_then(|j| windows::parse_active(&j).ok().flatten());
    let mut errors: std::collections::HashMap<String, String> = Default::default();
    let mut pending = todo.clone();
    let mut last = s0.clone();
    for round in 0..2 {
        for (a, n) in &pending {
            // the workspace the screen showed before the moves: focusing an app on a workspace no screen shows makes
            // a screen show THAT workspace (Hyprland and the simulator), so a fresh read would name the wrong one
            // (state audit 2026-10-18: "the terminals on 1" sent them to the off-screen workspace 8)
            let ws = s0.screen(*n).map(|s| s.ws).filter(|w| *w != 0).or_else(|| last.screen(*n).map(|s| s.ws)).unwrap_or(0);
            if ws == 0 {
                errors.insert(a.clone(), format!("{} shows no workspace", label(*n)));
                continue;
            }
            if let Err(e) = move_window(r, a, ws) {
                errors.insert(a.clone(), e);
            }
        }
        last = State::read(r)?;
        // the screens show what they showed before (an off-screen app brought its workspace onto a screen)
        let mut restored = false;
        for sc in s0.screens.iter().filter(|x| x.exists && x.ws != 0) {
            if last.screen(sc.n).is_some_and(|x| x.exists && x.ws != sc.ws) {
                let _ = windows::dispatch(r, &format!("hl.dsp.focus({{ workspace = \"{}\" }})", sc.ws));
                restored = true;
            }
        }
        if restored {
            last = State::read(r)?;
        }
        pending = diff(&last, &pending);
        if pending.is_empty() {
            break;
        }
        if round == 0 {
            report.retried = true;
        }
    }
    if let Some(f) = &focused {
        if last.win(&f.address).is_some() {
            let _ = windows::focus_address(r, &f.address);
        }
    }
    for (a, n) in &todo {
        let w = last.win(a).cloned().or_else(|| s0.win(a).cloned()).unwrap_or_default();
        if pending.iter().any(|(p, _)| p == a) {
            report.failed.push((w, *n, last.where_is(a), errors.get(a).cloned()));
        } else if last.screen_of(a) == Some(*n) {
            report.moved.push((w, *n));
        } else {
            // closed meanwhile
            report.failed.push((w, *n, last.where_is(a), None));
        }
    }
    Ok(report)
}

impl Default for Win {
    fn default() -> Self {
        Win { address: String::new(), class: "?".into(), title: String::new(), focus: i64::MAX }
    }
}

/// After a window was focused ("open the browser" when one is open): its screen shows, or, when no screen shows its
/// workspace, it comes to the focused screen (verified). The words say where it really is (live 2026-10-17:
/// "Browser (workspace 4)" while screen 4 was not drawn).
pub fn show_window(r: &mut dyn Runner, address: &str, name: &str) -> String {
    let Ok(st) = State::read(r) else { return name.to_string() };
    if let Some(n) = st.screen_of(address) {
        r.screens_on_demand();
        r.screen_unhide(usize::from(n));
        return format!("{name} on {}", label(n));
    }
    if st.win(address).is_none() {
        return format!("{name} is not open any more");
    }
    let to = st.focused.unwrap_or(1);
    match reach(r, &[(address.to_string(), to)]) {
        Ok(rep) => {
            let _ = windows::focus_address(r, address);
            rep.say(name)
        }
        Err(e) => e,
    }
}

/// The steps of one plan (or goal): one fresh read after each step, so the next one sees what it did (lesson 70),
/// and the window a step acted on. A move keeps the focus where it was (`follow = false`, the focus goes back), so
/// "move chrome to screen 3 and make it fullscreen" made the TERMINAL fullscreen (2026-10-18): a later step on
/// "the focused window" that names no window first focuses the window the step before moved, opened or focused.
pub struct Steps {
    on: bool,
    last: Option<State>,
    acted: Option<String>,
}

impl Steps {
    /// `on`: a plan of several steps (one step reads nothing extra).
    pub fn start(r: &mut dyn Runner, on: bool) -> Steps {
        Steps { on, last: if on { State::read(r).ok() } else { None }, acted: None }
    }

    /// The window the first window step acts on unless a step moves, opens or focuses another ("this": the app
    /// looked at when the command started).
    pub fn seed(&mut self, address: &str) {
        if self.last.as_ref().is_some_and(|s| s.win(address).is_some()) {
            self.acted = Some(address.to_string());
        }
    }

    /// Before a step. `focused_step`: it acts on the focused window and names none (fullscreen, float, close).
    pub fn aim(&mut self, r: &mut dyn Runner, focused_step: bool) {
        let (Some(a), Some(s)) = (self.acted.as_ref(), self.last.as_ref()) else { return };
        if !focused_step || s.win(a).is_none_or(|w| w.focus == 0) {
            return;
        }
        let _ = windows::focus_address(r, a);
        if let Some(w) = self.last.as_mut().and_then(|s| s.screens.iter_mut().flat_map(|x| x.wins.iter_mut()).find(|w| &w.address == a)) {
            w.focus = 0;
        }
    }

    /// After a step: read again; the window it moved (else the one it brought into focus) is the one acted on.
    pub fn after(&mut self, r: &mut dyn Runner) {
        if !self.on {
            return;
        }
        let Ok(now) = State::read(r) else { return };
        if let Some(before) = &self.last {
            if let Some(a) = acted_on(before, &now) {
                self.acted = Some(a);
            }
        }
        self.last = Some(now);
    }

    /// After a step that named an app (`app="chrome"`): when exactly one window fits the name, it is the one the
    /// next window step acts on, also when the step moved nothing (Chrome already on screen 2: "chrome on two
    /// fullscreen" made the terminal moved before it fullscreen, state audit 2026-10-18). Several fit ("the
    /// terminals"): the window the step moved stays the one acted on.
    pub fn named(&mut self, app: &str) {
        let Some(s) = self.last.as_ref() else { return };
        let words: Vec<String> = crate::command::words(app).into_iter().filter(|w| w.len() >= 3 && !matches!(w.as_str(), "the" | "app" | "window" | "one" | "this" | "that")).collect();
        if words.is_empty() {
            return;
        }
        let all: Vec<&Win> = s.screens.iter().flat_map(|x| x.wins.iter()).chain(s.off.iter().map(|(w, _)| w)).collect();
        let fits = |w: &Win| {
            let hay = format!("{} {}", w.class, w.title).to_lowercase();
            words.iter().all(|q| hay.contains(q.trim_end_matches('s')))
        };
        let hits: Vec<&&Win> = all.iter().filter(|w| fits(w)).collect();
        if hits.len() == 1 {
            self.acted = Some(hits[0].address.clone());
        }
    }

    /// The window the last step acted on (tests, logs).
    pub fn acted(&self) -> Option<&str> {
        self.acted.as_deref()
    }
}

/// Where a window is: its screen, else its off-screen workspace.
fn place(s: &State, address: &str) -> Option<(Option<u8>, i64)> {
    if let Some(n) = s.screen_of(address) {
        return Some((Some(n), 0));
    }
    s.off.iter().find(|(w, _)| w.address == address).map(|(_, ws)| (None, *ws))
}

/// The window a step acted on, from the reads before and after it: the first one that changed place, else a new
/// window or one that came into focus.
pub fn acted_on(before: &State, after: &State) -> Option<String> {
    let all = |s: &State| -> Vec<Win> { s.screens.iter().flat_map(|x| x.wins.iter().cloned()).chain(s.off.iter().map(|(w, _)| w.clone())).collect() };
    let now = all(after);
    if let Some(w) = now.iter().find(|w| place(before, &w.address).is_some_and(|p| Some(p) != place(after, &w.address))) {
        return Some(w.address.clone());
    }
    let was = all(before).into_iter().find(|w| w.focus == 0).map(|w| w.address);
    now.iter().find(|w| w.focus == 0 && Some(&w.address) != was.as_ref()).map(|w| w.address.clone())
}

/// Move one window to a workspace by its address. Hyprland 0.56's `window.move` acts on the focused window, so the
/// window is focused first by address; whether it really moved is checked by the caller's fresh read, never assumed
/// (a fullscreen window can keep the focus, and then the move would take the wrong window).
fn move_window(r: &mut dyn Runner, address: &str, ws: i64) -> Result<(), String> {
    windows::focus_address(r, address)?;
    windows::dispatch(r, &format!("hl.dsp.window.move({{ workspace = \"{ws}\", follow = false }})"))
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::{json, Value};

    /// A stateful fake Hyprland: monitors with their workspaces, clients that really move on
    /// `focus` + `window.move`. `ignore_moves` = how many moves are silently ignored ("ok", nothing moves; usize::MAX:
    /// every one), `fail_moves` = moves that return an error.
    pub struct Fake {
        pub mons: Vec<(String, i64, i64, i64, bool)>,
        pub wins: Vec<(String, String, String, i64, bool)>,
        pub focused: Option<String>,
        pub ignore_moves: usize,
        pub fail_moves: bool,
        pub dispatched: Vec<String>,
        pub toasts: Vec<String>,
        pub show: String,
        pub unhidden: Vec<usize>,
        pub ran: Vec<String>,
    }

    impl Fake {
        /// Today's (2026-10-17 17:49) real desk: SUNSHINE main ws5, PIMAX-LEFT ws1, PIMAX-RIGHT ws3, PIMAX-UP ws4;
        /// HDMI-A-1 mirrors SUNSHINE. foot "update-repo-docs-honesty" fullscreen on ws5 (main), foot "PC shutdown"
        /// and the omarchyvr app on ws3 (right), google-chrome on ws4 (up).
        pub fn today() -> Fake {
            Fake {
                mons: vec![
                    ("SUNSHINE".into(), 0, 0, 5, true),
                    ("PIMAX-LEFT".into(), -1920, 0, 1, false),
                    ("PIMAX-RIGHT".into(), 1920, 0, 3, false),
                    ("PIMAX-UP".into(), 0, -1080, 4, false),
                ],
                wins: vec![
                    ("0x607999351940".into(), "foot".into(), "◑ update-repo-docs-honesty".into(), 5, true),
                    ("0x6079993b3f80".into(), "foot".into(), "✳ PC shutdown".into(), 3, false),
                    ("0x6079994c57c0".into(), "omarchyvr".into(), "omarchyvr".into(), 3, false),
                    ("0x6079996f7dd0".into(), "google-chrome".into(), "Dune: Part Three | Official Trailer".into(), 4, false),
                ],
                focused: Some("0x6079996f7dd0".into()),
                ignore_moves: 0,
                fail_moves: false,
                dispatched: vec![],
                toasts: vec![],
                show: String::new(),
                unhidden: vec![],
                ran: vec![],
            }
        }

        pub fn ws_of(&self, a: &str) -> i64 {
            self.wins.iter().find(|w| w.0 == a).map(|w| w.3).unwrap_or(-1)
        }

        fn monitors_json(&self) -> String {
            let mut v: Vec<Value> = vec![json!({"name": "HDMI-A-1", "width": 1920, "height": 1080, "x": 0, "y": 0, "focused": false,
                "disabled": false, "mirrorOf": "2", "activeWorkspace": {"id": 0, "name": ""}})];
            for (n, x, y, ws, f) in &self.mons {
                v.push(json!({"name": n, "width": 1920, "height": 1080, "x": x, "y": y, "focused": f, "disabled": false,
                    "mirrorOf": "none", "activeWorkspace": {"id": ws, "name": ws.to_string()}}));
            }
            Value::Array(v).to_string()
        }

        fn clients_json(&self) -> String {
            let focus_rank = |a: &str| if self.focused.as_deref() == Some(a) { 0 } else { 1 };
            Value::Array(
                self.wins
                    .iter()
                    .map(|(a, c, t, ws, fs)| {
                        json!({"address": a, "mapped": true, "class": c, "initialClass": c, "title": t, "fullscreen": if *fs { 1 } else { 0 },
                            "workspace": {"id": ws, "name": ws.to_string()}, "pid": 1, "grouped": [], "focusHistoryID": focus_rank(a)})
                    })
                    .collect(),
            )
            .to_string()
        }
    }

    impl Runner for Fake {
        fn run(&mut self, argv: &[String], _c: bool) -> Result<String, String> {
            self.ran.push(argv.join(" "));
            if argv[0] != "hyprctl" {
                return Ok(String::new());
            }
            match (argv[1].as_str(), argv.get(2).map(String::as_str)) {
                ("-j", Some("clients")) => Ok(self.clients_json()),
                ("-j", Some("monitors")) => Ok(self.monitors_json()),
                ("-j", Some("activewindow")) => Ok(match &self.focused {
                    Some(a) => self.clients_json().parse::<Value>().ok().and_then(|v| v.as_array().and_then(|x| x.iter().find(|w| w["address"] == a.as_str()).cloned())).unwrap_or(json!({})).to_string(),
                    None => "{}".into(),
                }),
                ("dispatch", Some(lua)) => {
                    self.dispatched.push(lua.to_string());
                    if let Some(a) = lua.split("address:").nth(1).and_then(|x| x.split('"').next()) {
                        if lua.starts_with("hl.dsp.focus") {
                            self.focused = Some(a.to_string());
                        }
                        return Ok("ok".into());
                    }
                    if lua.starts_with("hl.dsp.window.fullscreen") {
                        let f = self.focused.clone();
                        if let Some(w) = self.wins.iter_mut().find(|w| Some(&w.0) == f.as_ref()) {
                            w.4 = !w.4;
                        }
                        return Ok("ok".into());
                    }
                    if lua.starts_with("hl.dsp.window.move") {
                        if self.fail_moves {
                            return Ok("error: no such workspace".into());
                        }
                        if self.ignore_moves > 0 {
                            self.ignore_moves = self.ignore_moves.saturating_sub(if self.ignore_moves == usize::MAX { 0 } else { 1 });
                            return Ok("ok".into());
                        }
                        let ws: i64 = lua.split("workspace = \"").nth(1).and_then(|x| x.split('"').next()).and_then(|x| x.parse().ok()).unwrap_or(0);
                        let f = self.focused.clone();
                        if let Some(w) = self.wins.iter_mut().find(|w| Some(&w.0) == f.as_ref()) {
                            w.3 = ws;
                        }
                    }
                    Ok("ok".into())
                }
                _ => Err(format!("unexpected {argv:?}")),
            }
        }
        fn passthrough(&mut self, _l: &str) -> Result<String, String> {
            Err("no".into())
        }
        fn toast(&mut self, t: &str) {
            self.toasts.push(t.to_string());
        }
        fn captions(&mut self, _on: bool) -> Result<(), String> {
            Ok(())
        }
        fn skyboxes(&mut self) -> Vec<String> {
            vec![]
        }
        fn screens_state(&mut self) -> String {
            self.show.clone()
        }
        fn screen_unhide(&mut self, n: usize) {
            self.unhidden.push(n);
        }
    }

    #[test]
    fn todays_state_maps_screens_by_output_name_not_order() {
        let mut f = Fake::today();
        let s = State::read(&mut f).unwrap();
        assert_eq!(s.existing(), vec![1, 2, 3, 4], "screens 5 and 6 do not exist (Screens = 6 in the app, 4 outputs)");
        assert_eq!(s.screen(1).unwrap().output, "SUNSHINE");
        assert_eq!(s.screen(1).unwrap().ws, 5);
        assert_eq!(s.screen_of("0x607999351940"), Some(1), "the fullscreen terminal on ws5 is on the main screen");
        assert_eq!(s.screen_of("0x6079993b3f80"), Some(3));
        assert_eq!(s.screen_of("0x6079996f7dd0"), Some(4), "chrome on ws4 is on the up screen");
        assert!(s.screen(2).unwrap().wins.is_empty());
        assert!(!s.screen(5).unwrap().exists && !s.screen(5).unwrap().shown);
        assert_eq!(s.screens_line(&|w: &Win| w.class.clone()), "1 main shown [foot] ; 2 left hidden [] ; 3 right shown [foot, omarchyvr] ; 4 up shown [google-chrome] ; 5 up-left missing ; 6 up-right missing");
    }

    #[test]
    fn shown_follows_the_watcher_rule() {
        let mut f = Fake::today();
        f.show = "limit = 1\nhidden = up\n".into();
        let s = State::read(&mut f).unwrap();
        assert!(s.screen(1).unwrap().shown);
        assert!(!s.screen(3).unwrap().shown, "cinema: limit 1, only main");
        assert!(!s.screen(4).unwrap().shown, "hidden by voice");
    }

    #[test]
    fn reach_moves_only_what_is_wrong_and_verifies() {
        let mut f = Fake::today();
        let want = vec![("0x607999351940".to_string(), 3), ("0x6079993b3f80".to_string(), 3)];
        let r = reach(&mut f, &want).unwrap();
        assert_eq!(f.ws_of("0x607999351940"), 3);
        assert_eq!(r.moved.len(), 1);
        assert_eq!(r.already.len(), 1);
        assert!(!r.retried && r.ok());
        assert_eq!(r.say("terminals"), "Moved foot 'update-repo-docs-honesty' to screen 3 (right); 1 already there");
        assert_eq!(f.dispatched.iter().filter(|d| d.starts_with("hl.dsp.window.move")).count(), 1, "one move, by address");
        assert_eq!(f.focused.as_deref(), Some("0x6079996f7dd0"), "the focus comes back");
    }

    #[test]
    fn reach_retries_once_when_the_first_move_is_ignored() {
        let mut f = Fake::today();
        f.ignore_moves = 1;
        let r = reach(&mut f, &[("0x607999351940".to_string(), 3)]).unwrap();
        assert!(r.retried && r.ok(), "{r:?}");
        assert_eq!(f.ws_of("0x607999351940"), 3);
        assert_eq!(f.dispatched.iter().filter(|d| d.starts_with("hl.dsp.window.move")).count(), 2);
        assert_eq!(r.say("terminals"), "Moved foot 'update-repo-docs-honesty' to screen 3 (right)");
    }

    #[test]
    fn reach_reports_honestly_after_exactly_one_retry() {
        let mut f = Fake::today();
        f.ignore_moves = usize::MAX;
        let r = reach(&mut f, &[("0x607999351940".to_string(), 3)]).unwrap();
        assert!(r.retried && !r.ok());
        assert_eq!(f.dispatched.iter().filter(|d| d.starts_with("hl.dsp.window.move")).count(), 2, "exactly one retry");
        assert_eq!(r.say("terminals"), "Could not move foot 'update-repo-docs-honesty' to screen 3 (right), it is still on screen 1 (main)");
    }

    #[test]
    fn a_move_error_is_reported_not_claimed() {
        let mut f = Fake::today();
        f.fail_moves = true;
        let r = reach(&mut f, &[("0x6079996f7dd0".to_string(), 1)]).unwrap();
        assert!(!r.ok());
        let s = r.say("x");
        assert_eq!(s, "Could not move google-chrome to screen 1 (main), it is still on screen 4 (up): error: no such workspace");
    }

    #[test]
    fn a_missing_screen_moves_nothing() {
        let mut f = Fake::today();
        let e = reach(&mut f, &[("0x6079993b3f80".to_string(), 5)]).unwrap_err();
        assert!(e.contains("There is no screen 5 (up-left) now") && e.contains("restart Work"), "{e}");
        assert!(f.dispatched.is_empty());
    }

    #[test]
    fn a_closed_window_is_said() {
        let mut f = Fake::today();
        assert!(reach(&mut f, &[("0xdead".to_string(), 3)]).unwrap_err().contains("not open"));
    }

    #[test]
    fn already_there_is_a_true_no_op() {
        let mut f = Fake::today();
        let r = reach(&mut f, &[("0x6079993b3f80".to_string(), 3)]).unwrap();
        assert!(f.dispatched.is_empty());
        assert_eq!(r.say("terminals"), "Foot 'PC shutdown' already on screen 3 (right)");
    }

    #[test]
    fn a_hidden_target_shows_first() {
        let mut f = Fake::today();
        f.show = "hidden = left\n".into();
        let r = reach(&mut f, &[("0x6079993b3f80".to_string(), 2)]).unwrap();
        assert_eq!(r.revealed, vec![2]);
        assert!(f.unhidden.contains(&2));
        assert_eq!(f.ws_of("0x6079993b3f80"), 1);
    }
}

/// The voice window commands against REAL states (fixtures of hyprctl output, today's exact case first): the
/// state they end in must be the DESIRED one, and the toast must say what really happened.
#[cfg(test)]
mod cases {
    use super::tests::Fake;
    use super::*;

    const TERM_MAIN: &str = "0x607999351940"; // foot "update-repo-docs-honesty", ws5 = screen 1 main, fullscreen
    const TERM_RIGHT: &str = "0x6079993b3f80"; // foot "PC shutdown", ws3 = screen 3 right
    const APP: &str = "0x6079994c57c0"; // omarchyvr, ws3
    const CHROME: &str = "0x6079996f7dd0"; // ws4 = screen 4 up

    struct Case {
        name: &'static str,
        setup: fn(&mut Fake),
        args: &'static [&'static str],
        /// (address, the screen it must end on)
        want: &'static [(&'static str, u8)],
        /// the toast must contain this
        say: &'static str,
        /// how many `window.move` dispatches (None: not checked)
        moves: Option<usize>,
    }

    fn none(_: &mut Fake) {}
    fn chrome_left(f: &mut Fake) {
        f.wins[3].3 = 1;
    }
    fn ignore_one(f: &mut Fake) {
        f.ignore_moves = 1;
    }
    fn ignore_all(f: &mut Fake) {
        f.ignore_moves = usize::MAX;
    }
    fn errors(f: &mut Fake) {
        f.fail_moves = true;
    }
    fn up_hidden(f: &mut Fake) {
        f.show = "limit = 1\n".into();
    }

    const CASES: &[Case] = &[
        // live 2026-10-17 17:50 "move all the terminals to the right": one terminal was on main
        Case { name: "today: terminals to the right", setup: none, args: &["put", "right", "terminals"], want: &[(TERM_MAIN, 3), (TERM_RIGHT, 3), (CHROME, 4)], say: "Moved foot 'update-repo-docs-honesty' to screen 3 (right); 1 already there", moves: Some(1) },
        // "move all the terminals to screen number three"
        Case { name: "today: terminals to screen 3", setup: none, args: &["put", "3", "terminals"], want: &[(TERM_MAIN, 3), (TERM_RIGHT, 3)], say: "Moved foot 'update-repo-docs-honesty' to screen 3 (right)", moves: Some(1) },
        // "move all the terminals" (together, where the one used last is)
        Case { name: "today: terminals together", setup: none, args: &["gather", "terminals"], want: &[(TERM_MAIN, 1), (TERM_RIGHT, 1)], say: "Moved foot 'PC shutdown' to screen 1 (main); 1 already there", moves: Some(1) },
        Case { name: "terminals from mixed screens to screen 4", setup: none, args: &["put", "4", "terminals"], want: &[(TERM_MAIN, 4), (TERM_RIGHT, 4), (APP, 3)], say: "Moved 2 terminals to screen 4 (up)", moves: Some(2) },
        Case { name: "one app to a screen", setup: none, args: &["put", "1", "chrome"], want: &[(CHROME, 1), (TERM_MAIN, 1)], say: "Moved google-chrome to screen 1 (main)", moves: Some(1) },
        Case { name: "already there: a true no-op", setup: none, args: &["put", "3", "omarchyvr"], want: &[(APP, 3)], say: "Omarchyvr already on screen 3 (right)", moves: Some(0) },
        Case { name: "target screen missing", setup: none, args: &["put", "5", "terminals"], want: &[(TERM_MAIN, 1), (TERM_RIGHT, 3)], say: "There is no screen 5 (up-left) now", moves: Some(0) },
        Case { name: "first move ignored: success after one retry", setup: ignore_one, args: &["put", "3", "terminals"], want: &[(TERM_MAIN, 3)], say: "Moved foot 'update-repo-docs-honesty' to screen 3 (right)", moves: Some(2) },
        Case { name: "moves never work: honest failure after one retry", setup: ignore_all, args: &["put", "3", "terminals"], want: &[(TERM_MAIN, 1)], say: "Could not move foot 'update-repo-docs-honesty' to screen 3 (right), it is still on screen 1 (main)", moves: Some(2) },
        Case { name: "move errors are reported", setup: errors, args: &["put", "3", "terminals"], want: &[(TERM_MAIN, 1)], say: "Could not move foot 'update-repo-docs-honesty'", moves: None },
        // "can you open the browser in in 4" (stutter): chrome comes to screen 4
        Case { name: "open the browser in in 4", setup: chrome_left, args: &["open_app", "can you open the browser in in 4"], want: &[(CHROME, 4)], say: "Moved google-chrome to screen 4 (up)", moves: Some(1) },
        Case { name: "open the browser on screen 1, moves ignored", setup: ignore_all, args: &["open_app", "browser on screen 1"], want: &[(CHROME, 4)], say: "Could not move google-chrome to screen 1 (main), it is still on screen 4 (up)", moves: Some(2) },
        // chrome already on 4 but screen 4 not shown (cinema limit): it shows, and the toast says so
        Case { name: "open the browser on 4: already there but hidden", setup: up_hidden, args: &["open_app", "browser on screen 4"], want: &[(CHROME, 4)], say: "google-chrome on screen 4 (up)", moves: Some(0) },
    ];

    #[test]
    fn window_commands_reach_the_desired_state() {
        let mut bad = Vec::new();
        for c in CASES {
            let mut f = Fake::today();
            (c.setup)(&mut f);
            let args: Vec<String> = c.args.iter().map(|s| s.to_string()).collect();
            crate::windows::execute(&args, "", &mut None, "wayvrctl", &mut f);
            let end = State::read(&mut f).unwrap();
            for (a, n) in c.want {
                if end.screen_of(a) != Some(*n) {
                    bad.push(format!("{}: {a} on {:?}, wanted screen {n}", c.name, end.screen_of(a)));
                }
            }
            let toast = f.toasts.join(" | ");
            if !toast.contains(c.say) {
                bad.push(format!("{}: said {toast:?}, wanted {:?}", c.name, c.say));
            }
            let moves = f.dispatched.iter().filter(|d| d.starts_with("hl.dsp.window.move")).count();
            if let Some(m) = c.moves.filter(|m| *m != moves) {
                bad.push(format!("{}: {moves} moves, wanted {m}", c.name));
            }
        }
        assert!(bad.is_empty(), "{} of {} cases wrong:\n{}", bad.len(), CASES.len(), bad.join("\n"));
    }

    struct Probe;
    impl crate::agent::Probe for Probe {
        fn vr_running(&mut self) -> bool {
            true
        }
        fn captions_on(&mut self) -> bool {
            false
        }
        fn now(&mut self) -> String {
            "Fri 17:50".into()
        }
    }

    fn context(f: &mut Fake, request: &str) -> crate::agent::Context {
        let cat = crate::catalogue::Catalogue::from_texts(crate::catalogue::BUILTIN, None).unwrap();
        let actor = crate::act::Actor::new(cat, Some("wayvrctl".into()), Default::default());
        crate::agent::Context::collect(request, f, None, &mut Probe, &actor, &Default::default(), false)
    }

    /// The model's per-command message for today's state (live 2026-10-17 17:50 sent "w2 foot ... ws3" for a
    /// terminal on the main screen, with no screens line): every app labelled by the screen it is on, from the
    /// same fresh read, and which screens exist.
    #[test]
    fn the_model_sees_the_screen_each_app_is_on() {
        let mut f = Fake::today();
        let ctx = context(&mut f, "can move all the terminals");
        let (user, ids) = crate::nlu::user_message(&ctx, &[], "can move all the terminals", false);
        assert!(user.contains("foot \"update-repo-docs-honesty\" on 1 main"), "{user}");
        assert!(user.contains("foot \"PC shutdown\" on 3 right"), "{user}");
        assert!(user.contains("google-chrome \"Dune: Part Three | Official Trai\" on 4 up"), "{user}");
        assert!(!user.contains(" ws"), "no bare workspace numbers: {user}");
        let w = |a: &str| format!("w{}", ids.iter().position(|x| x == a).unwrap() + 1);
        let line = user.lines().find(|l| l.starts_with("screens: ")).unwrap_or_else(|| panic!("a screens line: {user}"));
        assert_eq!(
            line,
            format!(
                "screens: 1 main shown [{} foot] ; 2 left hidden [] ; 3 right shown [{} foot, {} omarchyvr] ; 4 up shown [{} google-chrome] ; 5 up-left missing ; 6 up-right missing",
                w(TERM_MAIN),
                w(TERM_RIGHT),
                w(APP),
                w(CHROME)
            )
        );
    }

    /// "can you open the browser in in 4" (live 2026-10-17): app = browser, screen = 4, whatever the stutter.
    #[test]
    fn the_stutter_in_in_4_gives_browser_and_screen_4() {
        assert_eq!(crate::windows::open_target("can you open the browser in in 4"), ("browser".to_string(), Some("4".to_string())));
        let words = crate::command::words("can you open the browser in in 4");
        assert_eq!(crate::act::open_slot(&words), "browser on screen 4");
        assert_eq!(crate::act::open_slot(&crate::command::words("open chrome on the left screen")), "chrome on screen 2");
        assert_eq!(crate::act::open_slot(&crate::command::words("open chrome")), "chrome");
    }

    #[test]
    fn the_browser_launcher_shows_a_hidden_screen_and_says_where() {
        let mut f = Fake::today();
        f.show = "limit = 1\n".into();
        let said = crate::desk::browser(&mut f).unwrap();
        assert!(f.unhidden.contains(&4), "screen 4 shows: {:?}", f.unhidden);
        assert_eq!(said, "Browser on screen 4 (up)");
        // a browser on a workspace no screen shows comes to the main screen
        let mut f = Fake::today();
        f.wins[3].3 = 7;
        let said = crate::desk::browser(&mut f).unwrap();
        assert_eq!(f.ws_of(CHROME), 5, "{said}");
        assert_eq!(said, "Moved google-chrome to screen 1 (main)");
    }

    #[test]
    fn the_window_a_step_moved_is_the_one_a_later_window_step_acts_on() {
        let mut f = Fake::today();
        let term = "0x607999351940";
        let chrome = "0x6079996f7dd0";
        f.focused = Some(term.into());
        f.wins[0].4 = false;
        let mut st = Steps::start(&mut f, true);
        // step 1: chrome to screen 3 (the right one, ws3); the focus goes back to the terminal
        reach(&mut f, &[(chrome.to_string(), 3)]).unwrap();
        assert_eq!(f.focused.as_deref(), Some(term));
        st.after(&mut f);
        assert_eq!(st.acted(), Some(chrome));
        // step 2: fullscreen names no window: chrome first
        st.aim(&mut f, true);
        windows::dispatch(&mut f, "hl.dsp.window.fullscreen({ mode = \"fullscreen\" })").unwrap();
        assert!(f.wins.iter().find(|w| w.0 == chrome).unwrap().4, "chrome fullscreen");
        assert!(!f.wins[0].4, "the terminal untouched");
        // a step that only focuses another window: that one is acted on next
        st.after(&mut f);
        windows::focus_address(&mut f, term).unwrap();
        st.after(&mut f);
        assert_eq!(st.acted(), Some(term));
        // a step that names an app which did not move ("chrome on 4" while it is there): chrome is acted on next
        let mut f = Fake::today();
        f.focused = Some(term.into());
        let mut st = Steps::start(&mut f, true);
        reach(&mut f, &[(chrome.to_string(), 4)]).unwrap();
        st.after(&mut f);
        st.named("chrome");
        assert_eq!(st.acted(), Some(chrome));
        // several fit ("the terminals"): no change from the name
        st.named("foot");
        assert_eq!(st.acted(), Some(chrome));
        // "this": the window looked at when the command started is the first one acted on
        let mut f = Fake::today();
        let mut st = Steps::start(&mut f, true);
        st.seed(chrome);
        assert_eq!(st.acted(), Some(chrome));
        st.seed("0xdead");
        assert_eq!(st.acted(), Some(chrome), "a window that is not open is no seed");
        // one step: no extra reads
        let mut g = Fake::today();
        let mut one = Steps::start(&mut g, false);
        one.after(&mut g);
        one.aim(&mut g, true);
        assert!(g.ran.is_empty(), "{:?}", g.ran);
    }

}
