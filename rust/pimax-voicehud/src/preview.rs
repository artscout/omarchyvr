//! `preview OUT.png`: the texture exactly as it is uploaded (RGBA, straight alpha, sRGB-coded bytes, 1600 x 400),
//! rendered through the same display model and renderer as the running overlay. No headset needed.

use crate::cli::PreviewOpts;
use crate::color::{linear_to_srgb, srgb_to_linear};
use crate::display::{Config, Display, View};
use crate::font::{Font, FontMeasure, DEFAULT_FONTS};
use crate::hud::clamp_lines;
use crate::log;
use crate::placement::{TEX_H, TEX_W};
use crate::png;
use crate::protocol::{Event, State};
use crate::render::{self, Layout, Style};
use std::time::Instant;

/// The display state the options describe, built from the same events the server would send.
pub fn build_display(opts: &PreviewOpts, max_lines: usize, now: Instant) -> Display {
    let mut d = Display::new(Config {
        keep: opts.keep,
        max_lines,
        timeout: None,
        idle_dot: opts.idle_dot,
    });
    let mut texts: Vec<String> = opts.texts.clone();
    let mut partial = opts.partial.clone();
    if texts.is_empty() && partial.is_none() && opts.status.is_none() && !opts.speech {
        // nothing asked for: a sample
        texts.push("Open the browser.".into());
        partial = Some("and then go to the sett".into());
    }
    let mut seg = 0u64;
    for t in texts {
        seg += 1;
        d.on_event(
            &Event::Final {
                seg,
                text: t,
                lang: None,
                t0: 0.0,
                t1: 0.0,
                latency_ms: 0,
                decode_ms: 0,
            },
            now,
        );
    }
    if let Some(p) = partial {
        seg += 1;
        d.on_event(
            &Event::Partial {
                seg,
                text: p,
                t: 0.0,
            },
            now,
        );
    }
    if let Some((state, detail)) = &opts.status {
        d.on_event(
            &Event::Status {
                state: state.clone(),
                detail: detail.clone(),
            },
            now,
        );
    } else if opts.status.is_none() && (opts.speech || opts.level_db.is_some()) {
        d.on_event(
            &Event::Status {
                state: State::Listening,
                detail: String::new(),
            },
            now,
        );
    }
    if opts.speech || opts.level_db.is_some() {
        d.on_event(
            &Event::Level {
                db: opts.level_db.unwrap_or(-30.0),
                speech: opts.speech,
            },
            now,
        );
    }
    d
}

/// Composite straight-alpha RGBA over an opaque colour, in linear light as the compositor does for sRGB textures.
pub fn flatten(rgba: &[u8], backdrop: [u8; 3]) -> Vec<u8> {
    let back: Vec<f32> = backdrop
        .iter()
        .map(|&c| srgb_to_linear(f32::from(c) / 255.0))
        .collect();
    let mut out = Vec::with_capacity(rgba.len());
    for p in rgba.as_chunks::<4>().0 {
        let a = f32::from(p[3]) / 255.0;
        for c in 0..3 {
            let src = srgb_to_linear(f32::from(p[c]) / 255.0);
            let v = linear_to_srgb(src * a + back[c] * (1.0 - a));
            out.push((v * 255.0 + 0.5).clamp(0.0, 255.0) as u8);
        }
        out.push(255);
    }
    out
}

/// Everything `preview` computes: the view, the texture and where the box is.
pub struct Preview {
    pub view: View,
    pub rgba: Vec<u8>,
    pub layout: Layout,
    pub font_path: String,
}

pub fn render_preview(opts: &PreviewOpts, font: Font) -> Preview {
    let (max_lines, _) = clamp_lines(opts.lines, opts.font_px);
    let style = Style::new(opts.font_px, max_lines);
    let d = build_display(opts, max_lines, Instant::now());
    let measure = FontMeasure {
        font: &font,
        em: style.em,
    };
    let view = d.view(&measure, style.text_max_width());
    let rgba = render::render(&view, &font, &style);
    let layout = Layout::new(&view, &font, &style);
    Preview {
        view,
        rgba,
        layout,
        font_path: font.path().display().to_string(),
    }
}

