//! A single square of constant angular size on black, moved from the centre of the view to the edge (steps), at
//! full brightness or at a quarter of the light. Glare of the optics follows the square's position and scales with
//! its brightness; a software sampling artefact stays at a fixed offset from the mapped position.

use super::{Dir, Params};
use crate::color::{gray, level, Rgb, BLACK};
use crate::geometry::az_el_deg;

pub const DEFAULT_SIZE_DEG: f32 = 5.0;

/// Nominal centre positions in degrees from the optical axis; the last step always touches the image edge.
const NOMINAL: [f32; 5] = [0.0, 10.0, 20.0, 30.0, 40.0];

/// Half-angle of the rendered FOV in the direction of travel, degrees.
pub fn half_fov_deg(p: &Params) -> f32 {
    let [l, r, u, d] = p.fov.half_angles_deg();
    match p.square_dir {
        Dir::Right => r,
        Dir::Left => l,
        Dir::Up => u,
        Dir::Down => d,
    }
}

/// Centre positions of the steps, degrees from the axis: 0, 10, 20, 30, 40 as far as they fit, then the position at
/// which the square touches the edge of the image.
pub fn positions(p: &Params) -> Vec<f32> {
    let edge = (half_fov_deg(p) - p.square_deg / 2.0).max(0.0);
    let mut v: Vec<f32> = NOMINAL
        .iter()
        .copied()
        .filter(|&a| a < edge - 2.0)
        .collect();
    v.push(edge);
    v
}

/// Centre angle of the current step.
pub fn centre_angle(p: &Params) -> f32 {
    let v = positions(p);
    v[p.step.min(v.len() - 1)]
}

pub fn step_name(p: &Params, step: usize) -> String {
    let v = positions(p);
    let i = step.min(v.len() - 1);
    let a = v[i];
    if i == v.len() - 1 {
        format!("EDGE {a:.1} DEG {}", p.square_dir.name().to_uppercase())
    } else if a == 0.0 {
        "CENTRE".to_string()
    } else {
        format!("{a:.0} DEG {}", p.square_dir.name().to_uppercase())
    }
}

/// Centre of the square in (azimuth, elevation) degrees.
pub fn centre_az_el(p: &Params) -> (f32, f32) {
    let a = centre_angle(p);
    match p.square_dir {
        Dir::Right => (a, 0.0),
        Dir::Left => (-a, 0.0),
        Dir::Up => (0.0, a),
        Dir::Down => (0.0, -a),
    }
}

