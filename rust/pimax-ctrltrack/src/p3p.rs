//! P3P: camera pose from three bearing vectors and three model points (Grunert's method, Haralick et al. 1994),
//! with a Durand-Kerner quartic solver polished by Newton steps, then the exact triad alignment.

use pimax_cam::geom::{self, Pose, Vec3};

#[derive(Clone, Copy, Debug)]
struct C {
    re: f64,
    im: f64,
}
impl C {
    fn add(self, o: C) -> C {
        C {
            re: self.re + o.re,
            im: self.im + o.im,
        }
    }
    fn sub(self, o: C) -> C {
        C {
            re: self.re - o.re,
            im: self.im - o.im,
        }
    }
    fn mul(self, o: C) -> C {
        C {
            re: self.re * o.re - self.im * o.im,
            im: self.re * o.im + self.im * o.re,
        }
    }
    fn div(self, o: C) -> C {
        let d = o.re * o.re + o.im * o.im;
        C {
            re: (self.re * o.re + self.im * o.im) / d,
            im: (self.im * o.re - self.re * o.im) / d,
        }
    }
}

/// Real roots of `c[0] x^4 + c[1] x^3 + c[2] x^2 + c[3] x + c[4]`.
pub fn quartic_real_roots(c: [f64; 5]) -> Vec<f64> {
    if c[0].abs() < 1e-14 * c.iter().map(|x| x.abs()).fold(0.0, f64::max) {
        return Vec::new();
    }
    let a: Vec<f64> = c.iter().map(|x| x / c[0]).collect();
    let eval = |z: C| {
        let mut r = C { re: 1.0, im: 0.0 };
        for k in a.iter().skip(1) {
            r = r.mul(z).add(C { re: *k, im: 0.0 });
        }
        r
    };
    let bound = 1.0 + a.iter().skip(1).map(|x| x.abs()).fold(0.0, f64::max);
    let seed = C { re: 0.4, im: 0.9 };
    let mut z = [C { re: 1.0, im: 0.0 }; 4];
    for i in 1..4 {
        z[i] = z[i - 1].mul(seed);
    }
    for zi in z.iter_mut() {
        *zi = C {
            re: zi.re * bound * 0.5,
            im: zi.im * bound * 0.5,
        };
    }
    for _ in 0..80 {
        let mut delta = 0.0f64;
        for i in 0..4 {
            let mut den = C { re: 1.0, im: 0.0 };
            for j in 0..4 {
                if i != j {
                    den = den.mul(z[i].sub(z[j]));
                }
            }
            if den.re == 0.0 && den.im == 0.0 {
                den = C { re: 1e-12, im: 0.0 };
            }
            let step = eval(z[i]).div(den);
            z[i] = z[i].sub(step);
            delta = delta.max(step.re.abs() + step.im.abs());
        }
        if delta < 1e-9 * bound {
            break;
        }
    }
    let p = |x: f64| a.iter().skip(1).fold(1.0, |r, k| r * x + k);
    let dp = |x: f64| 4.0 * x * x * x + 3.0 * a[1] * x * x + 2.0 * a[2] * x + a[3];
    let mut out = Vec::new();
    for zi in z {
        if zi.im.abs() <= 1e-6 * (1.0 + zi.re.abs()) {
            let mut x = zi.re;
            for _ in 0..4 {
                let d = dp(x);
                if d.abs() < 1e-300 {
                    break;
                }
                x -= p(x) / d;
            }
            out.push(x);
        }
    }
    out
}

/// Rigid transform that maps the triangle `m` onto the triangle `c` (exact for congruent triangles).
pub fn triad_align(m: [Vec3; 3], c: [Vec3; 3]) -> Option<Pose> {
    let frame = |p: [Vec3; 3]| {
        let e1 = geom::normalize(geom::sub(p[1], p[0]));
        let e3 = geom::cross(e1, geom::sub(p[2], p[0]));
        if geom::norm(e3) < 1e-12 {
            return None;
        }
        let e3 = geom::normalize(e3);
        let e2 = geom::cross(e3, e1);
        Some(geom::from_cols(e1, e2, e3))
    };
    let fm = frame(m)?;
    let fc = frame(c)?;
    let r = geom::mat_mul(&fc, &geom::transpose(&fm));
    let cm = geom::scale(geom::add(geom::add(m[0], m[1]), m[2]), 1.0 / 3.0);
    let cc = geom::scale(geom::add(geom::add(c[0], c[1]), c[2]), 1.0 / 3.0);
    Some(Pose {
        r,
        t: geom::sub(cc, geom::mat_vec(&r, cm)),
    })
}

