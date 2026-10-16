//! The display model: what the HUD shows for the stream of server events. Pure (no fonts, no OpenXR, no clock of
//! its own): the caller passes `Instant`s and a [`Measure`] for the text width.
//!
//! * The last `keep` finals (default 2) plus the partial of the active segment are shown, oldest first, newest at the
//!   bottom, wrapped to the pixel width and limited to `max_lines` lines. A partial is replaced by newer partials of
//!   its segment and by the final of its segment; an empty final just drops the partial.
//! * Everything text-like disappears `timeout` seconds after the last speech / partial / final; errors stay.
//! * The mic dot: green while speech, grey while listening, amber for paused/stopped notices, red for
//!   `no_device`/`error` (with the status detail text). The level bar follows the `level` events while speech.

use crate::protocol::{Event, State};
use std::time::{Duration, Instant};

/// Text width in pixels, for wrapping.
pub trait Measure {
    fn width(&self, text: &str) -> f32;
}

/// Every character the same width (tests, and `dump` without a font).
pub struct FixedMeasure(pub f32);

impl Measure for FixedMeasure {
    fn width(&self, text: &str) -> f32 {
        self.0 * text.chars().count() as f32
    }
}

/// Level bar: `level` events from -60 to 0 dB map to 0..=LEVEL_STEPS.
pub const LEVEL_STEPS: u8 = 20;
pub const LEVEL_MIN_DB: f32 = -60.0;
pub const LEVEL_MAX_DB: f32 = 0.0;
/// The most characters of one transcript kept (the end of a longer one), a guard against a runaway server.
const MAX_TEXT_CHARS: usize = 2000;
/// Partials in flight at once (normally one; a second while the previous segment waits for its final). A server
/// that never sends finals cannot grow the model beyond this.
const MAX_PARTIALS: usize = 3;

/// Bar steps for a level in dB (NaN counts as silence).
pub fn level_step(db: f32) -> u8 {
    if db.is_nan() {
        return 0;
    }
    let f = ((db - LEVEL_MIN_DB) / (LEVEL_MAX_DB - LEVEL_MIN_DB)).clamp(0.0, 1.0);
    (f * f32::from(LEVEL_STEPS)).round() as u8
}

#[derive(Clone, Debug, PartialEq)]
pub struct Config {
    /// Finals kept next to the partial.
    pub keep: usize,
    /// Most wrapped lines shown.
    pub max_lines: usize,
    /// Hide the text after this long without speech / partial / final; `None`: never.
    pub timeout: Option<Duration>,
    /// Keep the dot and the level bar visible while connected, even without text.
    pub idle_dot: bool,
}

impl Default for Config {
    fn default() -> Config {
        Config {
            keep: 2,
            max_lines: 3,
            timeout: Some(Duration::from_secs(12)),
            idle_dot: false,
        }
    }
}

/// Style class of a displayed line.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Final,
    Partial,
    /// Status detail of `no_device` / `error`.
    Error,
    /// Status of `paused` / `stopped`.
    Notice,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Dot {
    Speech,
    Listening,
    Paused,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ViewLine {
    pub text: String,
    pub kind: Kind,
}

/// What is on screen right now. Two equal views draw the same texture.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct View {
    pub visible: bool,
    /// Wrapped lines, oldest first, the newest at the bottom.
    pub lines: Vec<ViewLine>,
    pub dot: Dot,
    /// Level bar fill, 0..=[`LEVEL_STEPS`].
    pub level: u8,
}

impl View {
    pub fn hidden() -> View {
        View {
            visible: false,
            lines: Vec::new(),
            dot: Dot::Listening,
            level: 0,
        }
    }
}

struct Entry {
    seg: u64,
    text: String,
    is_final: bool,
}

struct StatusInfo {
    state: State,
    detail: String,
    since: Instant,
}

pub struct Display {
    cfg: Config,
    /// Ascending by segment; at most `keep` finals plus the partial(s) in flight.
    entries: Vec<Entry>,
    speech: bool,
    level_db: Option<f32>,
    status: Option<StatusInfo>,
    last_activity: Option<Instant>,
    connected: bool,
    revision: u64,
}

