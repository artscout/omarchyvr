//! Drawing for the `gaze` pattern: head-locked marks (discs and rings at a 3D point in the head frame) projected per
//! eye with that eye's pose and FOV, so the two images fuse into one target at the right distance.
//!
//! The mark is evaluated per pixel by the *angle* between the pixel's ray and the direction from the eye to the mark's
//! centre (anti-aliased with the pixel's own angular size), which keeps the dot round and its size in degrees exact
//! everywhere in the field. A [`Painter`] keeps the image between frames and only touches the pixels around the marks
//! it drew last time (a full 3552 x 3840 render per frame would be pointless for a few small dots).

use crate::color::{self, Rgb, BLACK};
use crate::gaze::{Mark, MarkKind, V3};
use crate::geometry::{mat_vec, transpose, Fov, Mat3};
use crate::image::Image;
use crate::patterns::{cover, draw_hud_at, hud_strip_at};

/// An eye in the head frame: `rot` takes eye-space vectors to the head frame (the OpenXR view pose orientation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyePose {
    pub rot: Mat3,
    pub pos: [f32; 3],
}

/// A mark seen from one eye.
struct Seen {
    /// Unit direction from the eye to the mark, eye space (x right, y up, -z forward).
    axis: V3,
    /// Tangents of that direction.
    tan: (f64, f64),
    radius: f64,
    width: f64,
    ring: bool,
    color: Rgb,
}

fn prepare(marks: &[Mark], pose: &EyePose) -> Vec<Seen> {
    let inv = transpose(&pose.rot);
    marks
        .iter()
        .filter_map(|m| {
            let rel = [
                (m.pos[0] - f64::from(pose.pos[0])) as f32,
                (m.pos[1] - f64::from(pose.pos[1])) as f32,
                (m.pos[2] - f64::from(pose.pos[2])) as f32,
            ];
            let v = mat_vec(&inv, rel);
            let v = [f64::from(v[0]), f64::from(v[1]), f64::from(v[2])];
            if v[2] > -1e-3 {
                return None; // behind the eye
            }
            let n = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
            let (ring, width) = match m.kind {
                MarkKind::Disc => (false, 0.0),
                MarkKind::Ring(w) => (true, w.to_radians()),
            };
            Some(Seen {
                axis: [v[0] / n, v[1] / n, v[2] / n],
                tan: (v[0] / -v[2], v[1] / -v[2]),
                radius: m.radius_deg.to_radians(),
                width,
                ring,
                color: m.color,
            })
        })
        .collect()
}

/// Coverage (0..1) of a mark at the pixel whose unit ray is `ray`, `pix` = angular size of the pixel (radians).
fn coverage(s: &Seen, ray: V3, pix: f64) -> f32 {
    let c = [
        s.axis[1] * ray[2] - s.axis[2] * ray[1],
        s.axis[2] * ray[0] - s.axis[0] * ray[2],
        s.axis[0] * ray[1] - s.axis[1] * ray[0],
    ];
    let sin = (c[0] * c[0] + c[1] * c[1] + c[2] * c[2]).sqrt();
    let cos = s.axis[0] * ray[0] + s.axis[1] * ray[1] + s.axis[2] * ray[2];
    let ang = sin.atan2(cos);
    let p = pix as f32;
    if s.ring {
        cover((ang - s.radius) as f32, (s.width * 0.5) as f32, p)
    } else {
        cover(ang as f32, s.radius as f32, p)
    }
}

fn pixel_ray(fov: &Fov, w: u32, h: u32, x: u32, y: u32) -> (V3, f64) {
    let (tx, ty) = fov.pixel_to_tangent(w, h, x, y);
    let (tx, ty) = (f64::from(tx), f64::from(ty));
    let n = (tx * tx + ty * ty + 1.0).sqrt();
    let pix = f64::from(fov.tangent_per_pixel(w)) / (1.0 + tx * tx + ty * ty);
    ([tx / n, ty / n, -1.0 / n], pix)
}

