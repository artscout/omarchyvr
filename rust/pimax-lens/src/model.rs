//! The lens model: panel position -> client image UV, per colour channel.
//!
//! For a point (u, v) of the user-facing display (after the compositor's panel pre-rotation, v = 0 is the top),
//! `p = ((u-.5)*2*aspect, (.5-v)*2) - centre` in units of half the display height (+x right, +y up). Per colour
//! channel c: `p' = (p - shift_c) / ca_c`, `r2 = |p'|^2`, ray tangent
//! `t = base_c * scale * p' * (1 + k1 r2 + k2 r2^2 + k3 r2^3 + k4 r2^4 + k5 r2^5)`.
//! The client renders a pinhole image covering the configured FOV, so the sampled texture coordinate is the
//! linear map of the ray tangent into that image.
//!
//! With the optional features at their defaults (k4 = k5 = 0, equal `base`, no shift, no fit range, no mask) the
//! sequence of float operations is exactly the one of the C model this replaces, so the result is bit-identical
//! (`pimax-lens-difftest` checks this against the original C source).

use crate::config::{EyeConfig, LensConfig, MaskShape, CHANNELS, K_TERMS};
use crate::lut::Lut;
use std::sync::Arc;

/// `(float)(M_PI / 180.0)`, the constant of the C `DEG2RAD` macro.
pub const RAD_PER_DEG: f32 = (std::f64::consts::PI / 180.0) as f32;

/// How far (in client-image UV units) a fully hidden point is pushed out of the image. The compositor samples
/// the client image with a clamp-to-border-black sampler, so any UV outside [0,1] is black.
const PUSH: f32 = 4.0;

#[inline]
pub fn deg2rad(x: f32) -> f32 {
    x * RAD_PER_DEG
}

#[inline]
fn horner(k: &[f32; K_TERMS], r2: f32) -> f32 {
    k[0] + r2 * (k[1] + r2 * (k[2] + r2 * (k[3] + r2 * k[4])))
}

/// Tangent length and its slope at radius `r` for one channel (f64, used for the fit-range continuation, fits
/// and inverse queries).
pub fn radial_tangent(base: f64, scale: f64, k: &[f32; K_TERMS], r: f64) -> (f64, f64) {
    let r2 = r * r;
    let mut poly = 1.0;
    let mut dpoly = 1.0;
    let mut pw = 1.0;
    for (i, &ki) in k.iter().enumerate() {
        pw *= r2;
        poly += f64::from(ki) * pw;
        dpoly += (2 * (i + 1) + 1) as f64 * f64::from(ki) * pw;
    }
    (base * scale * r * poly, base * scale * dpoly)
}

