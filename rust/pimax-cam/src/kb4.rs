//! Kannala-Brandt fisheye model (OpenCV `fisheye`, Monado `T_DISTORTION_FISHEYE_KB4`).
//!
//! `theta = atan2(r, z)`, `theta_d = theta * (1 + k1 theta^2 + k2 theta^4 + k3 theta^6 + k4 theta^8 [+ k5 theta^10 + k6 theta^12])`,
//! `u = fx * theta_d * x / r + cx`, `v = fy * theta_d * y / r + cy` (OpenCV camera axes: x right, y down, z forward).
//! The factory XML says `FISHEYE_4_PARAMETERS` but lists 6 numbers; [`Kb4::k`] are the first four. The last two
//! are kept in [`Kb4::extra`] and can be evaluated as `k5, k6` with [`Kb4::with_extra_radial`] (used only to test
//! that hypothesis against real images).

use crate::geom::Vec3;

/// Intrinsics of one fisheye camera. Build with [`Kb4::new`] so that the angle limit is computed.
#[derive(Clone, Debug, PartialEq)]
pub struct Kb4 {
    /// Image width in pixels.
    pub width: u32,
    /// Image height in pixels.
    pub height: u32,
    /// Focal length x (pixels).
    pub fx: f64,
    /// Focal length y (pixels).
    pub fy: f64,
    /// Principal point x.
    pub cx: f64,
    /// Principal point y.
    pub cy: f64,
    /// k1..k4.
    pub k: [f64; 4],
    /// Further coefficients from the XML (unused by KB4).
    pub extra: Vec<f64>,
    /// Use `extra` as k5, k6 (radial theta^10, theta^12 terms). Hypothesis test only.
    pub use_extra: bool,
    /// Largest incidence angle (radians) where `theta_d` still increases; projections beyond it are rejected.
    pub theta_limit: f64,
}