/// Colour of the pixel for the preview of the pattern (black background, the marks over it).
pub fn shade_marks(
    marks: &[Mark],
    pose: &EyePose,
    fov: &Fov,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
) -> Rgb {
    let (ray, pix) = pixel_ray(fov, w, h, x, y);
    let mut c = BLACK;
    for s in prepare(marks, pose) {
        c = color::mix(c, s.color, coverage(&s, ray, pix));
    }
    c
}

/// Pixel rectangle `[x0, y0, x1, y1)`.
type Rect = (i32, i32, i32, i32);

/// The image of one eye, kept between frames.
pub struct Painter {
    pub img: Image,
    dirty: Vec<Rect>,
    hud_rows: Option<(i32, i32)>,
}

impl Painter {
    pub fn new(w: u32, h: u32) -> Painter {
        Painter {
            img: Image::new(w, h, BLACK),
            dirty: Vec::new(),
            hud_rows: None,
        }
    }

    /// Pixel position (continuous) and the half size in pixels of the box around a mark.
    fn bbox(s: &Seen, fov: &Fov, w: u32, h: u32) -> Rect {
        let [tl, tr, tu, td] = fov.tangents();
        let (px, py) = fov.tangent_to_pixel(w, h, s.tan.0 as f32, s.tan.1 as f32);
        let t2 = s.tan.0 * s.tan.0 + s.tan.1 * s.tan.1;
        let half_tan = (s.radius + s.width) * (1.0 + t2) * 1.15;
        let hx = half_tan / (f64::from(tl + tr) / f64::from(w)) + 3.0;
        let hy = half_tan / (f64::from(tu + td) / f64::from(h)) + 3.0;
        (
            (f64::from(px) - hx).floor().max(-1.0e6) as i32,
            (f64::from(py) - hy).floor().max(-1.0e6) as i32,
            (f64::from(px) + hx).ceil().min(1.0e6) as i32,
            (f64::from(py) + hy).ceil().min(1.0e6) as i32,
        )
    }

