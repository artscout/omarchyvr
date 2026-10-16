//! Pupil-swim pattern: a WORLD-locked room rendered as a real 3D projection per eye (view pose and FOV from the
//! runtime), so that the lines are straight in the world and any bending or moving of them while the head turns is
//! the lens model's residual (barrel / pincushion, swim), not the pattern.
//!
//! The room is a cube of half-size [`HALF`] (2 m) centred on the head position at start (key `C` re-centres it,
//! yaw-aligned with the head, gravity-aligned), flat faces with a line grid every [`GRID`] (25 cm, bright every metre),
//! plus vertical poles at [`POLES_DEG`] azimuth, [`POLE_DIST`] away. Flat faces keep every grid line a straight
//! line in the pinhole projection, so they must also look straight through the lens. Static text stays head-locked.

use super::{cover, eye_background, Params};
use crate::color::{gray, mix, Rgb, MAGENTA, ORANGE, RED, YELLOW};
use crate::geometry::{mat_vec, Mat3, IDENT3};

pub const HALF: f32 = 2.0;
pub const GRID: f32 = 0.25;
pub const MAJOR_EVERY: i64 = 4;
pub const POLE_DIST: f32 = 1.5;
pub const POLE_RADIUS: f32 = 0.006;
/// Azimuths of the poles in degrees (0 = straight ahead, + = right).
pub const POLES_DEG: [f32; 7] = [-60.0, -40.0, -20.0, 0.0, 20.0, 40.0, 60.0];

/// Pose of one eye in the room frame (x right, y up, -z forward at the start).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SwimView {
    /// Eye space -> room.
    pub rot: Mat3,
    pub pos: [f32; 3],
}

impl SwimView {
    /// Looking straight ahead from the middle of the room, eyes 64 mm apart (previews and tests).
    pub fn default_for_eye(eye: usize) -> SwimView {
        SwimView {
            rot: IDENT3,
            pos: [if eye == 0 { -0.032 } else { 0.032 }, 0.0, 0.0],
        }
    }
}

fn pole_color(az: f32) -> Rgb {
    match az.abs() as i32 {
        0 => RED,
        20 => YELLOW,
        40 => ORANGE,
        _ => [0.2, 0.9, 1.0],
    }
}

/// Line coverage of coordinate `v` (metres) for lines every `GRID`, `foot` = size of a pixel in metres there.
fn line_cover(v: f32, foot: f32) -> (f32, bool) {
    let i = (v / GRID).round();
    let major = (i as i64).rem_euclid(MAJOR_EVERY) == 0;
    let hw = foot * if major { 1.4 } else { 0.7 };
    (cover(v - i * GRID, hw, foot), major)
}

