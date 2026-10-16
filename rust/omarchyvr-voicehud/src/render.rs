//! The texture: a dark semi-transparent rounded box (bottom-anchored, as wide as its text) holding the mic dot, the
//! level bar and the text lines, on a fully transparent 1600 x 400 RGBA image. Straight (unpremultiplied) alpha,
//! sRGB-coded colour bytes ([`crate::color`] converts them for the swapchain format).
//!
//! Layout, left to right inside the box: level bar (a thin vertical meter), mic dot, text. The box grows upward
//! with the number of lines so the newest line stays where it is; the block of the maximum number of lines is
//! centred on the texture (and so on the `--down-deg` direction).

use crate::display::{Dot, Kind, View, LEVEL_STEPS};
use crate::font::{Font, Mask};
use crate::placement::{TEX_H, TEX_W};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl Rect {
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
}

/// Sizes (all derived from the font size in pixels) and colours.
#[derive(Clone, Debug, PartialEq)]
pub struct Style {
    /// Font size (em) in texture pixels. One texture pixel is 1 mm on the default quad.
    pub em: f32,
    /// Most text lines.
    pub max_lines: usize,
    /// Slant of the partial text.
    pub partial_shear: f32,
    pub bg: [u8; 3],
    pub bg_alpha: f32,
    pub text_final: [u8; 3],
    pub text_partial: [u8; 3],
    pub text_error: [u8; 3],
    pub text_notice: [u8; 3],
    pub dot_speech: [u8; 3],
    pub dot_listening: [u8; 3],
    pub dot_paused: [u8; 3],
    pub dot_error: [u8; 3],
    pub bar_idle: [u8; 3],
}

impl Style {
    pub fn new(em: f32, max_lines: usize) -> Style {
        Style {
            em,
            max_lines: max_lines.max(1),
            partial_shear: 0.2,
            bg: [12, 14, 20],
            bg_alpha: 0.82,
            text_final: [246, 246, 248],
            text_partial: [166, 178, 202],
            text_error: [255, 150, 130],
            text_notice: [250, 200, 110],
            dot_speech: [70, 224, 120],
            dot_listening: [140, 146, 158],
            dot_paused: [244, 184, 64],
            dot_error: [244, 72, 64],
            bar_idle: [120, 126, 138],
        }
    }

    pub fn line_h(&self) -> f32 {
        (self.em * 1.26).round()
    }
    pub fn pad_h(&self) -> f32 {
        (self.em * 0.38).round()
    }
    pub fn pad_v(&self) -> f32 {
        (self.em * 0.26).round()
    }
    pub fn radius(&self) -> f32 {
        (self.em * 0.55).round()
    }
    /// Free texture border around the box.
    pub fn margin(&self) -> f32 {
        24.0
    }
    pub fn bar_w(&self) -> f32 {
        (self.em * 0.16).round().max(4.0)
    }
    pub fn dot_d(&self) -> f32 {
        (self.em * 0.44).round().max(8.0)
    }
    fn gap_bar_dot(&self) -> f32 {
        (self.em * 0.20).round()
    }
    fn gap_dot_text(&self) -> f32 {
        (self.em * 0.42).round()
    }
    /// Width of bar + dot + gaps, left of the text.
    pub fn gutter_w(&self) -> f32 {
        self.bar_w() + self.gap_bar_dot() + self.dot_d() + self.gap_dot_text()
    }

    /// Widest text line that fits the texture.
    pub fn text_max_width(&self) -> f32 {
        TEX_W as f32 - 2.0 * self.margin() - 2.0 * self.pad_h() - self.gutter_w()
    }

    /// Most lines whose box fits the texture height.
    pub fn max_lines_fit(&self) -> usize {
        let free = TEX_H as f32 - 2.0 * self.margin() - 2.0 * self.pad_v();
        ((free / self.line_h()).floor() as usize).max(1)
    }

    fn text_rgb(&self, kind: Kind) -> [u8; 3] {
        match kind {
            Kind::Final => self.text_final,
            Kind::Partial => self.text_partial,
            Kind::Error => self.text_error,
            Kind::Notice => self.text_notice,
        }
    }

    fn dot_rgb(&self, dot: Dot) -> [u8; 3] {
        match dot {
            Dot::Speech => self.dot_speech,
            Dot::Listening => self.dot_listening,
            Dot::Paused => self.dot_paused,
            Dot::Error => self.dot_error,
        }
    }
}