impl Kb4 {
    /// New model; computes [`Kb4::theta_limit`].
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        width: u32,
        height: u32,
        fx: f64,
        fy: f64,
        cx: f64,
        cy: f64,
        k: [f64; 4],
        extra: Vec<f64>,
    ) -> Kb4 {
        let mut m = Kb4 {
            width,
            height,
            fx,
            fy,
            cx,
            cy,
            k,
            extra,
            use_extra: false,
            theta_limit: 0.0,
        };
        m.theta_limit = m.compute_theta_max();
        m
    }

    /// Copy that also applies the two extra coefficients as radial terms.
    pub fn with_extra_radial(&self) -> Kb4 {
        let mut m = Kb4 {
            use_extra: true,
            ..self.clone()
        };
        m.theta_limit = m.compute_theta_max();
        m
    }

    fn coeffs(&self) -> impl Iterator<Item = f64> + '_ {
        let n_extra = if self.use_extra { 2 } else { 0 };
        self.k
            .iter()
            .chain(self.extra.iter().take(n_extra))
            .copied()
    }

    /// Distorted angle for an incidence angle `theta` (radians).
    pub fn theta_d(&self, theta: f64) -> f64 {
        let t2 = theta * theta;
        let mut poly = 0.0;
        let mut p = t2;
        for k in self.coeffs() {
            poly += k * p;
            p *= t2;
        }
        theta * (1.0 + poly)
    }

    /// d theta_d / d theta.
    pub fn dtheta_d(&self, theta: f64) -> f64 {
        let t2 = theta * theta;
        let mut d = 1.0;
        let mut p = t2;
        let mut n = 3.0;
        for k in self.coeffs() {
            d += n * k * p;
            p *= t2;
            n += 2.0;
        }
        d
    }

    /// Largest incidence angle (radians, capped at 100 deg) for which `theta_d` is still increasing.
    pub fn compute_theta_max(&self) -> f64 {
        let step = 0.0005;
        let cap = 100f64.to_radians();
        let mut t = 0.0;
        while t < cap && self.dtheta_d(t + step) > 0.0 {
            t += step;
        }
        t
    }

    /// Project a 3D point (camera frame) to pixels. `None` for the origin, points behind or angles beyond
    /// [`Kb4::theta_limit`]. The result can lie outside the image; callers check bounds.
    pub fn project(&self, p: Vec3) -> Option<(f64, f64)> {
        let r = (p[0] * p[0] + p[1] * p[1]).sqrt();
        if r < 1e-12 {
            return (p[2] > 0.0).then_some((self.cx, self.cy));
        }
        let theta = r.atan2(p[2]);
        if theta > self.theta_limit {
            return None;
        }
        let td = self.theta_d(theta);
        Some((
            self.fx * td * p[0] / r + self.cx,
            self.fy * td * p[1] / r + self.cy,
        ))
    }

    /// Unit ray through pixel (u, v) (Newton iteration on theta_d). `None` outside the model's valid disc.
    pub fn unproject(&self, u: f64, v: f64) -> Option<Vec3> {
        let mx = (u - self.cx) / self.fx;
        let my = (v - self.cy) / self.fy;
        let td = (mx * mx + my * my).sqrt();
        if td < 1e-12 {
            return Some([0.0, 0.0, 1.0]);
        }
        let tmax = self.theta_limit;
        if td > self.theta_d(tmax) {
            return None;
        }
        let mut th = td.min(tmax);
        for _ in 0..50 {
            let f = self.theta_d(th) - td;
            let d = self.dtheta_d(th);
            if d.abs() < 1e-12 {
                break;
            }
            let step = f / d;
            th = (th - step).clamp(0.0, tmax);
            if step.abs() < 1e-13 {
                break;
            }
        }
        if (self.theta_d(th) - td).abs() > 1e-7 {
            return None;
        }
        let s = th.sin() / td;
        Some([mx * s, my * s, th.cos()])
    }

    /// Incidence angle of a pixel's ray in degrees.
    pub fn angle_of_pixel_deg(&self, u: f64, v: f64) -> Option<f64> {
        self.unproject(u, v)
            .map(|r| r[2].clamp(-1.0, 1.0).acos().to_degrees())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cam0() -> Kb4 {
        Kb4::new(
            640,
            480,
            234.7133343909,
            234.7063990438,
            325.3779138659,
            248.2829309197,
            [0.0589502951, -0.0603263194, 0.0320281449, -0.0082048556],
            vec![0.0011599741, -0.0001122173],
        )
    }

    #[test]
    fn roundtrip() {
        let c = cam0();
        for &(u, v) in &[
            (325.0, 248.0),
            (0.0, 248.0),
            (639.0, 248.0),
            (325.0, 0.0),
            (100.0, 400.0),
            (600.0, 60.0),
        ] {
            let r = c.unproject(u, v).expect("unproject");
            let (u2, v2) = c.project(r).expect("project");
            assert!(
                (u - u2).abs() < 1e-6 && (v - v2).abs() < 1e-6,
                "({u},{v}) -> ({u2},{v2})"
            );
        }
        let ce = c.with_extra_radial();
        let r = ce.unproject(10.0, 248.0).unwrap();
        let (u2, v2) = ce.project(r).unwrap();
        assert!((u2 - 10.0).abs() < 1e-6 && (v2 - 248.0).abs() < 1e-6);
    }

    #[test]
    fn image_corners_are_outside_the_model() {
        // max theta_d ~1.505 rad -> ~353 px from the principal point; the corners are ~390-410 px away.
        let c = cam0();
        assert!(c.unproject(0.0, 0.0).is_none());
        assert!(c.unproject(639.0, 479.0).is_none());
        let lim = c.theta_limit.to_degrees();
        assert!((90.0..95.0).contains(&lim), "theta limit {lim}");
    }

    #[test]
    fn fov_matches_camgeom() {
        // camgeom.py: trackingA HFOV ~155 deg over 640 px.
        let c = cam0();
        let l = c.angle_of_pixel_deg(0.0, c.cy).unwrap();
        let r = c.angle_of_pixel_deg(639.0, c.cy).unwrap();
        assert!((l + r - 155.0).abs() < 3.0, "hfov {}", l + r);
    }

    #[test]
    fn behind_camera_is_rejected() {
        let c = cam0();
        assert!(c.project([0.0, 0.0, -1.0]).is_none());
        assert!(c.project([1.0, 0.0, -1.0]).is_none());
        assert_eq!(c.project([0.0, 0.0, 1.0]), Some((c.cx, c.cy)));
    }
}
