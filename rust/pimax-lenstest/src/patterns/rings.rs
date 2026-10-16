//! Angle rings: circles of constant angle from the optical axis (tangent radius `tan(angle)`), bright and labelled at
//! 10/20/30/40/50 deg, thin grey every 5 deg, orange axes. Derived from the per-eye FOV and projection, so the axis
//! sits where the runtime says the optical axis of that eye image is.

use super::{cover, eye_background, text_scale, Label, Params, LABEL_BG};
use crate::color::{gray, mix, Rgb, CYAN, GREEN, MAGENTA, ORANGE, WHITE, YELLOW};

/// The labelled rings in degrees.
pub const MAJOR_DEG: [f32; 5] = [10.0, 20.0, 30.0, 40.0, 50.0];
/// Their colours (the same as `pimax-lens grid` uses for the predicted panel picture).
pub const COLORS: [Rgb; 5] = [WHITE, YELLOW, GREEN, CYAN, MAGENTA];

/// Ring angle step of the thin rings.
const MINOR_STEP_DEG: f32 = 5.0;

fn background(eye: usize) -> Rgb {
    let b = eye_background(eye);
    [b[0] * 0.6, b[1] * 0.6, b[2] * 0.6]
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let (tx, ty) = p.tangent(x, y);
    let pix = p.fov.tangent_per_pixel(p.w);
    let r = tx.hypot(ty);
    let theta = r.atan().to_degrees();
    let mut c = background(p.eye);
    // nearest ring of the 5 degree family
    let k = (theta / MINOR_STEP_DEG).round();
    let ring_deg = k * MINOR_STEP_DEG;
    if k >= 1.0 {
        let d = r - ring_deg.to_radians().tan();
        let major = (ring_deg / 10.0).fract() == 0.0 && ring_deg <= 50.0;
        if major {
            let idx = (ring_deg / 10.0) as usize - 1;
            c = mix(c, COLORS[idx], cover(d, pix * 1.6, pix));
        } else {
            c = mix(c, gray(0.34), cover(d, pix * 0.7, pix));
        }
    }
    // optical axis
    c = mix(c, ORANGE, cover(tx, pix * 1.0, pix) * 0.9);
    c = mix(c, ORANGE, cover(ty, pix * 1.0, pix) * 0.9);
    c
}

/// Pixel position of the point at `angle_deg` from the axis in direction (dx, dy) (tangent plane, y up).
fn ring_point(p: &Params, angle_deg: f32, dx: f32, dy: f32) -> (f32, f32) {
    let t = angle_deg.to_radians().tan();
    p.to_pixel(dx * t, dy * t)
}

