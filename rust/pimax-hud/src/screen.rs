//! The pure part: what is on the panel and how it is drawn (CPU, 5x7 font from `pimax-lens`).

use pimax_lenstest::color::{self, Rgb};
use pimax_lenstest::image::Image;
use std::path::Path;

/// Panel texture size in pixels; the quad is 1 mm per pixel (see [`crate::xr::Placement`]).
pub const TEX_W: u32 = 1536;
pub const TEX_H: u32 = 768;
/// Default text scale (font pixels are 5x7; at 1 mm per pixel scale 6 is a 42 mm, 1.6 degree tall letter at 1.5 m).
pub const DEFAULT_SCALE: u32 = 6;
const MAX_SCALE: u32 = 12;
/// Panel backdrop and border colours.
const BACKDROP: Rgb = [0.012, 0.014, 0.02];
const BORDER: Rgb = [0.12, 0.14, 0.2];

pub const WHITE: Rgb = [0.85, 0.85, 0.85];
pub const GREEN: Rgb = [0.2, 0.9, 0.3];
pub const YELLOW: Rgb = [0.95, 0.85, 0.2];
pub const RED: Rgb = [1.0, 0.25, 0.2];
pub const CYAN: Rgb = [0.3, 0.8, 0.95];

/// One line of text.
#[derive(Clone, Debug, PartialEq)]
pub struct Line {
    pub text: String,
    pub scale: u32,
    pub color: Rgb,
}

impl Line {
    pub fn new(text: impl Into<String>) -> Line {
        Line { text: text.into(), scale: DEFAULT_SCALE, color: WHITE }
    }

    /// Pixels per font pixel (1..=12); the renderer lowers it when the text does not fit.
    pub fn scale(mut self, scale: u32) -> Line {
        self.scale = scale.clamp(1, MAX_SCALE);
        self
    }

    pub fn color(mut self, color: Rgb) -> Line {
        self.color = color;
        self
    }
}

/// Everything on the panel.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Screen {
    pub lines: Vec<Line>,
    /// 0..=1 draws the bar at the bottom.
    pub progress: Option<f32>,
    /// Draw the sharpness test pattern (fine text, resolution lines, grid, centred cross) under the lines.
    pub pattern: bool,
}

impl Screen {
    pub fn from_lines<S: AsRef<str>>(lines: &[S]) -> Screen {
        Screen { lines: lines.iter().map(|l| Line::new(l.as_ref())).collect(), progress: None, pattern: false }
    }

    /// Whether the panel is drawn at all.
    pub fn is_empty(&self) -> bool {
        self.lines.is_empty() && self.progress.is_none() && !self.pattern
    }
}

const MARGIN: u32 = 56;
const BAR_H: u32 = 44;
const BAR_W: u32 = 1100;

fn line_height(scale: u32) -> u32 {
    10 * scale
}

/// Scale of `line` reduced until its text fits the panel width.
fn fitted_scale(line: &Line) -> u32 {
    let avail = TEX_W - 2 * MARGIN;
    (1..=line.scale.clamp(1, MAX_SCALE))
        .rev()
        .find(|&s| pimax_lens::font::text_size(&line.text, s).0 <= avail)
        .unwrap_or(1)
}