/// `brightness` is the fraction of the full-white luminance (1.0 or 0.25).
pub fn shade(p: &Params, x: u32, y: u32, brightness: f32) -> Rgb {
    let (tx, ty) = p.fov.pixel_to_tangent(p.w, p.h, x, y);
    let (az, el) = az_el_deg(tx, ty);
    let (az0, el0) = centre_az_el(p);
    let h = p.square_deg / 2.0;
    // degrees per pixel along azimuth and elevation at this position
    let pix = p.fov.tangent_per_pixel(p.w).to_degrees();
    let s = 1.0 + tx * tx;
    let d_az = pix / s;
    let d_el = pix * s.sqrt() / (s + ty * ty);
    let c_az = ((h - (az - az0).abs()) / d_az + 0.5).clamp(0.0, 1.0);
    let c_el = ((h - (el - el0).abs()) / d_el + 0.5).clamp(0.0, 1.0);
    let cov = c_az.min(c_el);
    if cov <= 0.0 {
        return BLACK;
    }
    let v = level(brightness) * cov;
    gray(v)
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::*;
    use crate::geometry::Fov;
    use crate::image::Image;

    fn render(p: &Params, b: f32) -> Image {
        Image::render(p.w, p.h, 2, &|x, y| shade(p, x, y, b))
    }

    /// Bounding box of the pixels brighter than `thr`.
    fn bbox(img: &Image, thr: u8) -> (u32, u32, u32, u32) {
        let (mut x0, mut y0, mut x1, mut y1) = (u32::MAX, u32::MAX, 0, 0);
        for y in 0..img.h {
            for x in 0..img.w {
                if img.get(x, y)[0] > thr {
                    x0 = x0.min(x);
                    y0 = y0.min(y);
                    x1 = x1.max(x);
                    y1 = y1.max(y);
                }
            }
        }
        (x0, y0, x1, y1)
    }

    #[test]
    fn positions_are_nominal_then_touch_the_edge() {
        let p = params(100, 100);
        // 50 deg to the right, 5 deg square: edge step at 47.5; 40 is more than 2 deg short of it -> kept
        assert_eq!(positions(&p), vec![0.0, 10.0, 20.0, 30.0, 40.0, 47.5]);
        let mut q = p.clone();
        q.square_dir = Dir::Up; // 44.5 deg: the edge step is at 42, so 40 (less than 2 deg short of it) is dropped
        assert_eq!(positions(&q), vec![0.0, 10.0, 20.0, 30.0, 42.0]);
    }

    #[test]
    fn step_names() {
        let p = params(100, 100);
        assert_eq!(step_name(&p, 0), "CENTRE");
        assert_eq!(step_name(&p, 3), "30 DEG RIGHT");
        assert_eq!(step_name(&p, 5), "EDGE 47.5 DEG RIGHT");
        assert_eq!(step_name(&p, 99), "EDGE 47.5 DEG RIGHT");
        let mut q = p.clone();
        q.square_dir = Dir::Left;
        assert_eq!(step_name(&q, 1), "10 DEG LEFT");
    }

    #[test]
    fn square_is_centred_at_the_step_angle_and_has_the_right_size() {
        let mut p = params(1600, 1400);
        for (step, deg) in [(0usize, 0.0f32), (2, 20.0), (3, 30.0)] {
            p.step = step;
            let img = render(&p, 1.0);
            let (x0, y0, x1, y1) = bbox(&img, 128);
            let cx = (x0 + x1 + 1) as f32 / 2.0;
            let want = p
                .fov
                .tangent_to_pixel(p.w, p.h, deg.to_radians().tan(), 0.0)
                .0;
            assert!(
                (cx - want).abs() < 2.0,
                "step {step}: centre {cx} vs {want}"
            );
            // width: from az-2.5 to az+2.5
            let w_want = p
                .fov
                .tangent_to_pixel(p.w, p.h, (deg + 2.5).to_radians().tan(), 0.0)
                .0
                - p.fov
                    .tangent_to_pixel(p.w, p.h, (deg - 2.5).to_radians().tan(), 0.0)
                    .0;
            assert!(
                ((x1 + 1 - x0) as f32 - w_want).abs() < 3.0,
                "step {step}: width {} vs {w_want}",
                x1 + 1 - x0
            );
            // vertical: elevation +-2.5 around the horizon
            let (_, ay) = p.fov.principal_point(p.w, p.h);
            assert!(((y0 + y1 + 1) as f32 / 2.0 - ay).abs() < 2.0);
            // the angular height is also 5 degrees: top edge pixel row has elevation 2.5 at the centre column
            let col = (x0 + x1) / 2;
            let (_, ty) = p.fov.pixel_to_tangent(p.w, p.h, col, y0);
            let (tx, _) = p.fov.pixel_to_tangent(p.w, p.h, col, y0);
            let (_, el) = az_el_deg(tx, ty);
            assert!((el - 2.5).abs() < 0.1, "{el}");
        }
    }

    #[test]
    fn brightness_levels() {
        let mut p = params(400, 360);
        p.step = 0;
        let full = render(&p, 1.0);
        let quarter = render(&p, 0.25);
        assert_eq!(full.get(200, 180), [255, 255, 255]);
        assert_eq!(quarter.get(200, 180), [137, 137, 137]);
        // everything outside is exactly black
        assert_eq!(full.get(5, 5), [0, 0, 0]);
        assert_eq!(quarter.get(395, 355), [0, 0, 0]);
        // only one blob: lit pixels form one rectangle
        let (x0, y0, x1, y1) = bbox(&quarter, 0);
        let lit = quarter.count(|c| c[0] > 0) as u32;
        assert!(
            lit <= (x1 - x0 + 1) * (y1 - y0 + 1)
                && lit as f32 > 0.9 * ((x1 - x0 + 1) * (y1 - y0 + 1)) as f32
        );
    }

    #[test]
    fn last_step_touches_the_image_edge_in_every_direction() {
        for dir in Dir::ALL {
            let mut p = params(1000, 900);
            p.square_dir = dir;
            p.step = positions(&p).len() - 1;
            let img = render(&p, 1.0);
            let (x0, y0, x1, y1) = bbox(&img, 100);
            match dir {
                Dir::Right => assert!(x1 >= p.w - 2, "{x1}"),
                Dir::Left => assert!(x0 <= 1, "{x0}"),
                Dir::Up => assert!(y0 <= 1, "{y0}"),
                Dir::Down => assert!(y1 >= p.h - 2, "{y1}"),
            }
        }
    }

    #[test]
    fn asymmetric_fov_uses_the_fov_of_the_direction_of_travel() {
        let mut p = params(1000, 900);
        p.fov = Fov::from_half_angles_deg(40.0, 60.0, 30.0, 50.0);
        for (dir, want) in [
            (Dir::Right, 57.5),
            (Dir::Left, 37.5),
            (Dir::Up, 27.5),
            (Dir::Down, 47.5),
        ] {
            p.square_dir = dir;
            assert!(
                (half_fov_deg(&p) - p.square_deg / 2.0 - want).abs() < 1e-3,
                "{dir:?}"
            );
            assert!((*positions(&p).last().unwrap() - want).abs() < 1e-3);
        }
        // a tiny FOV still yields one step
        let mut q = params(100, 100);
        q.fov = Fov::from_half_angles_deg(2.0, 2.0, 2.0, 2.0);
        assert_eq!(positions(&q), vec![0.0]);
        q.step = 7;
        assert_eq!(centre_angle(&q), 0.0);
    }

    #[test]
    fn size_parameter_changes_the_square() {
        let mut p = params(800, 720);
        let small = bbox(&render(&p, 1.0), 128);
        p.square_deg = 10.0;
        let big = bbox(&render(&p, 1.0), 128);
        assert!((big.2 - big.0) as f32 > 1.8 * (small.2 - small.0) as f32);
    }
}
