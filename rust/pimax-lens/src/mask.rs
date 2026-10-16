//! Visibility mask meshes (OpenXR `XR_KHR_visibility_mask`) derived from the lens model's visibility mask.
//!
//! The visible area of an eye is "inside the rendered FOV" and "not hidden by the mask". It is traced with rays from
//! the optical axis: along every ray the boundary is found by bisection, mapped to the ray tangent plane (green
//! channel), and turned into a line loop, a visible-area triangle fan and a hidden-area ring of triangles between
//! the polygon and the FOV rectangle.
//!
//! Vertex convention (the one of Monado's `u_visibility_mask_get_default`): `x` = tangent to the right, `y` =
//! minus the upward tangent.

use crate::model::Lens;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskKind {
    HiddenTriangleMesh = 0,
    VisibleTriangleMesh = 1,
    LineLoop = 2,
}

impl MaskKind {
    pub fn from_i32(v: i32) -> Option<MaskKind> {
        match v {
            0 => Some(MaskKind::HiddenTriangleMesh),
            1 => Some(MaskKind::VisibleTriangleMesh),
            2 => Some(MaskKind::LineLoop),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct MaskMesh {
    pub vertices: Vec<[f32; 2]>,
    pub indices: Vec<u32>,
}

/// Is panel point (xp, yp) (half-heights from the optical centre) visible: inside the rendered FOV and not masked?
fn visible(lens: &Lens, view: usize, xp: f32, yp: f32) -> bool {
    let e = lens.eye(view);
    let a = lens.config().aspect;
    let u = 0.5 + (xp + e.cx) / (2.0 * a);
    let v = 0.5 - (yp + e.cy) / 2.0;
    if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
        return false;
    }
    if lens.hide_amount(view, u, v) > 0.0 {
        return false;
    }
    let uv = lens.compute(view, u, v)[1];
    (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1])
}

/// Distance along a tangent-plane direction from the origin to the rectangle of the rendered FOV.
fn rect_distance(lens: &Lens, view: usize, dx: f32, dy: f32) -> f32 {
    let [tl, tr, tu, td] = lens.fov_tan(view);
    let mut t = f32::INFINITY;
    if dx > 1e-9 {
        t = t.min(tr / dx);
    } else if dx < -1e-9 {
        t = t.min(tl / -dx);
    }
    if dy > 1e-9 {
        t = t.min(tu / dy);
    } else if dy < -1e-9 {
        t = t.min(td / -dy);
    }
    t
}

/// Polygon (tangent plane, x right, y up) of the visible area traced with `n` rays. `None` if the optical axis
/// itself is not visible.
pub fn outline_tangent(lens: &Lens, view: usize, n: usize) -> Option<Vec<[f32; 2]>> {
    if !visible(lens, view, 0.0, 0.0) {
        return None;
    }
    let mut pts = Vec::with_capacity(n);
    for i in 0..n {
        let phi = std::f32::consts::TAU * i as f32 / n as f32;
        let (dx, dy) = (phi.cos(), phi.sin());
        let (mut lo, mut hi) = (0.0f32, 4.0f32);
        for _ in 0..40 {
            let mid = 0.5 * (lo + hi);
            if visible(lens, view, mid * dx, mid * dy) {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let e = lens.eye(view);
        let a = lens.config().aspect;
        // panel point of the boundary -> (u, v) -> green tangent
        let (xp, yp) = (lo * dx, lo * dy);
        let u = 0.5 + (xp + e.cx) / (2.0 * a);
        let v = 0.5 - (yp + e.cy) / 2.0;
        let t = lens.tangent(view, 1, u, v);
        // keep it inside the FOV rectangle despite rounding
        let [tl, tr, tu, td] = lens.fov_tan(view);
        pts.push([t[0].clamp(-tl, tr), t[1].clamp(-td, tu)]);
    }
    Some(pts)
}

/// Build the mask mesh of `kind` for `view` with `n` boundary samples. `None` when the eye has no mask configured
/// (use the generic FOV mask then) or when the optical axis is hidden.
pub fn visibility_mesh(lens: &Lens, view: usize, kind: MaskKind, n: usize) -> Option<MaskMesh> {
    if !lens.config().enabled || !lens.has_mask(view) || n < 8 {
        return None;
    }
    let poly = outline_tangent(lens, view, n)?;
    let flip = |p: [f32; 2]| [p[0], -p[1]];
    match kind {
        MaskKind::LineLoop => Some(MaskMesh {
            vertices: poly.iter().map(|&p| flip(p)).collect(),
            indices: (0..n as u32).collect(),
        }),
        MaskKind::VisibleTriangleMesh => {
            let mut vertices: Vec<[f32; 2]> = poly.iter().map(|&p| flip(p)).collect();
            vertices.push([0.0, 0.0]);
            let c = n as u32;
            let mut indices = Vec::with_capacity(n * 3);
            for i in 0..n as u32 {
                indices.extend_from_slice(&[c, i, (i + 1) % n as u32]);
            }
            Some(MaskMesh { vertices, indices })
        }
        MaskKind::HiddenTriangleMesh => {
            let mut vertices: Vec<[f32; 2]> = poly.iter().map(|&p| flip(p)).collect();
            for p in &poly {
                let len = p[0].hypot(p[1]);
                let (dx, dy) = if len > 1e-9 {
                    (p[0] / len, p[1] / len)
                } else {
                    (1.0, 0.0)
                };
                let d = rect_distance(lens, view, dx, dy);
                vertices.push(flip([dx * d, dy * d]));
            }
            let nn = n as u32;
            let mut indices = Vec::with_capacity(n * 6);
            for i in 0..nn {
                let j = (i + 1) % nn;
                indices.extend_from_slice(&[i, nn + i, nn + j, i, nn + j, j]);
            }
            Some(MaskMesh { vertices, indices })
        }
    }
}

/// Area of the triangles of a mesh (shoelace, absolute value per triangle).
pub fn mesh_area(m: &MaskMesh) -> f32 {
    m.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let (a, b, c) = (
                m.vertices[t[0] as usize],
                m.vertices[t[1] as usize],
                m.vertices[t[2] as usize],
            );
            0.5 * ((b[0] - a[0]) * (c[1] - a[1]) - (c[0] - a[0]) * (b[1] - a[1])).abs()
        })
        .sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse, LensConfig};

