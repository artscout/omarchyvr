//! Where the quad hangs in VIEW space (head-locked), and how big it is. Pure maths, angles in degrees.
//!
//! The quad is centred `down_deg` below the view centre at `distance` metres along that direction and turned to
//! face the head, so it keeps its apparent size and square pixels at any angle. The texture is 1600 x 400 pixels
//! (4:1) and the quad has the same aspect: with the default width of 1.6 m a pixel is 1 mm.

/// Texture size in pixels.
pub const TEX_W: u32 = 1600;
pub const TEX_H: u32 = 400;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Placement {
    /// Metres from the head to the quad centre.
    pub distance: f32,
    /// Quad width in metres (the height follows from the 4:1 texture).
    pub width: f32,
    /// Degrees below the view centre (positive = down).
    pub down_deg: f32,
}

impl Default for Placement {
    fn default() -> Placement {
        Placement {
            distance: 2.0,
            width: 1.6,
            down_deg: 20.0,
        }
    }
}

/// Pose and size of the quad layer: position and orientation (x, y, z, w) in VIEW space (x right, y up, -z forward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadPose {
    pub position: [f32; 3],
    pub orientation: [f32; 4],
    pub size: [f32; 2],
}

impl Placement {
    pub fn validate(&self) -> Result<(), String> {
        if !(0.3..=50.0).contains(&self.distance) {
            return Err(format!("--distance {} m is outside 0.3..50", self.distance));
        }
        if !(0.05..=30.0).contains(&self.width) {
            return Err(format!("--width {} m is outside 0.05..30", self.width));
        }
        if !(-80.0..=80.0).contains(&self.down_deg) {
            return Err(format!("--down-deg {} is outside -80..80", self.down_deg));
        }
        Ok(())
    }

    pub fn height(&self) -> f32 {
        self.width * TEX_H as f32 / TEX_W as f32
    }

    pub fn pose(&self) -> QuadPose {
        let a = self.down_deg.to_radians();
        let half = 0.5 * a;
        QuadPose {
            position: [0.0, -self.distance * a.sin(), -self.distance * a.cos()],
            // a rotation of -a about x turns the quad's normal (+z) towards the head: (0, sin a, cos a)
            orientation: [-half.sin(), 0.0, 0.0, half.cos()],
            size: [self.width, self.height()],
        }
    }

    /// Visual angle in degrees of `metres` on the quad, in the middle.
    pub fn angle_deg(&self, metres: f32) -> f32 {
        2.0 * (0.5 * metres / self.distance).atan().to_degrees()
    }

    /// Texture pixels per degree at the middle of the quad.
    pub fn px_per_deg(&self) -> f32 {
        TEX_W as f32 / self.angle_deg(self.width)
    }

    /// Elevation (degrees, positive up) of the top and bottom edge of the quad as seen from the head.
    pub fn edge_elevations_deg(&self) -> (f32, f32) {
        let q = self.pose();
        let h = 0.5 * q.size[1];
        let a = self.down_deg.to_radians();
        // the quad's up vector is (0, cos a, -sin a)
        let up = [0.0, a.cos(), -a.sin()];
        let edge = |s: f32| {
            let y = q.position[1] + s * h * up[1];
            let z = q.position[2] + s * h * up[2];
            y.atan2(-z).to_degrees()
        };
        (edge(1.0), edge(-1.0))
    }
}