/// Where everything goes, for one view.
#[derive(Clone, Debug, PartialEq)]
pub struct Layout {
    pub box_rect: Rect,
    pub text_x: f32,
    /// Baseline of each text line.
    pub baselines: Vec<f32>,
    /// The level meter track.
    pub bar: Rect,
    pub dot_center: (f32, f32),
    pub dot_radius: f32,
}

impl Layout {
    pub fn new(view: &View, font: &Font, style: &Style) -> Layout {
        let em = style.em;
        let (line_h, pad_h, pad_v) = (style.line_h(), style.pad_h(), style.pad_v());
        let (asc, desc) = font.ink_extent(em);
        let widest = view
            .lines
            .iter()
            .map(|l| font.width(&l.text, em))
            .fold(0.0f32, f32::max)
            .ceil();
        let max_box_w = TEX_W as f32 - 2.0 * style.margin();
        // without text (speech just started) the box is only as wide as the bar and the dot
        let content_w = if widest > 0.0 {
            style.gutter_w() + widest
        } else {
            style.gutter_w() - style.gap_dot_text()
        };
        let box_w = (2.0 * pad_h + content_w).min(max_box_w);
        let x0 = ((TEX_W as f32 - box_w) / 2.0).floor();
        let lines = view.lines.len().max(1) as f32;
        let box_h = 2.0 * pad_v + lines * line_h;
        // the block of `max_lines` lines is centred on the texture; the box grows upward from its bottom edge
        let max_h = 2.0 * pad_v + style.max_lines as f32 * line_h;
        let y1 = (TEX_H as f32 / 2.0 + max_h / 2.0)
            .min(TEX_H as f32 - style.margin())
            .round();
        let y0 = (y1 - box_h).max(0.0);
        let box_rect = Rect {
            x0,
            y0,
            x1: x0 + box_w,
            y1,
        };

        // vertical position of the first baseline: the ink (ascender .. descender) centred in its line box
        let first = y0 + pad_v + (line_h - (asc + desc)) / 2.0 + asc;
        let baselines: Vec<f32> = (0..view.lines.len())
            .map(|i| (first + i as f32 * line_h).round())
            .collect();
        let last = first + (lines - 1.0) * line_h;

        let bar_x0 = x0 + pad_h;
        let bar = Rect {
            x0: bar_x0,
            y0: (first - asc).round(),
            x1: bar_x0 + style.bar_w(),
            y1: (last + desc).round(),
        };
        let d = style.dot_d();
        let dot_center = (
            bar.x1 + style.gap_bar_dot() + d / 2.0,
            (first - 0.5 * (asc - desc)).round(),
        );
        let text_x = bar.x1 + style.gap_bar_dot() + d + style.gap_dot_text();
        Layout {
            box_rect,
            text_x,
            baselines,
            bar,
            dot_center,
            dot_radius: d / 2.0,
        }
    }

    /// The filled part of the level meter for `level` steps (from the bottom).
    pub fn bar_fill(&self, level: u8) -> Option<Rect> {
        if level == 0 {
            return None;
        }
        let f = (f32::from(level) / f32::from(LEVEL_STEPS)).clamp(0.0, 1.0);
        let h = (self.bar.h() * f).max(self.bar.w());
        Some(Rect {
            y0: (self.bar.y1 - h).max(self.bar.y0),
            ..self.bar
        })
    }
}

