//! Live captions for desk mode: the transcript of the `omarchyvr-voice` socket drawn on a WayVR panel
//! (`captions`, defined by the desk config `gui/captions.xml`) under the main screen. Nothing is typed anywhere.
//!
//! [`Captions`] is the pure state machine (events in, the two caption lines out); [`Wayvr`] feeds `wayvrctl batch`.
//!
//! Voice-AI debug: `omarchyvr-voice act` writes one line per command (matcher candidates > the model's decision >
//! the result > latency; see `act::Trace`) to `$XDG_RUNTIME_DIR/omarchyvr-voice-debug.line`. While it is fresh the
//! panel shows the last transcript line on top and the debug line under it in [`DEBUG_COLOR`] (the label colour is
//! set with `set-color`, so the panel XML needs no change).

use crate::protocol::Event;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// What the panel shows: up to two lines (the older one first); `visible` false hides the box.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct View {
    pub l1: String,
    pub l2: String,
    pub visible: bool,
    /// l2 is the debug line (drawn in [`DEBUG_COLOR`]).
    pub debug: bool,
}

/// The debug line's colour: a dim cyan, readable on the dark box and clearly not speech.
pub const DEBUG_COLOR: &str = "#5fd0e0ff";
/// The transcript colour (as in the panel XML).
pub const TEXT_COLOR: &str = "#ffffffff";

pub struct Captions {
    width: usize,
    lines: usize,
    clear_after: Duration,
    hist: String,
    partial: String,
    last: Option<Instant>,
    shown: View,
    debug: String,
}

impl Captions {
    pub fn new(width: usize, lines: usize, clear_after: Duration) -> Captions {
        Captions {
            width: width.max(10),
            lines: lines.clamp(1, 2),
            clear_after,
            hist: String::new(),
            partial: String::new(),
            last: None,
            shown: View::default(),
            debug: String::new(),
        }
    }

    /// A new debug line from `omarchyvr-voice act` (empty = none); shown until the silence timeout.
    pub fn set_debug(&mut self, line: &str, now: Instant) -> Option<View> {
        let line: String = line.trim().chars().take(self.width).collect();
        if line == self.debug {
            return None;
        }
        self.debug = line;
        if !self.debug.is_empty() {
            self.last = Some(now);
        }
        self.update()
    }

    /// Feed one event; returns the new view when it changed.
    pub fn on_event(&mut self, e: &Event, now: Instant) -> Option<View> {
        match e {
            Event::SpeechStart { .. } => {
                self.partial.clear();
                None
            }
            Event::Partial { text, .. } => {
                self.partial = text.trim().to_string();
                self.last = Some(now);
                self.update()
            }
            Event::Final { text, .. } => {
                self.partial.clear();
                let t = text.trim();
                if !t.is_empty() {
                    if !self.hist.is_empty() {
                        self.hist.push(' ');
                    }
                    self.hist.push_str(t);
                    let n = self.hist.chars().count();
                    if n > 600 {
                        self.hist = self.hist.chars().skip(n - 600).collect();
                    }
                }
                self.last = Some(now);
                self.update()
            }
            _ => None,
        }
    }

    /// Call periodically: clears the captions after the silence timeout.
    pub fn tick(&mut self, now: Instant) -> Option<View> {
        if let Some(t) = self.last {
            if now.duration_since(t) >= self.clear_after {
                self.last = None;
                self.hist.clear();
                self.partial.clear();
                self.debug.clear();
                return self.update();
            }
        }
        None
    }

    /// The view as of now (also what `update` compares against).
    fn view(&self) -> View {
        let mut all = self.hist.clone();
        if !self.partial.is_empty() {
            if !all.is_empty() {
                all.push(' ');
            }
            all.push_str(&self.partial);
        }
        let wrapped = wrap(&all, self.width);
        if !self.debug.is_empty() {
            // the newest transcript line on top, the debug line under it
            return View { l1: wrapped.last().cloned().unwrap_or_default(), l2: self.debug.clone(), visible: true, debug: true };
        }
        let keep: Vec<&String> = wrapped.iter().rev().take(self.lines).rev().collect();
        match keep.as_slice() {
            [] => View::default(),
            [a] => View {
                l1: String::new(),
                l2: (*a).clone(),
                visible: true,
                debug: false,
            },
            [a, b] => View {
                l1: (*a).clone(),
                l2: (*b).clone(),
                visible: true,
                debug: false,
            },
            _ => View::default(),
        }
    }

