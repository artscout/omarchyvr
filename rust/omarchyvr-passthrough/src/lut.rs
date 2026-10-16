//! Per-eye remap table: for every eye pixel, up to two camera samples and a blend weight.
//!
//! Geometry per eye pixel `(x, y)` (see `analysis/passthrough-design.md`, section 3):
//! ```text
//! d_eye  = ray(x + 0.5, y + 0.5) / |ray|                    unit ray, eye frame
//! P_head = head_from_eye · (D · d_eye)                      point on the sphere of radius D around the eye
//!          (D = inf: only the direction R_head_from_eye · d_eye is used)
//! X_cam  = cam_from_head · P_head                           OpenCV camera axes
//! (u, v) = KB4(X_cam)                                       raw tile pixel
//! ```
//! Camera choice: the eye's [`crate::rig::Rig::preference`] order. A camera's sample weight is 1 inside its
//! usable image and falls linearly to 0 over `feather_px` towards the image border or the vignetting circle
//! (`max_radius_px` from the principal point). The first camera with weight > 0 is sample A; if its weight `wa`
//! is below 1, the other camera with the largest weight `wb` is sample B and the two are cross-faded with
//! `alpha = wa / (wa + (1 - wa) wb)` (continuous where either weight reaches 0). Without a B the image fades to
//! black.

use crate::rig::Rig;
use crate::view::EyeView;
use pimax_cam::geom::{self, Vec3};

/// "No camera" marker in [`Tap::tile_a`] / [`Tap::tile_b`].
pub const NONE: u8 = 0xff;
/// Fixed-point scale of the stored camera coordinates (1/64 px).
pub const FP: f64 = 64.0;

/// One eye pixel's lookup.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Tap {
    /// Tile of sample A or [`NONE`].
    pub tile_a: u8,
    /// Tile of sample B or [`NONE`].
    pub tile_b: u8,
    /// Weight of A (0..=255); B gets `255 - w` if present, otherwise the pixel fades to black.
    pub w: u8,
    /// Padding.
    pub pad: u8,
    /// A: x in 1/64 px.
    pub ua: u16,
    /// A: y in 1/64 px.
    pub va: u16,
    /// B: x in 1/64 px.
    pub ub: u16,
    /// B: y in 1/64 px.
    pub vb: u16,
}

/// Parameters of the map.
#[derive(Clone, Debug, PartialEq)]
pub struct LutParams {
    /// Reprojection distance in metres (`f64::INFINITY` = rotation only).
    pub depth_m: f64,
    /// Camera preference; `None` = [`Rig::preference`] for the eye.
    pub order: Option<Vec<usize>>,
    /// Width of the cross-fade band in camera pixels.
    pub feather_px: f64,
    /// Usable radius around the principal point (the lens vignettes beyond it).
    pub max_radius_px: f64,
    /// Pixels kept clear of the tile border (bilinear needs 1).
    pub border_px: f64,
    /// Worker threads (0 = available parallelism).
    pub threads: usize,
}

impl Default for LutParams {
    fn default() -> LutParams {
        LutParams {
            depth_m: 2.0,
            order: None,
            feather_px: 24.0,
            max_radius_px: 330.0,
            border_px: 2.0,
            threads: 0,
        }
    }
}

/// The map of one eye.
#[derive(Clone, Debug, PartialEq)]
pub struct Lut {
    /// Eye image width.
    pub width: usize,
    /// Eye image height.
    pub height: usize,
    /// Row-major taps.
    pub taps: Vec<Tap>,
    /// Camera order used.
    pub order: Vec<usize>,
    /// Pixels whose sample A comes from tile i (index 4: no camera).
    pub primary_count: [usize; 5],
    /// Pixels with a cross-fade (B present).
    pub blended: usize,
}

/// Number of worker threads for `requested` (0 = all cores, capped at 16).
pub fn threads(requested: usize) -> usize {
    if requested > 0 {
        return requested;
    }
    std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16)
}