/// Collapse whitespace (newlines, tabs, runs), drop control characters, keep the end of absurdly long text.
pub fn clean_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len().min(256));
    for word in s
        .split(|c: char| c.is_whitespace() || c.is_control())
        .filter(|w| !w.is_empty())
    {
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(word);
    }
    let n = out.chars().count();
    if n > MAX_TEXT_CHARS {
        out = out.chars().skip(n - MAX_TEXT_CHARS).collect();
    }
    out
}

/// Greedy word wrap by pixel width. Words wider than `max_w` are broken between characters. No empty lines.
pub fn wrap_text(text: &str, max_w: f32, m: &dyn Measure) -> Vec<String> {
    let mut lines = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        if !cur.is_empty() {
            let cand = format!("{cur} {word}");
            if m.width(&cand) <= max_w {
                cur = cand;
                continue;
            }
            lines.push(std::mem::take(&mut cur));
        }
        if m.width(word) <= max_w {
            cur = word.to_string();
            continue;
        }
        // a single word wider than the line: break it between characters
        let mut piece = String::new();
        for ch in word.chars() {
            piece.push(ch);
            if m.width(&piece) > max_w && piece.chars().count() > 1 {
                piece.pop();
                lines.push(std::mem::take(&mut piece));
                piece.push(ch);
            }
        }
        cur = piece;
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines
}

impl Display {
    pub fn new(cfg: Config) -> Display {
        Display {
            cfg,
            entries: Vec::new(),
            speech: false,
            level_db: None,
            status: None,
            last_activity: None,
            connected: false,
            revision: 0,
        }
    }

    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Changes whenever the model changed; the caller recomputes the view only then.
    pub fn revision(&self) -> u64 {
        self.revision
    }

    fn bump(&mut self) {
        self.revision += 1;
    }

    fn reset_content(&mut self) {
        self.entries.clear();
        self.speech = false;
        self.level_db = None;
        self.status = None;
        self.last_activity = None;
    }

    /// A connection to the server was made: a new server session (segment numbers may restart).
    pub fn on_connected(&mut self) {
        self.reset_content();
        self.connected = true;
        self.bump();
    }

