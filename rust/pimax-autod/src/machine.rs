//! The auto-mode state machine. Pure: no I/O, no clock; the daemon, the simulator and the tests feed it
//! [`Signals`] snapshots with a time, commands and action results, and execute the [`Action`]s it returns.
//!
//! ```text
//! Unplugged --plugged (debounced)--> Plugged --worn (wear_on_s)--> Worn --start ok--> Active(desk|game)
//!     ^                                 ^  \-- session seen (started by hand) ----------> Active(..)
//!     |                                 |                                                   |  not worn dim_after_s
//!     |                       start failed (retry_after_s)                                  v
//!     +---- unplugged (debounced) from anywhere            Sleep <--sleep_after_s-- IdleDim(..) --worn--> Active
//!                                                            \--worn (wear_on_s)--> Worn (start again)
//! ```
//! Nothing flaps: plug/unplug are debounced, wear needs `wear_on_s` of continuous evidence, dimming needs
//! `dim_after_s` of continuous non-wear, sleep another `sleep_after_s`; a failed start waits `retry_after_s` AND a new
//! put-on. Games are never stopped unless `sleep_games`; sessions started by hand only with `sleep_foreign`.
use crate::config::{Config, Mode, Runtime};
use crate::motion::Motion;

/// What kind of session is running (observed or started by us).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Desk,
    Game,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Desk => "desk",
            Self::Game => "game",
        }
    }
}

/// One snapshot of the signals.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Signals {
    /// Headset 34a4:0044 on USB.
    pub plugged: bool,
    /// IMU micro-motion class.
    pub motion: Motion,
    /// An approved, verified wear sensor (none yet: see analysis/auto-mode.md); overrides `motion` when Some.
    pub visor: Option<bool>,
    /// Monado runs with our desk clients (passthrough / wayvr), or Monado alone.
    pub session: Option<Kind>,
    /// A game is running: a Steam game process, SteamVR's vrserver, or a non-overlay main OpenXR app in a
    /// Monado session that is not our desk.
    pub game: bool,
    /// User input this tick (controller buttons / stick / trigger changed).
    pub input: bool,
}

impl Default for Signals {
    fn default() -> Self {
        Self { plugged: false, motion: Motion::Unknown, visor: None, session: None, game: false, input: false }
    }
}

impl Signals {
    /// Wear evidence: the visor sensor if there is one, else the IMU, else recent controller input.
    pub fn worn(&self) -> bool {
        match self.visor {
            Some(v) => v,
            None => self.motion.is_wear() || self.input,
        }
    }
    /// Evidence that it is NOT worn (Unknown motion is no evidence either way).
    pub fn not_worn(&self) -> bool {
        match self.visor {
            Some(v) => !v,
            None => self.motion == Motion::Still && !self.input,
        }
    }
    /// The observed session kind, with a game winning over desk.
    pub fn kind(&self) -> Option<Kind> {
        if self.game {
            Some(Kind::Game)
        } else {
            self.session
        }
    }
}

/// States (see the module docs).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Unplugged,
    Plugged,
    /// Put on: a start is running.
    Worn,
    Active(Kind),
    IdleDim(Kind),
    /// Session stopped for idleness (or a stop is running); a put-on starts again.
    Sleep,
}

impl State {
    pub fn name(&self) -> String {
        match self {
            Self::Unplugged => "unplugged".into(),
            Self::Plugged => "plugged".into(),
            Self::Worn => "worn".into(),
            Self::Active(k) => format!("active({})", k.name()),
            Self::IdleDim(k) => format!("idle-dim({})", k.name()),
            Self::Sleep => "sleep".into(),
        }
    }
}

/// Side effects for the executor.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// Start a session: desk (with preset) or the game runtime. The executor also relocalises when configured.
    Start { mode: Mode, runtime: Runtime, preset: String },
    /// Stop the session of this kind through the launcher (`pimax desk stop --grace 30` / `pimax vr stop --grace 30`).
    Stop { kind: Kind, reason: &'static str },
    /// Stop then start (set-mode while a session of ours runs).
    Switch { from: Kind, mode: Mode, runtime: Runtime, preset: String },
    /// Tell the user (notify-send + log).
    Notify(String),
}