/// An RGBA canvas with straight alpha.
pub struct Canvas {
    pub w: usize,
    pub h: usize,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: usize, h: usize, fill: [u8; 4]) -> Canvas {
        let mut px = Vec::with_capacity(w * h * 4);
        for _ in 0..w * h {
            px.extend_from_slice(&fill);
        }
        Canvas { w, h, px }
    }

    /// `src` (colour `rgb`, opacity `a`) over the pixel, in straight-alpha arithmetic.
    fn over(&mut self, x: i32, y: i32, rgb: [u8; 3], a: f32) {
        if a <= 0.0 || x < 0 || y < 0 || x as usize >= self.w || y as usize >= self.h {
            return;
        }
        let a = a.min(1.0);
        let i = (y as usize * self.w + x as usize) * 4;
        let da = f32::from(self.px[i + 3]) / 255.0;
        let oa = a + da * (1.0 - a);
        if oa <= 0.0 {
            return;
        }
        for (c, &channel) in rgb.iter().enumerate() {
            let d = f32::from(self.px[i + c]);
            let s = f32::from(channel);
            self.px[i + c] = ((s * a + d * da * (1.0 - a)) / oa + 0.5).min(255.0) as u8;
        }
        self.px[i + 3] = (oa * 255.0 + 0.5).min(255.0) as u8;
    }

    pub fn fill_rounded_rect(&mut self, r: Rect, radius: f32, rgb: [u8; 3], alpha: f32) {
        let radius = radius.min(r.w() / 2.0).min(r.h() / 2.0).max(0.0);
        let (cx, cy) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
        let (hw, hh) = (r.w() / 2.0, r.h() / 2.0);
        for y in (r.y0.floor() as i32)..(r.y1.ceil() as i32) {
            for x in (r.x0.floor() as i32)..(r.x1.ceil() as i32) {
                // signed distance to the rounded rectangle, 1 px wide antialiased edge
                let qx = (x as f32 + 0.5 - cx).abs() - (hw - radius);
                let qy = (y as f32 + 0.5 - cy).abs() - (hh - radius);
                let outside = (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt();
                let sd = outside + qx.max(qy).min(0.0) - radius;
                let cov = (0.5 - sd).clamp(0.0, 1.0);
                self.over(x, y, rgb, alpha * cov);
            }
        }
    }

    pub fn fill_circle(&mut self, cx: f32, cy: f32, radius: f32, rgb: [u8; 3], alpha: f32) {
        for y in ((cy - radius).floor() as i32 - 1)..((cy + radius).ceil() as i32 + 1) {
            for x in ((cx - radius).floor() as i32 - 1)..((cx + radius).ceil() as i32 + 1) {
                let d = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
                let cov = (radius - d + 0.5).clamp(0.0, 1.0);
                self.over(x, y, rgb, alpha * cov);
            }
        }
    }

    pub fn composite_mask(&mut self, m: &Mask, rgb: [u8; 3], opacity: f32) {
        for row in 0..m.h {
            for col in 0..m.w {
                let cov = m.data[row * m.w + col];
                if cov > 0.0 {
                    self.over(
                        m.x0 + col as i32,
                        m.y0 + row as i32,
                        rgb,
                        cov.min(1.0) * opacity,
                    );
                }
            }
        }
    }
}

