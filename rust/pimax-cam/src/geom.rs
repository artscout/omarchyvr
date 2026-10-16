//! Small 3D helpers (f64): vectors, 3x3 matrices, rigid poses, Rodrigues, quaternions.

/// 3-vector.
pub type Vec3 = [f64; 3];
/// Row-major 3x3 matrix.
pub type Mat3 = [[f64; 3]; 3];

/// Identity matrix.
pub const IDENTITY: Mat3 = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];

/// Dot product.
pub fn dot(a: Vec3, b: Vec3) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// Cross product.
pub fn cross(a: Vec3, b: Vec3) -> Vec3 {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

/// Euclidean norm.
pub fn norm(a: Vec3) -> f64 {
    dot(a, a).sqrt()
}

/// Unit vector (zero stays zero).
pub fn normalize(a: Vec3) -> Vec3 {
    let n = norm(a);
    if n == 0.0 {
        a
    } else {
        scale(a, 1.0 / n)
    }
}

/// `a * s`.
pub fn scale(a: Vec3, s: f64) -> Vec3 {
    [a[0] * s, a[1] * s, a[2] * s]
}

/// `a + b`.
pub fn add(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

/// `a - b`.
pub fn sub(a: Vec3, b: Vec3) -> Vec3 {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

/// `m * v`.
pub fn mat_vec(m: &Mat3, v: Vec3) -> Vec3 {
    [dot(m[0], v), dot(m[1], v), dot(m[2], v)]
}

/// `a * b`.
pub fn mat_mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    for (i, row) in r.iter_mut().enumerate() {
        for (j, v) in row.iter_mut().enumerate() {
            *v = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
        }
    }
    r
}

/// Transpose.
pub fn transpose(m: &Mat3) -> Mat3 {
    [
        [m[0][0], m[1][0], m[2][0]],
        [m[0][1], m[1][1], m[2][1]],
        [m[0][2], m[1][2], m[2][2]],
    ]
}

/// Column `j`.
pub fn col(m: &Mat3, j: usize) -> Vec3 {
    [m[0][j], m[1][j], m[2][j]]
}

/// Matrix from columns.
pub fn from_cols(a: Vec3, b: Vec3, c: Vec3) -> Mat3 {
    [[a[0], b[0], c[0]], [a[1], b[1], c[1]], [a[2], b[2], c[2]]]
}

/// Determinant.
pub fn det(m: &Mat3) -> f64 {
    dot(m[0], cross(m[1], m[2]))
}

/// Largest deviation of `m^T m` from the identity (0 for a perfect rotation).
pub fn orthonormality_error(m: &Mat3) -> f64 {
    let p = mat_mul(&transpose(m), m);
    let mut e: f64 = 0.0;
    for (i, row) in p.iter().enumerate() {
        for (j, v) in row.iter().enumerate() {
            e = e.max((v - if i == j { 1.0 } else { 0.0 }).abs());
        }
    }
    e
}

/// Rotation matrix from a rotation vector (axis * angle, radians).
pub fn rodrigues(w: Vec3) -> Mat3 {
    let th = norm(w);
    if th < 1e-12 {
        return IDENTITY;
    }
    let k = scale(w, 1.0 / th);
    let (s, c) = th.sin_cos();
    let kk = [[0.0, -k[2], k[1]], [k[2], 0.0, -k[0]], [-k[1], k[0], 0.0]];
    let kk2 = mat_mul(&kk, &kk);
    let mut r = IDENTITY;
    for i in 0..3 {
        for j in 0..3 {
            r[i][j] += s * kk[i][j] + (1.0 - c) * kk2[i][j];
        }
    }
    r
}

/// Rotation about X by `deg` degrees.
pub fn rot_x(deg: f64) -> Mat3 {
    let (s, c) = deg.to_radians().sin_cos();
    [[1.0, 0.0, 0.0], [0.0, c, -s], [0.0, s, c]]
}

/// Angle between two unit-ish vectors in degrees.
pub fn angle_deg(a: Vec3, b: Vec3) -> f64 {
    (dot(normalize(a), normalize(b)).clamp(-1.0, 1.0))
        .acos()
        .to_degrees()
}

/// Quaternion (x, y, z, w) of a rotation matrix.
pub fn quat_from_mat(m: &Mat3) -> [f64; 4] {
    let tr = m[0][0] + m[1][1] + m[2][2];
    let (x, y, z, w);
    if tr > 0.0 {
        let s = (tr + 1.0).sqrt() * 2.0;
        w = 0.25 * s;
        x = (m[2][1] - m[1][2]) / s;
        y = (m[0][2] - m[2][0]) / s;
        z = (m[1][0] - m[0][1]) / s;
    } else if m[0][0] > m[1][1] && m[0][0] > m[2][2] {
        let s = (1.0 + m[0][0] - m[1][1] - m[2][2]).sqrt() * 2.0;
        w = (m[2][1] - m[1][2]) / s;
        x = 0.25 * s;
        y = (m[0][1] + m[1][0]) / s;
        z = (m[0][2] + m[2][0]) / s;
    } else if m[1][1] > m[2][2] {
        let s = (1.0 + m[1][1] - m[0][0] - m[2][2]).sqrt() * 2.0;
        w = (m[0][2] - m[2][0]) / s;
        x = (m[0][1] + m[1][0]) / s;
        y = 0.25 * s;
        z = (m[1][2] + m[2][1]) / s;
    } else {
        let s = (1.0 + m[2][2] - m[0][0] - m[1][1]).sqrt() * 2.0;
        w = (m[1][0] - m[0][1]) / s;
        x = (m[0][2] + m[2][0]) / s;
        y = (m[1][2] + m[2][1]) / s;
        z = 0.25 * s;
    }
    let n = (x * x + y * y + z * z + w * w).sqrt();
    [x / n, y / n, z / n, w / n]
}

/// Rigid transform `p' = r * p + t`. Named `a_from_b`: maps coordinates in frame b into frame a.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    /// Rotation.
    pub r: Mat3,
    /// Translation.
    pub t: Vec3,
}

impl Pose {
    /// Identity transform.
    pub const IDENTITY: Pose = Pose {
        r: IDENTITY,
        t: [0.0; 3],
    };

    /// Apply to a point.
    pub fn apply(&self, p: Vec3) -> Vec3 {
        add(mat_vec(&self.r, p), self.t)
    }

    /// Inverse transform.
    pub fn inverse(&self) -> Pose {
        let rt = transpose(&self.r);
        Pose {
            r: rt,
            t: scale(mat_vec(&rt, self.t), -1.0),
        }
    }

    /// `self ∘ other` (apply `other` first): with `self = a_from_b`, `other = b_from_c` gives `a_from_c`.
    pub fn compose(&self, other: &Pose) -> Pose {
        Pose {
            r: mat_mul(&self.r, &other.r),
            t: add(mat_vec(&self.r, other.t), self.t),
        }
    }

    /// Origin of frame b expressed in frame a (for `self = a_from_b`).
    pub fn origin(&self) -> Vec3 {
        self.t
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn close(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() < eps
    }

    #[test]
    fn rodrigues_basic() {
        let r = rodrigues([0.0, 0.0, std::f64::consts::FRAC_PI_2]);
        let v = mat_vec(&r, [1.0, 0.0, 0.0]);
        assert!(close(v[0], 0.0, 1e-12) && close(v[1], 1.0, 1e-12));
        assert!(orthonormality_error(&r) < 1e-12);
        assert!(close(det(&r), 1.0, 1e-12));
    }

    #[test]
    fn pose_inverse_compose() {
        let a = Pose {
            r: rodrigues([0.1, -0.7, 0.3]),
            t: [0.1, 0.2, -0.3],
        };
        let i = a.compose(&a.inverse());
        assert!(orthonormality_error(&i.r) < 1e-12 && norm(i.t) < 1e-12);
        let p = [0.3, -1.0, 2.0];
        let q = a.inverse().apply(a.apply(p));
        assert!(norm(sub(p, q)) < 1e-12);
    }

    #[test]
    fn quaternion_roundtrip() {
        for w in [
            [0.1, 0.2, 0.3],
            [3.0, 0.0, 0.0],
            [0.0, -3.1, 0.2],
            [-0.07417, -2.57303, 0.62804],
        ] {
            let m = rodrigues(w);
            let q = quat_from_mat(&m);
            // rotate a vector with q and with m
            let v = [0.3, -0.5, 0.8];
            let (qx, qy, qz, qw) = (q[0], q[1], q[2], q[3]);
            let u = [qx, qy, qz];
            let uv = cross(u, v);
            let uuv = cross(u, uv);
            let rv = add(v, add(scale(uv, 2.0 * qw), scale(uuv, 2.0)));
            assert!(norm(sub(rv, mat_vec(&m, v))) < 1e-9, "w={w:?}");
        }
    }
}
