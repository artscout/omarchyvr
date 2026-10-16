//! The `pimax-voice` wire protocol, subscriber side: one JSON object per line (UTF-8) from the server.
//!
//! Subscribers must ignore unknown event types and unknown fields, so everything here is tolerant: a line that is
//! not a JSON object, has no known `type`, or lacks a field the HUD needs gives `None` and is dropped. The version
//! field `v` is not checked (a newer server that keeps the field meanings still works).

use serde_json::Value;
use std::path::PathBuf;

/// Listener state of the `status` event. Unknown strings are kept in [`State::Other`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum State {
    Listening,
    Paused,
    NoDevice,
    Error,
    Stopped,
    Other(String),
}

impl State {
    pub fn parse(s: &str) -> State {
        match s {
            "listening" => State::Listening,
            "paused" => State::Paused,
            "no_device" => State::NoDevice,
            "error" => State::Error,
            "stopped" => State::Stopped,
            other => State::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            State::Listening => "listening",
            State::Paused => "paused",
            State::NoDevice => "no_device",
            State::Error => "error",
            State::Stopped => "stopped",
            State::Other(s) => s,
        }
    }
}

/// One server event.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    Hello {
        engine: String,
        source: String,
        rate: u32,
    },
    Status {
        state: State,
        detail: String,
    },
    Level {
        db: f32,
        speech: bool,
    },
    SpeechStart {
        seg: u64,
        t: f64,
    },
    Partial {
        seg: u64,
        text: String,
        t: f64,
    },
    Final {
        seg: u64,
        text: String,
        lang: Option<String>,
        t0: f64,
        t1: f64,
        latency_ms: u32,
        decode_ms: u32,
    },
    Intent {
        seg: u64,
        name: String,
        text: String,
    },
}

fn get_str(v: &Value, key: &str) -> Option<String> {
    v.get(key)?.as_str().map(str::to_string)
}

/// An unsigned integer; a whole-valued float (`7.0`) is accepted too.
fn get_u64(v: &Value, key: &str) -> Option<u64> {
    let x = v.get(key)?;
    x.as_u64().or_else(|| {
        let f = x.as_f64()?;
        (f >= 0.0 && f.fract() == 0.0 && f < 1.8e19).then_some(f as u64)
    })
}

fn get_f64(v: &Value, key: &str) -> Option<f64> {
    v.get(key)?.as_f64()
}

fn get_ms(v: &Value, key: &str) -> u32 {
    get_f64(v, key).map_or(0, |f| f.clamp(0.0, f64::from(u32::MAX)) as u32)
}

