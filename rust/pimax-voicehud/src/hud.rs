//! The caption state of the running program: the event source (socket or demo), the display model, the font, and the
//! texture. [`Hud::pump`] is called once per frame; it reads what the source has, expires old text, and re-renders
//! the texture only when the visible content actually changed.

use crate::client::{Client, Notice};
use crate::demo::Demo;
use crate::display::{Config, Display, View};
use crate::font::{Font, FontMeasure};
use crate::log;
use crate::protocol::Event;
use crate::render::{self, Style};
use std::time::Instant;

/// Where the events come from.
pub enum Source {
    /// Nothing (tests, preview).
    None,
    Socket(Client),
    Demo(Demo),
}

/// What the renderer needs to know after a pump.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    /// Submit the quad layer now (nothing visible: submit no layer).
    pub visible: bool,
    /// Changes with every re-render; the texture must be uploaded again when it differs from the last upload.
    pub generation: u64,
}

pub struct Hud {
    source: Source,
    display: Display,
    font: Font,
    style: Style,
    seen_revision: u64,
    current: View,
    rendered: Option<View>,
    texture: Vec<u8>,
    generation: u64,
}

/// `lines` limited to what fits the texture at `em`; the second value is a warning when it was reduced.
pub fn clamp_lines(lines: usize, em: f32) -> (usize, Option<String>) {
    let fit = Style::new(em, 1).max_lines_fit();
    if lines > fit {
        (
            fit,
            Some(format!(
                "--lines {lines} does not fit the 400 px texture at --font-size {em}; using {fit}"
            )),
        )
    } else {
        (lines, None)
    }
}

impl Hud {
    /// `cfg.max_lines` is limited to what fits the texture (see [`clamp_lines`]).
    pub fn new(source: Source, font: Font, em: f32, mut cfg: Config) -> Hud {
        cfg.max_lines = clamp_lines(cfg.max_lines, em).0;
        let style = Style::new(em, cfg.max_lines);
        let display = Display::new(cfg);
        Hud {
            source,
            display,
            font,
            style,
            seen_revision: u64::MAX,
            current: View::hidden(),
            rendered: None,
            texture: Vec::new(),
            generation: 0,
        }
    }

    pub fn font(&self) -> &Font {
        &self.font
    }

    pub fn style(&self) -> &Style {
        &self.style
    }

    pub fn view(&self) -> &View {
        &self.current
    }

