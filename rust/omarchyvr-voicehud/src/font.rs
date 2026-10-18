//! Fonts: loading the first existing candidate, measuring text, and rasterising a line with `ab_glyph` into a
//! coverage mask (with an optional slant for the partial text).

use crate::display::Measure;
use ab_glyph::{point, Font as _, FontVec, GlyphId, OutlinedGlyph, PxScale, ScaleFont as _};
use std::path::{Path, PathBuf};

/// Candidates after `--font`, in order: the Arch paths, then the Debian/Ubuntu ones (`/usr/share/fonts/truetype`).
pub const DEFAULT_FONTS: [&str; 7] = [
    "/usr/share/fonts/noto/NotoSans-Medium.ttf",
    "/usr/share/fonts/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/liberation/LiberationSans-Regular.ttf",
    "/usr/share/fonts/TTF/JetBrainsMonoNerdFont-Regular.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Medium.ttf",
    "/usr/share/fonts/truetype/noto/NotoSans-Regular.ttf",
    "/usr/share/fonts/truetype/liberation/LiberationSans-Regular.ttf",
];

/// A loaded font.
pub struct Font {
    font: FontVec,
    path: PathBuf,
}

/// Coverage (0..1) of a rendered line, in canvas coordinates: `data[(y - y0) * w + (x - x0)]`.
#[derive(Clone, Debug, Default)]
pub struct Mask {
    pub x0: i32,
    pub y0: i32,
    pub w: usize,
    pub h: usize,
    pub data: Vec<f32>,
}

impl Font {
    pub fn from_bytes(bytes: Vec<u8>, path: PathBuf) -> Result<Font, String> {
        let font = FontVec::try_from_vec(bytes)
            .map_err(|e| format!("{}: not a usable font ({e})", path.display()))?;
        Ok(Font { font, path })
    }