    const ASPECT: f32 = 3840.0 / 3552.0;

    fn lens(text: &str) -> Lens {
        let (cfg, w) = parse(text, ASPECT);
        assert!(w.is_empty(), "{w:?}");
        Lens::new(cfg)
    }

    #[test]
    fn no_mask_means_no_custom_mesh() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        for k in [
            MaskKind::HiddenTriangleMesh,
            MaskKind::VisibleTriangleMesh,
            MaskKind::LineLoop,
        ] {
            assert!(visibility_mesh(&l, 0, k, 32).is_none());
        }
        assert_eq!(MaskKind::from_i32(2), Some(MaskKind::LineLoop));
        assert_eq!(MaskKind::from_i32(3), None);
    }

    #[test]
    fn outline_of_an_ellipse_stays_on_the_ellipse_or_the_fov() {
        let l = lens("center = 0 0\nmask = ellipse 0.8 0.6\nmask_feather = 0\n");
        let poly = outline_tangent(&l, 0, 64).unwrap();
        assert_eq!(poly.len(), 64);
        let [tl, tr, tu, td] = l.fov_tan(0);
        for p in &poly {
            assert!(
                p[0] >= -tl - 1e-4 && p[0] <= tr + 1e-4 && p[1] >= -td - 1e-4 && p[1] <= tu + 1e-4
            );
        }
        // the +x ray: boundary at panel radius 0.8 -> tangent from the model
        let want = l.tangent(0, 1, 0.5 + 0.8 / (2.0 * ASPECT), 0.5)[0];
        assert!((poly[0][0] - want).abs() < 2e-3, "{} vs {want}", poly[0][0]);
        // a quarter turn later: +y at panel radius 0.6
        let want_y = l.tangent(0, 1, 0.5, 0.5 - 0.6 / 2.0)[1];
        assert!((poly[16][1] - want_y).abs() < 2e-3);
    }

    #[test]
    fn visible_plus_hidden_cover_the_fov_rectangle() {
        let l = lens("mask = ellipse 1.0 0.9\nblackout = 0.02\n");
        let n = 96;
        let vis = visibility_mesh(&l, 1, MaskKind::VisibleTriangleMesh, n).unwrap();
        let hid = visibility_mesh(&l, 1, MaskKind::HiddenTriangleMesh, n).unwrap();
        let [tl, tr, tu, td] = l.fov_tan(1);
        let rect = (tl + tr) * (tu + td);
        let total = mesh_area(&vis) + mesh_area(&hid);
        assert!((total - rect).abs() / rect < 0.03, "{total} vs {rect}");
        assert!(mesh_area(&vis) < rect * 0.99 && mesh_area(&hid) > 0.0);
        for m in [&vis, &hid] {
            assert!(m.indices.iter().all(|&i| (i as usize) < m.vertices.len()));
            assert_eq!(m.indices.len() % 3, 0);
        }
        let line = visibility_mesh(&l, 1, MaskKind::LineLoop, n).unwrap();
        assert_eq!(line.vertices.len(), n);
        assert_eq!(line.indices.len(), n);
    }

    #[test]
    fn hidden_axis_gives_no_mesh() {
        let l = lens("blackout = 5\n");
        assert!(visibility_mesh(&l, 0, MaskKind::LineLoop, 32).is_none());
    }

    #[test]
    fn y_is_flipped_like_the_default_mask() {
        // a mask that only cuts the top of the panel (user-facing up): the hidden mesh has area only at y < 0
        let l = lens("center = 0 0\nblackout = 0 0 0.9 0\nmask_feather = 0\n");
        let hid = visibility_mesh(&l, 0, MaskKind::HiddenTriangleMesh, 64).unwrap();
        let above: f32 = hid
            .vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::INFINITY, f32::min);
        let below: f32 = hid
            .vertices
            .iter()
            .map(|v| v[1])
            .fold(f32::NEG_INFINITY, f32::max);
        // hidden region is at the top of the image -> negative y in mask coordinates
        let [_, _, tu, td] = l.fov_tan(0);
        assert!((above + tu).abs() < 1e-3, "{above}");
        assert!(below <= td + 1e-3);
    }
}