/// Run `f(first_row, rows_slice)` over `height` rows of `row_len` items, split across `nthreads`.
pub fn par_rows<T: Send, F: Fn(usize, &mut [T]) + Sync>(
    data: &mut [T],
    row_len: usize,
    nthreads: usize,
    f: F,
) {
    let height = data.len() / row_len.max(1);
    let n = nthreads.clamp(1, height.max(1));
    let rows_per = height.div_ceil(n).max(1);
    std::thread::scope(|s| {
        for (i, chunk) in data.chunks_mut(rows_per * row_len).enumerate() {
            let f = &f;
            s.spawn(move || f(i * rows_per, chunk));
        }
    });
}

/// Weight (0..1) of a camera sample at `(u, v)`; 0 outside the usable image.
pub fn sample_weight(rig: &Rig, cam: usize, p: &LutParams, u: f64, v: f64) -> f64 {
    let k = &rig.cameras[cam].intr;
    let (w, h) = (k.width as f64, k.height as f64);
    let b = p.border_px.max(1.0);
    let edge = (u - b).min(w - 1.0 - b - u).min(v - b).min(h - 1.0 - b - v);
    let r = ((u - k.cx).powi(2) + (v - k.cy).powi(2)).sqrt();
    let d = edge.min(p.max_radius_px - r);
    if d <= 0.0 {
        0.0
    } else {
        (d / p.feather_px.max(1e-6)).min(1.0)
    }
}

/// Camera pixel of the eye-frame unit ray `d_eye` (see the module docs), or `None`.
pub fn project_ray(
    rig: &Rig,
    cam: usize,
    eye: &EyeView,
    d_eye: Vec3,
    depth_m: f64,
) -> Option<(f64, f64)> {
    let c = &rig.cameras[cam];
    let x_cam = if depth_m.is_finite() {
        let p_head = eye.head_from_eye.apply(geom::scale(d_eye, depth_m));
        c.cam_from_head.apply(p_head)
    } else {
        let dir_head = geom::mat_vec(&eye.head_from_eye.r, d_eye);
        geom::mat_vec(&c.cam_from_head.r, dir_head)
    };
    c.intr.project(x_cam)
}

fn to_fp(x: f64) -> u16 {
    (x * FP).round().clamp(0.0, u16::MAX as f64) as u16
}

impl Lut {
    /// Build the map of eye `eye_index` (0 left, 1 right; only used for the default camera order).
    pub fn build(rig: &Rig, eye: &EyeView, eye_index: usize, p: &LutParams) -> Lut {
        let order: Vec<usize> = p
            .order
            .clone()
            .unwrap_or_else(|| rig.preference(eye_index))
            .into_iter()
            .filter(|&c| c < rig.cameras.len() && c < NONE as usize)
            .collect();
        let (w, h) = (eye.width, eye.height);
        let mut taps = vec![Tap::default(); w * h];
        par_rows(&mut taps, w, threads(p.threads), |y0, rows| {
            for (i, tap) in rows.iter_mut().enumerate() {
                let (x, y) = (i % w, y0 + i / w);
                let d = geom::normalize(eye.ray(x as f64 + 0.5, y as f64 + 0.5));
                let mut t = Tap {
                    tile_a: NONE,
                    tile_b: NONE,
                    ..Tap::default()
                };
                // A: first camera in preference order that sees the point; B: the best of the others
                let mut a: Option<(usize, f64, f64, f64)> = None;
                let mut b: Option<(usize, f64, f64, f64)> = None;
                for &cam in &order {
                    let Some((u, v)) = project_ray(rig, cam, eye, d, p.depth_m) else {
                        continue;
                    };
                    let wt = sample_weight(rig, cam, p, u, v);
                    if wt <= 0.0 {
                        continue;
                    }
                    match a {
                        None => {
                            a = Some((cam, wt, u, v));
                            if wt >= 1.0 {
                                break;
                            }
                        }
                        Some(_) => {
                            if !matches!(b, Some((_, wb, _, _)) if wb >= wt) {
                                b = Some((cam, wt, u, v));
                            }
                        }
                    }
                }
                if let Some((cam, wa, u, v)) = a {
                    t.tile_a = cam as u8;
                    t.ua = to_fp(u);
                    t.va = to_fp(v);
                    let alpha = match b {
                        Some((cb, wb, ub, vb)) => {
                            t.tile_b = cb as u8;
                            t.ub = to_fp(ub);
                            t.vb = to_fp(vb);
                            // normalised cross-fade: continuous where B's own weight goes to 0
                            wa / (wa + (1.0 - wa) * wb)
                        }
                        None => wa,
                    };
                    t.w = (alpha * 255.0).round() as u8;
                }
                *tap = t;
            }
        });
        let mut primary_count = [0usize; 5];
        let mut blended = 0;
        for t in &taps {
            primary_count[if t.tile_a == NONE {
                4
            } else {
                (t.tile_a as usize).min(3)
            }] += 1;
            blended += (t.tile_b != NONE) as usize;
        }
        Lut {
            width: w,
            height: h,
            taps,
            order,
            primary_count,
            blended,
        }
    }