fn smoothstep(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A lens configuration ready to evaluate.
#[derive(Clone, Debug)]
pub struct Lens {
    cfg: LensConfig,
    /// tan of the FOV half-angles per eye: left, right, up, down.
    tans: [[f32; 4]; 2],
    /// Exact distortion tables (config `lut`), per eye.
    luts: [Option<Arc<Lut>>; 2],
}

impl Lens {
    pub fn new(cfg: LensConfig) -> Lens {
        let mut tans = [[0.0; 4]; 2];
        for (e, t) in cfg.eye.iter().zip(tans.iter_mut()) {
            *t = e.fov.map(|d| deg2rad(d).tan());
        }
        Lens {
            cfg,
            tans,
            luts: [None, None],
        }
    }

    /// Attach the LUTs that replace the radial model (the config's FOV and centre must already match them).
    pub fn with_luts(mut self, luts: [Option<Arc<Lut>>; 2]) -> Lens {
        self.luts = luts;
        self
    }

    /// The LUT in use for `view`, if any.
    pub fn lut(&self, view: usize) -> Option<&Lut> {
        self.luts[view].as_deref()
    }

    /// Does `view` have a visibility mask (configured, or the visible area of a LUT)?
    pub fn has_mask(&self, view: usize) -> bool {
        self.cfg.eye[view].has_mask() || self.luts[view].is_some()
    }

    pub fn config(&self) -> &LensConfig {
        &self.cfg
    }

    pub fn eye(&self, view: usize) -> &EyeConfig {
        &self.cfg.eye[view]
    }

    /// FOV half-angles in degrees: left, right, up, down.
    pub fn fov_deg(&self, view: usize) -> [f32; 4] {
        self.cfg.eye[view].fov
    }

    /// tan of the FOV half-angles: left, right, up, down.
    pub fn fov_tan(&self, view: usize) -> [f32; 4] {
        self.tans[view]
    }

    /// Panel position (u, v of the user-facing display) -> position relative to the optical centre, half-heights.
    #[inline]
    fn centred(&self, e: &EyeConfig, u: f32, v: f32) -> (f32, f32) {
        let px = (u - 0.5) * 2.0 * self.cfg.aspect - e.cx;
        let py = (0.5 - v) * 2.0 - e.cy;
        (px, py)
    }

    /// Ray tangent (x right, y up) that channel `c` shows at panel point (u, v), before the visibility mask.
    pub fn tangent(&self, view: usize, c: usize, u: f32, v: f32) -> [f32; 2] {
        let e = &self.cfg.eye[view];
        if let Some(l) = &self.luts[view] {
            return l.sample(view, c, u, v, e.lut_interp);
        }
        let (px, py) = self.centred(e, u, v);
        self.tangent_at(e, c, px, py)
    }

    #[inline]
    pub(crate) fn tangent_at(&self, e: &EyeConfig, c: usize, px: f32, py: f32) -> [f32; 2] {
        let x = (px - e.shift[c][0]) / e.ca[c];
        let y = (py - e.shift[c][1]) / e.ca[c];
        let r2 = x * x + y * y;
        if e.fit_range > 0.0 && r2 > e.fit_range * e.fit_range {
            let r = r2.sqrt();
            let (t_end, slope) = radial_tangent(
                f64::from(e.base[c]),
                f64::from(e.scale),
                &e.k[c],
                f64::from(e.fit_range),
            );
            let t = (t_end + slope * (f64::from(r) - f64::from(e.fit_range))) as f32;
            return [x / r * t, y / r * t];
        }
        let d = e.base[c] * e.scale * (1.0 + r2 * horner(&e.k[c], r2));
        [x * d, y * d]
    }

    /// Ray tangent -> client image UV.
    #[inline]
    pub fn tangent_to_uv(&self, view: usize, t: [f32; 2]) -> [f32; 2] {
        let [tl, tr, tu, td] = self.tans[view];
        [(t[0] + tl) / (tl + tr), (tu - t[1]) / (tu + td)]
    }

    /// Client image UV -> ray tangent (inverse of [`Lens::tangent_to_uv`]).
    pub fn uv_to_tangent(&self, view: usize, uv: [f32; 2]) -> [f32; 2] {
        let [tl, tr, tu, td] = self.tans[view];
        [uv[0] * (tl + tr) - tl, tu - uv[1] * (tu + td)]
    }

    /// The distortion callback: for the display point (u, v) of `view` the UV to sample for R, G and B.
    pub fn compute(&self, view: usize, u: f32, v: f32) -> [[f32; 2]; CHANNELS] {
        if !self.cfg.enabled {
            return [[u, v]; CHANNELS];
        }
        let e = &self.cfg.eye[view];
        let (px, py) = self.centred(e, u, v);
        let mut out = [[0.0f32; 2]; CHANNELS];
        for (c, o) in out.iter_mut().enumerate() {
            let t = match &self.luts[view] {
                Some(l) => l.sample(view, c, u, v, e.lut_interp),
                None => self.tangent_at(e, c, px, py),
            };
            *o = self.tangent_to_uv(view, t);
        }
        if e.has_mask() {
            let hide = self.hide_amount(view, u, v);
            if hide > 0.0 {
                for o in out.iter_mut() {
                    let (dx, dy) = (o[0] - 0.5, o[1] - 0.5);
                    let len = (dx * dx + dy * dy).sqrt();
                    let (ux, uy) = if len > 1e-6 {
                        (dx / len, dy / len)
                    } else {
                        (1.0, 0.0)
                    };
                    o[0] += ux * hide * PUSH;
                    o[1] += uy * hide * PUSH;
                }
            }
        }
        out
    }

    /// Signed distance (half-heights, approximate, positive inside) from the visible area boundary at panel
    /// point (u, v). `f32::INFINITY` when the eye has no mask.
    pub fn mask_distance(&self, view: usize, u: f32, v: f32) -> f32 {
        let e = &self.cfg.eye[view];
        let aspect = self.cfg.aspect;
        let xp = (u - 0.5) * 2.0 * aspect;
        let yp = (0.5 - v) * 2.0;
        let mut s = f32::INFINITY;
        if e.blackout.iter().any(|&b| b > 0.0) {
            s = s.min((xp + aspect) - e.blackout[0]);
            s = s.min((aspect - xp) - e.blackout[1]);
            s = s.min((1.0 - yp) - e.blackout[2]);
            s = s.min((yp + 1.0) - e.blackout[3]);
        }
        let (px, py) = (xp - e.cx, yp - e.cy);
        let m = &e.mask;
        match m.shape {
            MaskShape::Off => {}
            MaskShape::Ellipse => {
                let f = ((px / m.a).powi(2) + (py / m.b).powi(2)).sqrt();
                s = s.min((1.0 - f) * m.a.min(m.b));
            }
            MaskShape::Rect => {
                s = s.min((m.a - px.abs()).min(m.b - py.abs()));
            }
            MaskShape::Superellipse => {
                let f = ((px / m.a).abs().powf(m.n) + (py / m.b).abs().powf(m.n)).powf(1.0 / m.n);
                s = s.min((1.0 - f) * m.a.min(m.b));
            }
        }
        s
    }

    /// 0 = fully visible, 1 = fully hidden, smooth in between (transition band `mask_feather` outside the boundary).
    pub fn hide_amount(&self, view: usize, u: f32, v: f32) -> f32 {
        let e = &self.cfg.eye[view];
        if !e.has_mask() {
            return 0.0;
        }
        let s = self.mask_distance(view, u, v);
        if s >= 0.0 {
            0.0
        } else if e.mask_feather <= 0.0 {
            1.0
        } else {
            smoothstep(-s / e.mask_feather)
        }
    }

    /// Panel radius (half-heights, from the optical centre, along any direction, ignoring `shift`) at which channel
    /// `c` shows the ray `angle_deg` degrees off the optical axis. `None` if the model never reaches that angle
    /// within a radius of 4.
    pub fn radius_for_angle(&self, view: usize, c: usize, angle_deg: f32) -> Option<f32> {
        let e = &self.cfg.eye[view];
        let target = f64::from(angle_deg.to_radians()).tan();
        let t_of = |r: f64| -> f64 {
            let rr = f64::from(e.fit_range);
            if e.fit_range > 0.0 && r > rr {
                let (t, s) = radial_tangent(f64::from(e.base[c]), f64::from(e.scale), &e.k[c], rr);
                t + s * (r - rr)
            } else {
                radial_tangent(f64::from(e.base[c]), f64::from(e.scale), &e.k[c], r).0
            }
        };
        let (mut lo, mut hi) = (0.0f64, 4.0f64);
        if t_of(hi) < target {
            return None;
        }
        for _ in 0..80 {
            let mid = 0.5 * (lo + hi);
            if t_of(mid) < target {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Some((0.5 * (lo + hi) * f64::from(e.ca[c])) as f32)
    }

    /// First radius (in units of the channel's own p') at which the tangent stops increasing, within `rmax`.
    /// `None` if the model is monotonic on [0, rmax]. The fit-range continuation is taken into account.
    pub fn first_fold(&self, view: usize, c: usize, rmax: f32) -> Option<f32> {
        let e = &self.cfg.eye[view];
        let n = 3000;
        for i in 1..=n {
            let r = f64::from(rmax) * i as f64 / n as f64;
            let rr = f64::from(e.fit_range);
            let slope = if e.fit_range > 0.0 && r > rr {
                radial_tangent(f64::from(e.base[c]), f64::from(e.scale), &e.k[c], rr).1
            } else {
                radial_tangent(f64::from(e.base[c]), f64::from(e.scale), &e.k[c], r).1
            };
            if slope <= 0.0 {
                return Some(r as f32);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse, Mask};

    const ASPECT: f32 = 3840.0 / 3552.0;

    fn lens_from(text: &str) -> Lens {
        let (cfg, w) = parse(text, ASPECT);
        assert!(w.is_empty(), "{w:?}");
        Lens::new(cfg)
    }

    #[test]
    fn disabled_is_identity() {
        let l = lens_from("distortion = off\n");
        assert_eq!(l.compute(0, 0.25, 0.75), [[0.25, 0.75]; 3]);
        assert_eq!(l.compute(1, 0.0, 1.0), [[0.0, 1.0]; 3]);
    }

    #[test]
    fn symmetric_centre_maps_to_image_centre() {
        let l = lens_from("center = 0 0\nca_r = 1\nca_b = 1\n");
        for c in l.compute(0, 0.5, 0.5) {
            assert!(
                (c[0] - 0.5).abs() < 1e-6 && (c[1] - 0.5).abs() < 1e-6,
                "{c:?}"
            );
        }
    }

    #[test]
    fn default_centre_offset_moves_the_axis() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        // the left eye optical axis is 0.0271 half-heights to the right of the panel centre, 0.009 up
        let u = 0.5 + 0.0271 / (2.0 * ASPECT);
        let v = 0.5 - 0.009 / 2.0;
        let g = l.compute(0, u, v)[1];
        assert!(
            (g[0] - 0.5).abs() < 1e-4 && (g[1] - 0.5).abs() < 1e-4,
            "{g:?}"
        );
    }

    #[test]
    fn mirror_symmetry_with_symmetric_parameters() {
        let l = lens_from("center = 0 0\n");
        let a = l.compute(0, 0.2, 0.3);
        let b = l.compute(0, 0.8, 0.3);
        for c in 0..3 {
            assert!((a[c][0] + b[c][0] - 1.0).abs() < 1e-5);
            assert!((a[c][1] - b[c][1]).abs() < 1e-6);
        }
        let d = l.compute(0, 0.2, 0.7);
        for c in 0..3 {
            assert!((a[c][0] - d[c][0]).abs() < 1e-6);
            assert!((a[c][1] + d[c][1] - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn ring_radii_of_the_default_fit_match_the_table_roughly() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        // "Dream Air like Pimax" table, green: 10 deg -> 0.25641, 30 -> 0.73145, 50 -> 1.08985 (half-heights)
        for (deg, r) in [(10.0, 0.25641), (30.0, 0.73145), (50.0, 1.08985)] {
            let got = l.radius_for_angle(0, 1, deg).unwrap();
            assert!((got - r).abs() < 0.012, "{deg}: {got} vs {r}");
        }
    }

    #[test]
    fn red_and_blue_are_scaled_by_ca() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        let g = l.radius_for_angle(0, 1, 40.0).unwrap();
        let r = l.radius_for_angle(0, 0, 40.0).unwrap();
        let b = l.radius_for_angle(0, 2, 40.0).unwrap();
        assert!(r > g && g > b, "{r} {g} {b}");
        assert!(((r / g) - 1.0045).abs() < 0.003);
    }

    #[test]
    fn default_polynomial_is_monotonic_to_the_corners() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        for c in 0..3 {
            assert_eq!(l.first_fold(0, c, 1.6), None);
        }
        // an obviously folding polynomial is detected
        let bad = lens_from("k = 0 -3 0\n");
        assert!(bad.first_fold(0, 1, 1.6).is_some());
    }

    #[test]
    fn fit_range_continues_with_the_tangent_line() {
        let mut cfg = LensConfig::defaults(ASPECT);
        cfg.eye[0].fit_range = 1.0;
        let l = Lens::new(cfg);
        let e = l.eye(0).clone();
        let (t1, s1) = radial_tangent(f64::from(e.base[1]), 1.0, &e.k[1], 1.0);
        // continuity at the fit range and linear growth beyond it
        let at = |r: f32| {
            let t = l.tangent_at(&e, 1, r, 0.0);
            f64::from(t[0])
        };
        // use ca = 1 channel (green): p' = p
        assert!((at(1.0) - t1).abs() < 1e-4);
        assert!(
            (at(1.3) - (t1 + s1 * 0.3)).abs() < 1e-3,
            "{} vs {}",
            at(1.3),
            t1 + s1 * 0.3
        );
        assert!((at(1.001) - at(1.0) - s1 * 0.001).abs() < 1e-3);
        // without a fit range the polynomial keeps rising much faster
        let plain = Lens::new(LensConfig::defaults(ASPECT));
        assert!(f64::from(plain.tangent_at(plain.eye(0), 1, 1.5, 0.0)[0]) > at(1.5) + 1.0);
    }

    #[test]
    fn extra_polynomial_terms_enter_as_higher_powers() {
        let mut cfg = LensConfig::defaults(ASPECT);
        cfg.eye[0].k[1] = [0.0, 0.0, 0.0, 0.0, 0.0];
        cfg.eye[0].k[1][3] = 1.0; // k4 r^8
        let l = Lens::new(cfg);
        let e = l.eye(0);
        let t = l.tangent_at(e, 1, 0.5, 0.0)[0];
        let want = e.base[1] * 0.5 * (1.0 + 0.5f32.powi(8));
        assert!((t - want).abs() < 1e-6);
    }

    #[test]
    fn visibility_mask_hides_outside_and_pushes_uv_out_of_range() {
        let mut cfg = LensConfig::defaults(ASPECT);
        cfg.eye[0].mask = Mask {
            shape: MaskShape::Ellipse,
            a: 1.0,
            b: 1.0,
            n: 2.0,
        };
        let l = Lens::new(cfg);
        // panel centre-ish: visible, unchanged by the mask
        assert_eq!(l.hide_amount(0, 0.5, 0.5), 0.0);
        let plain = Lens::new(LensConfig::defaults(ASPECT));
        assert_eq!(l.compute(0, 0.5, 0.5), plain.compute(0, 0.5, 0.5));
        // far right edge of the panel (x = 1.081 half-heights from the panel centre): outside the unit circle
        // by more than the feather -> hidden, UV pushed out of the image
        let (u, v) = (1.0, 0.5);
        assert_eq!(l.hide_amount(0, u, v), 1.0);
        for uv in l.compute(0, u, v) {
            assert!(
                uv[0] > 1.0 || uv[0] < 0.0 || uv[1] > 1.0 || uv[1] < 0.0,
                "{uv:?}"
            );
        }
        // inside the transition band the amount is strictly between 0 and 1
        let mut cfg2 = LensConfig::defaults(ASPECT);
        cfg2.eye[0].mask = Mask {
            shape: MaskShape::Rect,
            a: 1.0,
            b: 1.0,
            n: 2.0,
        };
        cfg2.eye[0].cx = 0.0;
        cfg2.eye[0].cy = 0.0;
        cfg2.eye[0].mask_feather = 0.1;
        let l2 = Lens::new(cfg2);
        // x = 1.05 half-heights is 0.05 outside the rect -> half way through the band
        let u = 0.5 + 1.05 / (2.0 * ASPECT);
        let h = l2.hide_amount(0, u, 0.5);
        assert!((h - 0.5).abs() < 0.02, "{h}");
    }

    #[test]
    fn blackout_margins_cut_the_panel_edges() {
        let l = lens_from("blackout = 0.2 0 0 0\nmask_feather = 0\n");
        // left margin of 0.2 half-heights = 0.2/(2*aspect) of the width
        let edge = 0.2 / (2.0 * ASPECT);
        assert_eq!(l.hide_amount(0, edge * 0.5, 0.5), 1.0);
        assert_eq!(l.hide_amount(0, edge * 1.5, 0.5), 0.0);
        // the other sides are untouched
        assert_eq!(l.hide_amount(0, 0.999, 0.5), 0.0);
        assert_eq!(l.hide_amount(0, 0.5, 0.001), 0.0);
        assert_eq!(l.hide_amount(0, 0.5, 0.999), 0.0);
        let all = lens_from("blackout = 0.1\nmask_feather = 0\n");
        for (u, v) in [(0.001, 0.5), (0.999, 0.5), (0.5, 0.001), (0.5, 0.999)] {
            assert_eq!(all.hide_amount(0, u, v), 1.0, "{u} {v}");
        }
        assert_eq!(all.hide_amount(0, 0.5, 0.5), 0.0);
    }

    #[test]
    fn uv_tangent_roundtrip() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        for uv in [[0.1, 0.9], [0.5, 0.5], [1.0, 0.0], [0.33, 0.77]] {
            let t = l.uv_to_tangent(1, uv);
            let b = l.tangent_to_uv(1, t);
            assert!((b[0] - uv[0]).abs() < 1e-5 && (b[1] - uv[1]).abs() < 1e-5);
        }
    }

    #[test]
    fn radius_for_angle_inverts_the_forward_model() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        for deg in [5.0f32, 25.0, 45.0] {
            for c in 0..3 {
                let r = l.radius_for_angle(0, c, deg).unwrap();
                // forward: a point at panel radius r along +x of the optical axis
                let e = l.eye(0);
                let t = l.tangent_at(e, c, r, 0.0)[0];
                assert!(
                    (t.atan().to_degrees() - deg).abs() < 0.02,
                    "{c} {deg}: {}",
                    t.atan().to_degrees()
                );
            }
        }
        assert!(l.radius_for_angle(0, 1, 89.99).is_some());
    }
}
