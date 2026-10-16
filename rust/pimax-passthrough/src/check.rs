//! Offline consistency check with real images: render one eye twice, once only from camera A and once only from
//! camera B, and compare the two where both see the scene. With correct calibration the two agree up to the
//! parallax of the scene relative to the chosen depth: vertical offset ~0, horizontal offset smallest when the
//! reprojection depth matches the scene. Sweeping the depth therefore also estimates a good default depth.

use crate::lut::{Lut, LutParams, NONE};
use crate::remap::{self, Tiles};
use crate::rig::Rig;
use crate::view::EyeView;

/// Result for one depth.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Overlap {
    /// Reprojection depth (m).
    pub depth_m: f64,
    /// Pixels compared (both cameras fully valid, neither saturated nor black).
    pub pixels: usize,
    /// Normalised cross-correlation at zero shift.
    pub ncc0: f64,
    /// Best shift of camera B's image relative to A's (eye pixels).
    pub best_dx: i32,
    /// Best vertical shift.
    pub best_dy: i32,
    /// NCC at the best shift.
    pub best_ncc: f64,
    /// Best horizontal shift refined to a fraction of a pixel (parabola through the NCC peak and its two
    /// neighbours; equal to `best_dx` when the peak is at the edge of the search window).
    pub sub_dx: f64,
    /// Best vertical shift refined to a fraction of a pixel.
    pub sub_dy: f64,
}

/// Vertex offset (-0.5..0.5) of the parabola through three equally spaced samples around a maximum.
pub fn parabola_peak(left: f64, mid: f64, right: f64) -> f64 {
    let den = left - 2.0 * mid + right;
    if den >= -1e-12 {
        return 0.0;
    }
    (0.5 * (left - right) / den).clamp(-0.5, 0.5)
}

/// High-pass: pixel minus the mean of its `(2r+1)^2` box (integral image; borders use the clipped box). Removes
/// the cameras' opposite vignetting gradients across the overlap, which otherwise dominate the correlation.
pub fn high_pass(img: &[u8], w: usize, h: usize, r: usize) -> Vec<f32> {
    let mut ii = vec![0u64; (w + 1) * (h + 1)];
    for y in 0..h {
        let mut row = 0u64;
        for x in 0..w {
            row += img[y * w + x] as u64;
            ii[(y + 1) * (w + 1) + x + 1] = ii[y * (w + 1) + x + 1] + row;
        }
    }
    let mut out = vec![0f32; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let sum = ii[y1 * (w + 1) + x1] + ii[y0 * (w + 1) + x0]
                - ii[y0 * (w + 1) + x1]
                - ii[y1 * (w + 1) + x0];
            let n = ((y1 - y0) * (x1 - x0)) as f64;
            out[y * w + x] = (img[y * w + x] as f64 - sum as f64 / n) as f32;
        }
    }
    out
}

fn ncc(a: &[f32], b: &[f32], mask: &[bool], w: usize, h: usize, dx: i32, dy: i32) -> (f64, usize) {
    let (mut sa, mut sb, mut saa, mut sbb, mut sab, mut n) = (0.0, 0.0, 0.0, 0.0, 0.0, 0usize);
    for y in 0..h {
        let yb = y as i32 + dy;
        if yb < 0 || yb >= h as i32 {
            continue;
        }
        for x in 0..w {
            let xb = x as i32 + dx;
            if xb < 0 || xb >= w as i32 {
                continue;
            }
            let i = y * w + x;
            let j = yb as usize * w + xb as usize;
            if !mask[i] || !mask[j] {
                continue;
            }
            let (va, vb) = (a[i] as f64, b[j] as f64);
            sa += va;
            sb += vb;
            saa += va * va;
            sbb += vb * vb;
            sab += va * vb;
            n += 1;
        }
    }
    if n < 100 {
        return (0.0, n);
    }
    let nf = n as f64;
    let cov = sab - sa * sb / nf;
    let va = saa - sa * sa / nf;
    let vb = sbb - sb * sb / nf;
    if va <= 1e-9 || vb <= 1e-9 {
        return (0.0, n);
    }
    (cov / (va * vb).sqrt(), n)
}

