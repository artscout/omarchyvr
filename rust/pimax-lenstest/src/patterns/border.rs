//! FOV edge and border. The outermost frame is drawn on the outermost pixels of the eye image (the exact edge of
//! the rendered FOV), one colour per side; white frames sit 2, 5 and 10 degrees inside. If the compositor repeated
//! the edge pixels beyond the image (clamp to edge) the frame colours would streak outwards over the panel; with
//! the black border sampler (the right behaviour) everything beyond the frame is black. Steps 1 and 2 fill the
//! whole image with white and with 25% grey to show the exact visible shape and any halo at its edge.

use super::{cover, text_scale, Label, Params, LABEL_BG};
use crate::color::{gray, level, mix, Rgb, BLACK, BLUE, GREEN, RED, WHITE, YELLOW};

pub const STEPS: usize = 3;

/// Inset of the white frames from the FOV edge in degrees.
pub const INSETS_DEG: [f32; 3] = [2.0, 5.0, 10.0];

pub fn step_name(step: usize) -> &'static str {
    match step {
        0 => "FRAMES",
        1 => "WHITE FILL 100%",
        _ => "GREY FILL 25%",
    }
}

/// Thickness in pixels of the outermost frame.
pub fn frame_px(p: &Params) -> u32 {
    (p.w.min(p.h) / 300).max(2)
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    match p.step {
        1 => return WHITE,
        2 => return gray(level(0.25)),
        _ => {}
    }
    let t = frame_px(p);
    let (tx, ty) = p.fov.pixel_to_tangent(p.w, p.h, x, y);
    let pix = p.fov.tangent_per_pixel(p.w);
    let mut c = BLACK;
    // inset frames: rectangles at tan(half-angle - inset) on every side
    let [dl, dr, du, dd] = p.fov.half_angles_deg();
    for (i, inset) in INSETS_DEG.iter().enumerate() {
        let (l, r, u, d) = (dl - inset, dr - inset, du - inset, dd - inset);
        if l <= 0.0 || r <= 0.0 || u <= 0.0 || d <= 0.0 {
            continue;
        }
        let (tl, tr, tu, td) = (
            l.to_radians().tan(),
            r.to_radians().tan(),
            u.to_radians().tan(),
            d.to_radians().tan(),
        );
        let inside_x = tx >= -tl - pix && tx <= tr + pix;
        let inside_y = ty >= -td - pix && ty <= tu + pix;
        let mut a = 0.0f32;
        if inside_y {
            a = a
                .max(cover(tx + tl, pix * 0.9, pix))
                .max(cover(tx - tr, pix * 0.9, pix));
        }
        if inside_x {
            a = a
                .max(cover(ty - tu, pix * 0.9, pix))
                .max(cover(ty + td, pix * 0.9, pix));
        }
        c = mix(c, gray([0.95, 0.7, 0.5][i]), a);
    }
    // outermost frame on the outermost pixels, coloured by side; corners are white
    let (left, right, top, bottom) = (x < t, x >= p.w - t, y < t, y >= p.h - t);
    let sides = [left, right, top, bottom].iter().filter(|&&b| b).count();
    if sides >= 2 {
        c = WHITE;
    } else if left {
        c = RED;
    } else if right {
        c = GREEN;
    } else if top {
        c = BLUE;
    } else if bottom {
        c = YELLOW;
    }
    c
}

