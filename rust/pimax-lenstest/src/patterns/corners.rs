//! Corner frames: an L-shaped bracket in every corner of the eye image (top-left red, top-right green, bottom-left
//! blue, bottom-right yellow) flush with the image corner, a checkerboard patch behind each, labels with the
//! corner's angle from the optical axis. Wrong corner = wrong panel rotation or mirroring; cut brackets = corner
//! clipped by the lens.

use super::{text_scale, Label, Params, LABEL_BG};
use crate::color::{gray, Rgb, BLACK, BLUE, GREEN, RED, WHITE, YELLOW};
use crate::geometry::angle_from_axis_deg;

/// Checker cell size in tangent units.
pub const CELL: f32 = 0.05;

pub const NAMES: [&str; 4] = ["TL", "TR", "BL", "BR"];
pub const COLORS: [Rgb; 4] = [RED, GREEN, BLUE, YELLOW];

/// Bracket arm length and thickness in pixels.
pub fn arm_px(p: &Params) -> u32 {
    (p.w.min(p.h) as f32 * 0.14) as u32
}

pub fn thickness_px(p: &Params) -> u32 {
    (p.w.min(p.h) / 150).max(2)
}

/// Index of the corner nearest to the pixel: 0 TL, 1 TR, 2 BL, 3 BR.
fn corner_index(p: &Params, x: u32, y: u32) -> usize {
    usize::from(x >= p.w / 2) + 2 * usize::from(y >= p.h / 2)
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let dx = if x < p.w / 2 { x } else { p.w - 1 - x };
    let dy = if y < p.h / 2 { y } else { p.h - 1 - y };
    let (arm, t) = (arm_px(p), thickness_px(p));
    let k = corner_index(p, x, y);
    if (dx < t && dy < arm) || (dy < t && dx < arm) {
        return COLORS[k];
    }
    // checker patch in the corner boxes (22% of each side)
    if dx < (p.w as f32 * 0.22) as u32 && dy < (p.h as f32 * 0.22) as u32 {
        let (tx, ty) = p.fov.pixel_to_tangent(p.w, p.h, x, y);
        let parity = ((tx / CELL).floor() as i64 + (ty / CELL).floor() as i64).rem_euclid(2);
        return if parity == 0 { gray(0.9) } else { BLACK };
    }
    BLACK
}

pub fn labels(p: &Params) -> Vec<Label> {
    let scale = text_scale(p.h);
    let (arm, t) = (arm_px(p) as i32, thickness_px(p) as i32);
    let corners = [(0u32, 0u32), (p.w - 1, 0), (0, p.h - 1), (p.w - 1, p.h - 1)];
    let mut out = Vec::new();
    for (k, (cx, cy)) in corners.into_iter().enumerate() {
        let (tx, ty) = p.fov.pixel_to_tangent(p.w, p.h, cx, cy);
        let text = format!("{} {:.0}\u{b0}", NAMES[k], angle_from_axis_deg(tx, ty));
        let (tw, th) = pimax_lens::font::text_size(&text, scale);
        let (tw, th) = (tw as i32, th as i32);
        let x = if k % 2 == 0 {
            t + arm / 3
        } else {
            p.w as i32 - t - arm / 3 - tw
        };
        let y = if k < 2 {
            t + arm + 2 * scale as i32
        } else {
            p.h as i32 - t - arm - 2 * scale as i32 - th
        };
        out.push(Label {
            text,
            x,
            y,
            scale,
            fg: COLORS[k],
            bg: LABEL_BG,
        });
    }
    let _ = WHITE;
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
    fn brackets_sit_in_the_matching_corners() {
        let p = params(1200, 1100);
        let img = render(&p);
        assert_eq!(img.get(0, 0), [255, 0, 0]);
        assert_eq!(img.get(1199, 0), [0, 255, 0]);
        assert_eq!(img.get(0, 1099), [0, 0, 255]);
        assert_eq!(img.get(1199, 1099), [255, 255, 0]);
        // arm length and thickness
        let (arm, t) = (arm_px(&p), thickness_px(&p));
        assert_eq!(img.get(arm - 1, 0), [255, 0, 0]);
        assert_ne!(img.get(arm + 1, 0), [255, 0, 0]);
        assert_eq!(img.get(0, arm - 1), [255, 0, 0]);
        assert_eq!(img.get(t - 1, 50), [255, 0, 0]);
        assert_ne!(img.get(t, 50), [255, 0, 0]);
        // mirrored right-hand corners
        assert_eq!(img.get(1199 - arm + 1, 0), [0, 255, 0]);
        assert_eq!(img.get(1199, 1099 - arm + 1), [255, 255, 0]);
    }

    #[test]
    fn checker_patches_alternate_and_the_middle_is_black() {
        let p = params(1200, 1100);
        let img = render(&p);
        assert_eq!(img.get(600, 550), [0, 0, 0]);
        // neighbouring cells in the corner patch differ; cell size is CELL tangent units
        let cell_px = (CELL / p.fov.tangent_per_pixel(p.w)) as u32;
        let (x, y) = (150u32, 150u32);
        let a = img.get(x, y);
        let b = img.get(x + cell_px, y);
        let c = img.get(x + cell_px, y + cell_px);
        assert_ne!(a, b);
        assert_eq!(a, c);
        // both the light and the dark cells exist in each corner box
        for (x0, y0) in [(10u32, 10u32), (1000, 10), (10, 900), (1000, 900)] {
            let mut light = 0;
            let mut dark = 0;
            for y in y0..y0 + 150 {
                for x in x0..x0 + 150 {
                    match img.get(x, y)[0] {
                        0 => dark += 1,
                        v if v > 200 => light += 1,
                        _ => {}
                    }
                }
            }
            assert!(
                light > 1000 && dark > 1000,
                "corner at {x0},{y0}: {light} {dark}"
            );
        }
    }

    #[test]
    fn labels_name_the_corner_and_its_angle() {
        let p = params(1200, 1100);
        let ls = labels(&p);
        assert_eq!(ls.len(), 4);
        for (l, n) in ls.iter().zip(NAMES) {
            assert!(l.text.starts_with(n), "{l:?}");
            let (tw, th) = pimax_lens::font::text_size(&l.text, l.scale);
            assert!(
                l.x >= 0 && l.y >= 0 && l.x + tw as i32 <= 1200 && l.y + th as i32 <= 1100,
                "{l:?}"
            );
        }
        // the corner of a 50 x 44.5 deg image is 57 deg from the axis
        let n: f32 = ls[0].text["TL ".len()..]
            .strip_suffix('\u{b0}')
            .unwrap()
            .parse()
            .unwrap();
        assert!(
            (n - angle_from_axis_deg(50f32.to_radians().tan(), 44.5f32.to_radians().tan()).round())
                .abs()
                <= 1.0,
            "{n}"
        );
        // text colour follows the bracket colour
        assert_eq!(ls[1].fg, GREEN);
    }

    #[test]
    fn corner_index_quadrants() {
        let p = params(100, 80);
        assert_eq!(corner_index(&p, 0, 0), 0);
        assert_eq!(corner_index(&p, 99, 0), 1);
        assert_eq!(corner_index(&p, 0, 79), 2);
        assert_eq!(corner_index(&p, 99, 79), 3);
    }
}
