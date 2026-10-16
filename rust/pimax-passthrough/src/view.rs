//! Eye views: the pinhole image an OpenXR projection layer view describes (pose relative to the head, FOV
//! angles, pixel size). Pixel (0, 0) is the top-left corner (OpenXR / Vulkan image origin).

use pimax_cam::geom::{self, Pose, Vec3};

/// FOV in radians with OpenXR signs (`left` and `down` negative for a normal view).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fov {
    /// Angle of the left edge (negative).
    pub left: f64,
    /// Angle of the right edge.
    pub right: f64,
    /// Angle of the top edge.
    pub up: f64,
    /// Angle of the bottom edge (negative).
    pub down: f64,
}

impl Fov {
    /// From positive half angles in degrees (left, right, up, down), the `lens.conf` `fov` order.
    pub fn from_half_deg(l: f64, r: f64, u: f64, d: f64) -> Fov {
        Fov {
            left: -l.to_radians(),
            right: r.to_radians(),
            up: u.to_radians(),
            down: -d.to_radians(),
        }
    }

    /// The Dream Air default of `~/.config/pimax/lens.conf` (`fov = 50 50 44.5 44.5`).
    pub fn dream_air_default() -> Fov {
        Fov::from_half_deg(50.0, 50.0, 44.5, 44.5)
    }

    /// Positive half angles in degrees (left, right, up, down).
    pub fn half_deg(&self) -> [f64; 4] {
        [
            -self.left.to_degrees(),
            self.right.to_degrees(),
            self.up.to_degrees(),
            -self.down.to_degrees(),
        ]
    }
}

/// One eye.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeView {
    /// Eye (OpenXR view axes: x right, y up, -z forward) -> head.
    pub head_from_eye: Pose,
    /// Field of view.
    pub fov: Fov,
    /// Image width in pixels.
    pub width: usize,
    /// Image height in pixels.
    pub height: usize,
}

impl EyeView {
    /// The default eye pair: eyes at `∓ipd/2` on the head x axis, no rotation, same FOV for both.
    pub fn default_pair(width: usize, height: usize, ipd_m: f64, fov: Fov) -> [EyeView; 2] {
        let eye = |sx: f64| EyeView {
            head_from_eye: Pose {
                r: geom::IDENTITY,
                t: [sx * ipd_m / 2.0, 0.0, 0.0],
            },
            fov,
            width,
            height,
        };
        [eye(-1.0), eye(1.0)]
    }

    /// Ray (not normalised, z = -1) in the eye frame through the continuous pixel position `(x, y)`; pixel
    /// centres are at `i + 0.5`.
    pub fn ray(&self, x: f64, y: f64) -> Vec3 {
        let (tl, tr) = (self.fov.left.tan(), self.fov.right.tan());
        let (tu, td) = (self.fov.up.tan(), self.fov.down.tan());
        let u = x / self.width as f64;
        let v = y / self.height as f64;
        [tl + u * (tr - tl), tu - v * (tu - td), -1.0]
    }

    /// Inverse of [`EyeView::ray`]: continuous pixel position of an eye-frame direction (`None` if behind).
    pub fn pixel_of(&self, d: Vec3) -> Option<(f64, f64)> {
        if d[2] >= -1e-12 {
            return None;
        }
        let (tx, ty) = (d[0] / -d[2], d[1] / -d[2]);
        let (tl, tr) = (self.fov.left.tan(), self.fov.right.tan());
        let (tu, td) = (self.fov.up.tan(), self.fov.down.tan());
        Some((
            (tx - tl) / (tr - tl) * self.width as f64,
            (tu - ty) / (tu - td) * self.height as f64,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ray_and_pixel_round_trip() {
        let [l, r] = EyeView::default_pair(400, 432, 0.063, Fov::dream_air_default());
        assert!(l.head_from_eye.t[0] < 0.0 && r.head_from_eye.t[0] > 0.0);
        // centre pixel looks straight ahead (symmetric FOV)
        let c = l.ray(200.0, 216.0);
        assert!(c[0].abs() < 1e-12 && c[1].abs() < 1e-12 && c[2] == -1.0);
        // corners hit the FOV edges, top-left is left and up
        let tl = l.ray(0.0, 0.0);
        assert!((tl[0] - (-50f64).to_radians().tan()).abs() < 1e-12);
        assert!((tl[1] - 44.5f64.to_radians().tan()).abs() < 1e-12);
        for &(x, y) in &[(0.5, 0.5), (123.25, 400.75), (399.5, 10.0)] {
            let (x2, y2) = l.pixel_of(geom::scale(l.ray(x, y), 3.0)).unwrap();
            assert!((x - x2).abs() < 1e-9 && (y - y2).abs() < 1e-9);
        }
        assert!(l.pixel_of([0.0, 0.0, 1.0]).is_none());
        assert!((Fov::dream_air_default().half_deg()[3] - 44.5).abs() < 1e-12);
    }
}