/// Write the PNG(s); returns the summary line to print.
pub fn run(opts: &PreviewOpts) -> Result<String, String> {
    let mut notes = Vec::new();
    let font = Font::load_first(opts.font.as_deref(), &DEFAULT_FONTS, &mut notes)?;
    for n in &notes {
        log::warn(n);
    }
    if let (_, Some(w)) = clamp_lines(opts.lines, opts.font_px) {
        log::warn(w);
    }
    let p = render_preview(opts, font);
    let out = match opts.backdrop {
        Some(b) => flatten(&p.rgba, b),
        None => p.rgba.clone(),
    };
    png::write_rgba(&opts.out, TEX_W, TEX_H, &out)
        .map_err(|e| format!("{}: {e}", opts.out.display()))?;
    let b = p.layout.box_rect;
    Ok(format!(
        "{}: {TEX_W}x{TEX_H} RGBA {}, {} line(s), box {}x{} at ({}, {}), font {}",
        opts.out.display(),
        if opts.backdrop.is_some() {
            "flattened over the backdrop"
        } else {
            "straight alpha, sRGB bytes as uploaded"
        },
        p.view.lines.len(),
        b.w(),
        b.h(),
        b.x0,
        b.y0,
        p.font_path
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{self, Command};
    use crate::display::{Dot, Kind};
    use std::path::PathBuf;

    fn opts(args: &[&str]) -> PreviewOpts {
        let mut v = vec!["preview".to_string(), "/tmp/unused.png".to_string()];
        v.extend(args.iter().map(|s| (*s).to_string()));
        match cli::parse(&v).unwrap() {
            Command::Preview(o) => o,
            other => panic!("{other:?}"),
        }
    }

    fn font() -> Font {
        let mut notes = Vec::new();
        Font::load_first(None, &DEFAULT_FONTS, &mut notes).unwrap()
    }

    #[test]
    fn default_preview_shows_a_final_and_a_partial() {
        let p = render_preview(&opts(&[]), font());
        assert!(p.view.visible);
        let kinds: Vec<Kind> = p.view.lines.iter().map(|l| l.kind).collect();
        assert_eq!(kinds, [Kind::Final, Kind::Partial]);
        assert_eq!(p.view.lines[0].text, "Open the browser.");
        assert_eq!(p.rgba.len(), (TEX_W * TEX_H * 4) as usize);
    }

    #[test]
    fn text_partial_status_and_level_options() {
        let p = render_preview(
            &opts(&[
                "--text",
                "one",
                "--text",
                "two",
                "--text",
                "three",
                "--partial",
                "four",
            ]),
            font(),
        );
        // keep = 2 finals + the partial
        let t: Vec<&str> = p.view.lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(t, ["two", "three", "four"]);
        let p = render_preview(
            &opts(&["--text", "hi", "--speech", "--level", "-30"]),
            font(),
        );
        assert_eq!((p.view.dot, p.view.level), (Dot::Speech, 10));
        let p = render_preview(
            &opts(&["--status", "no_device", "--detail", "mic gone"]),
            font(),
        );
        assert_eq!(p.view.dot, Dot::Error);
        assert_eq!(p.view.lines[0].text, "mic gone");
        // speech only: the pill with the dot and no text
        let p = render_preview(&opts(&["--speech", "--level", "-20"]), font());
        assert!(p.view.visible && p.view.lines.is_empty());
        // --level without --speech has no visible fill
        let p = render_preview(&opts(&["--text", "x", "--level", "-20"]), font());
        assert_eq!(p.view.level, 0);
    }

    #[test]
    fn flatten_blends_in_linear_light() {
        let px = [255u8, 255, 255, 128, 0, 0, 0, 0, 10, 20, 30, 255];
        let f = flatten(&px, [0, 0, 0]);
        assert_eq!(f[3], 255);
        // half-transparent white over black is 50 % linear light = code 188
        assert!((i32::from(f[0]) - 188).abs() <= 1, "{}", f[0]);
        // fully transparent shows the backdrop, fully opaque shows the pixel
        assert_eq!(&f[4..8], &[0, 0, 0, 255]);
        assert_eq!(&f[8..12], &[10, 20, 30, 255]);
        let g = flatten(&px[4..8], [0x40, 0x50, 0x60]);
        assert_eq!(g, [0x40, 0x50, 0x60, 255]);
    }

    #[test]
    fn run_writes_a_decodable_png() {
        let dir = std::env::temp_dir().join(format!("pimax-voicehud-prev-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let out: PathBuf = dir.join("o.png");
        let mut o = opts(&["--text", "¿Qué tal, señor?"]);
        o.out = out.clone();
        let msg = run(&o).unwrap();
        assert!(
            msg.contains("1600x400") && msg.contains("straight alpha"),
            "{msg}"
        );
        let d = png::decode_rgba(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((d.width, d.height), (TEX_W, TEX_H));
        assert_eq!(
            d.rgba,
            render_preview(&o, font()).rgba,
            "the PNG is the uploaded texture"
        );
        // with a backdrop the image is opaque
        o.backdrop = Some([0x30, 0x30, 0x38]);
        run(&o).unwrap();
        let d = png::decode_rgba(&std::fs::read(&out).unwrap()).unwrap();
        assert!(d.rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255));
        // a bad output path is an error, not a panic
        o.out = dir.join("missing-dir/x.png");
        assert!(run(&o).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