/// Draw the panel. Pixels outside the rounded-off panel are exactly black (transparent in the quad).
pub fn render(screen: &Screen) -> Image {
    let mut img = Image::new(TEX_W, TEX_H, color::BLACK);
    if screen.is_empty() {
        return img;
    }
    let (w, h) = (TEX_W as i32, TEX_H as i32);
    img.fill_rect(8, 8, w - 8, h - 8, BORDER);
    img.fill_rect(14, 14, w - 14, h - 14, BACKDROP);
    let bar_room = if screen.progress.is_some() { BAR_H + 40 } else { 0 };
    let top = 40u32;
    let room = TEX_H - top - 40 - bar_room;
    // Shrink everything uniformly when the lines are too tall.
    let mut shrink = 0u32;
    let scales = |shrink: u32| -> Vec<u32> {
        screen.lines.iter().map(|l| fitted_scale(l).saturating_sub(shrink).max(1)).collect()
    };
    while scales(shrink).iter().map(|&s| line_height(s)).sum::<u32>() > room && shrink < MAX_SCALE {
        shrink += 1;
    }
    let mut y = top as i32;
    for (line, s) in screen.lines.iter().zip(scales(shrink)) {
        let (tw, _) = pimax_lens::font::text_size(&line.text, s);
        img.text(&line.text, (w - tw as i32) / 2, y, s, line.color);
        y += line_height(s) as i32;
    }
    if screen.pattern {
        draw_pattern(&mut img, y + 6, h - 26);
    }
    if let Some(p) = screen.progress {
        let p = if p.is_finite() { p.clamp(0.0, 1.0) } else { 0.0 };
        let x0 = (w - BAR_W as i32) / 2;
        let y0 = h - 40 - BAR_H as i32;
        img.fill_rect(x0 - 4, y0 - 4, x0 + BAR_W as i32 + 4, y0 + BAR_H as i32 + 4, CYAN);
        img.fill_rect(x0, y0, x0 + BAR_W as i32, y0 + BAR_H as i32, BACKDROP);
        img.fill_rect(x0, y0, x0 + (BAR_W as f32 * p) as i32, y0 + BAR_H as i32, GREEN);
    }
    img
}

/// The sharpness test pattern in the band `y0..y1`: a dim grid, a centred cross, fine text in several sizes on the
/// left, groups of 1 px lines at growing pitch (resolution wedges) on the right.
fn draw_pattern(img: &mut Image, y0: i32, y1: i32) {
    let (x0, x1) = (24, TEX_W as i32 - 24);
    if y1 - y0 < 120 {
        return;
    }
    let (cx, cy) = ((x0 + x1) / 2, (y0 + y1) / 2);
    let dim: Rgb = [0.16, 0.18, 0.24];
    let mut x = cx % 48;
    while x < x1 {
        if x > x0 {
            img.fill_rect(x, y0, x + 1, y1, dim);
        }
        x += 48;
    }
    let mut y = y0 + (cy - y0) % 48;
    while y < y1 {
        img.fill_rect(x0, y, x1, y + 1, dim);
        y += 48;
    }
    img.fill_rect(x0, y0, x1, y0 + 1, WHITE);
    img.fill_rect(x0, y1 - 1, x1, y1, WHITE);
    img.fill_rect(x0, y0, x0 + 1, y1, WHITE);
    img.fill_rect(x1 - 1, y0, x1, y1, WHITE);
    // Fine text, one block per size, on a clear backdrop (left half).
    let samples = [(1u32, "SHARP? 0123456789 THE QUICK BROWN FOX"), (2, "Sharp? 58 60 62 64 66 68 70 72"), (3, "Fine text 1234567890"), (4, "Clear? 65.4 mm")];
    let mut ty = y0 + 12;
    for (sc, t) in samples {
        let (tw, th) = pimax_lens::font::text_size(t, sc);
        if ty + th as i32 + 8 > y1 {
            break;
        }
        img.fill_rect(x0 + 8, ty - 3, x0 + 8 + tw as i32 + 6, ty + th as i32 + 3, BACKDROP);
        img.text(t, x0 + 11, ty, sc, WHITE);
        ty += th as i32 + 14;
    }
    // Resolution wedges (right half): vertical and horizontal 1 px lines at pitch 2, 3, 4, 6 px.
    let wx = cx + 60;
    let mut wy = y0 + 14;
    for pitch in [2, 3, 4, 6] {
        if wy + 44 > y1 {
            break;
        }
        img.fill_rect(wx - 3, wy - 3, wx + 303, wy + 41, BACKDROP);
        let mut k = 0;
        while k < 140 {
            img.fill_rect(wx + k, wy, wx + k + 1, wy + 38, WHITE);
            k += pitch;
        }
        let mut k = 0;
        while k < 38 {
            img.fill_rect(wx + 160, wy + k, wx + 300, wy + k + 1, WHITE);
            k += pitch;
        }
        wy += 52;
    }
    // Centred cross with a small box and a ring of ticks.
    img.fill_rect(cx - 70, cy, cx + 71, cy + 1, GREEN);
    img.fill_rect(cx, cy - 70, cx + 1, cy + 71, GREEN);
    img.fill_rect(cx - 12, cy - 12, cx + 13, cy - 11, GREEN);
    img.fill_rect(cx - 12, cy + 12, cx + 13, cy + 13, GREEN);
    img.fill_rect(cx - 12, cy - 12, cx - 11, cy + 13, GREEN);
    img.fill_rect(cx + 12, cy - 12, cx + 13, cy + 13, GREEN);
}