    fn update(&mut self) -> Option<View> {
        let v = self.view();
        if v == self.shown {
            None
        } else {
            self.shown = v.clone();
            Some(v)
        }
    }

    /// Forget what the panel shows (after the WayVR link was re-made) so the next update resends everything.
    pub fn resync(&mut self) -> View {
        self.shown = self.view();
        self.shown.clone()
    }
}

/// Greedy word wrap to `width` characters; a longer word is split.
pub fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    for word in text.split_whitespace() {
        let mut w: Vec<char> = word.chars().collect();
        while w.len() > width {
            if !cur.is_empty() {
                out.push(std::mem::take(&mut cur));
            }
            out.push(w.drain(..width).collect());
        }
        let word: String = w.into_iter().collect();
        let n = word.chars().count();
        if n == 0 {
            continue;
        }
        let cn = cur.chars().count();
        if cn == 0 {
            cur = word;
        } else if cn + 1 + n <= width {
            cur.push(' ');
            cur.push_str(&word);
        } else {
            out.push(std::mem::replace(&mut cur, word));
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
    out
}

/// Quote one argument for `wayvrctl batch` (shell-words syntax).
pub fn quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// The batch lines that make the panel show `v`.
pub fn lines_for(overlay: &str, v: &View) -> Vec<String> {
    let text = |id: &str, t: &str| {
        // an empty label is a single space (the command needs at least one word); `--` protects a leading dash
        let t = if t.is_empty() { " " } else { t };
        format!("panel-modify {overlay} {id} set-text -- {}", quote(t))
    };
    vec![
        text("cap_l1", &v.l1),
        text("cap_l2", &v.l2),
        format!("panel-modify {overlay} cap_l2 set-color {}", if v.debug { DEBUG_COLOR } else { TEXT_COLOR }),
        format!(
            "panel-modify {overlay} cap_box set-visible {}",
            u8::from(v.visible)
        ),
    ]
}

/// A `wayvrctl batch` child, started when needed and restarted when WayVR (and with it the IPC link) went away.
pub struct Wayvr {
    bin: PathBuf,
    overlay: String,
    child: Option<Child>,
    next_try: Instant,
}

impl Wayvr {
    pub fn new(bin: PathBuf, overlay: &str) -> Wayvr {
        Wayvr {
            bin,
            overlay: overlay.to_string(),
            child: None,
            next_try: Instant::now(),
        }
    }

    /// Send the view; returns false when there is no link (it is retried at most once a second) and `true` plus
    /// `fresh` set when a new link was just made (the caller then resends the full state).
    pub fn show(&mut self, v: &View, fresh: &mut bool) -> bool {
        *fresh = false;
        if let Some(c) = self.child.as_mut() {
            if !matches!(c.try_wait(), Ok(None)) {
                self.child = None;
            }
        }
        if self.child.is_none() {
            let now = Instant::now();
            if now < self.next_try {
                return false;
            }
            self.next_try = now + Duration::from_secs(1);
            match Command::new(&self.bin)
                .arg("batch")
                .stdin(Stdio::piped())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
            {
                Ok(c) => {
                    self.child = Some(c);
                    *fresh = true;
                }
                Err(e) => {
                    eprintln!("captions: cannot start {}: {e}", self.bin.display());
                    return false;
                }
            }
        }
        let mut data = lines_for(&self.overlay, v).join("\n");
        data.push('\n');
        let ok = self
            .child
            .as_mut()
            .and_then(|c| c.stdin.as_mut())
            .map(|s| s.write_all(data.as_bytes()).and_then(|_| s.flush()).is_ok())
            .unwrap_or(false);
        if !ok {
            self.stop();
        }
        ok
    }

    pub fn stop(&mut self) {
        if let Some(mut c) = self.child.take() {
            drop(c.stdin.take());
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

impl Drop for Wayvr {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fin(t: &str) -> Event {
        Event::Final {
            seg: 1,
            text: t.into(),
            lang: None,
            t0: 0.0,
            t1: 1.0,
            latency_ms: 0,
            decode_ms: 0,
        }
    }
    fn part(t: &str) -> Event {
        Event::Partial {
            seg: 1,
            text: t.into(),
            t: 0.0,
        }
    }

    #[test]
    fn wraps_words_and_splits_long_ones() {
        assert_eq!(wrap("aa bb cc", 5), ["aa bb", "cc"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert!(wrap("   ", 5).is_empty());
    }

    #[test]
    fn shows_partial_then_final_and_keeps_two_lines() {
        let t0 = Instant::now();
        let mut c = Captions::new(10, 2, Duration::from_secs(6));
        let v = c.on_event(&part("hello"), t0).unwrap();
        assert_eq!((v.l1.as_str(), v.l2.as_str(), v.visible), ("", "hello", true));
        assert!(c.on_event(&part("hello"), t0).is_none());
        let v = c.on_event(&fin("hello there you all"), t0).unwrap();
        assert_eq!((v.l1.as_str(), v.l2.as_str()), ("there you", "all"));
        let v = c.on_event(&fin("again"), t0).unwrap();
        assert_eq!((v.l1.as_str(), v.l2.as_str()), ("there you", "all again"));
    }

    #[test]
    fn clears_after_silence_and_not_before() {
        let t0 = Instant::now();
        let mut c = Captions::new(40, 2, Duration::from_secs(6));
        c.on_event(&fin("hi"), t0);
        assert!(c.tick(t0 + Duration::from_secs(5)).is_none());
        let v = c.tick(t0 + Duration::from_secs(6)).unwrap();
        assert!(!v.visible);
        assert!(c.tick(t0 + Duration::from_secs(9)).is_none());
        // a partial refreshes the timer
        c.on_event(&part("x"), t0 + Duration::from_secs(10));
        assert!(c.tick(t0 + Duration::from_secs(15)).is_none());
    }

    #[test]
    fn empty_final_drops_the_partial() {
        let t0 = Instant::now();
        let mut c = Captions::new(40, 2, Duration::from_secs(6));
        c.on_event(&part("noise"), t0);
        let v = c.on_event(&fin(""), t0).unwrap();
        assert!(!v.visible);
    }

    #[test]
    fn batch_lines_are_quoted() {
        let v = View {
            l1: "it's -x".into(),
            l2: String::new(),
            visible: true,
            debug: false,
        };
        let l = lines_for("captions", &v);
        assert_eq!(l[0], "panel-modify captions cap_l1 set-text -- 'it'\\''s -x'");
        assert_eq!(l[1], "panel-modify captions cap_l2 set-text -- ' '");
        assert_eq!(l[2], "panel-modify captions cap_l2 set-color #ffffffff");
        assert_eq!(l[3], "panel-modify captions cap_box set-visible 1");
        let l = lines_for("captions", &View { debug: true, ..v });
        assert_eq!(l[2], format!("panel-modify captions cap_l2 set-color {DEBUG_COLOR}"));
    }

    #[test]
    fn debug_line_goes_under_the_last_transcript_line() {
        let t0 = Instant::now();
        let mut c = Captions::new(10, 2, Duration::from_secs(6));
        c.on_event(&fin("max liz windows please"), t0);
        let v = c.set_debug("~win.list .95 > fuzzy > 3 windows 12ms", t0).unwrap();
        assert_eq!((v.l1.as_str(), v.l2.as_str(), v.debug), ("please", "~win.list ", true));
        assert!(c.set_debug("~win.list .95 > fuzzy > 3 windows 12ms", t0).is_none());
        // cleared with the captions after the silence
        let v = c.tick(t0 + Duration::from_secs(6)).unwrap();
        assert!(!v.visible && !v.debug);
        // an empty line removes it
        c.on_event(&fin("hello there"), t0);
        c.set_debug("x", t0);
        let v = c.set_debug("", t0).unwrap();
        assert!(!v.debug && v.l2 == "there");
    }
}
