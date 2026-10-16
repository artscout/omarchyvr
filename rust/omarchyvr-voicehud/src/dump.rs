//! `dump`: connect to the socket and print what arrives and what the display would show, for debugging the protocol
//! without a headset.

use crate::cli::DumpOpts;
use crate::client::{Client, Notice};
use crate::display::{Config, Display, FixedMeasure, Kind, Measure, View};
use crate::font::{Font, FontMeasure, DEFAULT_FONTS};
use crate::hud::clamp_lines;
use crate::log;
use crate::protocol::{self, Event};
use crate::render::Style;
use std::io::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

fn quote(s: &str) -> String {
    format!("{s:?}")
}

/// One event as one line.
pub fn format_event(e: &Event) -> String {
    match e {
        Event::Hello {
            engine,
            source,
            rate,
        } => format!("hello engine={engine} source={} rate={rate}", quote(source)),
        Event::Status { state, detail } => {
            if detail.is_empty() {
                format!("status {}", state.as_str())
            } else {
                format!("status {} {}", state.as_str(), quote(detail))
            }
        }
        Event::Level { db, speech } => {
            format!("level {db:.1} dB{}", if *speech { " speech" } else { "" })
        }
        Event::SpeechStart { seg, t } => format!("speech_start seg={seg} t={t}"),
        Event::Partial { seg, text, t } => format!("partial seg={seg} t={t} {}", quote(text)),
        Event::Final {
            seg,
            text,
            lang,
            t0,
            t1,
            latency_ms,
            decode_ms,
        } => format!(
            "final seg={seg} lang={} t0={t0} t1={t1} latency={latency_ms}ms decode={decode_ms}ms {}",
            lang.as_deref().unwrap_or("-"),
            quote(text)
        ),
        Event::Intent { seg, name, text } => {
            format!("intent seg={seg} name={name} {}", quote(text))
        }
    }
}

/// The display lines of a view; `|` final, `~` partial, `!` error, `*` notice.
pub fn format_view(v: &View) -> String {
    if !v.visible {
        return "display: hidden".to_string();
    }
    let mut s = format!(
        "display: visible dot={:?} level={}/{}",
        v.dot,
        v.level,
        crate::display::LEVEL_STEPS
    );
    for l in &v.lines {
        let mark = match l.kind {
            Kind::Final => '|',
            Kind::Partial => '~',
            Kind::Error => '!',
            Kind::Notice => '*',
        };
        s.push_str(&format!("\n  {mark} {}", l.text));
    }
    s
}