    /// The first usable file of `explicit` (`--font`) followed by `defaults`. Missing files are skipped; a missing
    /// or broken `--font` is noted in `notes` (the caller logs it).
    pub fn load_first(
        explicit: Option<&Path>,
        defaults: &[&str],
        notes: &mut Vec<String>,
    ) -> Result<Font, String> {
        let mut tried: Vec<PathBuf> = Vec::new();
        let candidates = explicit
            .map(|p| (p.to_path_buf(), true))
            .into_iter()
            .chain(defaults.iter().map(|d| (PathBuf::from(d), false)));
        for (path, is_explicit) in candidates {
            tried.push(path.clone());
            match std::fs::read(&path) {
                Ok(bytes) => match Font::from_bytes(bytes, path.clone()) {
                    Ok(f) => return Ok(f),
                    Err(e) => notes.push(e),
                },
                Err(e) => {
                    if is_explicit {
                        notes.push(format!("--font {}: {e}", path.display()));
                    }
                }
            }
        }
        Err(format!(
            "no usable font found (tried {}); pass --font PATH to a .ttf/.otf file",
            tried
                .iter()
                .map(|p| p.display().to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Glyph index of `c` (0 is `.notdef`, the missing-glyph box).
    pub fn glyph_id(&self, c: char) -> u16 {
        self.font.glyph_id(c).0
    }

    pub fn has_glyph(&self, c: char) -> bool {
        self.glyph_id(c) != 0
    }

    /// The character that is drawn for `c`: itself, or `?` when the font has no glyph for it (instead of the
    /// `.notdef` box).
    pub fn map_char(&self, c: char) -> char {
        if self.has_glyph(c) {
            c
        } else {
            '?'
        }
    }

    /// ab_glyph's scale is the height of ascent + descent; this gives the scale for an em size in pixels.
    pub fn px_scale(&self, em: f32) -> PxScale {
        let height = self.font.height_unscaled();
        let upm = self.font.units_per_em().unwrap_or(height);
        PxScale::from(em * height / upm)
    }

    /// Advance width of `text` in pixels at `em`.
    pub fn width(&self, text: &str, em: f32) -> f32 {
        let sf = self.font.as_scaled(self.px_scale(em));
        let mut x = 0.0;
        let mut prev: Option<GlyphId> = None;
        for ch in text.chars() {
            let id = sf.glyph_id(self.map_char(ch));
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
        x
    }

    /// How far the ink of ordinary text reaches above and below the baseline (ascender of `d`, descender of `p`),
    /// in pixels at `em`. Used to centre a line in its box.
    pub fn ink_extent(&self, em: f32) -> (f32, f32) {
        let scale = self.px_scale(em);
        let bounds = |c: char| {
            self.font
                .outline_glyph(
                    self.font
                        .glyph_id(c)
                        .with_scale_and_position(scale, point(0.0, 0.0)),
                )
                .map(|o| o.px_bounds())
        };
        let up = bounds('d').map_or(0.74 * em, |b| -b.min.y);
        let down = bounds('p').map_or(0.21 * em, |b| b.max.y);
        (up, down)
    }

    /// Rasterise `text` with the pen starting at (`pen_x`, `baseline`). `shear` slants the glyphs (0.2 is about 11
    /// degrees, to the right above the baseline), done per pixel row with sub-pixel shifts.
    pub fn render_line(&self, text: &str, em: f32, pen_x: f32, baseline: f32, shear: f32) -> Mask {
        let scale = self.px_scale(em);
        let sf = self.font.as_scaled(scale);
        let mut glyphs: Vec<OutlinedGlyph> = Vec::new();
        let mut x = pen_x;
        let mut prev: Option<GlyphId> = None;
        for ch in text.chars() {
            let id = sf.glyph_id(self.map_char(ch));
            if let Some(p) = prev {
                x += sf.kern(p, id);
            }
            if let Some(o) = self
                .font
                .outline_glyph(id.with_scale_and_position(scale, point(x, baseline)))
            {
                glyphs.push(o);
            }
            x += sf.h_advance(id);
            prev = Some(id);
        }
        if glyphs.is_empty() {
            return Mask::default();
        }
        // bounds of all glyphs, widened by the slant
        let (mut min_x, mut min_y, mut max_x, mut max_y) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for g in &glyphs {
            let b = g.px_bounds();
            min_x = min_x.min(b.min.x);
            min_y = min_y.min(b.min.y);
            max_x = max_x.max(b.max.x);
            max_y = max_y.max(b.max.y);
        }
        let slant = shear.abs() * em * 1.3;
        let x0 = (min_x - slant).floor() as i32 - 1;
        let y0 = min_y.floor() as i32;
        let x1 = (max_x + slant).ceil() as i32 + 2;
        let y1 = max_y.ceil() as i32 + 1;
        let (w, h) = ((x1 - x0).max(0) as usize, (y1 - y0).max(0) as usize);
        let mut data = vec![0.0f32; w * h];
        for g in &glyphs {
            let b = g.px_bounds();
            let (bx, by) = (b.min.x as i32, b.min.y as i32);
            g.draw(|gx, gy, cov| {
                if cov <= 0.0 {
                    return;
                }
                let px = bx + gx as i32;
                let py = by + gy as i32;
                let row = py - y0;
                if row < 0 || row as usize >= h {
                    return;
                }
                // pixel rows above the baseline move right by `shear` per pixel of height
                let shift = (baseline - (py as f32 + 0.5)) * shear;
                let xf = px as f32 + shift;
                let xi = xf.floor();
                let frac = xf - xi;
                let col = xi as i32 - x0;
                let base = row as usize * w;
                if col >= 0 && (col as usize) < w {
                    data[base + col as usize] += cov * (1.0 - frac);
                }
                if frac > 0.0 && col + 1 >= 0 && ((col + 1) as usize) < w {
                    data[base + (col + 1) as usize] += cov * frac;
                }
            });
        }
        Mask { x0, y0, w, h, data }
    }
}

/// [`Measure`] for a font at an em size.
pub struct FontMeasure<'a> {
    pub font: &'a Font,
    pub em: f32,
}

impl Measure for FontMeasure<'_> {
    fn width(&self, text: &str) -> f32 {
        self.font.width(text, self.em)
    }
}

#[cfg(test)]
pub(crate) mod testfont {
    use super::*;
    use std::sync::OnceLock;

    /// The real font the tests render with: the same search as the program (no `--font`).
    pub fn font() -> &'static Font {
        static F: OnceLock<Font> = OnceLock::new();
        F.get_or_init(|| {
            let mut notes = Vec::new();
            Font::load_first(None, &DEFAULT_FONTS, &mut notes)
                .unwrap_or_else(|e| panic!("the tests need one of the default fonts: {e}"))
        })
    }
}

#[cfg(test)]
mod tests {
    use super::testfont::font;
    use super::*;
    use crate::display::wrap_text;

    const SPANISH: &str = "áéíóúñüÁÉÍÓÚÑÜ¿¡";

    #[test]
    fn spanish_characters_have_real_glyphs() {
        let f = font();
        for c in SPANISH.chars() {
            assert_ne!(f.glyph_id(c), 0, "{c:?} would render as the .notdef box");
            assert_eq!(f.map_char(c), c);
        }
        // plain ASCII, digits and the usual punctuation, and the ellipsis / quotes dictation may produce
        for c in "Hello, World! 0123456789 .?:;-'\"()…“”".chars() {
            assert!(f.has_glyph(c), "{c:?}");
        }
        // a character outside the font is replaced by '?', never drawn as the .notdef box
        assert_eq!(f.glyph_id('\u{10FFFF}'), 0);
        assert_eq!(f.map_char('\u{10FFFF}'), '?');
        assert_eq!(f.width("\u{10FFFF}", 40.0), f.width("?", 40.0));
    }

    #[test]
    fn the_default_candidates_include_the_requested_fonts() {
        assert_eq!(
            DEFAULT_FONTS[0],
            "/usr/share/fonts/noto/NotoSans-Medium.ttf"
        );
        assert_eq!(DEFAULT_FONTS.len(), 7);
        // on this machine the first one exists, so it is the one in use
        if Path::new(DEFAULT_FONTS[0]).exists() {
            assert_eq!(font().path(), Path::new(DEFAULT_FONTS[0]));
        }
    }

    #[test]
    fn font_search_order_and_notes() {
        let mut notes = Vec::new();
        // a missing explicit font is noted and the next candidate is used
        let f = Font::load_first(
            Some(Path::new("/nonexistent/none.ttf")),
            &DEFAULT_FONTS,
            &mut notes,
        )
        .unwrap();
        assert!(f.has_glyph('a'));
        assert_eq!(notes.len(), 1);
        assert!(notes[0].contains("/nonexistent/none.ttf"), "{notes:?}");
        // an explicit existing font wins
        let mut notes = Vec::new();
        let lib = "/usr/share/fonts/liberation/LiberationSans-Regular.ttf";
        if Path::new(lib).exists() {
            let f = Font::load_first(Some(Path::new(lib)), &DEFAULT_FONTS, &mut notes).unwrap();
            assert_eq!(f.path(), Path::new(lib));
            assert!(notes.is_empty());
        }
        // a file that is not a font is noted and skipped
        let mut notes = Vec::new();
        let dir = std::env::temp_dir().join(format!("omarchyvr-voicehud-font-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.ttf");
        std::fs::write(&junk, b"this is not a font").unwrap();
        let f = Font::load_first(Some(&junk), &DEFAULT_FONTS, &mut notes).unwrap();
        assert!(f.has_glyph('a') && notes.len() == 1, "{notes:?}");
        // nothing usable: the error lists what was tried
        let mut notes = Vec::new();
        let e = Font::load_first(Some(&junk), &["/nonexistent/a.ttf"], &mut notes)
            .err()
            .unwrap();
        assert!(e.contains("junk.ttf") && e.contains("/nonexistent/a.ttf") && e.contains("--font"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn widths_scale_with_size_and_make_sense() {
        let f = font();
        assert_eq!(f.width("", 64.0), 0.0);
        let w = f.width("Hello", 64.0);
        assert!(w > 100.0 && w < 300.0, "{w}");
        assert!((f.width("Hello", 128.0) / w - 2.0).abs() < 0.01);
        assert!(f.width("iiii", 64.0) < f.width("MMMM", 64.0));
        // the em size is the em: a typical lower-case letter is about half an em wide
        let avg = f.width("abcdefghijklmnopqrstuvwxyz", 64.0) / 26.0;
        assert!(avg > 0.45 * 64.0 && avg < 0.65 * 64.0, "{avg}");
    }

    #[test]
    fn ink_extent_is_the_ascender_and_descender() {
        let (up, down) = font().ink_extent(64.0);
        assert!(up > 0.65 * 64.0 && up < 0.95 * 64.0, "{up}");
        assert!(down > 0.1 * 64.0 && down < 0.3 * 64.0, "{down}");
    }

    #[test]
    fn wrapping_by_pixel_width_with_the_real_font() {
        let f = font();
        let em = 48.0;
        let m = FontMeasure { font: f, em };
        let text = "Move the terminal to the left panel and recenter the view, then launch the browser please";
        for max_w in [300.0, 600.0, 900.0, 1400.0] {
            let lines = wrap_text(text, max_w, &m);
            assert!(!lines.is_empty());
            for l in &lines {
                assert!(
                    m.width(l) <= max_w + 0.01,
                    "{l:?} is {} > {max_w}",
                    m.width(l)
                );
            }
            // greedy: the first word of the next line would not have fitted on the previous one
            for pair in lines.windows(2) {
                let next_first = pair[1].split(' ').next().unwrap();
                let joined = format!("{} {}", pair[0], next_first);
                assert!(
                    m.width(&joined) > max_w || m.width(next_first) > max_w,
                    "{pair:?}"
                );
            }
            assert_eq!(lines.join(" "), text);
        }
        // narrower text needs more lines
        assert!(wrap_text(text, 300.0, &m).len() > wrap_text(text, 1400.0, &m).len());
        // Spanish text wraps the same way and keeps its accents
        let es =
            "¿Podrías abrir el navegador y buscar el pronóstico del tiempo para mañana en Bogotá?";
        let lines = wrap_text(es, 700.0, &m);
        assert!(lines.len() >= 2);
        assert_eq!(lines.join(" "), es);
    }

    fn ink(mask: &Mask) -> f32 {
        mask.data.iter().sum()
    }

    #[test]
    fn render_line_draws_ink_where_the_text_is() {
        let f = font();
        let m = f.render_line("Hello", 64.0, 100.0, 200.0, 0.0);
        assert!(!m.data.is_empty() && ink(&m) > 100.0);
        assert!(m.data.iter().all(|&c| (0.0..=1.001).contains(&c)));
        // the ink lies right of the pen start and around the baseline (cap height above, nothing far below)
        assert!(m.x0 >= 90 && m.x0 <= 105, "{}", m.x0);
        assert!((m.y0 as f32) < 200.0 - 0.6 * 64.0 && (m.y0 as f32) > 200.0 - 1.1 * 64.0);
        assert!(((m.y0 + m.h as i32) as f32) < 200.0 + 0.1 * 64.0);
        // the width is about the advance width
        let adv = f.width("Hello", 64.0);
        assert!((m.w as f32 - adv).abs() < 0.2 * adv, "{} vs {adv}", m.w);
        // an empty or blank string has no ink
        assert!(f.render_line("", 64.0, 0.0, 50.0, 0.0).data.is_empty());
        assert_eq!(ink(&f.render_line("   ", 64.0, 0.0, 50.0, 0.0)), 0.0);
    }

    #[test]
    fn accented_letters_draw_more_ink_than_their_bases() {
        let f = font();
        for (accented, plain) in [("á", "a"), ("ñ", "n"), ("ü", "u"), ("É", "E"), ("¿", "?")] {
            let a = ink(&f.render_line(accented, 64.0, 10.0, 100.0, 0.0));
            let p = ink(&f.render_line(plain, 64.0, 10.0, 100.0, 0.0));
            assert!(a > 1.0, "{accented}");
            if accented != "¿" {
                assert!(a > p * 1.02, "{accented}: {a} vs {p}");
            }
        }
    }

    #[test]
    fn slant_moves_the_top_right_and_keeps_the_ink() {
        let f = font();
        let up = f.render_line("l", 64.0, 100.0, 200.0, 0.0);
        let sl = f.render_line("l", 64.0, 100.0, 200.0, 0.2);
        // total ink is conserved by the sub-pixel shifts (within rounding)
        assert!(
            (ink(&up) - ink(&sl)).abs() < 0.02 * ink(&up),
            "{} {}",
            ink(&up),
            ink(&sl)
        );
        // centre of mass of the top quarter vs the bottom quarter
        let centre = |m: &Mask, rows: std::ops::Range<usize>| {
            let (mut s, mut n) = (0.0f32, 0.0f32);
            for r in rows {
                for c in 0..m.w {
                    let v = m.data[r * m.w + c];
                    s += v * (m.x0 + c as i32) as f32;
                    n += v;
                }
            }
            s / n
        };
        let h = sl.h;
        let top = centre(&sl, 0..h / 4);
        let bottom = centre(&sl, 3 * h / 4..h);
        // 0.75 of the 49 px glyph height between the two centres, times the 0.2 slant: about 7.4 px
        assert!(
            top > bottom + 6.0 && top < bottom + 9.0,
            "top {top} bottom {bottom}"
        );
        let h = up.h;
        let top = centre(&up, 0..h / 4);
        let bottom = centre(&up, 3 * h / 4..h);
        assert!(
            (top - bottom).abs() < 1.0,
            "upright: top {top} bottom {bottom}"
        );
    }
}