/// Rotate `v` by the unit quaternion `q` (x, y, z, w).
pub fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (x, y, z, w) = (q[0], q[1], q[2], q[3]);
    // v' = v + 2w (u x v) + 2 u x (u x v), u = (x, y, z)
    let t = [
        2.0 * (y * v[2] - z * v[1]),
        2.0 * (z * v[0] - x * v[2]),
        2.0 * (x * v[1] - y * v[0]),
    ];
    [
        v[0] + w * t[0] + (y * t[2] - z * t[1]),
        v[1] + w * t[1] + (z * t[0] - x * t[2]),
        v[2] + w * t[2] + (x * t[1] - y * t[0]),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f32, b: f32, eps: f32) -> bool {
        (a - b).abs() <= eps
    }

    fn norm(v: [f32; 3]) -> f32 {
        (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt()
    }

    #[test]
    fn defaults_are_the_documented_ones() {
        let p = Placement::default();
        assert_eq!((p.distance, p.width, p.down_deg), (2.0, 1.6, 20.0));
        assert!(close(p.height(), 0.4, 1e-6));
        let q = p.pose();
        assert_eq!(q.size, [1.6, 0.4]);
        // 1 mm per texture pixel, square pixels
        assert!(close(p.width / TEX_W as f32, 0.001, 1e-9));
        assert!(close(
            p.width / TEX_W as f32,
            p.height() / TEX_H as f32,
            1e-9
        ));
    }

    #[test]
    fn straight_ahead_has_no_rotation() {
        let q = Placement {
            down_deg: 0.0,
            ..Placement::default()
        }
        .pose();
        assert!(close(q.position[0], 0.0, 1e-6));
        assert!(close(q.position[1], 0.0, 1e-6));
        assert!(close(q.position[2], -2.0, 1e-6));
        assert!(close(q.orientation[3], 1.0, 1e-6));
        assert!(close(q.orientation[0], 0.0, 1e-6));
    }

    #[test]
    fn centre_is_down_deg_below_the_view_axis_at_the_distance() {
        for (d, a) in [(2.0, 20.0), (1.0, 35.0), (3.0, 5.0), (2.0, -10.0)] {
            let p = Placement {
                distance: d,
                width: 1.6,
                down_deg: a,
            };
            let q = p.pose();
            assert!(close(norm(q.position), d, 1e-5), "{d} {a}");
            // angle below the forward axis (-z)
            let below = (-q.position[1]).atan2(-q.position[2]).to_degrees();
            assert!(close(below, a, 1e-3), "{a}: {below}");
            assert_eq!(q.position[0], 0.0);
            // below the horizon means negative y
            assert_eq!(q.position[1] < 0.0, a > 0.0);
        }
    }

    #[test]
    fn the_quad_faces_the_head() {
        let p = Placement::default();
        let q = p.pose();
        let nq = norm([q.orientation[0], q.orientation[1], q.orientation[2]]).powi(2)
            + q.orientation[3].powi(2);
        assert!(close(nq, 1.0, 1e-6), "unit quaternion");
        // the normal (+z) points from the quad back to the origin
        let n = rotate(q.orientation, [0.0, 0.0, 1.0]);
        let to_head = [
            -q.position[0] / p.distance,
            -q.position[1] / p.distance,
            -q.position[2] / p.distance,
        ];
        for i in 0..3 {
            assert!(close(n[i], to_head[i], 1e-5), "{n:?} vs {to_head:?}");
        }
        // the right edge stays horizontal and the top is up
        let right = rotate(q.orientation, [1.0, 0.0, 0.0]);
        assert!(close(right[0], 1.0, 1e-6) && close(right[1], 0.0, 1e-6));
        let up = rotate(q.orientation, [0.0, 1.0, 0.0]);
        assert!(up[1] > 0.9, "{up:?}");
    }

    #[test]
    fn angles_and_resolution_on_the_default_quad() {
        let p = Placement::default();
        // 1.6 m at 2 m is 43.6 degrees wide, 0.4 m is 11.4 degrees tall
        assert!(close(p.angle_deg(1.6), 43.6, 0.05), "{}", p.angle_deg(1.6));
        assert!(close(p.angle_deg(0.4), 11.42, 0.02));
        assert!(close(p.px_per_deg(), 1600.0 / 43.603, 0.1));
        // the quad spans 14.3 .. 25.7 degrees below the centre; it stays inside the lens (about 44 degrees down)
        let (top, bottom) = p.edge_elevations_deg();
        assert!(close(top, -14.3, 0.1), "{top}");
        assert!(close(bottom, -25.7, 0.1), "{bottom}");
        assert!(bottom > -44.0);
        // centred on the axis: symmetric
        let (t, b) = Placement { down_deg: 0.0, ..p }.edge_elevations_deg();
        assert!(close(t, -b, 1e-4) && close(t, 5.71, 0.01));
    }

    #[test]
    fn validation() {
        assert!(Placement::default().validate().is_ok());
        assert!(Placement {
            distance: 0.1,
            ..Placement::default()
        }
        .validate()
        .is_err());
        assert!(Placement {
            width: 0.0,
            ..Placement::default()
        }
        .validate()
        .is_err());
        assert!(Placement {
            down_deg: 95.0,
            ..Placement::default()
        }
        .validate()
        .is_err());
        assert!(Placement {
            down_deg: f32::NAN,
            ..Placement::default()
        }
        .validate()
        .is_err());
    }

    #[test]
    fn rotate_is_a_rotation() {
        // 90 degrees about y turns +x into -z
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let r = rotate([0.0, h, 0.0, h], [1.0, 0.0, 0.0]);
        assert!(close(r[0], 0.0, 1e-6) && close(r[2], -1.0, 1e-6), "{r:?}");
    }
}