    /// The server is gone: the partial, the speech flag and the status are stale; finals stay until the timeout.
    pub fn on_down(&mut self) {
        self.entries.retain(|e| e.is_final);
        self.speech = false;
        self.level_db = None;
        self.status = None;
        self.connected = false;
        self.bump();
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    fn touch(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    fn prune_finals(&mut self) {
        let finals = self.entries.iter().filter(|e| e.is_final).count();
        let mut drop = finals.saturating_sub(self.cfg.keep);
        if drop > 0 {
            self.entries.retain(|e| {
                if e.is_final && drop > 0 {
                    drop -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    fn insert(&mut self, entry: Entry) {
        let at = self
            .entries
            .iter()
            .position(|e| e.seg > entry.seg)
            .unwrap_or(self.entries.len());
        self.entries.insert(at, entry);
        // bound the partials: the oldest ones are the stalest
        let mut extra = self
            .entries
            .iter()
            .filter(|e| !e.is_final)
            .count()
            .saturating_sub(MAX_PARTIALS);
        if extra > 0 {
            self.entries.retain(|e| {
                if !e.is_final && extra > 0 {
                    extra -= 1;
                    false
                } else {
                    true
                }
            });
        }
    }

    /// Apply one server event.
    pub fn on_event(&mut self, ev: &Event, now: Instant) {
        self.connected = true;
        match ev {
            Event::Hello { .. } => self.reset_content(),
            Event::Status { state, detail } => match state {
                State::Listening => self.status = None,
                State::Other(_) => {}
                State::NoDevice | State::Error | State::Paused | State::Stopped => {
                    self.speech = false;
                    self.status = Some(StatusInfo {
                        state: state.clone(),
                        detail: clean_text(detail),
                        since: now,
                    });
                }
            },
            Event::Level { db, speech } => {
                self.level_db = Some(*db);
                self.speech = *speech;
                if *speech {
                    self.touch(now);
                }
            }
            Event::SpeechStart { .. } => {
                self.speech = true;
                self.touch(now);
            }
            Event::Partial { seg, text, .. } => {
                self.touch(now);
                let text = clean_text(text);
                if self.entries.iter().any(|e| e.seg == *seg && e.is_final) {
                    // a partial that arrives after its own final is stale
                } else {
                    self.entries.retain(|e| e.seg != *seg);
                    if !text.is_empty() {
                        self.insert(Entry {
                            seg: *seg,
                            text,
                            is_final: false,
                        });
                    }
                }
            }
            Event::Final { seg, text, .. } => {
                let text = clean_text(text);
                // the final replaces its partial; partials of older segments never got their final: stale
                self.entries
                    .retain(|e| e.seg != *seg && (e.is_final || e.seg > *seg));
                if !self.entries.iter().any(|e| !e.is_final && e.seg > *seg) {
                    self.speech = false;
                }
                if !text.is_empty() {
                    self.touch(now);
                    self.insert(Entry {
                        seg: *seg,
                        text,
                        is_final: true,
                    });
                    self.prune_finals();
                }
            }
            Event::Intent { name, .. } => {
                if name == "hud.clear" {
                    self.entries.clear();
                }
            }
        }
        self.bump();
    }

    /// Drop what has timed out. Call it every frame; it changes the model (and the revision) only when something
    /// expired.
    pub fn expire(&mut self, now: Instant) {
        let Some(timeout) = self.cfg.timeout else {
            return;
        };
        let mut changed = false;
        if self
            .last_activity
            .is_some_and(|t| now.saturating_duration_since(t) >= timeout)
        {
            changed |= !self.entries.is_empty() || self.speech;
            self.entries.clear();
            self.speech = false;
            self.last_activity = None;
        }
        if self.status.as_ref().is_some_and(|s| {
            matches!(s.state, State::Paused | State::Stopped)
                && now.saturating_duration_since(s.since) >= timeout
        }) {
            self.status = None;
            changed = true;
        }
        if changed {
            self.bump();
        }
    }

    fn error_active(&self) -> bool {
        matches!(&self.status, Some(s) if matches!(s.state, State::NoDevice | State::Error))
    }

    fn notice_active(&self) -> bool {
        matches!(&self.status, Some(s) if matches!(s.state, State::Paused | State::Stopped))
    }

    /// The status line, if the status wants one: detail text, or a default when the server sent none.
    fn status_message(&self) -> Option<(String, Kind)> {
        let s = self.status.as_ref()?;
        let (default, kind) = match s.state {
            State::NoDevice => ("No microphone", Kind::Error),
            State::Error => ("Voice error", Kind::Error),
            State::Paused => ("Voice paused", Kind::Notice),
            State::Stopped => ("Voice stopped", Kind::Notice),
            State::Listening | State::Other(_) => return None,
        };
        let text = if s.detail.is_empty() {
            default.to_string()
        } else {
            s.detail.clone()
        };
        Some((text, kind))
    }

    /// The current view: wrapped to `max_w` pixels, at most `max_lines` lines, newest at the bottom.
    pub fn view(&self, m: &dyn Measure, max_w: f32) -> View {
        let mut lines: Vec<ViewLine> = Vec::new();
        for e in &self.entries {
            let kind = if e.is_final {
                Kind::Final
            } else {
                Kind::Partial
            };
            lines.extend(
                wrap_text(&e.text, max_w, m)
                    .into_iter()
                    .map(|text| ViewLine { text, kind }),
            );
        }
        if let Some((msg, kind)) = self.status_message() {
            lines.extend(
                wrap_text(&msg, max_w, m)
                    .into_iter()
                    .map(|text| ViewLine { text, kind }),
            );
        }
        let max = self.cfg.max_lines.max(1);
        if lines.len() > max {
            lines.drain(..lines.len() - max);
        }
        let error = self.error_active();
        let dot = if error {
            Dot::Error
        } else if self.speech {
            Dot::Speech
        } else if self.notice_active() {
            Dot::Paused
        } else {
            Dot::Listening
        };
        let visible = !lines.is_empty()
            || self.speech
            || error
            || self.notice_active()
            || (self.cfg.idle_dot && self.connected);
        let level = if visible && (self.speech || self.cfg.idle_dot) {
            self.level_db.map_or(0, level_step)
        } else {
            0
        };
        View {
            visible,
            lines,
            dot,
            level,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::parse_line;

    /// 10 px per character, 200 px wide lines: 20 characters.
    const M: FixedMeasure = FixedMeasure(10.0);
    const W: f32 = 200.0;

    struct H {
        d: Display,
        t0: Instant,
    }

    impl H {
        fn new() -> H {
            H::with(Config::default())
        }
        fn with(cfg: Config) -> H {
            H {
                d: Display::new(cfg),
                t0: Instant::now(),
            }
        }
        fn at(&self, secs: f32) -> Instant {
            self.t0 + Duration::from_secs_f32(secs)
        }
        /// Feed a JSON line at `secs`.
        fn ev(&mut self, secs: f32, line: &str) {
            let e = parse_line(line).unwrap_or_else(|| panic!("bad test line {line}"));
            let now = self.at(secs);
            self.d.on_event(&e, now);
        }
        fn partial(&mut self, secs: f32, seg: u64, text: &str) {
            self.ev(
                secs,
                &format!(r#"{{"type":"partial","seg":{seg},"text":"{text}","t":1}}"#),
            );
        }
        fn fin(&mut self, secs: f32, seg: u64, text: &str) {
            self.ev(
                secs,
                &format!(
                    r#"{{"type":"final","seg":{seg},"text":"{text}","lang":"en","t0":0,"t1":1,"latency_ms":5,"decode_ms":5}}"#
                ),
            );
        }
        fn status(&mut self, secs: f32, state: &str, detail: &str) {
            self.ev(
                secs,
                &format!(r#"{{"type":"status","state":"{state}","detail":"{detail}"}}"#),
            );
        }
        fn level(&mut self, secs: f32, db: f32, speech: bool) {
            self.ev(
                secs,
                &format!(r#"{{"type":"level","db":{db},"speech":{speech}}}"#),
            );
        }
        fn view(&mut self, secs: f32) -> View {
            let now = self.at(secs);
            self.d.expire(now);
            self.d.view(&M, W)
        }
        fn texts(&mut self, secs: f32) -> Vec<String> {
            self.view(secs).lines.into_iter().map(|l| l.text).collect()
        }
    }

    #[test]
    fn nothing_is_visible_at_start_and_for_idle_level_events() {
        let mut h = H::new();
        assert_eq!(h.view(0.0), View::hidden());
        h.ev(
            0.0,
            r#"{"type":"hello","engine":"e","source":"s","rate":16000}"#,
        );
        h.status(0.0, "listening", "");
        for i in 0..20 {
            h.level(i as f32 * 0.25, -52.0, false);
        }
        let v = h.view(5.0);
        assert!(!v.visible && v.lines.is_empty() && v.level == 0, "{v:?}");
    }

    #[test]
    fn partial_then_final_replaces_in_place() {
        let mut h = H::new();
        h.partial(0.0, 7, "open the bro");
        let v = h.view(0.0);
        assert!(v.visible);
        assert_eq!(
            v.lines,
            [ViewLine {
                text: "open the bro".into(),
                kind: Kind::Partial
            }]
        );
        h.partial(0.2, 7, "open the browser");
        assert_eq!(h.texts(0.2), ["open the browser"]);
        h.fin(0.5, 7, "Open the browser.");
        let v = h.view(0.5);
        assert_eq!(
            v.lines,
            [ViewLine {
                text: "Open the browser.".into(),
                kind: Kind::Final
            }]
        );
        // a straggling partial of the same segment after its final changes nothing
        h.partial(0.6, 7, "open the brow");
        assert_eq!(h.texts(0.6), ["Open the browser."]);
    }

    #[test]
    fn newest_at_the_bottom_and_only_the_last_n_finals() {
        let mut h = H::new(); // keep 2
        h.fin(0.0, 1, "one");
        h.fin(1.0, 2, "two");
        h.fin(2.0, 3, "three");
        assert_eq!(h.texts(2.0), ["two", "three"]);
        h.partial(2.5, 4, "fo");
        // 2 finals + the partial = 3 lines, the partial last and dimmer
        let v = h.view(2.5);
        assert_eq!(
            v.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["two", "three", "fo"]
        );
        assert_eq!(v.lines[2].kind, Kind::Partial);
        assert_eq!(v.lines[1].kind, Kind::Final);
        // keep = 0 shows only the partial
        let mut h = H::with(Config {
            keep: 0,
            ..Config::default()
        });
        h.fin(0.0, 1, "gone");
        assert!(h.texts(0.0).is_empty());
        h.partial(0.1, 2, "live");
        assert_eq!(h.texts(0.1), ["live"]);
    }

    #[test]
    fn a_newer_partial_replaces_the_text_of_its_segment_only() {
        let mut h = H::new();
        h.partial(0.0, 1, "alpha");
        h.partial(0.1, 1, "alpha beta");
        h.partial(0.2, 1, "alpha beta gamma");
        assert_eq!(h.texts(0.2), ["alpha beta gamma"]);
        // the final of segment 1 arrives late, after segment 2 already has a partial: both stay in order
        h.partial(0.3, 2, "delta");
        assert_eq!(h.texts(0.3), ["alpha beta gamma", "delta"]);
        h.fin(0.4, 1, "Alpha beta gamma.");
        let v = h.view(0.4);
        assert_eq!(
            v.lines,
            [
                ViewLine {
                    text: "Alpha beta gamma.".into(),
                    kind: Kind::Final
                },
                ViewLine {
                    text: "delta".into(),
                    kind: Kind::Partial
                }
            ]
        );
    }

    #[test]
    fn empty_final_drops_the_partial() {
        let mut h = H::new();
        h.fin(0.0, 1, "kept");
        h.partial(1.0, 2, "noise");
        assert_eq!(h.texts(1.0), ["kept", "noise"]);
        h.fin(1.5, 2, "");
        assert_eq!(h.texts(1.5), ["kept"]);
        // whitespace-only counts as empty; an empty partial removes the text too
        h.partial(2.0, 3, "x");
        h.partial(2.1, 3, "  ");
        assert_eq!(h.texts(2.1), ["kept"]);
        h.partial(2.2, 4, "y");
        h.fin(2.3, 4, " \\n ");
        assert_eq!(h.texts(2.3), ["kept"]);
    }

    #[test]
    fn partials_in_flight_are_bounded() {
        let mut h = H::with(Config {
            max_lines: 8,
            ..Config::default()
        });
        for seg in 1..=10u64 {
            h.partial(0.1 * seg as f32, seg, &format!("p{seg}"));
        }
        // only the newest three are kept
        assert_eq!(h.texts(1.0), ["p8", "p9", "p10"]);
        // finals are not affected by the cap
        h.fin(1.1, 11, "done");
        assert_eq!(h.texts(1.1), ["done"]);
    }

    #[test]
    fn a_final_makes_older_partials_stale() {
        let mut h = H::new();
        h.partial(0.0, 1, "lost one");
        h.fin(1.0, 2, "second");
        // segment 1 never got its final; segment 2's final proves it is over
        assert_eq!(h.texts(1.0), ["second"]);
        // out-of-order finals keep their order by segment
        h.fin(1.5, 1, "first");
        assert_eq!(h.texts(1.5), ["first", "second"]);
    }

    #[test]
    fn hides_after_the_timeout_and_activity_extends_it() {
        let mut h = H::new(); // 12 s
        h.fin(0.0, 1, "hello there");
        assert!(h.view(11.9).visible);
        let v = h.view(12.0);
        assert!(!v.visible && v.lines.is_empty());
        // the text does not come back
        assert!(!h.view(13.0).visible);
        // each partial restarts the clock
        h.partial(20.0, 2, "a");
        h.partial(30.0, 2, "a b");
        assert!(h.view(41.9).visible);
        assert!(!h.view(42.0).visible);
        // speech keeps the HUD alive too
        h.ev(50.0, r#"{"type":"speech_start","seg":3,"t":1}"#);
        h.level(55.0, -30.0, true);
        assert!(h.view(66.9).visible);
        assert!(!h.view(67.0).visible);
    }

    #[test]
    fn timeout_none_never_hides() {
        let mut h = H::with(Config {
            timeout: None,
            ..Config::default()
        });
        h.fin(0.0, 1, "stays");
        assert_eq!(h.texts(100000.0), ["stays"]);
    }

    #[test]
    fn expire_changes_the_revision_only_when_something_expired() {
        let mut h = H::new();
        h.fin(0.0, 1, "x");
        let r = h.d.revision();
        h.d.expire(h.t0 + Duration::from_secs(5));
        assert_eq!(h.d.revision(), r);
        h.d.expire(h.t0 + Duration::from_secs(12));
        assert_eq!(h.d.revision(), r + 1);
        h.d.expire(h.t0 + Duration::from_secs(13));
        assert_eq!(h.d.revision(), r + 1);
    }

    #[test]
    fn hud_clear_intent_clears_but_other_intents_do_not() {
        let mut h = H::new();
        h.fin(0.0, 1, "one");
        h.fin(1.0, 2, "clear the screen");
        h.ev(
            1.1,
            r#"{"type":"intent","seg":2,"name":"app.open","text":"x","slots":{"a":"b"}}"#,
        );
        assert_eq!(h.texts(1.1), ["one", "clear the screen"]);
        h.ev(
            1.2,
            r#"{"type":"intent","seg":2,"name":"hud.clear","text":"clear the screen"}"#,
        );
        let v = h.view(1.2);
        assert!(v.lines.is_empty() && !v.visible, "{v:?}");
        // new speech shows up normally afterwards
        h.fin(2.0, 3, "back");
        assert_eq!(h.texts(2.0), ["back"]);
    }

    #[test]
    fn mic_dot_colours_and_level_bar() {
        let mut h = H::new();
        h.fin(0.0, 1, "text");
        // listening: grey dot, no bar fill
        h.level(0.5, -20.0, false);
        let v = h.view(0.5);
        assert_eq!((v.dot, v.level), (Dot::Listening, 0));
        // speech: green dot, bar follows the level (-60..0 dB over 20 steps)
        h.level(1.0, -30.0, true);
        let v = h.view(1.0);
        assert_eq!((v.dot, v.level), (Dot::Speech, 10));
        h.level(1.25, 0.0, true);
        assert_eq!(h.view(1.25).level, LEVEL_STEPS);
        h.level(1.5, -90.0, true);
        assert_eq!(h.view(1.5).level, 0);
        // the end of speech greys the dot again
        h.level(2.0, -45.0, false);
        assert_eq!(h.view(2.0).dot, Dot::Listening);
        assert_eq!(level_step(f32::NAN), 0);
        assert_eq!(level_step(-60.0), 0);
        assert_eq!(level_step(-3.0), 19);
        assert_eq!(level_step(12.0), LEVEL_STEPS);
    }

    #[test]
    fn speech_alone_shows_the_dot_before_any_text() {
        let mut h = H::new();
        h.ev(0.0, r#"{"type":"speech_start","seg":1,"t":0.1}"#);
        let v = h.view(0.0);
        assert!(v.visible && v.lines.is_empty());
        assert_eq!(v.dot, Dot::Speech);
        // a final sets the dot back to listening (no other segment in flight)
        h.fin(1.0, 1, "ok");
        assert_eq!(h.view(1.0).dot, Dot::Listening);
        // ... but not when the next segment already has a partial
        h.ev(2.0, r#"{"type":"speech_start","seg":2,"t":2}"#);
        h.partial(2.1, 2, "nex");
        h.fin(2.2, 1, "ok again");
        assert_eq!(h.view(2.2).dot, Dot::Speech);
    }

    #[test]
    fn errors_stay_and_show_their_detail() {
        let mut h = H::new();
        h.status(0.0, "no_device", "Pimax Dream Air Analog Stereo not found");
        let v = h.view(1000.0); // far beyond the timeout
        assert!(v.visible);
        assert_eq!(v.dot, Dot::Error);
        assert_eq!(
            v.lines,
            [
                ViewLine {
                    text: "Pimax Dream Air".into(),
                    kind: Kind::Error
                },
                ViewLine {
                    text: "Analog Stereo not".into(),
                    kind: Kind::Error
                },
                ViewLine {
                    text: "found".into(),
                    kind: Kind::Error
                }
            ]
        );
        // listening again clears it
        h.status(1001.0, "listening", "");
        assert!(!h.view(1001.0).visible);
        // defaults when the detail is empty
        h.status(1002.0, "error", "");
        assert_eq!(h.texts(1002.0), ["Voice error"]);
        h.status(1003.0, "no_device", "");
        assert_eq!(h.texts(1003.0), ["No microphone"]);
        // an unknown state changes nothing
        h.status(1004.0, "warming_up", "x");
        assert_eq!(h.texts(1004.0), ["No microphone"]);
    }

    #[test]
    fn error_line_is_the_newest_and_wins_the_line_limit() {
        let mut h = H::new();
        h.fin(0.0, 1, "first final");
        h.fin(0.1, 2, "second final");
        h.partial(0.2, 3, "a partial");
        h.status(0.3, "error", "boom");
        let v = h.view(0.3);
        assert_eq!(
            v.lines.iter().map(|l| l.text.as_str()).collect::<Vec<_>>(),
            ["second final", "a partial", "boom"]
        );
        assert_eq!(v.dot, Dot::Error);
        // the error also silences the speech dot
        h.level(0.4, -20.0, true);
        assert_eq!(h.view(0.4).dot, Dot::Error);
    }

    #[test]
    fn paused_is_a_notice_that_times_out() {
        let mut h = H::new();
        h.status(0.0, "paused", "");
        let v = h.view(1.0);
        assert!(v.visible);
        assert_eq!(v.dot, Dot::Paused);
        assert_eq!(
            v.lines,
            [ViewLine {
                text: "Voice paused".into(),
                kind: Kind::Notice
            }]
        );
        assert!(!h.view(12.0).visible);
        h.status(20.0, "stopped", "bye");
        assert_eq!(h.texts(20.0), ["bye"]);
    }

    #[test]
    fn hello_and_reconnect_reset_the_content() {
        let mut h = H::new();
        h.fin(0.0, 7, "old session");
        h.partial(0.1, 8, "half");
        h.status(0.2, "error", "x");
        h.ev(
            0.3,
            r#"{"type":"hello","engine":"e","source":"s","rate":16000}"#,
        );
        assert!(!h.view(0.3).visible);
        // segment numbers restart: seg 7 again must not be mistaken for a duplicate
        h.fin(1.0, 7, "new session");
        assert_eq!(h.texts(1.0), ["new session"]);
        h.d.on_connected();
        assert!(!h.view(1.1).visible);
    }

    #[test]
    fn disconnect_drops_partial_and_status_but_keeps_finals() {
        let mut h = H::new();
        h.fin(0.0, 1, "kept");
        h.partial(0.5, 2, "half");
        h.status(0.6, "error", "oops");
        h.d.on_down();
        let v = h.view(1.0);
        assert_eq!(
            v.lines,
            [ViewLine {
                text: "kept".into(),
                kind: Kind::Final
            }]
        );
        assert_eq!(v.dot, Dot::Listening);
        assert!(!h.d.is_connected());
        // and the timeout (counted from the last activity, the partial at 0.5 s) still removes the final
        assert!(h.view(12.4).visible);
        assert!(!h.view(12.5).visible);
    }

    #[test]
    fn idle_dot_keeps_the_dot_while_connected() {
        let mut h = H::with(Config {
            idle_dot: true,
            ..Config::default()
        });
        assert!(!h.view(0.0).visible, "not connected yet");
        h.status(0.0, "listening", "");
        h.level(0.1, -30.0, false);
        let v = h.view(100.0);
        assert!(v.visible && v.lines.is_empty());
        assert_eq!((v.dot, v.level), (Dot::Listening, 10));
        h.d.on_down();
        assert!(!h.view(101.0).visible);
    }

    #[test]
    fn wrapping_and_the_line_limit() {
        let mut h = H::new(); // 20 chars per line, 3 lines
        h.fin(
            0.0,
            1,
            "one two three four five six seven eight nine ten eleven twelve",
        );
        // 12 words wrap into more than 3 lines: the last 3 are shown
        let all = wrap_text(
            "one two three four five six seven eight nine ten eleven twelve",
            W,
            &M,
        );
        assert!(all.len() > 3);
        assert_eq!(h.texts(0.0), all[all.len() - 3..]);
        // finals and partial share the limit: older text scrolls out
        let mut h = H::new();
        h.fin(0.0, 1, "first first first first first");
        h.fin(0.1, 2, "second second second second second");
        h.partial(0.2, 3, "third third third third third third");
        let t = h.texts(0.2);
        assert_eq!(t.len(), 3);
        assert_eq!(t[2], "third third third");
    }

    #[test]
    fn invariants_hold_under_random_event_sequences() {
        let mut seed = 0xdead_beef_cafe_f00du64;
        let mut next = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        let words = [
            "open",
            "the",
            "browser",
            "¿qué",
            "hora",
            "es?",
            "año",
            "señor",
            "ü",
            "a",
            "supercalifragilisticexpialidocious_and_more_letters_here",
        ];
        for max_lines in [1usize, 2, 3, 5] {
            for keep in [0usize, 1, 2, 4] {
                let mut h = H::with(Config {
                    keep,
                    max_lines,
                    timeout: Some(Duration::from_secs(12)),
                    idle_dot: next() % 2 == 0,
                });
                let mut t = 0.0f32;
                for _ in 0..400 {
                    t += (next() % 400) as f32 / 100.0;
                    let seg = next() % 12;
                    let text: Vec<&str> = (0..next() % 12)
                        .map(|_| words[(next() % words.len() as u64) as usize])
                        .collect();
                    let text = text.join(" ");
                    match next() % 11 {
                        0 => h.ev(
                            t,
                            &format!(r#"{{"type":"speech_start","seg":{seg},"t":1}}"#),
                        ),
                        1 | 2 => h.partial(t, seg, &text),
                        3 | 4 => h.fin(t, seg, &text),
                        5 => h.level(t, -80.0 + (next() % 100) as f32, next() % 2 == 0),
                        6 => h.status(
                            t,
                            ["listening", "paused", "no_device", "error", "stopped", "x"]
                                [(next() % 6) as usize],
                            &text,
                        ),
                        7 => h.ev(
                            t,
                            r#"{"type":"intent","seg":1,"name":"hud.clear","text":"c"}"#,
                        ),
                        8 => h.d.on_down(),
                        9 => h.d.on_connected(),
                        _ => h.ev(t, r#"{"type":"hello","engine":"e","source":"s","rate":1}"#),
                    }
                    let v = h.view(t);
                    assert!(v.lines.len() <= max_lines, "{v:?}");
                    assert!(v.level <= LEVEL_STEPS);
                    assert!(v.lines.iter().all(|l| !l.text.is_empty()));
                    if !v.visible {
                        assert!(v.lines.is_empty() && v.level == 0, "{v:?}");
                    }
                    let finals = v.lines.iter().filter(|l| l.kind == Kind::Final).count();
                    assert!(finals <= max_lines);
                    // a line can be wider than the limit only when a single character does not fit
                    assert!(v.lines.iter().all(|l| M.width(&l.text) <= W));
                    // the same state always gives the same view
                    assert_eq!(v, h.d.view(&M, W));
                }
            }
        }
    }

    #[test]
    fn wrap_text_rules() {
        assert_eq!(wrap_text("", W, &M), Vec::<String>::new());
        assert_eq!(wrap_text("   ", W, &M), Vec::<String>::new());
        assert_eq!(wrap_text("hello world", W, &M), ["hello world"]);
        // exactly 20 chars fit, 21 do not
        assert_eq!(
            wrap_text("aaaaaaaaa bbbbbbbbbb", W, &M),
            ["aaaaaaaaa bbbbbbbbbb"]
        );
        assert_eq!(
            wrap_text("aaaaaaaaaa bbbbbbbbbb", W, &M),
            ["aaaaaaaaaa", "bbbbbbbbbb"]
        );
        // a word longer than the line is broken between characters, and the rest continues
        assert_eq!(
            wrap_text("x abcdefghijklmnopqrstuvwxyz y", W, &M),
            ["x", "abcdefghijklmnopqrst", "uvwxyz y"]
        );
        // narrower than one character: one character per line, no endless loop
        assert_eq!(wrap_text("abc", 3.0, &M), ["a", "b", "c"]);
        // nothing is lost: the words come back in order
        let text = "the quick brown fox jumps over the lazy dog and keeps running far away";
        let joined = wrap_text(text, 150.0, &M).join(" ");
        assert_eq!(joined, text);
    }

    #[test]
    fn clean_text_normalises() {
        assert_eq!(clean_text("  a\n\tb   c \u{7} d "), "a b c d");
        assert_eq!(clean_text(""), "");
        assert_eq!(clean_text("¿Qué tal? ¡Bien!"), "¿Qué tal? ¡Bien!");
        let long = "x".repeat(MAX_TEXT_CHARS + 500);
        assert_eq!(clean_text(&long).chars().count(), MAX_TEXT_CHARS);
        let tail = format!("{}END", "y ".repeat(MAX_TEXT_CHARS));
        assert!(clean_text(&tail).ends_with("END"));
    }

    #[test]
    fn views_are_equal_when_nothing_visible_changed() {
        let mut h = H::new();
        h.fin(0.0, 1, "same");
        let a = h.view(1.0);
        // idle level events do not change the view (no fill without speech)
        h.level(1.1, -50.0, false);
        h.level(1.35, -48.0, false);
        let b = h.view(1.4);
        assert_eq!(a, b);
    }
}
