//! View geometry: the mapping between pixels of an eye image and ray tangents, derived from the per-eye FOV the
//! runtime reports (OpenXR convention: angles in radians, `left` and `down` negative).
//!
//! An eye image is a pinhole projection: pixel (x, y) (centre at +0.5) sees the ray with tangents
//! `tx = -tl + (x+.5)/W * (tl+tr)` (to the right) and `ty = tu - (y+.5)/H * (tu+td)` (up), where `tl, tr, tu, td` are
//! the tangents of the four half-angles. The optical axis is the ray (0, 0): with an asymmetric FOV it is not at the
//! image centre.

/// Per-eye FOV in OpenXR convention (radians, `left` and `down` are negative).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Fov {
    pub left: f32,
    pub right: f32,
    pub up: f32,
    pub down: f32,
}

impl Fov {
    /// From the four positive half-angles in degrees: left, right, up, down.
    pub fn from_half_angles_deg(l: f32, r: f32, u: f32, d: f32) -> Fov {
        Fov {
            left: -l.to_radians(),
            right: r.to_radians(),
            up: u.to_radians(),
            down: -d.to_radians(),
        }
    }

    /// Positive half-angles in degrees: left, right, up, down.
    pub fn half_angles_deg(&self) -> [f32; 4] {
        [
            -self.left.to_degrees(),
            self.right.to_degrees(),
            self.up.to_degrees(),
            -self.down.to_degrees(),
        ]
    }

    /// Tangents of the half-angles, all positive: left, right, up, down.
    pub fn tangents(&self) -> [f32; 4] {
        [
            (-self.left).tan(),
            self.right.tan(),
            self.up.tan(),
            (-self.down).tan(),
        ]
    }

    /// Tangent-plane coordinates (right, up) of the centre of pixel (x, y) of a `w` x `h` image.
    pub fn pixel_to_tangent(&self, w: u32, h: u32, x: u32, y: u32) -> (f32, f32) {
        let [tl, tr, tu, td] = self.tangents();
        let tx = -tl + (x as f32 + 0.5) / w as f32 * (tl + tr);
        let ty = tu - (y as f32 + 0.5) / h as f32 * (tu + td);
        (tx, ty)
    }

    /// Continuous pixel position (pixel centres at +0.5) of tangent-plane point (tx, ty).
    pub fn tangent_to_pixel(&self, w: u32, h: u32, tx: f32, ty: f32) -> (f32, f32) {
        let [tl, tr, tu, td] = self.tangents();
        (
            (tx + tl) / (tl + tr) * w as f32,
            (tu - ty) / (tu + td) * h as f32,
        )
    }

    /// Size of one pixel in tangent units (horizontal; the image is square-pixel by construction of the runtime's
    /// recommended size, vertical is within a few percent).
    pub fn tangent_per_pixel(&self, w: u32) -> f32 {
        let [tl, tr, _, _] = self.tangents();
        (tl + tr) / w as f32
    }

    /// Pixel position of the optical axis (tangent 0, 0).
    pub fn principal_point(&self, w: u32, h: u32) -> (f32, f32) {
        self.tangent_to_pixel(w, h, 0.0, 0.0)
    }
}

/// Angle in degrees between the ray with tangents (tx, ty) and the optical axis.
pub fn angle_from_axis_deg(tx: f32, ty: f32) -> f32 {
    tx.hypot(ty).atan().to_degrees()
}

/// Azimuth and elevation in degrees of a ray (azimuth to the right, elevation up, elevation measured from the
/// horizontal plane).
pub fn az_el_deg(tx: f32, ty: f32) -> (f32, f32) {
    let az = tx.atan();
    let el = (ty / (1.0 + tx * tx).sqrt()).atan();
    (az.to_degrees(), el.to_degrees())
}

/// Tangents (right, up) of the ray at azimuth/elevation (degrees); inverse of [`az_el_deg`].
pub fn tangent_from_az_el(az_deg: f32, el_deg: f32) -> (f32, f32) {
    let tx = az_deg.to_radians().tan();
    let ty = el_deg.to_radians().tan() * (1.0 + tx * tx).sqrt();
    (tx, ty)
}

/// Row-major 3x3 rotation matrix.
pub type Mat3 = [[f32; 3]; 3];
pub const IDENT3: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

