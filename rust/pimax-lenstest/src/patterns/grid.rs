//! Straight-line grid: the lines of a flat wall in front of the viewer (spacing 0.1 in tangent units = 0.2 m at 2 m,
//! a bright line every 0.5), a red cross on the optical axis. Straight in the scene means straight on the lens when
//! the distortion model is right.

use super::{cover, eye_background, text_scale, Label, Params, LABEL_BG};
use crate::color::{gray, mix, Rgb, RED};
use crate::geometry::angle_from_axis_deg;

/// Line spacing in tangent units.
pub const STEP: f32 = 0.1;
/// Every `MAJOR`-th line is bright and thick.
pub const MAJOR: i64 = 5;

const MINOR_COLOR: Rgb = [0.38, 0.38, 0.40];
const MAJOR_COLOR: Rgb = [0.88, 0.88, 0.90];

/// Nearest line index of coordinate `v` and the signed distance to it.
fn nearest(v: f32) -> (i64, f32) {
    let i = (v / STEP).round() as i64;
    (i, v - i as f32 * STEP)
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let (tx, ty) = p.tangent(x, y);
    let pix = p.fov.tangent_per_pixel(p.w);
    let mut c = eye_background(p.eye);
    let (ix, dx) = nearest(tx);
    let (iy, dy) = nearest(ty);
    let (mx, my) = (ix % MAJOR == 0, iy % MAJOR == 0);
    let ax = cover(dx, pix * if mx { 1.5 } else { 0.8 }, pix);
    let ay = cover(dy, pix * if my { 1.5 } else { 0.8 }, pix);
    c = mix(c, if mx { MAJOR_COLOR } else { MINOR_COLOR }, ax);
    c = mix(c, if my { MAJOR_COLOR } else { MINOR_COLOR }, ay);
    // optical axis cross
    c = mix(c, RED, cover(tx, pix * 2.0, pix));
    c = mix(c, RED, cover(ty, pix * 2.0, pix));
    c
}