/// All `cam_from_model` poses (up to 4) with `y[i]` (unit bearing in the camera) looking at `x[i]` (model points).
pub fn p3p(y: [Vec3; 3], x: [Vec3; 3]) -> Vec<Pose> {
    let y = y.map(geom::normalize);
    // side lengths: a opposite x0 (x1-x2), b opposite x1 (x0-x2), c opposite x2 (x0-x1)
    let a2 = {
        let d = geom::sub(x[1], x[2]);
        geom::dot(d, d)
    };
    let b2 = {
        let d = geom::sub(x[0], x[2]);
        geom::dot(d, d)
    };
    let c2 = {
        let d = geom::sub(x[0], x[1]);
        geom::dot(d, d)
    };
    if a2 < 1e-12 || b2 < 1e-12 || c2 < 1e-12 {
        return Vec::new();
    }
    let ca = geom::dot(y[1], y[2]); // cos alpha
    let cb = geom::dot(y[0], y[2]); // cos beta
    let cg = geom::dot(y[0], y[1]); // cos gamma
    let amc = (a2 - c2) / b2;
    let apc = (a2 + c2) / b2;
    let bmc = (b2 - c2) / b2;
    let bma = (b2 - a2) / b2;
    let c4 = (amc - 1.0).powi(2) - 4.0 * c2 / b2 * ca * ca;
    let c3 = 4.0 * (amc * (1.0 - amc) * cb - (1.0 - apc) * ca * cg + 2.0 * c2 / b2 * ca * ca * cb);
    let c2q = 2.0
        * (amc * amc - 1.0 + 2.0 * amc * amc * cb * cb + 2.0 * bmc * ca * ca
            - 4.0 * apc * ca * cb * cg
            + 2.0 * bma * cg * cg);
    let c1 = 4.0 * (-amc * (1.0 + amc) * cb + 2.0 * a2 / b2 * cg * cg * cb - (1.0 - apc) * ca * cg);
    let c0 = (1.0 + amc).powi(2) - 4.0 * a2 / b2 * cg * cg;
    let mut out = Vec::new();
    for v in quartic_real_roots([c4, c3, c2q, c1, c0]) {
        if v <= 0.0 {
            continue;
        }
        let den = 2.0 * (cg - v * ca);
        if den.abs() < 1e-12 {
            continue;
        }
        let u = ((-1.0 + amc) * v * v - 2.0 * amc * cb * v + 1.0 + amc) / den;
        if u <= 0.0 {
            continue;
        }
        let s1sq = c2 / (1.0 + u * u - 2.0 * u * cg);
        if s1sq <= 0.0 {
            continue;
        }
        let s1 = s1sq.sqrt();
        let pc = [
            geom::scale(y[0], s1),
            geom::scale(y[1], u * s1),
            geom::scale(y[2], v * s1),
        ];
        if let Some(p) = triad_align(x, pc) {
            out.push(p);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quartic_known_roots() {
        // (x-1)(x+2)(x-3)(x^2... ) -> use (x-1)(x+2)(x-3)(x-0.5)
        let r = [1.0, -2.0, 3.0, 0.5];
        // expand
        let mut c = vec![1.0];
        for ri in r {
            let mut n = vec![0.0; c.len() + 1];
            for (i, ci) in c.iter().enumerate() {
                n[i] += ci;
                n[i + 1] -= ci * ri;
            }
            c = n;
        }
        let mut got = quartic_real_roots([c[0], c[1], c[2], c[3], c[4]]);
        got.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let want = [-2.0, 0.5, 1.0, 3.0];
        for (g, w) in got.iter().zip(want) {
            assert!((g - w).abs() < 1e-9, "{got:?}");
        }
    }

    #[test]
    fn recovers_pose() {
        let x = [
            [0.03, -0.01, -0.016],
            [0.029, 0.001, -0.024],
            [0.044, 0.019, -0.010],
        ];
        let truth = Pose {
            r: geom::mat_mul(&geom::rodrigues([0.3, -0.5, 0.2]), &geom::rot_x(20.0)),
            t: [0.05, -0.1, 0.45],
        };
        let y = x.map(|p| geom::normalize(truth.apply(p)));
        let sols = p3p(y, x);
        assert!(!sols.is_empty());
        let best = sols
            .iter()
            .map(|s| {
                let mut e = geom::norm(geom::sub(s.t, truth.t));
                for i in 0..3 {
                    for j in 0..3 {
                        e += (s.r[i][j] - truth.r[i][j]).abs();
                    }
                }
                e
            })
            .fold(f64::MAX, f64::min);
        assert!(best < 1e-6, "best error {best}, {} solutions", sols.len());
    }
}