pub fn mat_vec(m: &Mat3, v: [f32; 3]) -> [f32; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = (0..3).map(|k| a[i][k] * b[k][j]).sum();
        }
    }
    o
}

pub fn transpose(m: &Mat3) -> Mat3 {
    let mut o = [[0.0; 3]; 3];
    for (i, row) in o.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = m[j][i];
        }
    }
    o
}

/// Rotation about +Y (up) by `deg` (positive = counter-clockwise seen from above, i.e. turning left).
pub fn rot_y(deg: f32) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// Rotation matrix of the quaternion `[x, y, z, w]`.
pub fn quat_to_mat(q: [f32; 4]) -> Mat3 {
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let [x, y, z, w] = if n > 0.0 {
        q.map(|v| v / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    };
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - z * w),
            2.0 * (x * z + y * w),
        ],
        [
            2.0 * (x * y + z * w),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - x * w),
        ],
        [
            2.0 * (x * z - y * w),
            2.0 * (y * z + x * w),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Normalised linear mean of two orientations (the head orientation from the two eye views).
pub fn quat_mean(a: [f32; 4], b: [f32; 4]) -> [f32; 4] {
    let dot: f32 = a.iter().zip(&b).map(|(x, y)| x * y).sum();
    let sg = if dot < 0.0 { -1.0 } else { 1.0 };
    let m = [0, 1, 2, 3].map(|i| a[i] + sg * b[i]);
    let n = m.iter().map(|v| v * v).sum::<f32>().sqrt();
    if n > 0.0 {
        m.map(|v| v / n)
    } else {
        [0.0, 0.0, 0.0, 1.0]
    }
}

/// Rotation of each eye view relative to the head (eye space -> head space), the head being the mean orientation
/// of the two views (OpenXR view poses, same space). Rounded to 1e-5 so that float noise does not look like change.
pub fn eyes_to_head(ql: [f32; 4], qr: [f32; 4]) -> [Mat3; 2] {
    let head = transpose(&quat_to_mat(quat_mean(ql, qr)));
    [ql, qr].map(|q| {
        let mut m = mat_mul(&head, &quat_to_mat(q));
        for v in m.iter_mut().flatten() {
            *v = (*v * 1e5).round() / 1e5;
        }
        m
    })
}

/// Tangents (right, up) of the direction `v` (looking down -Z).
pub fn dir_to_tangent(v: [f32; 3]) -> (f32, f32) {
    (v[0] / -v[2], v[1] / -v[2])
}

/// Yaw in degrees of the forward direction (-Z) of a rotation (positive = turned left).
pub fn yaw_deg(m: &Mat3) -> f32 {
    let f = mat_vec(m, [0.0, 0.0, -1.0]);
    (-f[0]).atan2(-f[2]).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sym() -> Fov {
        Fov::from_half_angles_deg(50.0, 50.0, 44.5, 44.5)
    }

    #[test]
    fn half_angles_roundtrip() {
        let f = Fov::from_half_angles_deg(40.0, 50.0, 30.0, 45.0);
        let d = f.half_angles_deg();
        for (a, b) in d.iter().zip([40.0, 50.0, 30.0, 45.0]) {
            assert!((a - b).abs() < 1e-4, "{d:?}");
        }
        assert!(f.left < 0.0 && f.down < 0.0 && f.right > 0.0 && f.up > 0.0);
    }

    #[test]
    fn pixel_tangent_roundtrip_symmetric_and_asymmetric() {
        for f in [sym(), Fov::from_half_angles_deg(40.0, 55.0, 30.0, 47.0)] {
            for (x, y) in [(0, 0), (10, 20), (99, 79), (50, 40)] {
                let (tx, ty) = f.pixel_to_tangent(100, 80, x, y);
                let (px, py) = f.tangent_to_pixel(100, 80, tx, ty);
                assert!(
                    (px - (x as f32 + 0.5)).abs() < 1e-3 && (py - (y as f32 + 0.5)).abs() < 1e-3
                );
            }
        }
    }

    #[test]
    fn image_edges_are_the_fov_tangents() {
        let f = Fov::from_half_angles_deg(40.0, 55.0, 30.0, 47.0);
        let [tl, tr, tu, td] = f.tangents();
        let (tx0, ty0) = f.pixel_to_tangent(1000, 800, 0, 0);
        let (tx1, ty1) = f.pixel_to_tangent(1000, 800, 999, 799);
        assert!((tx0 + tl).abs() < (tl + tr) / 1000.0);
        assert!((tx1 - tr).abs() < (tl + tr) / 1000.0);
        assert!((ty0 - tu).abs() < (tu + td) / 800.0);
        assert!((ty1 + td).abs() < (tu + td) / 800.0);
    }

    #[test]
    fn principal_point_is_off_centre_for_asymmetric_fov() {
        let (px, py) = sym().principal_point(1000, 800);
        assert!((px - 500.0).abs() < 1e-2 && (py - 400.0).abs() < 1e-2);
        let f = Fov::from_half_angles_deg(40.0, 60.0, 30.0, 50.0);
        let (px, py) = f.principal_point(1000, 800);
        assert!(px < 500.0 && py < 400.0, "{px} {py}"); // less FOV to the left and up: axis is nearer the left/top
        assert!(
            (f.tangent_per_pixel(1000)
                - (40f32.to_radians().tan() + 60f32.to_radians().tan()) / 1000.0)
                .abs()
                < 1e-7
        );
    }

    #[test]
    fn angle_helpers() {
        assert!((angle_from_axis_deg(1.0, 0.0) - 45.0).abs() < 1e-4);
        assert!((angle_from_axis_deg(0.0, 3f32.sqrt()) - 60.0).abs() < 1e-3);
        let (az, el) = az_el_deg(1.0, 0.0);
        assert!((az - 45.0).abs() < 1e-4 && el.abs() < 1e-6);
        for (a, e) in [(0.0f32, 0.0f32), (30.0, 10.0), (-20.0, -35.0), (45.0, 5.0)] {
            let (tx, ty) = tangent_from_az_el(a, e);
            let (a2, e2) = az_el_deg(tx, ty);
            assert!(
                (a - a2).abs() < 1e-3 && (e - e2).abs() < 1e-3,
                "{a} {e} -> {a2} {e2}"
            );
        }
    }

    #[test]
    fn head_forward_offset_follows_the_view_yaw() {
        // left view yawed +c (outward), right -c: head forward is at +tan(c) in the left eye image, -tan(c) in the right
        for c in [0.0f32, 2.0, 3.5, -1.0] {
            let h = c.to_radians() * 0.5;
            let ql = [0.0, h.sin(), 0.0, h.cos()];
            let qr = [0.0, -h.sin(), 0.0, h.cos()];
            let m = eyes_to_head(ql, qr);
            // eye-space direction of head forward = m^T * (0,0,-1)
            let l = mat_vec(&transpose(&m[0]), [0.0, 0.0, -1.0]);
            let r = mat_vec(&transpose(&m[1]), [0.0, 0.0, -1.0]);
            let (tl, _) = dir_to_tangent(l);
            let (tr, _) = dir_to_tangent(r);
            assert!((tl - c.to_radians().tan()).abs() < 2e-5, "{c}: {tl}");
            assert!((tr + c.to_radians().tan()).abs() < 2e-5, "{c}: {tr}");
            // and the eye axis seen from the head is rotated the other way
            let a = dir_to_tangent(mat_vec(&m[0], [0.0, 0.0, -1.0]));
            assert!((a.0 + c.to_radians().tan()).abs() < 2e-5);
        }
        // identical orientations: identity
        let q = [0.0, 0.2588, 0.0, 0.9659];
        let m = eyes_to_head(q, q);
        for k in 0..3 {
            for j in 0..3 {
                assert!((m[0][k][j] - IDENT3[k][j]).abs() < 1e-4);
            }
        }
    }

    #[test]
    fn matrix_helpers() {
        let m = rot_y(30.0);
        let i = mat_mul(&m, &transpose(&m));
        for k in 0..3 {
            for j in 0..3 {
                assert!((i[k][j] - IDENT3[k][j]).abs() < 1e-6);
            }
        }
        assert!((yaw_deg(&m) - 30.0).abs() < 1e-4);
        let q = quat_to_mat([
            0.0,
            (15f32.to_radians()).sin(),
            0.0,
            (15f32.to_radians()).cos(),
        ]);
        assert!((yaw_deg(&q) - 30.0).abs() < 1e-3);
    }
}