pub fn labels(p: &Params) -> Vec<Label> {
    if p.step != 0 {
        return Vec::new();
    }
    let scale = text_scale(p.h);
    let t = frame_px(p) as i32;
    let m = 3 * scale as i32 + t;
    let (ax, ay) = p.fov.principal_point(p.w, p.h);
    let mk = |text: &str, x: i32, y: i32, fg: Rgb| Label {
        text: text.to_string(),
        x,
        y,
        scale,
        fg,
        bg: LABEL_BG,
    };
    let w = |s: &str| pimax_lens::font::text_size(s, scale).0 as i32;
    let mut out = vec![
        mk("LEFT", m, p.h as i32 / 2 - 4 * scale as i32, RED),
        mk(
            "RIGHT",
            p.w as i32 - m - w("RIGHT"),
            p.h as i32 / 2 - 4 * scale as i32,
            GREEN,
        ),
        mk("TOP", p.w as i32 / 2 - w("TOP") / 2, m, BLUE),
        mk(
            "BOTTOM",
            p.w as i32 / 2 - w("BOTTOM") / 2,
            p.h as i32 - m - 7 * scale as i32,
            YELLOW,
        ),
    ];
    // the inset labels sit on the top inset line, left of the axis, so they read in order
    let [_, _, du, _] = p.fov.half_angles_deg();
    for (i, inset) in INSETS_DEG.iter().enumerate() {
        if du - inset > 0.0 {
            let (_, py) = p
                .fov
                .tangent_to_pixel(p.w, p.h, 0.0, (du - inset).to_radians().tan());
            let text = format!("{inset:.0}\u{b0} IN");
            out.push(mk(
                &text,
                ax as i32 + 6 * scale as i32,
                py as i32 + 2 * scale as i32,
                gray([0.95, 0.7, 0.5][i]),
            ));
        }
    }
    let _ = ay;
    out
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
    fn outer_frame_is_on_the_outermost_pixels_with_one_colour_per_side() {
        let p = params(900, 800);
        let img = render(&p);
        let t = frame_px(&p);
        assert_eq!(t, 2);
        assert_eq!(img.get(0, 400), [255, 0, 0]);
        assert_eq!(img.get(1, 400), [255, 0, 0]);
        assert_eq!(img.get(899, 400), [0, 255, 0]);
        assert_eq!(img.get(450, 0), [0, 0, 255]);
        assert_eq!(img.get(450, 799), [255, 255, 0]);
        // corners are white, the pixel just inside the frame is not frame coloured
        for (x, y) in [(0, 0), (899, 0), (0, 799), (899, 799)] {
            assert_eq!(img.get(x, y), [255, 255, 255], "{x},{y}");
        }
        assert_eq!(img.get(2, 400), [0, 0, 0]);
        assert_eq!(img.get(450, 2), [0, 0, 0]);
    }

    #[test]
    fn inset_frames_are_where_the_angles_say() {
        let p = params(2000, 1800);
        let img = render(&p);
        // the 10 degree inset frame: left side at tangent -tan(50-10), on a row through the middle
        let want = p
            .fov
            .tangent_to_pixel(p.w, p.h, -(40f32.to_radians().tan()), 0.0)
            .0;
        let row = 900;
        let mut hit = None;
        for x in (want as u32 - 4)..(want as u32 + 4) {
            if img.get(x, row)[0] > 100 {
                hit = Some(x);
            }
        }
        let x = hit.expect("10 degree inset line not found") as f32 + 0.5;
        assert!((x - want).abs() < 2.0, "{x} vs {want}");
        // top side of the 5 degree frame: tan(44.5 - 5)
        let want_y = p
            .fov
            .tangent_to_pixel(p.w, p.h, 0.0, 39.5f32.to_radians().tan())
            .1;
        let col = 1000;
        let found = ((want_y as u32 - 4)..(want_y as u32 + 4)).any(|y| img.get(col, y)[0] > 150);
        assert!(found, "5 degree top line not near row {want_y}");
        // asymmetric FOV: insets follow each side's own angle
        let mut q = params(2000, 1800);
        q.fov = crate::geometry::Fov::from_half_angles_deg(40.0, 60.0, 30.0, 50.0);
        let img = render(&q);
        let want = q
            .fov
            .tangent_to_pixel(q.w, q.h, 50f32.to_radians().tan(), 0.0)
            .0; // right side, 60 - 10
        assert!(
            ((want as u32 - 4)..(want as u32 + 4)).any(|x| img.get(x, 900)[0] > 100),
            "right inset"
        );
    }

    #[test]
    fn fill_steps() {
        let mut p = params(64, 64);
        p.step = 1;
        let w = render(&p);
        assert_eq!(w.count(|c| c == [255, 255, 255]), 64 * 64);
        p.step = 2;
        let g = render(&p);
        assert_eq!(g.count(|c| c == [137, 137, 137]), 64 * 64);
        assert!(labels(&p).is_empty());
    }

    #[test]
    fn nothing_is_drawn_outside_the_frame_and_insets_skip_too_large_angles() {
        // 8 degree FOV: only the 2 and 5 degree insets exist (10 would be negative)
        let mut p = params(400, 400);
        p.fov = crate::geometry::Fov::from_half_angles_deg(8.0, 8.0, 8.0, 8.0);
        let img = render(&p);
        let ls = labels(&p);
        assert_eq!(ls.iter().filter(|l| l.text.ends_with("IN")).count(), 2);
        assert!(img.count(|c| c != [0, 0, 0]) > 0);
    }

    #[test]
    fn labels_cover_all_four_sides_and_stay_inside() {
        let p = params(1500, 1350);
        let ls = labels(&p);
        for want in [
            "LEFT",
            "RIGHT",
            "TOP",
            "BOTTOM",
            "2\u{b0} IN",
            "5\u{b0} IN",
            "10\u{b0} IN",
        ] {
            assert!(ls.iter().any(|l| l.text == want), "{want}");
        }
        for l in &ls {
            let (tw, th) = pimax_lens::font::text_size(&l.text, l.scale);
            assert!(
                l.x >= 0 && l.y >= 0 && l.x + tw as i32 <= 1500 && l.y + th as i32 <= 1350,
                "{l:?}"
            );
        }
    }
}