/// Control commands (IPC / CLI).
#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Pause,
    Resume,
    /// Change the preferred mode / runtime / preset; restarts our own idle-safe session to apply it.
    SetMode { mode: Option<Mode>, runtime: Option<Runtime>, preset: Option<String> },
    /// "I am wearing it": counts as wear evidence for `wear_on_s` (manual start without the IMU).
    Wake,
    /// Stop now (our session or, with `sleep_foreign`, any non-game session).
    Sleep,
}

/// Inputs.
#[derive(Debug, Clone, PartialEq)]
pub enum Input {
    Tick(Signals),
    Cmd(Command),
    /// Result of the last Start / Stop / Switch.
    Done { ok: bool, detail: String },
}

/// A state change, for the event stream.
#[derive(Debug, Clone, PartialEq)]
pub struct Transition {
    pub t: f64,
    pub from: State,
    pub to: State,
    pub why: String,
}

/// The machine.
#[derive(Debug, Clone)]
pub struct Machine {
    pub cfg: Config,
    pub state: State,
    pub paused: bool,
    /// The running session was started by us.
    pub owned: bool,
    /// An action is running (no new Start/Stop until Done).
    pub busy: Option<Action>,
    last: Signals,
    now: f64,
    plug_since: Option<f64>,
    unplug_since: Option<f64>,
    wear_since: Option<f64>,
    unworn_since: Option<f64>,
    no_session_since: Option<f64>,
    dim_since: Option<f64>,
    wake_until: f64,
    /// Failed start: no retry before this time, and not before the headset was taken off once.
    retry_at: f64,
    need_takeoff: bool,
    ignore_session_until: f64,
    pub transitions: Vec<Transition>,
}

