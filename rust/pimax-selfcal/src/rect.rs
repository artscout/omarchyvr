//! Stereo rectification of two fisheye tiles into a common pinhole frame, with float images and a validity mask
//! that also excludes the dark vignetting ring of the lenses.
//!
//! The frame is the one `pimax_cam::rectify::StereoRectifier` uses (x from camera a's centre to camera b's,
//! z the mean optical axis made orthogonal to x, y = z x x), so rows, `dy` and disparities here are directly
//! comparable with `pimax-cam rectify` / `hypotheses`. A rectified pixel `(u, v)` is the ray
//! `[(u - cx) / f, (v - cy) / f, 1]` in that frame, `cx = (W - 1) / 2`, `cy = (H - 1) / 2`.

use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::kb4::Kb4;

/// Usable image radius around the principal point (pixels). The lenses' image circle ends at ~330 px; past
/// ~315 px the picture is dark and its hard edge would make fake corners.
pub const DEFAULT_MASK_RADIUS_PX: f64 = 315.0;

/// Geometry of a rectified pair.
#[derive(Clone, Copy, Debug)]
pub struct RectGeom {
    /// Output width.
    pub width: usize,
    /// Output height.
    pub height: usize,
    /// Focal length (pixels).
    pub focal: f64,
    /// Rectified axes (columns) in camera-a coordinates.
    pub a_from_rect: Mat3,
    /// Rectified axes (columns) in camera-b coordinates.
    pub b_from_rect: Mat3,
    /// Baseline length (metres).
    pub baseline: f64,
}

impl RectGeom {
    /// Rectifying rotation for a pair with extrinsic `b_from_a` (`X_b = R X_a + t`).
    pub fn new(b_from_a: &Pose, width: usize, height: usize, focal: f64) -> RectGeom {
        let a_from_b = b_from_a.inverse();
        let x = geom::normalize(a_from_b.t);
        let za = [0.0, 0.0, 1.0];
        let zb = geom::col(&a_from_b.r, 2);
        let avg = geom::normalize(geom::add(za, zb));
        let z = geom::normalize(geom::sub(avg, geom::scale(x, geom::dot(avg, x))));
        let y = geom::cross(z, x);
        let a_from_rect = geom::from_cols(x, y, z);
        RectGeom {
            width,
            height,
            focal,
            a_from_rect,
            b_from_rect: geom::mat_mul(&b_from_a.r, &a_from_rect),
            baseline: geom::norm(a_from_b.t),
        }
    }

    /// Principal point.
    pub fn centre(&self) -> (f64, f64) {
        (
            (self.width as f64 - 1.0) / 2.0,
            (self.height as f64 - 1.0) / 2.0,
        )
    }

    /// Rectified-frame ray of pixel (x, y) (not normalised; z = 1).
    pub fn rect_ray(&self, x: f64, y: f64) -> Vec3 {
        let (cx, cy) = self.centre();
        [(x - cx) / self.focal, (y - cy) / self.focal, 1.0]
    }

    /// Unit ray in camera `which` (0 = a, 1 = b) of rectified pixel (x, y).
    pub fn ray(&self, which: usize, x: f64, y: f64) -> Vec3 {
        let m = if which == 0 {
            &self.a_from_rect
        } else {
            &self.b_from_rect
        };
        geom::normalize(geom::mat_vec(m, self.rect_ray(x, y)))
    }

    /// Rectified pixel of a ray given in camera `which` (`None` behind the rectified image plane).
    pub fn pixel(&self, which: usize, ray: Vec3) -> Option<(f64, f64)> {
        let m = if which == 0 {
            &self.a_from_rect
        } else {
            &self.b_from_rect
        };
        let d = geom::mat_vec(&geom::transpose(m), ray);
        if d[2] <= 1e-9 {
            return None;
        }
        let (cx, cy) = self.centre();
        Some((cx + self.focal * d[0] / d[2], cy + self.focal * d[1] / d[2]))
    }
}

/// A rectified float image with its validity mask.
#[derive(Clone, Debug)]
pub struct RectImage {
    /// Pixels (grey levels, 0..255).
    pub px: Vec<f32>,
    /// True where the source pixel lies inside the tile and the usable lens circle.
    pub valid: Vec<bool>,
}

impl RectImage {
    /// 8-bit copy (invalid pixels 0).
    pub fn to_u8(&self) -> Vec<u8> {
        self.px
            .iter()
            .zip(&self.valid)
            .map(|(&p, &v)| {
                if v {
                    (p + 0.5).clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            })
            .collect()
    }
}

fn bilinear(src: &[u8], w: usize, h: usize, x: f64, y: f64) -> f32 {
    let x0 = (x.floor().max(0.0) as usize).min(w - 1);
    let y0 = (y.floor().max(0.0) as usize).min(h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;
    let p = |xx: usize, yy: usize| src[yy * w + xx] as f32;
    p(x0, y0) * (1.0 - fx) * (1.0 - fy)
        + p(x1, y0) * fx * (1.0 - fy)
        + p(x0, y1) * (1.0 - fx) * fy
        + p(x1, y1) * fx * fy
}

/// Resample one camera's tile (`src_w` x `src_h`) into the rectified frame.
pub fn rectify_one(
    g: &RectGeom,
    which: usize,
    k: &Kb4,
    src: &[u8],
    src_w: usize,
    src_h: usize,
    mask_radius: f64,
) -> RectImage {
    let m = if which == 0 {
        g.a_from_rect
    } else {
        g.b_from_rect
    };
    let n = g.width * g.height;
    let mut px = vec![0f32; n];
    let mut valid = vec![false; n];
    for v in 0..g.height {
        for u in 0..g.width {
            let ray = geom::mat_vec(&m, g.rect_ray(u as f64, v as f64));
            let Some((su, sv)) = k.project(ray) else {
                continue;
            };
            let r = ((su - k.cx).powi(2) + (sv - k.cy).powi(2)).sqrt();
            if su < 0.0
                || sv < 0.0
                || su > (src_w - 1) as f64
                || sv > (src_h - 1) as f64
                || r > mask_radius
            {
                continue;
            }
            px[v * g.width + u] = bilinear(src, src_w, src_h, su, sv);
            valid[v * g.width + u] = true;
        }
    }
    RectImage { px, valid }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rays_and_pixels_invert() {
        let r = pimax_cam::geom::rodrigues([0.1, -0.9, 0.2]);
        let p = Pose {
            r,
            t: [-0.1, 0.007, -0.05],
        };
        let g = RectGeom::new(&p, 640, 480, 220.0);
        for which in 0..2 {
            for &(x, y) in &[(10.0, 20.0), (319.5, 239.5), (600.25, 470.75)] {
                let ray = g.ray(which, x, y);
                let (x2, y2) = g.pixel(which, ray).unwrap();
                assert!((x - x2).abs() < 1e-9 && (y - y2).abs() < 1e-9);
            }
        }
        // a point far away along the baseline-perpendicular direction lands on the same row in both
        let ra = g.ray(0, 300.0, 100.0);
        let far = geom::scale(ra, 1e6);
        let pb = p.apply(far);
        let (_, yb) = g.pixel(1, geom::normalize(pb)).unwrap();
        assert!((yb - 100.0).abs() < 1e-3, "{yb}");
        assert!((g.baseline - geom::norm(p.inverse().t)).abs() < 1e-12);
    }
}
