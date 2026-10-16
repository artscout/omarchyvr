//! Per-channel grids: thin straight lines and 10-degree rings on black, either white (all channels: coloured
//! fringes show chromatic aberration of the lens model) or one of red, green, blue alone (same geometry, so the
//! three can be compared position by position).

use super::{cover, text_scale, Label, Params, LABEL_BG};
use crate::color::{gray, Rgb, BLACK, BLUE, GREEN, RED, WHITE};

pub const STEPS: usize = 4;

/// Grid spacing in tangent units (coarser than the plain grid: thin lines, easier to compare).
pub const STEP: f32 = 0.2;

pub fn step_name(step: usize) -> &'static str {
    match step {
        0 => "ALL (WHITE)",
        1 => "RED ONLY",
        2 => "GREEN ONLY",
        _ => "BLUE ONLY",
    }
}

/// The colour of the lines at this step.
pub fn line_color(step: usize) -> Rgb {
    match step {
        0 => WHITE,
        1 => RED,
        2 => GREEN,
        _ => BLUE,
    }
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let (tx, ty) = p.tangent(x, y);
    let pix = p.fov.tangent_per_pixel(p.w);
    let hw = pix * 0.9; // about two pixels wide
    let line = |v: f32| -> f32 {
        let i = (v / STEP).round();
        cover(v - i * STEP, hw, pix)
    };
    let mut a = line(tx).max(line(ty));
    // rings every 10 degrees
    let r = tx.hypot(ty);
    let k = (r.atan().to_degrees() / 10.0).round();
    if k >= 1.0 {
        a = a.max(cover(r - (k * 10.0).to_radians().tan(), hw, pix));
    }
    let col = line_color(p.step);
    [
        BLACK[0] + col[0] * a,
        BLACK[1] + col[1] * a,
        BLACK[2] + col[2] * a,
    ]
}

pub fn labels(p: &Params) -> Vec<Label> {
    let scale = text_scale(p.h);
    let (ax, ay) = p.principal();
    let col = if p.step == 0 {
        gray(0.85)
    } else {
        line_color(p.step)
    };
    let text = step_name(p.step).to_string();
    let (tw, _) = pimax_lens::font::text_size(&text, scale);
    vec![Label {
        text,
        x: (ax as i32 - tw as i32 / 2).max(0),
        y: ay as i32 + 14 * scale as i32,
        scale,
        fg: col,
        bg: LABEL_BG,
    }]
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::*;
    use crate::image::Image;

    fn render(p: &Params) -> Image {
        Image::render(p.w, p.h, 2, &|x, y| shade(p, x, y))
    }

    #[test]
    fn single_channel_steps_light_only_their_channel() {
        for step in 1..=3usize {
            let mut p = params(500, 450);
            p.step = step;
            let img = render(&p);
            let mut lit = 0;
            for px in img.rgb.chunks_exact(3) {
                for (c, &v) in px.iter().enumerate() {
                    if c != step - 1 {
                        assert_eq!(v, 0, "step {step}");
                    }
                }
                if px[step - 1] > 200 {
                    lit += 1;
                }
            }
            assert!(lit > 500, "step {step}: {lit} lit pixels");
        }
    }

    #[test]
    fn all_step_is_white_lines_on_black() {
        let p = params(500, 450);
        let img = render(&p);
        assert!(img
            .rgb
            .chunks_exact(3)
            .all(|c| c[0] == c[1] && c[1] == c[2]));
        assert!(img.count(|c| c[0] == 255) > 500);
        assert!(img.count(|c| c[0] == 0) > 500 * 450 / 2);
    }

    #[test]
    fn same_geometry_in_every_step() {
        let mut p = params(300, 270);
        let base = render(&p);
        for step in 1..=3usize {
            p.step = step;
            let img = render(&p);
            // lit pixels are the same set (the channel value equals the white value there)
            for (a, b) in base.rgb.chunks_exact(3).zip(img.rgb.chunks_exact(3)) {
                assert_eq!(a[0], b[step - 1]);
            }
        }
    }

    #[test]
    fn rings_every_ten_degrees_are_drawn() {
        let p = params(1200, 1100);
        let img = render(&p);
        let (ax, ay) = p.principal();
        // along a diagonal (away from the straight lines), the 30 degree ring pixel is lit
        let r = 30f32.to_radians().tan();
        let d = r / 2f32.sqrt();
        let (px, py) = p.to_pixel(d, d);
        let mut found = false;
        for dx in -3..=3 {
            for dy in -3..=3 {
                if img.get((px as i32 + dx) as u32, (py as i32 + dy) as u32)[0] > 200 {
                    found = true;
                }
            }
        }
        assert!(
            found,
            "no ring pixel near the 30 degree point (axis at {ax},{ay})"
        );
    }

    #[test]
    fn step_names_and_labels() {
        assert_eq!(step_name(0), "ALL (WHITE)");
        assert_eq!(step_name(3), "BLUE ONLY");
        let mut p = params(1200, 1100);
        p.step = 1;
        let l = labels(&p);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].text, "RED ONLY");
        assert_eq!(l[0].fg, RED);
    }
}