/// Colour seen along the room-space ray `o + t d` (`d` unit), `ang` = angle of one pixel in radians.
pub fn shade_ray(o: [f32; 3], d: [f32; 3], ang: f32, eye: usize) -> Rgb {
    // exit through the face of the cube
    let mut t_wall = f32::MAX;
    let mut axis = 0;
    for i in 0..3 {
        let t = if d[i] > 1e-7 {
            (HALF - o[i]) / d[i]
        } else if d[i] < -1e-7 {
            (-HALF - o[i]) / d[i]
        } else {
            f32::MAX
        };
        if t > 0.0 && t < t_wall {
            t_wall = t;
            axis = i;
        }
    }
    if t_wall == f32::MAX {
        return eye_background(eye);
    }
    let h = [
        o[0] + d[0] * t_wall,
        o[1] + d[1] * t_wall,
        o[2] + d[2] * t_wall,
    ];
    let foot = t_wall * ang / d[axis].abs().max(0.15);
    let (a, b) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    let base = eye_background(eye);
    let mut c = [base[0] * 0.8, base[1] * 0.8, base[2] * 0.8];
    let (la, ma) = line_cover(h[a], foot);
    let (lb, mb) = line_cover(h[b], foot);
    c = mix(c, if ma { gray(0.85) } else { gray(0.36) }, la);
    c = mix(c, if mb { gray(0.85) } else { gray(0.36) }, lb);
    // the eye-level horizon on the walls
    if axis != 1 {
        c = mix(c, [0.2, 0.75, 0.3], cover(h[1], foot * 1.2, foot) * 0.9);
    }
    // fixation cross on the front wall
    if axis == 2 && d[2] < 0.0 {
        let w = foot * 1.6;
        if h[1].abs() < 0.08 {
            c = mix(c, MAGENTA, cover(h[0], w, foot));
        }
        if h[0].abs() < 0.08 {
            c = mix(c, MAGENTA, cover(h[1], w, foot));
        }
    }
    // poles: nearest one in front of the wall along the ray
    let mut best = f32::MAX;
    for az in POLES_DEG {
        let (s, co) = az.to_radians().sin_cos();
        let (cx, cz) = (POLE_DIST * s, -POLE_DIST * co);
        let h2 = d[0] * d[0] + d[2] * d[2];
        if h2 < 1e-9 {
            continue;
        }
        let tc = ((cx - o[0]) * d[0] + (cz - o[2]) * d[2]) / h2;
        if tc <= 0.0 || tc >= t_wall || tc >= best {
            continue;
        }
        let px = o[0] + d[0] * tc - cx;
        let pz = o[2] + d[2] * tc - cz;
        let dist = px.hypot(pz);
        let f = tc * ang / h2.sqrt();
        let cv = cover(dist, POLE_RADIUS, f.max(1e-6));
        if cv > 0.0 {
            c = mix(c, pole_color(az), cv);
            best = tc;
        }
    }
    c
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let (tx, ty) = p.warp.apply(p.fov.pixel_to_tangent(p.w, p.h, x, y));
    let n = (tx * tx + ty * ty + 1.0).sqrt();
    let d = mat_vec(&p.view.rot, [tx / n, ty / n, -1.0 / n]);
    let r2 = tx * tx + ty * ty;
    let ang = p.fov.tangent_per_pixel(p.w) / (1.0 + r2);
    shade_ray(p.view.pos, d, ang, p.eye)
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::super::{render, Pattern};
    use super::*;
    use crate::geometry::rot_y;

    fn bright(c: [u8; 3]) -> bool {
        c[0] > 120 && c[1] > 120 && c[2] > 120
    }

    #[test]
    fn grid_lines_are_straight_in_the_projection() {
        // on the front wall (x = 1 m line, 4 lines of a 25 cm grid from the centre): vertical in the image at every height
        let mut p = params(1200, 1100);
        p.view.pos = [0.0, 0.0, 0.0];
        let img = render(Pattern::Swim, &p, None, 4);
        // x = 1.0 at z = -2: tangent 0.5; find the bright pixel columns in several rows
        let mut cols = Vec::new();
        for y in [150u32, 400, 700, 950] {
            let want = p.fov.tangent_to_pixel(p.w, p.h, 0.5, 0.0).0;
            let mut found = None;
            for x in (want as u32 - 6)..(want as u32 + 6) {
                if bright(img.get(x, y)) {
                    found = Some(x);
                }
            }
            cols.push(found.unwrap_or_else(|| panic!("no line near column {want} in row {y}")));
        }
        let (mn, mx) = (cols.iter().min().unwrap(), cols.iter().max().unwrap());
        assert!(mx - mn <= 2, "{cols:?}");
    }

    #[test]
    fn horizontal_lines_are_straight_too() {
        // y = 1.0 m on the front wall is at tangent 0.5 vertically for every column
        let mut p = params(1200, 1100);
        p.view.pos = [0.0; 3];
        let img = render(Pattern::Swim, &p, None, 4);
        let want = p.fov.tangent_to_pixel(p.w, p.h, 0.0, 0.5).1;
        for x in [250u32, 450, 750, 950] {
            let mut ok = false;
            for y in (want as u32 - 4)..(want as u32 + 4) {
                ok |= bright(img.get(x, y));
            }
            assert!(ok, "no line at row {want} in column {x}");
        }
    }

    #[test]
    fn poles_sit_at_their_azimuths_and_move_with_the_head() {
        let mut p = params(1400, 1100);
        p.view.pos = [0.0; 3];
        // pole at +20 deg azimuth: seen at tangent tan(20) when looking straight ahead
        let c = shade_ray(
            [0.0; 3],
            {
                let a = 20f32.to_radians();
                [a.sin(), 0.0, -a.cos()]
            },
            1e-4,
            0,
        );
        assert!(c[0] > 0.9 && c[1] > 0.9 && c[2] < 0.3, "{c:?}");
        // turn the head 20 deg to the left (yaw +20): the pole is now straight ahead + 40... it moves right in view
        p.view.rot = rot_y(20.0);
        let ahead = shade(&p, 700, 550);
        // pole 0 (red) is at -20 deg relative to the view now, pole +20 (yellow) is at the centre
        assert!(
            ahead[0] > 0.9 && ahead[1] > 0.9 && ahead[2] < 0.4,
            "{ahead:?}"
        );
    }

    #[test]
    fn eyes_see_parallax_on_the_poles() {
        // the red centre pole at 1.5 m: left and right eye (64 mm apart) see it at different angles
        let ang = |x: f32| (x / POLE_DIST).atan();
        let (l, r) = (ang(-0.032), ang(0.032));
        assert!((r - l).to_degrees() > 2.0);
        let mut seen = [false; 2];
        for (i, eye) in [0usize, 1].into_iter().enumerate() {
            let o = SwimView::default_for_eye(eye).pos;
            for k in -300..=300 {
                let a = k as f32 * 1e-4;
                let c = shade_ray(o, [a.sin(), 0.0, -a.cos()], 1e-4, eye);
                if c[0] > 0.9 && c[1] < 0.2 && c[2] < 0.2 {
                    seen[i] = true;
                    // the pole is at -x/1.5 = angle from the eye
                    let want = ang(-o[0]);
                    assert!((a - want).abs() < 0.02, "{a} vs {want}");
                }
            }
        }
        assert_eq!(seen, [true, true]);
    }

    #[test]
    fn a_ray_always_finds_a_wall() {
        for k in 0..200 {
            let a = k as f32 * 0.031;
            let d = [a.sin() * 0.6, a.cos() * 0.5, -(a * 0.7).cos() * 0.62];
            let n = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
            let c = shade_ray([0.1, -0.2, 0.3], d.map(|v| v / n), 1e-3, 1);
            assert!(c.iter().all(|v| v.is_finite()));
        }
    }
}
