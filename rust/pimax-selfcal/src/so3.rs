//! Rotation helpers on top of `pimax_cam::geom` (rotation vectors, exp/log maps, skew matrices).

use pimax_cam::geom::{self, Mat3, Vec3};

/// `Exp(w)`: rotation matrix of a rotation vector (axis * angle, radians).
pub fn exp(w: Vec3) -> Mat3 {
    geom::rodrigues(w)
}

/// `Log(R)`: rotation vector of a rotation matrix (angle in `[0, pi]`).
///
/// The angle comes from `atan2(sin, cos)` (well conditioned everywhere, unlike `acos` near 0 and pi). The axis
/// comes from the antisymmetric part `v = 2 sin(theta) a`, except near pi where `v` vanishes: there it comes from
/// the symmetric part `(R + R^T) / 2 = cos(theta) I + (1 - cos(theta)) a a^T`, with the sign taken from `v`.
pub fn log(r: &Mat3) -> Vec3 {
    let tr = r[0][0] + r[1][1] + r[2][2];
    let v = [r[2][1] - r[1][2], r[0][2] - r[2][0], r[1][0] - r[0][1]];
    let s = geom::norm(v) / 2.0; // sin(theta) >= 0
    let c = (tr - 1.0) / 2.0; // cos(theta)
    let th = s.atan2(c);
    if th < 1e-7 {
        // first order: v / 2 (the error is O(theta^3))
        return geom::scale(v, 0.5);
    }
    if th < std::f64::consts::PI - 1e-3 {
        return geom::scale(v, th / (2.0 * s));
    }
    // near pi: a a^T = (sym - c I) / (1 - c); the column with the largest diagonal is the best conditioned
    let sym = |i: usize, j: usize| (r[i][j] + r[j][i]) / 2.0 - if i == j { c } else { 0.0 };
    let k = (0..3)
        .max_by(|&i, &j| sym(i, i).partial_cmp(&sym(j, j)).unwrap())
        .unwrap();
    let mut a = geom::normalize([sym(0, k), sym(1, k), sym(2, k)]);
    if geom::dot(a, v) < 0.0 {
        a = geom::scale(a, -1.0);
    }
    geom::scale(a, th)
}

/// `[v]_x`: the matrix with `[v]_x u = v x u`.
pub fn skew(v: Vec3) -> Mat3 {
    [[0.0, -v[2], v[1]], [v[2], 0.0, -v[0]], [-v[1], v[0], 0.0]]
}

/// `a * s` for matrices.
pub fn mat_scale(a: &Mat3, s: f64) -> Mat3 {
    let mut r = *a;
    for row in r.iter_mut() {
        for v in row.iter_mut() {
            *v *= s;
        }
    }
    r
}

/// Angle of a rotation vector in degrees.
pub fn angle_deg(w: Vec3) -> f64 {
    geom::norm(w).to_degrees()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn log_inverts_exp() {
        for w in [
            [0.0, 0.0, 0.0],
            [1e-9, -2e-9, 3e-10],
            [0.003, -0.001, 0.002],
            [0.4, -1.1, 0.7],
            [0.0, 3.0, 0.0],
            [2.0, 1.0, -1.5],
            [0.0, 0.0, std::f64::consts::PI],
            [0.0, std::f64::consts::PI - 1e-9, 0.0],
            [0.6, -0.8, 0.0].map(|x| x * (std::f64::consts::PI - 2e-4)),
            [-0.48, 0.6, 0.64].map(|x| x * (std::f64::consts::PI - 1e-6)),
        ] {
            let r = exp(w);
            let w2 = log(&r);
            let r2 = exp(w2);
            for i in 0..3 {
                for j in 0..3 {
                    assert!((r[i][j] - r2[i][j]).abs() < 1e-9, "w={w:?} w2={w2:?}");
                }
            }
            // unique below pi (at exactly pi, w and -w are the same rotation)
            if geom::norm(w) < std::f64::consts::PI - 1e-12 {
                assert!(geom::norm(geom::sub(w, w2)) < 1e-7, "w={w:?} w2={w2:?}");
            }
        }
    }

    #[test]
    fn skew_is_cross() {
        let (a, b) = ([0.3, -1.0, 2.0], [-0.5, 0.25, 4.0]);
        let c = geom::mat_vec(&skew(a), b);
        let d = geom::cross(a, b);
        assert!(geom::norm(geom::sub(c, d)) < 1e-15);
        assert!((angle_deg([0.0, 0.0, std::f64::consts::PI]) - 180.0).abs() < 1e-12);
        assert_eq!(mat_scale(&geom::IDENTITY, 2.0)[1][1], 2.0);
    }
}