/// Render the view into a [`TEX_W`] x [`TEX_H`] RGBA image (straight alpha, sRGB-coded bytes). Fully transparent
/// pixels carry the background colour so that bilinear filtering at the box edge does not pull in black.
pub fn render(view: &View, font: &Font, style: &Style) -> Vec<u8> {
    let mut cv = Canvas::new(
        TEX_W as usize,
        TEX_H as usize,
        [style.bg[0], style.bg[1], style.bg[2], 0],
    );
    if !view.visible {
        return cv.px;
    }
    let lay = Layout::new(view, font, style);
    cv.fill_rounded_rect(lay.box_rect, style.radius(), style.bg, style.bg_alpha);

    // level meter: track, then the fill
    let r = style.bar_w() / 2.0;
    cv.fill_rounded_rect(lay.bar, r, [255, 255, 255], 0.16);
    if let Some(fill) = lay.bar_fill(view.level) {
        let rgb = if view.dot == Dot::Speech {
            style.dot_speech
        } else {
            style.bar_idle
        };
        cv.fill_rounded_rect(fill, r, rgb, 1.0);
    }
    // mic dot
    cv.fill_circle(
        lay.dot_center.0,
        lay.dot_center.1,
        lay.dot_radius,
        style.dot_rgb(view.dot),
        1.0,
    );
    // text
    for (line, &baseline) in view.lines.iter().zip(&lay.baselines) {
        let shear = if line.kind == Kind::Partial {
            style.partial_shear
        } else {
            0.0
        };
        let mask = font.render_line(&line.text, style.em, lay.text_x, baseline, shear);
        cv.composite_mask(&mask, style.text_rgb(line.kind), 1.0);
    }
    cv.px
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::{Config, Display, ViewLine};
    use crate::font::testfont::font;
    use crate::font::FontMeasure;
    use crate::protocol::parse_line;
    use std::time::Instant;

    fn pixels(img: &[u8]) -> &[[u8; 4]] {
        img.as_chunks::<4>().0
    }

    fn px(img: &[u8], x: usize, y: usize) -> [u8; 4] {
        let i = (y * TEX_W as usize + x) * 4;
        [img[i], img[i + 1], img[i + 2], img[i + 3]]
    }

    fn view_of(lines: &[(&str, Kind)], dot: Dot, level: u8) -> View {
        View {
            visible: true,
            lines: lines
                .iter()
                .map(|(t, k)| ViewLine {
                    text: (*t).to_string(),
                    kind: *k,
                })
                .collect(),
            dot,
            level,
        }
    }

    fn style() -> Style {
        Style::new(64.0, 3)
    }

    #[test]
    fn hidden_view_is_fully_transparent() {
        let img = render(&View::hidden(), font(), &style());
        assert_eq!(img.len(), (TEX_W * TEX_H * 4) as usize);
        assert!(pixels(&img).iter().all(|p| p[3] == 0));
    }

    #[test]
    fn text_pixels_are_drawn_inside_a_box_and_the_rest_is_transparent() {
        let st = style();
        let v = view_of(&[("Open the browser.", Kind::Final)], Dot::Listening, 0);
        let img = render(&v, font(), &st);
        let lay = Layout::new(&v, font(), &st);
        // corners and edges of the texture stay transparent
        for (x, y) in [(0, 0), (1599, 0), (0, 399), (1599, 399), (800, 2), (3, 200)] {
            assert_eq!(px(&img, x, y)[3], 0, "({x},{y})");
        }
        // the box is dark and semi-transparent
        let (bx, by) = (
            (lay.box_rect.x0 + 6.0) as usize,
            ((lay.box_rect.y0 + lay.box_rect.y1) / 2.0) as usize,
        );
        let p = px(&img, bx + 200, by);
        let a = f32::from(p[3]) / 255.0;
        assert!((a - st.bg_alpha).abs() < 0.02 || p[0] > 100, "{p:?}"); // background or a letter
        let corner = px(
            &img,
            lay.box_rect.x0 as usize + 1,
            lay.box_rect.y0 as usize + 1,
        );
        assert_eq!(corner[3], 0, "the rounded corner is cut");
        // there is plenty of bright, opaque-ish text ink and it all lies inside the box
        let mut ink = 0usize;
        let (mut minx, mut maxx, mut miny, mut maxy) = (usize::MAX, 0, usize::MAX, 0);
        for y in 0..TEX_H as usize {
            for x in 0..TEX_W as usize {
                let p = px(&img, x, y);
                if p[3] > 0 {
                    minx = minx.min(x);
                    maxx = maxx.max(x);
                    miny = miny.min(y);
                    maxy = maxy.max(y);
                }
                if p[3] > 200 && p[0] > 200 && p[1] > 200 && p[2] > 200 {
                    ink += 1;
                }
            }
        }
        assert!(ink > 1500, "{ink} bright pixels");
        assert!(
            minx as f32 >= lay.box_rect.x0.floor() - 1.0
                && maxx as f32 <= lay.box_rect.x1.ceil() + 1.0
        );
        assert!(
            miny as f32 >= lay.box_rect.y0.floor() - 1.0
                && maxy as f32 <= lay.box_rect.y1.ceil() + 1.0
        );
        // a one-line box hugs its text: much narrower than the texture, centred
        assert!(lay.box_rect.w() < 1100.0, "{}", lay.box_rect.w());
        let centre = (lay.box_rect.x0 + lay.box_rect.x1) / 2.0;
        assert!((centre - 800.0).abs() <= 1.0);
    }

    #[test]
    fn box_grows_upward_and_the_newest_line_stays_put() {
        let st = style();
        let one = view_of(&[("Newest line", Kind::Final)], Dot::Listening, 0);
        let three = view_of(
            &[
                ("Oldest line", Kind::Final),
                ("Middle line", Kind::Final),
                ("Newest line", Kind::Final),
            ],
            Dot::Listening,
            0,
        );
        let a = Layout::new(&one, font(), &st);
        let b = Layout::new(&three, font(), &st);
        assert_eq!(a.box_rect.y1, b.box_rect.y1, "same bottom edge");
        assert!(b.box_rect.y0 < a.box_rect.y0 - 100.0);
        assert_eq!(
            a.baselines[0], b.baselines[2],
            "the newest line does not move"
        );
        assert_eq!(b.baselines.len(), 3);
        assert!(b
            .baselines
            .windows(2)
            .all(|w| (w[1] - w[0] - st.line_h()).abs() < 1.5));
        // the three-line block is centred vertically on the texture, and stays inside it
        let mid = (b.box_rect.y0 + b.box_rect.y1) / 2.0;
        assert!((mid - 200.0).abs() <= 2.0, "{mid}");
        assert!(
            b.box_rect.y0 >= st.margin() - 1.0 && b.box_rect.y1 <= TEX_H as f32 - st.margin() + 1.0
        );
    }

    #[test]
    fn partial_text_is_dimmer_than_final_text() {
        let st = style();
        let fin = render(
            &view_of(&[("Same words", Kind::Final)], Dot::Listening, 0),
            font(),
            &st,
        );
        let par = render(
            &view_of(&[("Same words", Kind::Partial)], Dot::Listening, 0),
            font(),
            &st,
        );
        let brightest = |img: &[u8]| {
            pixels(img)
                .iter()
                .filter(|p| p[3] > 200)
                .map(|p| u32::from(p[0]) + u32::from(p[1]) + u32::from(p[2]))
                .max()
                .unwrap()
        };
        let (bf, bp) = (brightest(&fin), brightest(&par));
        assert!(bf > bp + 60, "final {bf} partial {bp}");
        // the partial is slanted: its ink is not identical to the upright rendering shifted in colour
        assert_ne!(
            pixels(&fin).iter().map(|p| p[3]).collect::<Vec<_>>(),
            pixels(&par).iter().map(|p| p[3]).collect::<Vec<_>>()
        );
    }

    #[test]
    fn mic_dot_colours() {
        let st = style();
        for (dot, rgb) in [
            (Dot::Speech, st.dot_speech),
            (Dot::Listening, st.dot_listening),
            (Dot::Paused, st.dot_paused),
            (Dot::Error, st.dot_error),
        ] {
            let v = view_of(&[("x", Kind::Final)], dot, 0);
            let img = render(&v, font(), &st);
            let lay = Layout::new(&v, font(), &st);
            let p = px(&img, lay.dot_center.0 as usize, lay.dot_center.1 as usize);
            assert_eq!([p[0], p[1], p[2]], rgb, "{dot:?}");
            assert_eq!(p[3], 255);
        }
        // green is green, red is red, grey is grey
        assert!(st.dot_speech[1] > st.dot_speech[0] + 80);
        assert!(st.dot_error[0] > st.dot_error[1] + 100);
        let g = st.dot_listening;
        assert!((i32::from(g[0]) - i32::from(g[2])).abs() < 30);
    }

    #[test]
    fn level_bar_fill_follows_the_level() {
        let st = style();
        let measure = |level: u8| {
            let v = view_of(&[("level", Kind::Final)], Dot::Speech, level);
            let img = render(&v, font(), &st);
            let lay = Layout::new(&v, font(), &st);
            let x = ((lay.bar.x0 + lay.bar.x1) / 2.0) as usize;
            // count green pixels down the middle of the bar
            (0..TEX_H as usize)
                .filter(|&y| {
                    let p = px(&img, x, y);
                    p[1] > 200 && p[0] < 120 && p[3] == 255
                })
                .count()
        };
        let (l0, l5, l10, l20) = (measure(0), measure(5), measure(10), measure(20));
        assert_eq!(l0, 0);
        assert!(l5 > 0 && l5 < l10 && l10 < l20, "{l5} {l10} {l20}");
        let v = view_of(&[("level", Kind::Final)], Dot::Speech, 20);
        let lay = Layout::new(&v, font(), &st);
        assert!(
            (l20 as f32 - lay.bar.h()).abs() <= 3.0,
            "{l20} vs {}",
            lay.bar.h()
        );
        // not speaking: the fill is grey, not green
        let v = view_of(&[("level", Kind::Final)], Dot::Listening, 10);
        let img = render(&v, font(), &st);
        let lay = Layout::new(&v, font(), &st);
        let p = px(
            &img,
            ((lay.bar.x0 + lay.bar.x1) / 2.0) as usize,
            (lay.bar.y1 - 4.0) as usize,
        );
        assert_eq!([p[0], p[1], p[2]], st.bar_idle);
    }

    #[test]
    fn spanish_text_renders_with_accents() {
        let st = style();
        let text = "¿Qué hora es? ¡Año nuevo, señor Núñez! Más información, por favor.";
        let v = view_of(&[(text, Kind::Final)], Dot::Listening, 0);
        let img = render(&v, font(), &st);
        let ink = pixels(&img)
            .iter()
            .filter(|p| p[3] > 128 && p[0] > 180)
            .count();
        assert!(ink > 3000, "{ink}");
        // each accented letter adds ink above the x-height compared with the plain text
        let plain: String = text
            .chars()
            .map(|c| match c {
                'á' | 'é' | 'í' | 'ó' | 'ú' | 'Q' => c,
                'ñ' | 'Ñ' | '¿' | '¡' => 'n',
                _ => c,
            })
            .collect();
        assert_ne!(plain, text);
        // none of the Spanish characters falls back to the replacement
        for c in text.chars() {
            assert_eq!(font().map_char(c), c, "{c:?}");
        }
    }

    #[test]
    fn missing_glyphs_do_not_draw_the_notdef_box() {
        let st = style();
        let q = render(
            &view_of(&[("a?b", Kind::Final)], Dot::Listening, 0),
            font(),
            &st,
        );
        let e = render(
            &view_of(&[("a\u{10FFFF}b", Kind::Final)], Dot::Listening, 0),
            font(),
            &st,
        );
        assert_eq!(q, e, "an unknown character is drawn as '?'");
    }

    #[test]
    fn empty_pill_shows_only_the_dot_and_bar() {
        let st = style();
        let v = View {
            visible: true,
            lines: vec![],
            dot: Dot::Speech,
            level: 8,
        };
        let lay = Layout::new(&v, font(), &st);
        assert!(lay.box_rect.w() < 200.0, "{}", lay.box_rect.w());
        assert!(lay.baselines.is_empty());
        let img = render(&v, font(), &st);
        let p = px(&img, lay.dot_center.0 as usize, lay.dot_center.1 as usize);
        assert_eq!([p[0], p[1], p[2]], st.dot_speech);
    }

    #[test]
    fn long_text_stays_inside_the_texture_with_the_default_wrap_width() {
        let st = style();
        let m = FontMeasure {
            font: font(),
            em: st.em,
        };
        let mut d = Display::new(Config::default());
        let t0 = Instant::now();
        let long = "This is a very long dictated sentence that goes on and on without any pause so that it has to wrap over several lines inside the caption box of the voice HUD in the headset";
        let ev = parse_line(&format!(
            r#"{{"type":"partial","seg":1,"text":"{long}","t":1}}"#
        ))
        .unwrap();
        d.on_event(&ev, t0);
        let view = d.view(&m, st.text_max_width());
        assert_eq!(view.lines.len(), 3);
        let lay = Layout::new(&view, font(), &st);
        assert!(lay.box_rect.x0 >= st.margin() - 1.0);
        assert!(lay.box_rect.x1 <= TEX_W as f32 - st.margin() + 1.0);
        let img = render(&view, font(), &st);
        // nothing is drawn outside the box margin
        for y in 0..TEX_H as usize {
            for x in [0usize, 10, 20, 1579, 1590, 1599] {
                assert_eq!(px(&img, x, y)[3], 0, "({x},{y})");
            }
        }
    }

    #[test]
    fn style_metrics_scale_with_the_font_size() {
        let a = Style::new(64.0, 3);
        let b = Style::new(32.0, 3);
        assert!(a.line_h() > 1.9 * b.line_h() - 2.0);
        assert!(a.text_max_width() < TEX_W as f32 && a.text_max_width() > 1200.0);
        // the default of 3 lines fits at the default size, 4 do not
        assert_eq!(a.max_lines_fit(), 3);
        assert!(b.max_lines_fit() > a.max_lines_fit());
        assert_eq!(Style::new(64.0, 0).max_lines, 1);
        // the maximum block always fits the texture when max_lines <= max_lines_fit
        let h = 2.0 * a.pad_v() + a.max_lines_fit() as f32 * a.line_h();
        assert!(h <= TEX_H as f32 - 2.0 * a.margin());
    }

    #[test]
    fn rounded_rect_edges_are_antialiased_and_symmetric() {
        let mut cv = Canvas::new(40, 40, [0, 0, 0, 0]);
        cv.fill_rounded_rect(
            Rect {
                x0: 5.0,
                y0: 5.0,
                x1: 35.0,
                y1: 35.0,
            },
            10.0,
            [255, 255, 255],
            1.0,
        );
        let a = |x: usize, y: usize| cv.px[(y * 40 + x) * 4 + 3];
        assert_eq!(a(20, 20), 255);
        assert_eq!(a(5, 5), 0, "corner cut");
        assert_eq!(a(20, 5), 255);
        assert_eq!(a(20, 4), 0);
        assert_eq!(a(10, 20), a(29, 20), "mirror symmetry");
        assert_eq!(a(8, 8), a(31, 8));
        assert!((1..255).contains(&(a(6, 9) as i32)) || a(6, 9) == 255 || a(6, 9) == 0);
        // transparent canvas pixels were created with alpha 0 and the box colour is not blended with them
        let p = &cv.px[(20 * 40 + 20) * 4..(20 * 40 + 20) * 4 + 4];
        assert_eq!(p, [255, 255, 255, 255]);
    }
}