impl Machine {
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Unplugged,
            paused: false,
            owned: false,
            busy: None,
            last: Signals::default(),
            now: 0.0,
            plug_since: None,
            unplug_since: None,
            wear_since: None,
            unworn_since: None,
            no_session_since: None,
            dim_since: None,
            wake_until: f64::NEG_INFINITY,
            retry_at: f64::NEG_INFINITY,
            need_takeoff: false,
            ignore_session_until: f64::NEG_INFINITY,
            transitions: Vec::new(),
        }
    }

    pub fn signals(&self) -> &Signals {
        &self.last
    }

    fn go(&mut self, to: State, why: impl Into<String>) {
        if to != self.state {
            self.transitions.push(Transition { t: self.now, from: self.state, to, why: why.into() });
            self.state = to;
        }
        if !matches!(to, State::IdleDim(_)) {
            self.dim_since = None;
        }
    }

    fn act(&mut self, out: &mut Vec<Action>, a: Action) {
        if self.paused {
            if let Action::Notify(_) = a {
                out.push(a);
            }
            return;
        }
        if !matches!(a, Action::Notify(_)) {
            self.busy = Some(a.clone());
        }
        out.push(a);
    }

    fn start_action(&self) -> Action {
        Action::Start { mode: self.cfg.mode, runtime: self.cfg.game_runtime, preset: self.cfg.preset.clone() }
    }

    /// May we stop the running session of `kind` for idleness?
    fn may_stop(&self, _kind: Kind) -> bool {
        (!self.last.game || self.cfg.sleep_games) && (self.owned || self.cfg.sleep_foreign)
    }

    /// Feed one input at time `t` (seconds, monotonic). Returns the actions to execute.
    pub fn step(&mut self, t: f64, input: Input) -> Vec<Action> {
        self.now = t;
        let mut out = Vec::new();
        match input {
            Input::Cmd(c) => self.command(c, &mut out),
            Input::Done { ok, detail } => self.done(ok, detail, &mut out),
            Input::Tick(s) => self.tick(s, &mut out),
        }
        out
    }

    fn command(&mut self, c: Command, out: &mut Vec<Action>) {
        match c {
            Command::Pause => self.paused = true,
            Command::Resume => {
                self.paused = false;
                // stale timers must not fire at once
                self.wear_since = None;
                self.unworn_since = None;
                if matches!(self.state, State::IdleDim(_)) {
                    self.dim_since = Some(self.now);
                }
            }
            Command::Wake => self.wake_until = self.now + self.cfg.wear_on_s + 2.0 * self.cfg.poll_s,
            Command::SetMode { mode, runtime, preset } => {
                let before = (self.cfg.mode, self.cfg.game_runtime, self.cfg.preset.clone());
                if let Some(m) = mode {
                    self.cfg.mode = m;
                }
                if let Some(r) = runtime {
                    self.cfg.game_runtime = r;
                }
                if let Some(p) = preset {
                    self.cfg.preset = p;
                }
                let changed = before != (self.cfg.mode, self.cfg.game_runtime, self.cfg.preset.clone());
                if let (true, State::Active(k) | State::IdleDim(k)) = (changed, self.state) {
                    // switch only our own session, never a running game
                    if self.owned && !self.last.game && self.busy.is_none() {
                        let a = Action::Switch {
                            from: k,
                            mode: self.cfg.mode,
                            runtime: self.cfg.game_runtime,
                            preset: self.cfg.preset.clone(),
                        };
                        self.act(out, a);
                        self.go(State::Worn, "set-mode: restarting the session");
                    }
                }
            }
            Command::Sleep => {
                if let State::Active(k) | State::IdleDim(k) = self.state {
                    if self.busy.is_none() && self.may_stop(k) {
                        self.act(out, Action::Stop { kind: k, reason: "sleep command" });
                        self.go(State::Sleep, "sleep command");
                        self.need_takeoff = true;
                    } else {
                        out.push(Action::Notify(format!("not stopping the {} session (not ours or a game)", k.name())));
                    }
                }
            }
        }
    }

    fn done(&mut self, ok: bool, detail: String, out: &mut Vec<Action>) {
        let Some(a) = self.busy.take() else { return };
        match a {
            Action::Start { mode, .. } | Action::Switch { mode, .. } => {
                let k = if mode == Mode::Game { Kind::Game } else { Kind::Desk };
                if ok && self.state == State::Unplugged {
                    // unplugged while starting: undo
                    self.owned = true;
                    self.act(out, Action::Stop { kind: k, reason: "headset unplugged during the start" });
                } else if ok {
                    self.owned = true;
                    self.no_session_since = None;
                    self.go(State::Active(k), "session started");
                } else {
                    self.owned = false;
                    self.retry_at = self.now + self.cfg.retry_after_s;
                    self.need_takeoff = true;
                    self.go(State::Plugged, format!("start failed: {detail}"));
                    out.push(Action::Notify(format!("Pimax auto: start failed: {detail}")));
                }
            }
            Action::Stop { .. } => {
                self.owned = false;
                // the processes may still show for a poll or two: do not adopt them as a new session
                self.ignore_session_until = self.now + self.cfg.session_lost_s;
                if !ok {
                    out.push(Action::Notify(format!("Pimax auto: stop failed: {detail}")));
                }
            }
            Action::Notify(_) => {}
        }
    }

    fn tick(&mut self, mut s: Signals, out: &mut Vec<Action>) {
        let now = self.now;
        if now < self.ignore_session_until && self.busy.is_none() {
            s.session = None;
            s.game = false;
        }
        self.last = s;
        // --- plug debounce
        if s.plugged {
            self.unplug_since = None;
            self.plug_since.get_or_insert(now);
        } else {
            self.plug_since = None;
            self.unplug_since.get_or_insert(now);
        }
        let plugged_stable = self.plug_since.is_some_and(|t| now - t >= self.cfg.plug_debounce_s);
        let unplugged_stable = self.unplug_since.is_some_and(|t| now - t >= self.cfg.unplug_debounce_s);
        // --- wear timers
        let worn = s.worn() || now < self.wake_until;
        if worn {
            self.wear_since.get_or_insert(now);
        } else {
            self.wear_since = None;
        }
        if s.not_worn() && now >= self.wake_until {
            self.unworn_since.get_or_insert(now);
            self.need_takeoff = false;
        } else {
            self.unworn_since = None;
        }
        let worn_stable = self.wear_since.is_some_and(|t| now - t >= self.cfg.wear_on_s);
        let unworn_for = self.unworn_since.map_or(0.0, |t| now - t);
        // --- session presence
        let kind = s.kind();
        if kind.is_some() {
            self.no_session_since = None;
        } else {
            self.no_session_since.get_or_insert(now);
        }
        let session_gone = self.no_session_since.is_some_and(|t| now - t >= self.cfg.session_lost_s);

        match self.state {
            State::Unplugged => {
                if plugged_stable {
                    self.go(State::Plugged, "headset plugged in");
                    self.tick_plugged(s, worn_stable, out);
                }
            }
            _ if unplugged_stable => {
                if let (Some(k), true) = (kind, self.busy.is_none()) {
                    if self.owned {
                        self.act(out, Action::Stop { kind: k, reason: "headset unplugged" });
                    }
                }
                self.owned = false;
                self.go(State::Unplugged, "headset unplugged");
            }
            State::Plugged | State::Sleep => self.tick_plugged(s, worn_stable, out),
            State::Worn => {
                // waiting for Done; a session that shows up by itself (started by hand) is adopted
                if self.busy.is_none() {
                    match kind {
                        Some(k) => self.go(State::Active(k), "session seen"),
                        None => self.go(State::Plugged, "no start running"),
                    }
                }
            }
            State::Active(k) => {
                if self.busy.is_some() {
                    return;
                }
                if session_gone {
                    self.owned = false;
                    self.go(if unworn_for > 0.0 { State::Sleep } else { State::Plugged }, "session ended");
                    self.need_takeoff = true; // do not restart a session the user just closed
                    return;
                }
                if let Some(nk) = kind {
                    if nk != k {
                        self.go(State::Active(nk), format!("{} running", nk.name()));
                    }
                }
                if unworn_for >= self.cfg.dim_after_s {
                    let k = kind.unwrap_or(k);
                    self.go(State::IdleDim(k), format!("not worn for {:.0} s", unworn_for));
                    self.dim_since = Some(now);
                    out.push(Action::Notify(format!(
                        "Pimax: headset not worn; {} in {:.0} s",
                        if self.may_stop(k) { "session stops" } else { "session kept (game/not ours)" },
                        self.cfg.sleep_after_s
                    )));
                }
            }
            State::IdleDim(k) => {
                if self.busy.is_some() {
                    return;
                }
                let k = kind.unwrap_or(k);
                if session_gone {
                    self.owned = false;
                    self.go(State::Sleep, "session ended");
                    return;
                }
                if worn_stable {
                    self.go(State::Active(k), "worn again");
                    return;
                }
                let dim_for = self.dim_since.map_or(0.0, |t| now - t);
                if dim_for >= self.cfg.sleep_after_s && !self.paused && self.may_stop(k) {
                    self.act(out, Action::Stop { kind: k, reason: "idle" });
                    self.go(State::Sleep, format!("idle for {:.0} s", dim_for + self.cfg.dim_after_s));
                }
            }
        }
    }

    /// Plugged / Sleep: adopt a session started by hand, or start one on put-on.
    fn tick_plugged(&mut self, s: Signals, worn_stable: bool, out: &mut Vec<Action>) {
        if self.busy.is_some() {
            return;
        }
        if let Some(k) = s.kind() {
            self.owned = false;
            self.go(State::Active(k), "session seen (started outside auto mode)");
            return;
        }
        if worn_stable && self.cfg.auto_start && !self.paused && !self.need_takeoff && self.now >= self.retry_at {
            if self.cfg.mode == Mode::Game && self.cfg.game_runtime == Runtime::SteamVr {
                // phase 1: SteamVR is started by the user from Steam; auto mode only tells them once
                out.push(Action::Notify("Pimax: headset on. Game runtime is SteamVR: start it from Steam.".into()));
                self.need_takeoff = true;
                return;
            }
            let a = self.start_action();
            self.act(out, a);
            self.go(State::Worn, "headset put on");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    fn cfg() -> Config {
        let mut c = Config::defaults(Path::new("/h"));
        c.plug_debounce_s = 2.0;
        c.unplug_debounce_s = 3.0;
        c.wear_on_s = 3.0;
        c.dim_after_s = 10.0;
        c.sleep_after_s = 20.0;
        c.session_lost_s = 5.0;
        c.retry_after_s = 30.0;
        c
    }

    struct H {
        m: Machine,
        t: f64,
        s: Signals,
        acts: Vec<Action>,
    }

    impl H {
        fn new(c: Config) -> Self {
            Self { m: Machine::new(c), t: 0.0, s: Signals::default(), acts: vec![] }
        }
        /// Run 1 s ticks for `secs`.
        fn run(&mut self, secs: u32) {
            for _ in 0..secs {
                self.t += 1.0;
                let a = self.m.step(self.t, Input::Tick(self.s));
                self.acts.extend(a);
            }
        }
        fn done(&mut self, ok: bool) {
            let a = self.m.step(self.t, Input::Done { ok, detail: "x".into() });
            self.acts.extend(a);
        }
        fn cmd(&mut self, c: Command) {
            let a = self.m.step(self.t, Input::Cmd(c));
            self.acts.extend(a);
        }
        fn starts(&self) -> usize {
            self.acts.iter().filter(|a| matches!(a, Action::Start { .. })).count()
        }
        fn stops(&self) -> usize {
            self.acts.iter().filter(|a| matches!(a, Action::Stop { .. })).count()
        }
    }

    fn worn_desk() -> H {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.s.motion = Motion::Still;
        h.run(3);
        assert_eq!(h.m.state, State::Plugged);
        h.s.motion = Motion::Worn;
        h.run(4);
        assert_eq!(h.m.state, State::Worn);
        assert_eq!(h.starts(), 1);
        h.s.session = Some(Kind::Desk);
        h.done(true);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        h
    }

    #[test]
    fn put_on_starts_desk_once() {
        let mut h = worn_desk();
        h.run(100);
        assert_eq!(h.starts(), 1);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        assert!(h.m.owned);
    }

    #[test]
    fn plug_glitch_does_not_flap() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.run(1);
        h.s.plugged = false;
        h.run(1);
        h.s.plugged = true;
        h.run(1);
        assert_eq!(h.m.state, State::Unplugged);
        h.run(2);
        assert_eq!(h.m.state, State::Plugged);
        // a 2 s USB re-enumeration while active does not stop anything
        let mut h = worn_desk();
        h.s.plugged = false;
        h.run(2);
        h.s.plugged = true;
        h.run(5);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        assert_eq!(h.stops(), 0);
    }

    #[test]
    fn short_motion_does_not_start() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.s.motion = Motion::Still;
        h.run(3);
        h.s.motion = Motion::Moving; // picked up, put down
        h.run(2);
        h.s.motion = Motion::Still;
        h.run(10);
        assert_eq!(h.starts(), 0);
        assert_eq!(h.m.state, State::Plugged);
    }

    #[test]
    fn take_off_dims_then_sleeps_and_put_on_restarts() {
        let mut h = worn_desk();
        h.s.motion = Motion::Still;
        h.run(9);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        h.run(2);
        assert_eq!(h.m.state, State::IdleDim(Kind::Desk));
        // briefly worn (1 s) is not enough to undim
        h.s.motion = Motion::Worn;
        h.run(1);
        h.s.motion = Motion::Still;
        h.run(21);
        assert_eq!(h.m.state, State::Sleep);
        assert_eq!(h.stops(), 1);
        h.done(true);
        h.s.session = None;
        h.run(10);
        assert_eq!(h.m.state, State::Sleep);
        h.s.motion = Motion::Worn;
        h.run(4);
        assert_eq!(h.m.state, State::Worn);
        assert_eq!(h.starts(), 2);
    }

    #[test]
    fn dim_recovers_when_worn_again() {
        let mut h = worn_desk();
        h.s.motion = Motion::Still;
        h.run(12);
        assert_eq!(h.m.state, State::IdleDim(Kind::Desk));
        h.s.motion = Motion::Worn;
        h.run(4);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        assert_eq!(h.stops(), 0);
    }

    #[test]
    fn unknown_motion_never_dims() {
        let mut h = worn_desk();
        h.s.motion = Motion::Unknown;
        h.run(1000);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
    }

    #[test]
    fn games_are_never_stopped_by_default() {
        let mut h = worn_desk();
        h.s.game = true;
        h.run(1);
        assert_eq!(h.m.state, State::Active(Kind::Game));
        h.s.motion = Motion::Still;
        h.run(1000);
        assert_eq!(h.m.state, State::IdleDim(Kind::Game));
        assert_eq!(h.stops(), 0);
        // the user allows it
        let mut c = cfg();
        c.sleep_games = true;
        h.m.cfg = c;
        h.run(1);
        assert_eq!(h.stops(), 1);
        assert_eq!(h.m.state, State::Sleep);
    }

    #[test]
    fn foreign_sessions_are_adopted_but_not_stopped() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.s.motion = Motion::Still;
        h.s.session = Some(Kind::Desk);
        h.run(3);
        assert_eq!(h.m.state, State::Active(Kind::Desk));
        assert!(!h.m.owned);
        h.run(100);
        assert_eq!(h.m.state, State::IdleDim(Kind::Desk));
        assert_eq!(h.stops(), 0);
        assert_eq!(h.starts(), 0);
    }

    #[test]
    fn failed_start_backs_off_until_taken_off_and_timeout() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.s.motion = Motion::Worn;
        h.run(6);
        assert_eq!(h.starts(), 1);
        h.done(false);
        assert_eq!(h.m.state, State::Plugged);
        h.run(60);
        assert_eq!(h.starts(), 1, "still worn: no retry loop");
        h.s.motion = Motion::Still;
        h.run(2);
        h.s.motion = Motion::Worn;
        h.run(5);
        assert_eq!(h.starts(), 2);
    }

    #[test]
    fn unplug_stops_our_session() {
        let mut h = worn_desk();
        h.s.plugged = false;
        h.run(4);
        assert_eq!(h.m.state, State::Unplugged);
        assert_eq!(h.stops(), 1);
    }

    #[test]
    fn pause_suppresses_actions() {
        let mut h = H::new(cfg());
        h.cmd(Command::Pause);
        h.s.plugged = true;
        h.s.motion = Motion::Worn;
        h.run(20);
        assert_eq!(h.starts(), 0);
        assert_eq!(h.m.state, State::Plugged);
        h.cmd(Command::Resume);
        h.run(4);
        assert_eq!(h.starts(), 1);
    }

    #[test]
    fn user_closing_the_session_is_not_undone() {
        let mut h = worn_desk();
        h.s.session = None; // `pimax desk stop` by hand while worn
        h.run(6);
        assert_eq!(h.m.state, State::Plugged);
        h.run(100);
        assert_eq!(h.starts(), 1, "no restart until taken off and put on again");
    }

    #[test]
    fn set_mode_switches_our_session_but_not_a_game() {
        let mut h = worn_desk();
        h.cmd(Command::SetMode { mode: Some(Mode::Game), runtime: None, preset: None });
        assert!(matches!(h.acts.last(), Some(Action::Switch { from: Kind::Desk, mode: Mode::Game, .. })));
        h.done(true);
        assert_eq!(h.m.state, State::Active(Kind::Game));
        h.s.game = true;
        h.run(1);
        let n = h.acts.len();
        h.cmd(Command::SetMode { mode: Some(Mode::Desk), runtime: None, preset: None });
        assert_eq!(h.acts.len(), n, "game running: only the preference changes");
        assert_eq!(h.m.cfg.mode, Mode::Desk);
    }

    #[test]
    fn steamvr_runtime_only_notifies() {
        let mut c = cfg();
        c.mode = Mode::Game;
        c.game_runtime = Runtime::SteamVr;
        let mut h = H::new(c);
        h.s.plugged = true;
        h.s.motion = Motion::Worn;
        h.run(30);
        assert_eq!(h.starts(), 0);
        assert_eq!(h.acts.iter().filter(|a| matches!(a, Action::Notify(_))).count(), 1);
    }

    #[test]
    fn wake_command_starts_without_imu() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.run(3);
        h.cmd(Command::Wake);
        h.run(4);
        assert_eq!(h.starts(), 1);
    }

    #[test]
    fn unplug_during_start_undoes_it_and_stopping_processes_are_not_adopted() {
        let mut h = H::new(cfg());
        h.s.plugged = true;
        h.s.motion = Motion::Worn;
        h.run(6);
        assert_eq!(h.m.state, State::Worn);
        h.s.plugged = false;
        h.run(4);
        assert_eq!(h.m.state, State::Unplugged);
        h.done(true);
        assert_eq!(h.stops(), 1);
        // the stop finishes while the processes are still listed for a moment
        h.s.plugged = true;
        h.s.session = Some(Kind::Desk);
        h.done(true);
        h.s.motion = Motion::Still;
        h.run(3);
        assert_eq!(h.m.state, State::Plugged);
        h.s.session = None;
        h.run(5);
        assert_eq!(h.m.state, State::Plugged);
    }

    #[test]
    fn visor_overrides_the_imu() {
        let s = Signals { motion: Motion::Moving, visor: Some(false), ..Default::default() };
        assert!(!s.worn() && s.not_worn());
        let s = Signals { motion: Motion::Still, visor: Some(true), ..Default::default() };
        assert!(s.worn());
    }
}