/// Parse one line. `None` for blank lines, garbage, unknown event types and events missing a required field.
pub fn parse_line(line: &str) -> Option<Event> {
    let line = line.trim();
    if line.is_empty() {
        return None;
    }
    let v: Value = serde_json::from_str(line).ok()?;
    if !v.is_object() {
        return None;
    }
    let ty = v.get("type")?.as_str()?;
    Some(match ty {
        "hello" => Event::Hello {
            engine: get_str(&v, "engine").unwrap_or_default(),
            source: get_str(&v, "source").unwrap_or_default(),
            rate: get_u64(&v, "rate").map_or(0, |r| r.min(u64::from(u32::MAX)) as u32),
        },
        "status" => Event::Status {
            state: State::parse(&get_str(&v, "state")?),
            detail: get_str(&v, "detail").unwrap_or_default(),
        },
        "level" => {
            let db = get_f64(&v, "db")?;
            Event::Level {
                db: db as f32,
                speech: v.get("speech").and_then(Value::as_bool).unwrap_or(false),
            }
        }
        "speech_start" => Event::SpeechStart {
            seg: get_u64(&v, "seg")?,
            t: get_f64(&v, "t").unwrap_or(0.0),
        },
        "partial" => Event::Partial {
            seg: get_u64(&v, "seg")?,
            text: get_str(&v, "text")?,
            t: get_f64(&v, "t").unwrap_or(0.0),
        },
        "final" => Event::Final {
            seg: get_u64(&v, "seg")?,
            text: get_str(&v, "text")?,
            lang: get_str(&v, "lang"),
            t0: get_f64(&v, "t0").unwrap_or(0.0),
            t1: get_f64(&v, "t1").unwrap_or(0.0),
            latency_ms: get_ms(&v, "latency_ms"),
            decode_ms: get_ms(&v, "decode_ms"),
        },
        "intent" => Event::Intent {
            seg: get_u64(&v, "seg").unwrap_or(0),
            name: get_str(&v, "name")?,
            text: get_str(&v, "text").unwrap_or_default(),
        },
        _ => return None,
    })
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

/// The path rule of [`socket_path`] with the environment passed in (empty values count as unset).
pub fn socket_path_from(
    explicit: Option<PathBuf>,
    runtime_dir: Option<PathBuf>,
    uid: u32,
) -> PathBuf {
    if let Some(p) = explicit.filter(|p| !p.as_os_str().is_empty()) {
        return p;
    }
    match runtime_dir.filter(|p| !p.as_os_str().is_empty()) {
        Some(dir) => dir.join("pimax-voice.sock"),
        None => PathBuf::from(format!("/tmp/pimax-voice-{uid}.sock")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hello() {
        let e = parse_line(
            r#"{"v":1,"type":"hello","engine":"parakeet-tdt-0.6b-v3-int8","source":"Pimax Dream Air Analog Stereo","rate":16000}"#,
        );
        assert_eq!(
            e,
            Some(Event::Hello {
                engine: "parakeet-tdt-0.6b-v3-int8".into(),
                source: "Pimax Dream Air Analog Stereo".into(),
                rate: 16000
            })
        );
    }

    #[test]
    fn status_all_states_and_unknown() {
        for (s, state) in [
            ("listening", State::Listening),
            ("paused", State::Paused),
            ("no_device", State::NoDevice),
            ("error", State::Error),
            ("stopped", State::Stopped),
            ("warming_up", State::Other("warming_up".into())),
        ] {
            let line = format!(r#"{{"v":1,"type":"status","state":"{s}","detail":"d x"}}"#);
            assert_eq!(
                parse_line(&line),
                Some(Event::Status {
                    state: state.clone(),
                    detail: "d x".into()
                }),
                "{s}"
            );
            assert_eq!(state.as_str(), s);
        }
        // detail is optional
        assert_eq!(
            parse_line(r#"{"type":"status","state":"no_device"}"#),
            Some(Event::Status {
                state: State::NoDevice,
                detail: String::new()
            })
        );
        // state is required
        assert_eq!(parse_line(r#"{"type":"status","detail":"x"}"#), None);
        assert_eq!(parse_line(r#"{"type":"status","state":5}"#), None);
    }

    #[test]
    fn level() {
        assert_eq!(
            parse_line(r#"{"v":1,"type":"level","db":-38.5,"speech":true}"#),
            Some(Event::Level {
                db: -38.5,
                speech: true
            })
        );
        // an integer dB value and a missing speech flag
        assert_eq!(
            parse_line(r#"{"type":"level","db":-60}"#),
            Some(Event::Level {
                db: -60.0,
                speech: false
            })
        );
        assert_eq!(parse_line(r#"{"type":"level","speech":true}"#), None);
        assert_eq!(parse_line(r#"{"type":"level","db":"loud"}"#), None);
    }

    #[test]
    fn speech_start_and_partial() {
        assert_eq!(
            parse_line(r#"{"v":1,"type":"speech_start","seg":7,"t":12.34}"#),
            Some(Event::SpeechStart { seg: 7, t: 12.34 })
        );
        assert_eq!(
            parse_line(r#"{"v":1,"type":"partial","seg":7,"text":"open the bro","t":12.9}"#),
            Some(Event::Partial {
                seg: 7,
                text: "open the bro".into(),
                t: 12.9
            })
        );
        // seg is required, t is not
        assert_eq!(parse_line(r#"{"type":"partial","text":"x"}"#), None);
        assert_eq!(
            parse_line(r#"{"type":"partial","seg":2,"text":"x"}"#),
            Some(Event::Partial {
                seg: 2,
                text: "x".into(),
                t: 0.0
            })
        );
        assert_eq!(parse_line(r#"{"type":"partial","seg":2}"#), None);
        assert_eq!(
            parse_line(r#"{"type":"partial","seg":-2,"text":"x"}"#),
            None
        );
        // an integral float seg is accepted
        assert!(matches!(
            parse_line(r#"{"type":"speech_start","seg":3.0}"#),
            Some(Event::SpeechStart { seg: 3, .. })
        ));
        assert_eq!(parse_line(r#"{"type":"speech_start","seg":3.5}"#), None);
    }

    #[test]
    fn final_event() {
        let line = r#"{"v":1,"type":"final","seg":7,"text":"Open the browser.","lang":"en","t0":12.30,"t1":13.82,"latency_ms":540,"decode_ms":71}"#;
        assert_eq!(
            parse_line(line),
            Some(Event::Final {
                seg: 7,
                text: "Open the browser.".into(),
                lang: Some("en".into()),
                t0: 12.30,
                t1: 13.82,
                latency_ms: 540,
                decode_ms: 71
            })
        );
        // empty text is a valid final (the HUD drops the partial of that segment)
        assert!(matches!(
            parse_line(r#"{"type":"final","seg":9,"text":""}"#),
            Some(Event::Final { seg: 9, ref text, lang: None, latency_ms: 0, .. }) if text.is_empty()
        ));
        assert_eq!(parse_line(r#"{"type":"final","seg":9}"#), None);
    }

    #[test]
    fn intent() {
        assert_eq!(
            parse_line(
                r#"{"v":1,"type":"intent","seg":7,"name":"hud.clear","text":"clear the screen"}"#
            ),
            Some(Event::Intent {
                seg: 7,
                name: "hud.clear".into(),
                text: "clear the screen".into()
            })
        );
        // slots and other fields are ignored
        assert!(matches!(
            parse_line(
                r#"{"type":"intent","seg":1,"name":"app.open","text":"x","slots":{"app":"firefox"}}"#
            ),
            Some(Event::Intent { .. })
        ));
        assert_eq!(parse_line(r#"{"type":"intent","seg":1}"#), None);
    }

    #[test]
    fn unknown_types_fields_and_garbage_are_ignored() {
        assert_eq!(parse_line(r#"{"v":1,"type":"pong"}"#), None);
        assert_eq!(parse_line(r#"{"v":1,"type":"future_thing","x":1}"#), None);
        assert_eq!(parse_line(""), None);
        assert_eq!(parse_line("   \t "), None);
        assert_eq!(parse_line("not json at all"), None);
        assert_eq!(parse_line("{"), None);
        assert_eq!(
            parse_line(r#"{"v":1,"type":"partial","seg":1,"text":"cut o"#),
            None
        );
        assert_eq!(parse_line("[1,2,3]"), None);
        assert_eq!(parse_line("42"), None);
        assert_eq!(parse_line("null"), None);
        assert_eq!(parse_line(r#""type""#), None);
        assert_eq!(parse_line(r#"{"type":5}"#), None);
        assert_eq!(parse_line(r#"{"text":"no type"}"#), None);
        // unknown fields and a newer version number do not matter
        assert_eq!(
            parse_line(
                r#"{"v":2,"type":"speech_start","seg":1,"t":2.0,"new_field":[1,2],"x":{"y":null}}"#
            ),
            Some(Event::SpeechStart { seg: 1, t: 2.0 })
        );
    }

    #[test]
    fn text_with_unicode_and_escapes() {
        // raw UTF-8, \u escapes, a surrogate pair, embedded newline and quote
        let e = parse_line(
            "{\"type\":\"final\",\"seg\":1,\"text\":\"¿Qué hora es? ¡Año nuevo! \\u00f1 \\ud83d\\ude00 l1\\nl2 \\\"q\\\"\"}",
        );
        match e {
            Some(Event::Final { text, .. }) => {
                assert!(
                    text.starts_with("¿Qué hora es? ¡Año nuevo! ñ 😀 l1\nl2 \"q\""),
                    "{text}"
                );
            }
            other => panic!("{other:?}"),
        }
        // CRLF line ending and leading whitespace
        assert!(parse_line("  {\"type\":\"speech_start\",\"seg\":1}\r").is_some());
    }

    /// xorshift64: deterministic pseudo-random numbers for the robustness tests.
    fn rng(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn never_panics_on_damaged_lines() {
        let valid = [
            r#"{"v":1,"type":"hello","engine":"e","source":"s","rate":16000}"#,
            r#"{"v":1,"type":"status","state":"error","detail":"x"}"#,
            r#"{"v":1,"type":"level","db":-38.5,"speech":true}"#,
            r#"{"v":1,"type":"partial","seg":7,"text":"open the bro ñ ¿","t":12.9}"#,
            r#"{"v":1,"type":"final","seg":7,"text":"Open.","lang":"en","t0":1,"t1":2,"latency_ms":5,"decode_ms":6}"#,
            r#"{"v":1,"type":"intent","seg":7,"name":"hud.clear","text":"x"}"#,
        ];
        let mut seed = 0x9e37_79b9_7f4a_7c15u64;
        let mut parsed = 0;
        for line in valid {
            // every prefix and every single-character corruption
            let chars: Vec<char> = line.chars().collect();
            for cut in 0..=chars.len() {
                let s: String = chars[..cut].iter().collect();
                if parse_line(&s).is_some() {
                    parsed += 1;
                }
            }
            for _ in 0..300 {
                let mut c = chars.clone();
                let i = (rng(&mut seed) as usize) % c.len();
                c[i] = char::from_u32((rng(&mut seed) % 0x300) as u32).unwrap_or('?');
                let _ = parse_line(&c.iter().collect::<String>());
            }
        }
        // the complete lines themselves parse
        assert!(parsed >= valid.len());
        // random printable garbage and random bytes (lossy UTF-8) never panic
        for _ in 0..2000 {
            let n = (rng(&mut seed) % 80) as usize;
            let bytes: Vec<u8> = (0..n).map(|_| rng(&mut seed) as u8).collect();
            let _ = parse_line(&String::from_utf8_lossy(&bytes));
        }
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
            p(Some(""), Some("")),
            PathBuf::from("/tmp/pimax-voice-1000.sock")
        );
        assert_eq!(p(None, None), PathBuf::from("/tmp/pimax-voice-1000.sock"));
    }
}