    /// Fraction of pixels with any camera.
    pub fn coverage(&self) -> f64 {
        1.0 - self.primary_count[4] as f64 / self.taps.len().max(1) as f64
    }

    /// One-line summary.
    pub fn summary(&self) -> String {
        let n = self.taps.len().max(1) as f64;
        let pc: Vec<String> = (0..4)
            .map(|i| format!("cam{i} {:.1}%", 100.0 * self.primary_count[i] as f64 / n))
            .collect();
        format!(
            "{}x{} order {:?}: {}, none {:.1}%, cross-faded {:.1}%",
            self.width,
            self.height,
            self.order,
            pc.join(" "),
            100.0 * self.primary_count[4] as f64 / n,
            100.0 * self.blended as f64 / n
        )
    }

    /// RGB picture of which camera feeds each pixel (A colour mixed with B colour by the weight), for the docs.
    pub fn coverage_rgb(&self) -> Vec<u8> {
        const C: [[f64; 3]; 5] = [
            [230.0, 90.0, 60.0],
            [60.0, 140.0, 230.0],
            [240.0, 200.0, 60.0],
            [90.0, 200.0, 120.0],
            [0.0, 0.0, 0.0],
        ];
        let col = |t: u8| C[if t == NONE { 4 } else { (t as usize).min(3) }];
        let mut out = Vec::with_capacity(self.taps.len() * 3);
        for t in &self.taps {
            let a = col(t.tile_a);
            let b = col(t.tile_b);
            let w = t.w as f64 / 255.0;
            for ch in 0..3 {
                out.push((a[ch] * w + b[ch] * (1.0 - w)).round() as u8);
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rig::tests::rig;
    use crate::view::Fov;

    fn eyes(w: usize, h: usize) -> [EyeView; 2] {
        EyeView::default_pair(w, h, 0.063, Fov::dream_air_default())
    }

    #[test]
    fn taps_are_small_and_fixed_point_fits() {
        assert_eq!(std::mem::size_of::<Tap>(), 12);
        assert!(640.0 * FP < u16::MAX as f64);
    }

    /// The defining property: a world point on the depth sphere seen by eye pixel (x, y) lands on the tile pixel
    /// stored in the map (checked independently with the camera's own projection from the head frame).
    #[test]
    fn map_matches_direct_projection_of_world_points() {
        let r = rig();
        let ev = eyes(120, 130);
        for depth in [0.5, 2.0, f64::INFINITY] {
            let p = LutParams {
                depth_m: depth,
                ..LutParams::default()
            };
            for (e, eye) in ev.iter().enumerate() {
                let lut = Lut::build(&r, eye, e, &p);
                let mut checked = 0;
                for y in (0..130).step_by(7) {
                    for x in (0..120).step_by(5) {
                        let t = lut.taps[y * 120 + x];
                        if t.tile_a == NONE {
                            continue;
                        }
                        let d = geom::normalize(eye.ray(x as f64 + 0.5, y as f64 + 0.5));
                        let cam = &r.cameras[t.tile_a as usize];
                        let x_cam = if depth.is_finite() {
                            let world = eye.head_from_eye.apply(geom::scale(d, depth));
                            cam.cam_from_head.apply(world)
                        } else {
                            // far away along the ray: translation no longer matters
                            let world = eye.head_from_eye.apply(geom::scale(d, 1e7));
                            cam.cam_from_head.apply(world)
                        };
                        let (u, v) = cam.intr.project(x_cam).unwrap();
                        assert!(
                            (u - t.ua as f64 / FP).abs() < 0.02,
                            "{depth} {e} {x} {y}: u {u} vs {}",
                            t.ua
                        );
                        assert!((v - t.va as f64 / FP).abs() < 0.02);
                        checked += 1;
                    }
                }
                assert!(checked > 200);
            }
        }
    }

    #[test]
    fn each_eye_is_fed_mostly_by_its_own_tracking_camera_and_fully_covered() {
        let r = rig();
        let ev = eyes(100, 108);
        let p = LutParams::default();
        let l = Lut::build(&r, &ev[0], 0, &p);
        let rr = Lut::build(&r, &ev[1], 1, &p);
        assert_eq!(l.order[0], 0);
        assert_eq!(rr.order[0], 1);
        assert!(l.primary_count[0] > l.taps.len() / 2, "{}", l.summary());
        assert!(rr.primary_count[1] > rr.taps.len() / 2, "{}", rr.summary());
        // with all four cameras the default eye FOV is covered almost completely
        assert!(l.coverage() > 0.97, "{}", l.summary());
        assert!(rr.coverage() > 0.97, "{}", rr.summary());
        assert_eq!(l.coverage_rgb().len(), 100 * 108 * 3);
    }

    #[test]
    fn eyes_mirror_each_other() {
        // the rig is nearly mirror-symmetric, so the right eye's camera usage mirrors the left eye's
        let r = rig();
        let ev = eyes(80, 86);
        let p = LutParams::default();
        let l = Lut::build(&r, &ev[0], 0, &p);
        let rr = Lut::build(&r, &ev[1], 1, &p);
        let a = l.primary_count[0] as f64;
        let b = rr.primary_count[1] as f64;
        assert!(
            (a - b).abs() / a < 0.1,
            "{} | {}",
            l.summary(),
            rr.summary()
        );
    }

    #[test]
    fn weights_fade_to_the_border() {
        let r = rig();
        let p = LutParams::default();
        let k = &r.cameras[0].intr;
        assert_eq!(sample_weight(&r, 0, &p, k.cx, k.cy), 1.0);
        assert_eq!(sample_weight(&r, 0, &p, 0.5, k.cy), 0.0);
        let mid = sample_weight(&r, 0, &p, 2.0 + p.feather_px / 2.0, k.cy);
        assert!((mid - 0.5).abs() < 0.05, "{mid}");
        // outside the vignetting circle
        assert_eq!(sample_weight(&r, 0, &p, 20.0, 20.0), 0.0);
    }

    #[test]
    fn explicit_order_and_threads_do_not_change_the_result() {
        let r = rig();
        let ev = eyes(64, 70);
        let mut p = LutParams {
            order: Some(vec![0]),
            threads: 1,
            ..LutParams::default()
        };
        let a = Lut::build(&r, &ev[0], 0, &p);
        p.threads = 7;
        let b = Lut::build(&r, &ev[0], 0, &p);
        assert_eq!(a, b);
        assert!(a
            .taps
            .iter()
            .all(|t| t.tile_b == NONE && (t.tile_a == 0 || t.tile_a == NONE)));
    }
}