/// Angle labels on the bright lines along both axes: "27" at tangent 0.5, "45" at 1.0 (degrees off axis).
pub fn labels(p: &Params) -> Vec<Label> {
    let scale = text_scale(p.h);
    let mut out = Vec::new();
    let (ax, ay) = p.principal();
    for k in [-10i64, -5, 5, 10] {
        let t = k as f32 * STEP;
        let deg = angle_from_axis_deg(t, 0.0);
        let text = format!("{deg:.0}");
        // along the horizontal axis, just above the axis line
        let (px, _) = p.to_pixel(t, 0.0);
        out.push(Label {
            text: text.clone(),
            x: px as i32 + 3 * scale as i32,
            y: ay as i32 - 10 * scale as i32,
            scale,
            fg: gray(0.9),
            bg: LABEL_BG,
        });
        // along the vertical axis, just right of the axis line
        let (_, py) = p.to_pixel(0.0, t);
        out.push(Label {
            text,
            x: ax as i32 + 4 * scale as i32,
            y: py as i32 + 2 * scale as i32,
            scale,
            fg: gray(0.9),
            bg: LABEL_BG,
        });
    }
    // keep only labels that fit completely
    out.retain(|l| {
        let (tw, th) = pimax_lens::font::text_size(&l.text, l.scale);
        l.x >= 0 && l.y >= 0 && l.x + tw as i32 <= p.w as i32 && l.y + th as i32 <= p.h as i32
    });
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::*;
    use crate::image::Image;

    fn render(p: &Params) -> Image {
        Image::render(p.w, p.h, 1, &|x, y| shade(p, x, y))
    }

    fn brightness(c: [u8; 3]) -> u32 {
        c.iter().map(|&v| u32::from(v)).sum()
    }

    #[test]
    fn nearest_line() {
        assert_eq!(nearest(0.34).0, 3);
        assert!((nearest(0.34).1 - 0.04).abs() < 1e-6);
        assert_eq!(nearest(-0.26).0, -3);
    }

    #[test]
    fn vertical_lines_are_straight_and_spaced_by_the_step() {
        let p = params(1000, 900);
        let img = render(&p);
        // scan one row away from the axes, find the bright columns
        let row = 200;
        let cols: Vec<u32> = (0..p.w)
            .filter(|&x| brightness(img.get(x, row)) > 250)
            .collect();
        assert!(cols.len() >= 20, "{}", cols.len());
        // group consecutive columns into lines
        let mut lines: Vec<f32> = Vec::new();
        let mut start = cols[0];
        let mut prev = cols[0];
        for &c in &cols[1..] {
            if c != prev + 1 {
                lines.push((start + prev) as f32 / 2.0 + 0.5);
                start = c;
            }
            prev = c;
        }
        lines.push((start + prev) as f32 / 2.0 + 0.5);
        // spacing of lines in pixels = STEP / (tangent per pixel); equal everywhere (a flat wall is linear in tangent)
        let spacing = STEP / p.fov.tangent_per_pixel(p.w);
        for w in lines.windows(2) {
            assert!(
                (w[1] - w[0] - spacing).abs() < 1.0,
                "{} -> {} (want {spacing})",
                w[0],
                w[1]
            );
        }
        // straight: the same columns are bright in a very different row
        for &x in cols.iter().step_by(3) {
            assert!(brightness(img.get(x, 780)) > 200, "column {x} not straight");
        }
    }

    #[test]
    fn axes_are_red_and_major_lines_are_brighter_than_minor_lines() {
        let p = params(1000, 900);
        let img = render(&p);
        let (ax, ay) = p.principal();
        let c = img.get(ax as u32, 100);
        assert!(c[0] > 200 && c[1] < 60 && c[2] < 60, "{c:?}");
        let c = img.get(100, ay as u32);
        assert!(c[0] > 200 && c[1] < 60, "{c:?}");
        // line index 5 (major) vs 4 (minor), at a row away from the axes
        let major_x = p.to_pixel(5.0 * STEP, 0.0).0 as u32;
        let minor_x = p.to_pixel(4.0 * STEP, 0.0).0 as u32;
        assert!(brightness(img.get(major_x, 100)) > brightness(img.get(minor_x, 100)) + 100);
    }

    #[test]
    fn background_differs_per_eye_only_slightly() {
        let mut p = params(200, 180);
        let l = render(&p);
        p.eye = 1;
        let r = render(&p);
        assert_ne!(l, r);
        // a pixel between lines: left bluish, right reddish
        let (x, y) = (5u32, 3u32);
        assert!(l.get(x, y)[2] > l.get(x, y)[0] && r.get(x, y)[0] > r.get(x, y)[2]);
    }

    #[test]
    fn labels_are_inside_the_image_and_read_27_and_45() {
        let p = params(1000, 900);
        let labels = labels(&p);
        let texts: Vec<&str> = labels.iter().map(|l| l.text.as_str()).collect();
        assert!(texts.contains(&"27") && texts.contains(&"45"), "{texts:?}");
        for l in &labels {
            let (tw, th) = pimax_lens::font::text_size(&l.text, l.scale);
            assert!(l.x >= 0 && l.y >= 0 && l.x + tw as i32 <= 1000 && l.y + th as i32 <= 900);
        }
        // 4 labels per axis at most (+-0.5, +-1.0 = 4 horizontal, 4 vertical); vertical 1.0 is outside the 44.5 deg image
        assert!(labels.len() >= 6 && labels.len() <= 8, "{}", labels.len());
    }

    #[test]
    fn asymmetric_fov_keeps_the_axis_on_the_right_pixel() {
        let mut p = params(1000, 900);
        p.fov = crate::geometry::Fov::from_half_angles_deg(40.0, 60.0, 30.0, 50.0);
        let img = render(&p);
        let (ax, _) = p.principal();
        assert!(ax < 450.0);
        let c = img.get(ax as u32, 100);
        assert!(c[0] > 200 && c[1] < 60, "{c:?}");
    }
}