pub fn labels(p: &Params) -> Vec<Label> {
    let scale = text_scale(p.h);
    let mut out = Vec::new();
    // label directions in order of preference: along +x, -x, +y, -y of the tangent plane
    let dirs = [(1.0f32, 0.0f32), (-1.0, 0.0), (0.0, 1.0), (0.0, -1.0)];
    for (i, &deg) in MAJOR_DEG.iter().enumerate() {
        let text = format!("{deg:.0}\u{b0}");
        let (tw, th) = pimax_lens::font::text_size(&text, scale);
        let (tw, th) = (tw as i32, th as i32);
        for (dx, dy) in dirs {
            let (px, py) = ring_point(p, deg, dx, dy);
            // label sits just outside the crossing, above the axis for horizontal directions, right of it otherwise
            let (x, y) = if dx != 0.0 {
                (px as i32 - tw / 2, py as i32 - th - 4 * scale as i32)
            } else {
                (px as i32 + 4 * scale as i32, py as i32 - th / 2)
            };
            let margin = 2 * scale as i32;
            if x >= margin
                && y >= margin
                && x + tw <= p.w as i32 - margin
                && y + th <= p.h as i32 - margin
            {
                out.push(Label {
                    text: text.clone(),
                    x,
                    y,
                    scale,
                    fg: COLORS[i],
                    bg: LABEL_BG,
                });
                break;
            }
        }
    }
    // the axis itself
    let (ax, ay) = p.principal();
    out.push(Label {
        text: "0".to_string(),
        x: ax as i32 + 4 * scale as i32,
        y: ay as i32 + 3 * scale as i32,
        scale,
        fg: ORANGE,
        bg: LABEL_BG,
    });
    out
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::*;
    use crate::geometry::{angle_from_axis_deg, Fov};
    use crate::image::Image;

    fn render(p: &Params) -> Image {
        Image::render(p.w, p.h, 2, &|x, y| shade(p, x, y))
    }

    /// Pixels close to the pure ring colour.
    fn near(c: [u8; 3], want: Rgb, tol: i32) -> bool {
        (0..3).all(|i| (i32::from(c[i]) - (want[i] * 255.0) as i32).abs() <= tol)
    }

    #[test]
    fn major_rings_lie_at_the_right_angle_for_symmetric_and_asymmetric_fov() {
        for fov in [
            Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5),
            Fov::from_half_angles_deg(42.0, 58.0, 36.0, 47.0),
        ] {
            let mut p = params(1200, 1100);
            p.fov = fov;
            let img = render(&p);
            for (i, &deg) in MAJOR_DEG.iter().enumerate() {
                let mut seen = 0;
                for y in (0..p.h).step_by(3) {
                    for x in 0..p.w {
                        if near(img.get(x, y), COLORS[i], 12) {
                            let (tx, ty) = p.tangent(x, y);
                            let a = angle_from_axis_deg(tx, ty);
                            assert!(
                                (a - deg).abs() < 0.25,
                                "ring {deg}: pixel ({x},{y}) is at {a}"
                            );
                            seen += 1;
                        }
                    }
                }
                // rings that are (partly) inside the image must show up; the 50 deg ring may be clipped by a 44.5 deg edge
                let inside = ring_point(&p, deg, 1.0, 0.0).0 < p.w as f32
                    || ring_point(&p, deg, -1.0, 0.0).0 > 0.0;
                if inside {
                    assert!(seen > 200, "ring {deg} has {seen} pixels");
                }
            }
        }
    }

    #[test]
    fn ring_is_round_in_tangent_space() {
        // along 8 directions the 30 deg ring is crossed at the same tangent radius
        let p = params(1600, 1600);
        let img = render(&p);
        let want = 30f32.to_radians().tan();
        for k in 0..8 {
            let phi = k as f32 * std::f32::consts::FRAC_PI_4;
            let (mut best, mut best_d) = (0.0f32, f32::MAX);
            let mut r = want * 0.9;
            while r < want * 1.1 {
                let (px, py) = p.to_pixel(r * phi.cos(), r * phi.sin());
                let c = img.get(px as u32, py as u32);
                // greenest pixel along the ray
                let d = (255 - i32::from(c[1])) as f32
                    + i32::from(c[0]) as f32
                    + i32::from(c[2]) as f32;
                if d < best_d {
                    best_d = d;
                    best = r;
                }
                r += 0.0005;
            }
            assert!(
                (best - want).abs() < 0.004,
                "direction {k}: ring at {best}, want {want}"
            );
        }
    }

    #[test]
    fn minor_rings_are_dim_grey_and_axes_orange() {
        let p = params(1000, 900);
        let img = render(&p);
        // 15 deg ring along +x of the axis
        let (px, py) = ring_point(&p, 15.0, 1.0, 0.0);
        let c = img.get(px as u32, (py as u32) + 60);
        let _ = c;
        let (x15, y15) = ring_point(
            &p,
            15.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        );
        let c = img.get(x15 as u32, y15 as u32);
        assert!(c.iter().all(|&v| (60..110).contains(&v)), "{c:?}");
        let (ax, _) = p.principal();
        let top = img.get(ax as u32, 20);
        assert!(top[0] > 150 && top[1] > 60 && top[2] < 80, "{top:?}");
    }

    #[test]
    fn labels_are_complete_inside_and_fall_back_when_the_ring_is_clipped() {
        let p = params(1200, 1100);
        let ls = labels(&p);
        let texts: Vec<&str> = ls.iter().map(|l| l.text.as_str()).collect();
        for want in ["10\u{b0}", "20\u{b0}", "30\u{b0}", "40\u{b0}", "0"] {
            assert!(texts.contains(&want), "{texts:?}");
        }
        for l in &ls {
            let (tw, th) = pimax_lens::font::text_size(&l.text, l.scale);
            assert!(
                l.x >= 0 && l.y >= 0 && l.x + tw as i32 <= 1200 && l.y + th as i32 <= 1100,
                "{l:?}"
            );
        }
        // 50 deg is at the right edge for a 50 deg FOV: its label moved to another direction or was dropped, but
        // a 10 deg label sits on the right of the axis
        let l10 = ls.iter().find(|l| l.text == "10\u{b0}").unwrap();
        let (ax, _) = p.principal();
        assert!(l10.x as f32 > ax);
        // with a very wide FOV every ring label fits
        let mut wide = params(1200, 1100);
        wide.fov = Fov::from_half_angles_deg(70.0, 70.0, 60.0, 60.0);
        assert_eq!(labels(&wide).len(), 6);
    }

    #[test]
    fn background_is_dark() {
        let p = params(200, 180);
        let img = render(&p);
        assert!(img.get(3, 3).iter().map(|&v| u32::from(v)).sum::<u32>() < 60);
    }
}