/// Run until `stop`, or `opts.seconds`. Output errors (a closed pipe) end the run quietly.
pub fn run(opts: &DumpOpts, out: &mut dyn Write, stop: &AtomicBool) -> Result<(), String> {
    let path = opts.socket.clone().unwrap_or_else(protocol::socket_path);
    let mut notes = Vec::new();
    let font = Font::load_first(opts.font.as_deref(), &DEFAULT_FONTS, &mut notes);
    for n in &notes {
        log::warn(n);
    }
    let (lines, _) = clamp_lines(opts.lines, opts.font_px);
    let style = Style::new(opts.font_px, lines);
    let max_w = style.text_max_width();
    let measure: Box<dyn Measure + '_> = match &font {
        Ok(f) => Box::new(FontMeasure {
            font: f,
            em: style.em,
        }),
        Err(e) => {
            log::warn(format!("{e}; wrapping with a fixed character width"));
            Box::new(FixedMeasure(0.55 * style.em))
        }
    };
    let mut display = Display::new(Config {
        keep: opts.keep,
        max_lines: lines,
        timeout: (opts.timeout > 0.0).then(|| Duration::from_secs_f32(opts.timeout)),
        idle_dot: false,
    });
    let mut client = Client::new(path.clone(), Duration::from_secs(1));
    client.set_raw_tap(opts.raw);
    let start = Instant::now();
    let mut seen_revision = u64::MAX;
    let mut last_view: Option<View> = None;

    macro_rules! say {
        ($t:expr, $($arg:tt)*) => {{
            let r = writeln!(out, "[{:7.2}s] {}", $t, format!($($arg)*));
            if let Err(e) = r {
                return if e.kind() == std::io::ErrorKind::BrokenPipe { Ok(()) } else { Err(e.to_string()) };
            }
        }};
    }

    say!(0.0, "# listening on {}", path.display());
    loop {
        if stop.load(Ordering::Relaxed) {
            break;
        }
        let now = Instant::now();
        let t = now.duration_since(start).as_secs_f32();
        if opts.seconds.is_some_and(|s| t >= s) {
            break;
        }
        let mut notices = Vec::new();
        client.poll(now, &mut notices);
        for n in notices {
            match n {
                Notice::Connected => {
                    say!(t, "connected");
                    display.on_connected();
                }
                Notice::Down(why) => {
                    say!(t, "down: {why}");
                    display.on_down();
                }
                Notice::Raw(line) => {
                    if protocol::parse_line(&line).is_none() {
                        say!(t, "ignored {line}");
                    } else {
                        say!(t, "raw {line}");
                    }
                }
                Notice::Event(e) => {
                    say!(t, "{}", format_event(&e));
                    display.on_event(&e, now);
                }
            }
        }
        display.expire(now);
        if display.revision() != seen_revision {
            seen_revision = display.revision();
            let v = display.view(measure.as_ref(), max_w);
            if last_view.as_ref() != Some(&v) {
                for l in format_view(&v).lines() {
                    say!(t, "{l}");
                }
                last_view = Some(v);
            }
        }
        if let Err(e) = out.flush() {
            return if e.kind() == std::io::ErrorKind::BrokenPipe {
                Ok(())
            } else {
                Err(e.to_string())
            };
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Dot, ViewLine};
    use crate::protocol::{parse_line, State};
    use std::os::unix::net::UnixListener;

    #[test]
    fn events_format_on_one_line() {
        let f = |l: &str| format_event(&parse_line(l).unwrap());
        assert_eq!(
            f(
                r#"{"v":1,"type":"hello","engine":"p","source":"Pimax Dream Air Analog Stereo","rate":16000}"#
            ),
            "hello engine=p source=\"Pimax Dream Air Analog Stereo\" rate=16000"
        );
        assert_eq!(
            f(r#"{"type":"status","state":"listening"}"#),
            "status listening"
        );
        assert_eq!(
            f(r#"{"type":"status","state":"no_device","detail":"gone"}"#),
            "status no_device \"gone\""
        );
        assert_eq!(
            f(r#"{"type":"level","db":-38.54,"speech":true}"#),
            "level -38.5 dB speech"
        );
        assert_eq!(
            f(r#"{"type":"level","db":-50,"speech":false}"#),
            "level -50.0 dB"
        );
        assert_eq!(
            f(r#"{"type":"speech_start","seg":7,"t":12.34}"#),
            "speech_start seg=7 t=12.34"
        );
        assert_eq!(
            f(r#"{"type":"partial","seg":7,"text":"open the bro","t":12.9}"#),
            "partial seg=7 t=12.9 \"open the bro\""
        );
        assert_eq!(
            f(
                r#"{"type":"final","seg":7,"text":"Open the browser.","lang":"en","t0":12.3,"t1":13.82,"latency_ms":540,"decode_ms":71}"#
            ),
            "final seg=7 lang=en t0=12.3 t1=13.82 latency=540ms decode=71ms \"Open the browser.\""
        );
        assert_eq!(
            f(r#"{"type":"intent","seg":7,"name":"hud.clear","text":"clear the screen"}"#),
            "intent seg=7 name=hud.clear \"clear the screen\""
        );
        // no newline can leak into the dump
        assert!(!f(r#"{"type":"partial","seg":1,"text":"a\nb","t":0}"#).contains('\n'));
        assert_eq!(State::Paused.as_str(), "paused");
    }

    #[test]
    fn views_format_with_line_marks() {
        assert_eq!(format_view(&View::hidden()), "display: hidden");
        let v = View {
            visible: true,
            lines: vec![
                ViewLine {
                    text: "Open the browser.".into(),
                    kind: Kind::Final,
                },
                ViewLine {
                    text: "and then".into(),
                    kind: Kind::Partial,
                },
                ViewLine {
                    text: "mic gone".into(),
                    kind: Kind::Error,
                },
                ViewLine {
                    text: "paused".into(),
                    kind: Kind::Notice,
                },
            ],
            dot: Dot::Error,
            level: 7,
        };
        assert_eq!(
            format_view(&v),
            "display: visible dot=Error level=7/20\n  | Open the browser.\n  ~ and then\n  ! mic gone\n  * paused"
        );
    }

    #[test]
    fn dumps_a_live_stream() {
        let dir = std::env::temp_dir().join(format!("omarchyvr-voicehud-dump-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("v.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let server = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            for l in [
                r#"{"v":1,"type":"hello","engine":"e","source":"s","rate":16000}"#,
                r#"{"v":1,"type":"mystery","x":1}"#,
                "garbage",
                r#"{"v":1,"type":"partial","seg":1,"text":"hola","t":1}"#,
                r#"{"v":1,"type":"final","seg":1,"text":"Hola, ¿qué tal?","lang":"es","t0":0,"t1":1,"latency_ms":9,"decode_ms":8}"#,
            ] {
                s.write_all(l.as_bytes()).unwrap();
                s.write_all(b"\n").unwrap();
                std::thread::sleep(Duration::from_millis(30));
            }
            std::thread::sleep(Duration::from_millis(300));
        });
        let opts = DumpOpts {
            socket: Some(path.clone()),
            seconds: Some(1.5),
            raw: true,
            lines: 3,
            keep: 2,
            timeout: 12.0,
            font: None,
            font_px: 64.0,
        };
        let mut out = Vec::new();
        run(&opts, &mut out, &AtomicBool::new(false)).unwrap();
        server.join().unwrap();
        let text = String::from_utf8(out).unwrap();
        for needle in [
            "# listening on",
            "connected",
            "hello engine=e",
            "ignored garbage",
            "ignored {\"v\":1,\"type\":\"mystery\"",
            "partial seg=1",
            "~ hola",
            "final seg=1 lang=es",
            "| Hola, ¿qué tal?",
            "down: connection closed by the server",
        ] {
            assert!(text.contains(needle), "missing {needle:?} in\n{text}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_stop_flag_ends_the_dump_and_a_closed_pipe_is_not_an_error() {
        struct Closed;
        impl Write for Closed {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let opts = DumpOpts {
            socket: Some("/nonexistent/none.sock".into()),
            seconds: None,
            raw: false,
            lines: 3,
            keep: 2,
            timeout: 12.0,
            font: None,
            font_px: 64.0,
        };
        assert!(run(&opts, &mut Closed, &AtomicBool::new(false)).is_ok());
        let mut out = Vec::new();
        let t = Instant::now();
        run(&opts, &mut out, &AtomicBool::new(true)).unwrap();
        assert!(t.elapsed() < Duration::from_secs(1));
    }
}
