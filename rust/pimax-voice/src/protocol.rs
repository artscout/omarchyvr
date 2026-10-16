//! The wire protocol: JSON lines on a Unix stream socket.
//!
//! Server -> client: one [`Event`] per line, `{"v":1,"type":...}`. Subscribers must ignore unknown `type`s and
//! fields, so new event kinds can be added without breaking the HUD. Client -> server: one [`Command`] per line,
//! `{"cmd":"pause"}`. Everything is UTF-8; a line never contains a raw newline (JSON escapes it).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Protocol version carried in every line.
pub const VERSION: u32 = 1;

/// Listener state, sent in [`Event::Status`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum State {
    /// Capturing and transcribing.
    Listening,
    /// Paused by a client (`{"cmd":"pause"}`): audio is dropped.
    Paused,
    /// No capture device (yet): waiting for it.
    NoDevice,
    /// Something failed; `detail` says what.
    Error,
    /// The listener is shutting down.
    Stopped,
}

/// One event, server -> clients.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    /// First line on every connection.
    Hello {
        engine: String,
        source: String,
        rate: u32,
    },
    /// Listener state; also sent to every new connection.
    Status {
        state: State,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        detail: String,
    },
    /// Input level (RMS dBFS over the last ~250 ms) and whether a speech segment is open; about 4 per second.
    Level { db: f32, speech: bool },
    /// A speech segment opened; `t` is the audio time in seconds since the listener started.
    SpeechStart { seg: u64, t: f64 },
    /// Hypothesis for the open segment so far (replaced by later partials and by the final).
    Partial { seg: u64, text: String, t: f64 },
    /// The segment's transcript. `text` can be empty (noise): subscribers then just drop the partial.
    /// `latency_ms`: end of speech -> this event; `decode_ms`: engine time for this decode.
    Final {
        seg: u64,
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lang: Option<String>,
        t0: f64,
        t1: f64,
        latency_ms: u32,
        decode_ms: u32,
    },
    /// A recognised command (see [`crate::intent`]). Only reported; acting on it is up to subscribers.
    Intent {
        seg: u64,
        name: String,
        text: String,
        #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
        slots: BTreeMap<String, String>,
    },
    /// Answer to [`Command::Ping`].
    Pong,
}

/// One command, client -> server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    Ping,
    /// Stop feeding audio to the recogniser (the open segment is dropped).
    Pause,
    Resume,
    /// The finals said before the first command client was there (sent once, to the first client asking;
    /// `pimax-voice act` asks when it starts: it starts ~12 s after the listener, live 2026-10-16).
    Backlog,
}

#[derive(Serialize)]
struct OutLine<'a> {
    v: u32,
    #[serde(flatten)]
    event: &'a Event,
}

#[derive(Deserialize)]
struct InLine {
    #[serde(flatten)]
    event: Event,
}

/// Round to millisecond precision (keeps the lines short and stable in tests).
pub fn ms(t: f64) -> f64 {
    (t * 1000.0).round() / 1000.0
}

/// The event as one line, without the trailing newline.
pub fn encode(event: &Event) -> String {
    serde_json::to_string(&OutLine { v: VERSION, event }).expect("events always serialise")
}

/// Parse one event line; `None` for blank lines, garbage and unknown event types.
pub fn decode(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    serde_json::from_str::<InLine>(line).ok().map(|l| l.event)
}

/// Parse one command line; `None` for anything else.
pub fn decode_command(line: &str) -> Option<Command> {
    serde_json::from_str(line.trim()).ok()
}

/// A command as one line, without the trailing newline.
pub fn encode_command(cmd: Command) -> String {
    serde_json::to_string(&cmd).expect("commands always serialise")
}

/// Socket path: `$PIMAX_VOICE_SOCKET`, else `$XDG_RUNTIME_DIR/pimax-voice.sock`, else `/tmp/pimax-voice-<uid>.sock`.
pub fn socket_path() -> PathBuf {
    socket_path_from(
        std::env::var_os("PIMAX_VOICE_SOCKET").map(PathBuf::from),
        std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from),
        // SAFETY: getuid cannot fail.
        unsafe { libc::getuid() },
    )
}