/// Render and write a PNG (no headset needed).
pub fn write_png(screen: &Screen, path: &Path) -> std::io::Result<()> {
    let img = render(screen);
    pimax_lens::png::write_rgb8(path, img.w, img.h, &img.rgb)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lit(img: &Image) -> usize {
        img.count(|c| c != [0, 0, 0])
    }

    #[test]
    fn empty_screen_is_fully_transparent() {
        assert_eq!(lit(&render(&Screen::default())), 0);
    }

    #[test]
    fn text_is_centred_and_inside_the_panel() {
        let img = render(&Screen::from_lines(&["LOOK STRAIGHT AHEAD"]));
        assert!(lit(&img) > 10_000);
        // corner outside the border stays transparent
        assert_eq!(img.get(0, 0), [0, 0, 0]);
        let bright = |c: [u8; 3]| c[1] > 120;
        let xs: Vec<u32> = (20..TEX_W - 20).filter(|&x| (20..200).any(|y| bright(img.get(x, y)))).collect();
        let mid = (xs[0] + xs[xs.len() - 1]) as i32 / 2;
        assert!((mid - TEX_W as i32 / 2).abs() < 8, "{mid}");
    }

    #[test]
    fn long_text_shrinks_to_fit_and_many_lines_fit() {
        let l = Line::new("X".repeat(100));
        assert!(fitted_scale(&l) < DEFAULT_SCALE);
        let lines: Vec<String> = (0..20).map(|i| format!("line {i}")).collect();
        let img = render(&Screen { lines: lines.iter().map(Line::new_ref).collect(), progress: Some(0.5), pattern: false });
        assert!(lit(&img) > 0);
    }

    impl Line {
        fn new_ref(s: &String) -> Line {
            Line::new(s.clone())
        }
    }

    #[test]
    fn progress_bar_fills_proportionally() {
        let count_green = |p: f32| {
            let img = render(&Screen { lines: vec![], progress: Some(p), pattern: false });
            img.count(|c| c[1] > 150 && c[0] < 120 && c[2] < 120)
        };
        assert_eq!(count_green(0.0), 0);
        let (a, b) = (count_green(0.25), count_green(0.5));
        assert!(a > 1000 && (b as f32 / a as f32 - 2.0).abs() < 0.05, "{a} {b}");
        assert_eq!(count_green(7.0), count_green(1.0));
        assert_eq!(count_green(f32::NAN), 0);
    }

    #[test]
    fn pattern_draws_in_the_band_below_the_lines() {
        let plain = render(&Screen::from_lines(&["A", "B"]));
        let pat = render(&Screen { lines: vec![Line::new("A"), Line::new("B")], progress: None, pattern: true });
        let bright = |i: &Image| i.count(|c| c[0] > 150);
        assert!(bright(&pat) > bright(&plain) + 2_000, "{} {}", bright(&pat), bright(&plain));
        // the centred cross is on the panel centre
        let (cx, cy) = (TEX_W / 2, (pat.h / 2) + 60);
        assert!((cy - 80..cy + 80).any(|y| pat.get(cx, y)[1] > 200 && pat.get(cx, y)[0] < 100));
        assert!(lit(&render(&Screen { lines: vec![], progress: None, pattern: true })) > 0);
    }

    #[test]
    fn png_is_written() {
        let p = std::env::temp_dir().join(format!("pimax-hud-test-{}.png", std::process::id()));
        write_png(&Screen::from_lines(&["HI"]), &p).unwrap();
        let bytes = std::fs::read(&p).unwrap();
        assert_eq!(&bytes[1..4], b"PNG");
        let _ = std::fs::remove_file(&p);
    }
}