/// Compare the single-camera renders of cameras `a` and `b` in `eye` at `depth_m`, searching shifts up to
/// `±search` eye pixels.
pub fn overlap(
    rig: &Rig,
    eye: &EyeView,
    tiles: &Tiles,
    (a, b): (usize, usize),
    depth_m: f64,
    search: i32,
) -> Overlap {
    let p = |cam: usize| LutParams {
        depth_m,
        order: Some(vec![cam]),
        ..LutParams::default()
    };
    let la = Lut::build(rig, eye, 0, &p(a));
    let lb = Lut::build(rig, eye, 0, &p(b));
    let (w, h) = (eye.width, eye.height);
    let mut ia = vec![0u8; w * h];
    let mut ib = vec![0u8; w * h];
    remap::remap_grey(&la, tiles, &mut ia, 0);
    remap::remap_grey(&lb, tiles, &mut ib, 0);
    let ok = |t: &crate::lut::Tap, v: u8| t.tile_a != NONE && t.w == 255 && (8..=245).contains(&v);
    let ma: Vec<bool> = la.taps.iter().zip(&ia).map(|(t, &v)| ok(t, v)).collect();
    let mb: Vec<bool> = lb.taps.iter().zip(&ib).map(|(t, &v)| ok(t, v)).collect();
    let mask: Vec<bool> = ma.iter().zip(&mb).map(|(&x, &y)| x && y).collect();
    let (ia, ib) = (high_pass(&ia, w, h, 6), high_pass(&ib, w, h, 6));
    let (ncc0, pixels) = ncc(&ia, &ib, &mask, w, h, 0, 0);
    let mut best = (ncc0, 0, 0);
    for dy in -search..=search {
        for dx in -search..=search {
            let (c, _) = ncc(&ia, &ib, &mask, w, h, dx, dy);
            if c > best.0 {
                best = (c, dx, dy);
            }
        }
    }
    let at = |dx: i32, dy: i32| ncc(&ia, &ib, &mask, w, h, dx, dy).0;
    let (mut sub_dx, mut sub_dy) = (best.1 as f64, best.2 as f64);
    if best.1.abs() < search {
        sub_dx += parabola_peak(at(best.1 - 1, best.2), best.0, at(best.1 + 1, best.2));
    }
    if best.2.abs() < search {
        sub_dy += parabola_peak(at(best.1, best.2 - 1), best.0, at(best.1, best.2 + 1));
    }
    Overlap {
        depth_m,
        pixels,
        ncc0,
        best_dx: best.1,
        best_dy: best.2,
        best_ncc: best.0,
        sub_dx,
        sub_dy,
    }
}

/// Depths used by the sweep.
pub const SWEEP_DEPTHS: [f64; 9] = [0.5, 0.75, 1.0, 1.5, 2.0, 3.0, 5.0, 10.0, f64::INFINITY];

/// Run [`overlap`] for each depth.
pub fn depth_sweep(
    rig: &Rig,
    eye: &EyeView,
    tiles: &Tiles,
    pair: (usize, usize),
    search: i32,
) -> Vec<Overlap> {
    SWEEP_DEPTHS
        .iter()
        .map(|&d| overlap(rig, eye, tiles, pair, d, search))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn high_pass_removes_a_gradient() {
        let (w, h) = (40, 30);
        let g: Vec<u8> = (0..w * h).map(|i| (i % w * 5) as u8).collect();
        let hp = high_pass(&g, w, h, 3);
        // away from the borders a linear ramp has zero high-pass
        assert!(hp[15 * w + 20].abs() < 1e-3, "{}", hp[15 * w + 20]);
    }

    #[test]
    fn ncc_finds_a_known_shift() {
        let (w, h) = (60, 50);
        let f = |x: f64, y: f64| (128.0 + 60.0 * (x * 0.31).sin() * (y * 0.23).cos()) as u8;
        let a: Vec<u8> = (0..w * h)
            .map(|i| f((i % w) as f64, (i / w) as f64))
            .collect();
        // b(x, y) = a(x - 3, y + 2)
        let b: Vec<u8> = (0..w * h)
            .map(|i| f((i % w) as f64 - 3.0, (i / w) as f64 + 2.0))
            .collect();
        let mask = vec![true; w * h];
        let (a, b) = (high_pass(&a, w, h, 4), high_pass(&b, w, h, 4));
        let mut best = (f64::MIN, 0, 0);
        for dy in -5..=5 {
            for dx in -5..=5 {
                let (c, _) = ncc(&a, &b, &mask, w, h, dx, dy);
                if c > best.0 {
                    best = (c, dx, dy);
                }
            }
        }
        assert_eq!((best.1, best.2), (3, -2));
        assert!(best.0 > 0.95, "{best:?}");
        // sub-pixel: a parabola through samples of y = 1 - (x - 0.3)^2 at -1, 0, 1 peaks at 0.3
        let f2 = |x: f64| 1.0 - (x - 0.3) * (x - 0.3);
        assert!((parabola_peak(f2(-1.0), f2(0.0), f2(1.0)) - 0.3).abs() < 1e-12);
        assert_eq!(parabola_peak(1.0, 1.0, 1.0), 0.0);
        assert_eq!(parabola_peak(0.0, 1.0, 2.0), 0.0);
        let (c0, _) = ncc(&a, &b, &mask, w, h, 0, 0);
        assert!(c0 < best.0 - 0.2, "{c0}");
    }
}