fn socket_path_from(explicit: Option<PathBuf>, runtime: Option<PathBuf>, uid: u32) -> PathBuf {
    if let Some(p) = explicit.filter(|p| !p.as_os_str().is_empty()) {
        return p;
    }
    match runtime.filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join("pimax-voice.sock"),
        None => PathBuf::from(format!("/tmp/pimax-voice-{uid}.sock")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_have_the_documented_shape() {
        let fin = Event::Final {
            seg: 7,
            text: "Open the browser.".into(),
            lang: Some("en".into()),
            t0: 12.3,
            t1: 13.82,
            latency_ms: 540,
            decode_ms: 71,
        };
        assert_eq!(
            encode(&fin),
            r#"{"v":1,"type":"final","seg":7,"text":"Open the browser.","lang":"en","t0":12.3,"t1":13.82,"latency_ms":540,"decode_ms":71}"#
        );
        assert_eq!(
            encode(&Event::Status {
                state: State::NoDevice,
                detail: String::new()
            }),
            r#"{"v":1,"type":"status","state":"no_device"}"#
        );
        assert_eq!(
            encode(&Event::Partial {
                seg: 1,
                text: "¿qué?".into(),
                t: 0.5
            }),
            r#"{"v":1,"type":"partial","seg":1,"text":"¿qué?","t":0.5}"#
        );
        assert_eq!(
            encode(&Event::SpeechStart { seg: 2, t: 1.25 }),
            r#"{"v":1,"type":"speech_start","seg":2,"t":1.25}"#
        );
        assert_eq!(
            encode(&Event::Level {
                db: -38.5,
                speech: true
            }),
            r#"{"v":1,"type":"level","db":-38.5,"speech":true}"#
        );
        assert_eq!(encode(&Event::Pong), r#"{"v":1,"type":"pong"}"#);
        let hello = Event::Hello {
            engine: "e".into(),
            source: "s".into(),
            rate: 16000,
        };
        assert_eq!(
            encode(&hello),
            r#"{"v":1,"type":"hello","engine":"e","source":"s","rate":16000}"#
        );
    }

    #[test]
    fn round_trip_and_tolerant_parsing() {
        let mut slots = BTreeMap::new();
        slots.insert("app".to_string(), "firefox".to_string());
        let all = [
            Event::Hello {
                engine: "parakeet".into(),
                source: "Pimax".into(),
                rate: 16000,
            },
            Event::Status {
                state: State::Error,
                detail: "no model".into(),
            },
            Event::Level {
                db: -60.0,
                speech: false,
            },
            Event::SpeechStart { seg: 3, t: 0.0 },
            Event::Partial {
                seg: 3,
                text: "line\nbreak \"quoted\"".into(),
                t: 1.0,
            },
            Event::Final {
                seg: 3,
                text: String::new(),
                lang: None,
                t0: 0.0,
                t1: 1.0,
                latency_ms: 0,
                decode_ms: 0,
            },
            Event::Intent {
                seg: 3,
                name: "app.open".into(),
                text: "open firefox".into(),
                slots,
            },
            Event::Pong,
        ];
        for e in &all {
            let line = encode(e);
            assert!(!line.contains('\n'), "{line}");
            assert_eq!(decode(&line).as_ref(), Some(e), "{line}");
        }
        // unknown fields are ignored, unknown types / garbage / blank give None
        assert_eq!(
            decode(r#"{"v":2,"type":"speech_start","seg":1,"t":2.0,"new_field":[1,2]}"#),
            Some(Event::SpeechStart { seg: 1, t: 2.0 })
        );
        assert_eq!(decode(r#"{"v":1,"type":"future_thing","x":1}"#), None);
        assert_eq!(decode("not json"), None);
        assert_eq!(decode("   "), None);
        // a final without lang parses (lang is optional)
        assert!(matches!(
            decode(
                r#"{"type":"final","seg":1,"text":"x","t0":0,"t1":1,"latency_ms":5,"decode_ms":2}"#
            ),
            Some(Event::Final { lang: None, .. })
        ));
    }

    #[test]
    fn commands() {
        assert_eq!(decode_command(r#"{"cmd":"pause"}"#), Some(Command::Pause));
        assert_eq!(
            decode_command(" {\"cmd\":\"resume\"}\n"),
            Some(Command::Resume)
        );
        assert_eq!(decode_command(r#"{"cmd":"ping"}"#), Some(Command::Ping));
        assert_eq!(decode_command(r#"{"cmd":"rm -rf"}"#), None);
        assert_eq!(
            decode_command(&encode_command(Command::Pause)),
            Some(Command::Pause)
        );
    }

    #[test]
    fn socket_path_precedence() {
        let p = |e: Option<&str>, r: Option<&str>| {
            socket_path_from(e.map(PathBuf::from), r.map(PathBuf::from), 1000)
        };
        assert_eq!(
            p(Some("/x/s.sock"), Some("/run/user/1000")),
            PathBuf::from("/x/s.sock")
        );
        assert_eq!(
            p(None, Some("/run/user/1000")),
            PathBuf::from("/run/user/1000/pimax-voice.sock")
        );
        assert_eq!(
            p(Some(""), None),
            PathBuf::from("/tmp/pimax-voice-1000.sock")
        );
    }

    #[test]
    fn ms_rounding() {
        assert_eq!(ms(1.23456), 1.235);
        assert_eq!(ms(0.0), 0.0);
    }
}