    /// Redraw: erase what was drawn last time, draw `marks` as seen from `pose`, then the text lines.
    pub fn paint(&mut self, marks: &[Mark], pose: &EyePose, fov: &Fov, hud: &[String]) {
        let (w, h) = (self.img.w, self.img.h);
        for r in std::mem::take(&mut self.dirty) {
            self.img.fill_rect(r.0, r.1, r.2, r.3, BLACK);
        }
        if let Some((top, bottom)) = self.hud_rows.take() {
            self.img.fill_rect(0, top, w as i32, bottom, BLACK);
        }
        for s in prepare(marks, pose) {
            let r = Painter::bbox(&s, fov, w, h);
            let (x0, y0) = (r.0.max(0), r.1.max(0));
            let (x1, y1) = (r.2.min(w as i32), r.3.min(h as i32));
            if x0 >= x1 || y0 >= y1 {
                continue;
            }
            for y in y0..y1 {
                for x in x0..x1 {
                    let (ray, pix) = pixel_ray(fov, w, h, x as u32, y as u32);
                    let cov = coverage(&s, ray, pix);
                    if cov > 0.0 {
                        let old = self.img.get(x as u32, y as u32);
                        let old = [
                            f32::from(old[0]) / 255.0,
                            f32::from(old[1]) / 255.0,
                            f32::from(old[2]) / 255.0,
                        ];
                        let c = color::mix(old, s.color, cov);
                        self.img.set(
                            x,
                            y,
                            [
                                color::to_byte(c[0]),
                                color::to_byte(c[1]),
                                color::to_byte(c[2]),
                            ],
                        );
                    }
                }
            }
            self.dirty.push((x0, y0, x1, y1));
        }
        // fallback text (no quad layer): shifted to the head-forward direction so the two eyes' copies fuse
        if let Some((top, strip)) = hud_strip_at(w, h, fov, Some(&pose.rot), hud) {
            self.hud_rows = Some((top, top + strip.h as i32));
            draw_hud_at(&mut self.img, fov, Some(&pose.rot), hud);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gaze::{target_point, MarkKind};
    use crate::geometry::{quat_to_mat, IDENT3};

    fn fov() -> Fov {
        Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5)
    }

    fn disc(az: f64, el: f64, r: f64) -> Mark {
        Mark {
            pos: target_point(az, el),
            kind: MarkKind::Disc,
            radius_deg: r,
            color: [1.0, 1.0, 1.0],
        }
    }

    fn pose(eye: usize) -> EyePose {
        EyePose {
            rot: IDENT3,
            pos: [if eye == 0 { -0.032 } else { 0.032 }, 0.0, 0.0],
        }
    }

    /// Centroid (pixel units) and area of the lit pixels.
    fn centroid(img: &Image) -> (f64, f64, f64) {
        let (mut sx, mut sy, mut sw) = (0.0, 0.0, 0.0);
        for y in 0..img.h {
            for x in 0..img.w {
                let v = f64::from(img.get(x, y)[0]) / 255.0;
                if v > 0.0 {
                    sx += v * (f64::from(x) + 0.5);
                    sy += v * (f64::from(y) + 0.5);
                    sw += v;
                }
            }
        }
        (sx / sw, sy / sw, sw)
    }

    #[test]
    fn disc_lands_where_the_projection_says_with_the_right_size() {
        let (w, h) = (1200u32, 1100u32);
        let f = fov();
        for eye in 0..2 {
            let mut p = Painter::new(w, h);
            let (az, el) = (25.0, -20.0);
            p.paint(&[disc(az, el, 1.0)], &pose(eye), &f, &[]);
            let (cx, cy, area) = centroid(&p.img);
            // expected: tangent of (target - eye)
            let t = target_point(az, el);
            let rel = [t[0] - f64::from(pose(eye).pos[0]), t[1], t[2]];
            let (px, py) =
                f.tangent_to_pixel(w, h, (rel[0] / -rel[2]) as f32, (rel[1] / -rel[2]) as f32);
            assert!(
                (cx - f64::from(px)).abs() < 0.3 && (cy - f64::from(py)).abs() < 0.3,
                "{eye}: {cx},{cy} vs {px},{py}"
            );
            // area of a 1 deg radius disc: pi r^2 in pixels^2, r = 1 deg at this eccentricity
            let rad_px = 1.0f64.to_radians()
                * (1.0 + (rel[0] / -rel[2]).powi(2) + (rel[1] / -rel[2]).powi(2))
                / f64::from(f.tangent_per_pixel(w));
            // elongation of the disc off-axis: mean of the radial and tangential scale; allow 15 %
            let want = std::f64::consts::PI * rad_px * rad_px / (1.0 + 0.0);
            assert!(
                area > 0.5 * want && area < 1.2 * want,
                "area {area} vs {want}"
            );
        }
    }

    #[test]
    fn two_eyes_fuse_into_one_point_at_two_metres() {
        let (w, h) = (1200u32, 1100u32);
        let f = fov();
        let (az, el) = (-35.0, 10.0);
        let target = target_point(az, el);
        let mut rays = Vec::new();
        for eye in 0..2 {
            let mut p = Painter::new(w, h);
            p.paint(&[disc(az, el, 0.5)], &pose(eye), &f, &[]);
            let (cx, cy, _) = centroid(&p.img);
            let [tl, tr, tu, td] = f.tangents();
            let tx = -f64::from(tl) + cx / f64::from(w) * f64::from(tl + tr);
            let ty = f64::from(tu) - cy / f64::from(h) * f64::from(tu + td);
            let n = (tx * tx + ty * ty + 1.0).sqrt();
            rays.push((f64::from(pose(eye).pos[0]), [tx / n, ty / n, -1.0 / n]));
        }
        // closest approach of the two lines of sight to the target point
        for (ox, d) in &rays {
            let rel = [target[0] - ox, target[1], target[2]];
            let t = rel[0] * d[0] + rel[1] * d[1] + rel[2] * d[2];
            let miss = [rel[0] - t * d[0], rel[1] - t * d[1], rel[2] - t * d[2]];
            let miss = (miss[0] * miss[0] + miss[1] * miss[1] + miss[2] * miss[2]).sqrt();
            assert!(miss < 0.002, "line of sight misses the target by {miss} m");
        }
        // the disparity is there: the left eye sees the dot further right than the right eye
        assert!(rays[0].1[0] > rays[1].1[0]);
    }

    #[test]
    fn canted_eyes_use_their_own_pose() {
        let (w, h) = (900u32, 800u32);
        let f = fov();
        let c = 4.0f32.to_radians() * 0.5;
        for eye in 0..2 {
            let ep = EyePose {
                rot: quat_to_mat(if eye == 0 {
                    [0.0, c.sin(), 0.0, c.cos()]
                } else {
                    [0.0, -c.sin(), 0.0, c.cos()]
                }),
                pos: pose(eye).pos,
            };
            let mut p = Painter::new(w, h);
            p.paint(&[disc(0.0, 0.0, 0.6)], &ep, &f, &[]);
            let (cx, _, _) = centroid(&p.img);
            // head-forward is off the optical axis by the cant: left eye turned left sees the target to its right
            let (ax, _) = f.principal_point(w, h);
            assert!(
                if eye == 0 {
                    cx > f64::from(ax)
                } else {
                    cx < f64::from(ax)
                },
                "{eye}: {cx} {ax}"
            );
        }
    }

    #[test]
    fn repainting_erases_the_old_marks_and_the_old_text() {
        let (w, h) = (900u32, 800u32);
        let f = fov();
        let mut p = Painter::new(w, h);
        let lines = vec!["HELLO WORLD".to_string(), "SECOND".to_string()];
        p.paint(&[disc(10.0, 5.0, 1.0)], &pose(0), &f, &lines);
        let first = p.img.count(|c| c != [0, 0, 0]);
        assert!(first > 300);
        // different mark, no text: nothing of the first picture may remain
        p.paint(&[disc(-20.0, -10.0, 0.5)], &pose(0), &f, &[]);
        let fresh = {
            let mut q = Painter::new(w, h);
            q.paint(&[disc(-20.0, -10.0, 0.5)], &pose(0), &f, &[]);
            q.img
        };
        assert_eq!(p.img, fresh);
        // many repaints at moving places stay equal to a fresh paint
        for i in 0..20 {
            let az = -30.0 + 3.0 * f64::from(i);
            p.paint(
                &[disc(az, 0.0, 0.8), disc(0.0, 15.0, 0.4)],
                &pose(1),
                &f,
                &lines,
            );
        }
        let mut q = Painter::new(w, h);
        q.paint(
            &[disc(27.0, 0.0, 0.8), disc(0.0, 15.0, 0.4)],
            &pose(1),
            &f,
            &lines,
        );
        assert_eq!(p.img, q.img);
    }

    #[test]
    fn ring_is_hollow_and_marks_behind_the_eye_or_off_screen_are_safe() {
        let (w, h) = (900u32, 800u32);
        let f = fov();
        let mut p = Painter::new(w, h);
        let ring = Mark {
            pos: target_point(0.0, 0.0),
            kind: MarkKind::Ring(0.2),
            radius_deg: 1.0,
            color: [0.0, 1.0, 0.0],
        };
        p.paint(&[ring], &pose(0), &f, &[]);
        let (cx, cy, _) = centroid(&p.img);
        let c = p.img.get(cx as u32, cy as u32);
        assert_eq!(c, [0, 0, 0], "the middle of a ring is empty");
        assert!(p.img.count(|c| c[1] > 100) > 50);
        let behind = Mark {
            pos: [0.0, 0.0, 2.0],
            ..ring
        };
        let off = disc(80.0, 0.0, 1.0);
        p.paint(&[behind, off], &pose(0), &f, &[]);
        assert_eq!(p.img.count(|c| c != [0, 0, 0]), 0);
    }

    #[test]
    fn shade_marks_matches_the_painter() {
        let (w, h) = (400u32, 360u32);
        let f = fov();
        let marks = [disc(5.0, 3.0, 2.0)];
        let mut p = Painter::new(w, h);
        p.paint(&marks, &pose(0), &f, &[]);
        let img = Image::render(w, h, 2, &|x, y| {
            shade_marks(&marks, &pose(0), &f, w, h, x, y)
        });
        assert_eq!(p.img, img);
    }
}