    /// The last rendered texture: RGBA, straight alpha, sRGB-coded; empty until something was visible.
    pub fn texture(&self) -> &[u8] {
        &self.texture
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    fn apply(&mut self, notice: Notice, now: Instant) {
        match notice {
            Notice::Connected => {
                if let Source::Socket(c) = &self.source {
                    log::info(format!("voice socket connected: {}", c.path().display()));
                }
                self.display.on_connected();
            }
            Notice::Down(why) => {
                log::info(format!("voice socket down: {why} (retrying every 1 s)"));
                self.display.on_down();
            }
            Notice::Event(e) => {
                if let Event::Hello {
                    engine,
                    source,
                    rate,
                } = &e
                {
                    log::info(format!(
                        "voice engine {engine}, source \"{source}\", {rate} Hz"
                    ));
                }
                self.display.on_event(&e, now);
            }
            Notice::Raw(_) => {}
        }
    }

    /// Read the source, expire old text, and re-render if the visible content changed. Never blocks.
    pub fn pump(&mut self, now: Instant) -> Frame {
        let mut notices = Vec::new();
        match &mut self.source {
            Source::None => {}
            Source::Socket(c) => c.poll(now, &mut notices),
            Source::Demo(d) => d.poll(now, &mut notices),
        }
        for n in notices {
            self.apply(n, now);
        }
        self.display.expire(now);
        if self.display.revision() != self.seen_revision {
            self.seen_revision = self.display.revision();
            let measure = FontMeasure {
                font: &self.font,
                em: self.style.em,
            };
            self.current = self.display.view(&measure, self.style.text_max_width());
        }
        if self.current.visible && self.rendered.as_ref() != Some(&self.current) {
            self.texture = render::render(&self.current, &self.font, &self.style);
            self.rendered = Some(self.current.clone());
            self.generation += 1;
        }
        Frame {
            visible: self.current.visible,
            generation: self.generation,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::font::testfont::font;
    use crate::font::{Font, DEFAULT_FONTS};
    use crate::placement::{TEX_H, TEX_W};
    use crate::protocol::parse_line;
    use std::io::Write;
    use std::os::unix::net::UnixListener;
    use std::path::PathBuf;
    use std::time::Duration;

    fn load_font() -> Font {
        let mut notes = Vec::new();
        Font::load_first(None, &DEFAULT_FONTS, &mut notes).unwrap()
    }

    fn hud() -> Hud {
        let _ = font();
        Hud::new(Source::None, load_font(), 64.0, Config::default())
    }

    fn feed(h: &mut Hud, t: Instant, line: &str) {
        h.display.on_event(&parse_line(line).unwrap(), t);
    }

    #[test]
    fn renders_only_when_the_visible_content_changes() {
        let mut h = hud();
        let t0 = Instant::now();
        let f = h.pump(t0);
        assert_eq!(
            f,
            Frame {
                visible: false,
                generation: 0
            }
        );
        assert!(h.texture().is_empty(), "nothing rendered while hidden");
        // idle level events change nothing visible
        for i in 0..8 {
            feed(
                &mut h,
                t0 + Duration::from_millis(i * 250),
                r#"{"type":"level","db":-50,"speech":false}"#,
            );
            assert_eq!(h.pump(t0 + Duration::from_millis(i * 250)).generation, 0);
        }
        // the first partial renders
        feed(
            &mut h,
            t0,
            r#"{"type":"partial","seg":1,"text":"open the","t":1}"#,
        );
        let f = h.pump(t0);
        assert_eq!(
            f,
            Frame {
                visible: true,
                generation: 1
            }
        );
        assert_eq!(h.texture().len(), (TEX_W * TEX_H * 4) as usize);
        // pumping again without changes does not render
        for k in 0..30 {
            assert_eq!(h.pump(t0 + Duration::from_millis(k * 14)).generation, 1);
        }
        // a different partial renders again; the same text again does not
        feed(
            &mut h,
            t0,
            r#"{"type":"partial","seg":1,"text":"open the bro","t":1}"#,
        );
        assert_eq!(h.pump(t0).generation, 2);
        feed(
            &mut h,
            t0,
            r#"{"type":"partial","seg":1,"text":"open the bro","t":1}"#,
        );
        assert_eq!(h.pump(t0).generation, 2);
        // the level bar (speech) is a visible change, but quantised: steps of 3 dB
        feed(&mut h, t0, r#"{"type":"level","db":-30.0,"speech":true}"#);
        assert_eq!(h.pump(t0).generation, 3);
        feed(&mut h, t0, r#"{"type":"level","db":-29.0,"speech":true}"#);
        assert_eq!(h.pump(t0).generation, 3, "same step");
        feed(&mut h, t0, r#"{"type":"level","db":-20.0,"speech":true}"#);
        assert_eq!(h.pump(t0).generation, 4);
    }

    #[test]
    fn hides_after_the_timeout_and_shows_the_same_content_without_rendering_again() {
        let mut h = hud();
        let t0 = Instant::now();
        feed(
            &mut h,
            t0,
            r#"{"type":"status","state":"no_device","detail":"mic unplugged"}"#,
        );
        let f = h.pump(t0);
        assert!(f.visible);
        let g = f.generation;
        // errors stay forever
        let f = h.pump(t0 + Duration::from_secs(3600));
        assert!(f.visible && f.generation == g);
        // an error that clears hides; the same error again shows the same view: no new render needed
        feed(
            &mut h,
            t0,
            r#"{"type":"status","state":"listening","detail":""}"#,
        );
        assert_eq!(
            h.pump(t0),
            Frame {
                visible: false,
                generation: g
            }
        );
        feed(
            &mut h,
            t0,
            r#"{"type":"status","state":"no_device","detail":"mic unplugged"}"#,
        );
        assert_eq!(
            h.pump(t0),
            Frame {
                visible: true,
                generation: g
            }
        );
        // text times out after 12 s
        feed(
            &mut h,
            t0,
            r#"{"type":"status","state":"listening","detail":""}"#,
        );
        feed(
            &mut h,
            t0,
            r#"{"type":"final","seg":1,"text":"Hola","t0":0,"t1":1,"latency_ms":1,"decode_ms":1}"#,
        );
        assert!(h.pump(t0 + Duration::from_secs(11)).visible);
        assert!(!h.pump(t0 + Duration::from_secs(12)).visible);
    }

    #[test]
    fn lines_are_limited_to_what_fits() {
        let (n, warn) = clamp_lines(3, 64.0);
        assert_eq!((n, warn), (3, None));
        let (n, warn) = clamp_lines(8, 64.0);
        assert_eq!(n, 3);
        assert!(warn.unwrap().contains("--lines 8"));
        assert!(clamp_lines(8, 32.0).0 > 3);
        // the constructor applies it
        let h = Hud::new(
            Source::None,
            load_font(),
            64.0,
            Config {
                max_lines: 8,
                ..Config::default()
            },
        );
        assert_eq!(h.display.config().max_lines, 3);
    }

    #[test]
    fn works_end_to_end_with_a_socket() {
        let dir = std::env::temp_dir().join(format!("pimax-voicehud-hud-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path: PathBuf = dir.join("v.sock");
        let listener = UnixListener::bind(&path).unwrap();
        let client = Client::new(path.clone(), Duration::from_secs(1));
        let mut h = Hud::new(Source::Socket(client), load_font(), 64.0, Config::default());
        let mut now = Instant::now();
        h.pump(now);
        let (mut server, _) = listener.accept().unwrap();
        server
            .write_all(
                b"{\"v\":1,\"type\":\"hello\",\"engine\":\"e\",\"source\":\"s\",\"rate\":16000}\n{\"v\":1,\"type\":\"final\",\"seg\":1,\"text\":\"\xc2\xbfQu\xc3\xa9 tal?\",\"t0\":0,\"t1\":1,\"latency_ms\":1,\"decode_ms\":1}\n",
            )
            .unwrap();
        let end = Instant::now() + Duration::from_secs(2);
        while !h.view().visible && Instant::now() < end {
            now += Duration::from_millis(14);
            h.pump(now);
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(h.view().visible);
        assert_eq!(h.view().lines[0].text, "¿Qué tal?");
        assert_eq!(h.generation(), 1);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
