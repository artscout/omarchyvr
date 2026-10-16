//! Stereo rectification of two fisheye tiles into a common pinhole frame, plus an epipolar check: block-match
//! textured points of the left rectified image along the right image and measure the vertical offset `dy`.
//! With a correct calibration (intrinsics, rig convention, tile order) `dy` is ~0 for every match; a wrong
//! convention gives large, inconsistent offsets. This is how the conventions in [`crate::calib`] were verified.

use crate::geom::{self, Mat3, Pose};
use crate::kb4::Kb4;

/// Output geometry.
#[derive(Clone, Copy, Debug)]
pub struct RectifyParams {
    /// Width in pixels.
    pub width: usize,
    /// Height in pixels.
    pub height: usize,
    /// Focal length in pixels (principal point at the centre).
    pub focal: f64,
}

impl Default for RectifyParams {
    fn default() -> Self {
        RectifyParams {
            width: 640,
            height: 480,
            focal: 220.0,
        }
    }
}

/// Precomputed maps from rectified pixels to source pixels of both cameras.
pub struct StereoRectifier {
    /// Parameters.
    pub p: RectifyParams,
    map: [Vec<(f32, f32)>; 2],
    /// `a_from_rect`: rectified axes (columns) in camera-a coordinates.
    pub a_from_rect: Mat3,
    /// Baseline length (metres).
    pub baseline: f64,
}

const INVALID: (f32, f32) = (f32::NAN, f32::NAN);

impl StereoRectifier {
    /// Rectify camera a (left in the output) and b. `b_from_a` maps a-coordinates to b-coordinates.
    /// The rectified x axis points from a's centre to b's centre, z is the mean optical axis made orthogonal.
    pub fn new(ka: &Kb4, kb: &Kb4, b_from_a: &Pose, p: RectifyParams) -> StereoRectifier {
        let a_from_b = b_from_a.inverse();
        let x = geom::normalize(a_from_b.t);
        let za = [0.0, 0.0, 1.0];
        let zb = geom::col(&a_from_b.r, 2);
        let avg = geom::normalize(geom::add(za, zb));
        let z = geom::normalize(geom::sub(avg, geom::scale(x, geom::dot(avg, x))));
        let y = geom::cross(z, x);
        let a_from_rect = geom::from_cols(x, y, z);
        let b_from_rect = geom::mat_mul(&b_from_a.r, &a_from_rect);
        let (cx, cy) = ((p.width as f64 - 1.0) / 2.0, (p.height as f64 - 1.0) / 2.0);
        let mut map = [
            vec![INVALID; p.width * p.height],
            vec![INVALID; p.width * p.height],
        ];
        for v in 0..p.height {
            for u in 0..p.width {
                let d = [(u as f64 - cx) / p.focal, (v as f64 - cy) / p.focal, 1.0];
                for (k, (m, cam)) in [(&a_from_rect, ka), (&b_from_rect, kb)]
                    .into_iter()
                    .enumerate()
                {
                    let ray = geom::mat_vec(m, d);
                    if let Some((su, sv)) = cam.project(ray) {
                        if su >= 0.0
                            && sv >= 0.0
                            && su <= cam.width as f64 - 1.0
                            && sv <= cam.height as f64 - 1.0
                        {
                            map[k][v * p.width + u] = (su as f32, sv as f32);
                        }
                    }
                }
            }
        }
        StereoRectifier {
            p,
            map,
            a_from_rect,
            baseline: geom::norm(a_from_b.t),
        }
    }

    /// Resample image `which` (0 = a, 1 = b; 8-bit, `src_w` x `src_h`, pitch `src_w`). Invalid pixels are 0.
    pub fn remap(&self, which: usize, src: &[u8], src_w: usize, src_h: usize) -> Vec<u8> {
        let mut out = vec![0u8; self.p.width * self.p.height];
        for (o, &(su, sv)) in out.iter_mut().zip(self.map[which].iter()) {
            if su.is_nan() {
                continue;
            }
            *o = bilinear(src, src_w, src_h, su, sv);
        }
        out
    }

    /// Mask of rectified pixels valid in both cameras.
    pub fn valid_both(&self) -> Vec<bool> {
        self.map[0]
            .iter()
            .zip(self.map[1].iter())
            .map(|(a, b)| !a.0.is_nan() && !b.0.is_nan())
            .collect()
    }

    /// Mask for one camera.
    pub fn valid(&self, which: usize) -> Vec<bool> {
        self.map[which].iter().map(|a| !a.0.is_nan()).collect()
    }
}

fn bilinear(src: &[u8], w: usize, h: usize, x: f32, y: f32) -> u8 {
    let x0 = (x.floor() as usize).min(w - 1);
    let y0 = (y.floor() as usize).min(h - 1);
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let fx = x - x0 as f32;
    let fy = y - y0 as f32;
    let p = |xx: usize, yy: usize| src[yy * w + xx] as f32;
    let v = p(x0, y0) * (1.0 - fx) * (1.0 - fy)
        + p(x1, y0) * fx * (1.0 - fy)
        + p(x0, y1) * (1.0 - fx) * fy
        + p(x1, y1) * fx * fy;
    (v + 0.5).clamp(0.0, 255.0) as u8
}

/// Block matching parameters.
#[derive(Clone, Copy, Debug)]
pub struct MatchParams {
    /// Patch half size (patch = 2*half+1).
    pub half: usize,
    /// Grid cell for picking at most one feature.
    pub cell: usize,
    /// Vertical search range +- (px).
    pub max_dy: i32,
    /// Disparity search range (u_left - u_right).
    pub min_disp: i32,
    /// Upper disparity.
    pub max_disp: i32,
    /// Minimum ZNCC of the best match.
    pub min_score: f64,
    /// Minimum gap between best and second-best (positions > 2 px apart).
    pub min_margin: f64,
    /// Minimum patch standard deviation (grey levels).
    pub min_std: f64,
}

impl Default for MatchParams {
    fn default() -> Self {
        MatchParams {
            half: 6,
            cell: 16,
            max_dy: 14,
            min_disp: -10,
            max_disp: 120,
            min_score: 0.85,
            min_margin: 0.02,
            min_std: 4.0,
        }
    }
}

/// One accepted match.
#[derive(Clone, Copy, Debug)]
pub struct Match {
    /// Left x.
    pub x: usize,
    /// Left y.
    pub y: usize,
    /// Disparity (left x - right x).
    pub disp: f64,
    /// Vertical offset right y - left y (sub-pixel).
    pub dy: f64,
    /// ZNCC.
    pub score: f64,
}

/// Result of [`epipolar_check`].
#[derive(Clone, Debug, Default)]
pub struct EpipolarReport {
    /// Textured points tried.
    pub candidates: usize,
    /// Matches that passed score, uniqueness and left-right checks.
    pub matches: Vec<Match>,
}

impl EpipolarReport {
    fn dys(&self) -> Vec<f64> {
        let mut v: Vec<f64> = self.matches.iter().map(|m| m.dy).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v
    }

    /// Median vertical offset.
    pub fn median_dy(&self) -> f64 {
        median(&self.dys())
    }

    /// Median absolute deviation of `dy` around its median.
    pub fn mad_dy(&self) -> f64 {
        let m = self.median_dy();
        let mut d: Vec<f64> = self.matches.iter().map(|x| (x.dy - m).abs()).collect();
        d.sort_by(|a, b| a.partial_cmp(b).unwrap());
        median(&d)
    }

    /// Fraction of matches with |dy| <= `px`.
    pub fn frac_within(&self, px: f64) -> f64 {
        if self.matches.is_empty() {
            return 0.0;
        }
        self.matches.iter().filter(|m| m.dy.abs() <= px).count() as f64 / self.matches.len() as f64
    }

    /// Median disparity.
    pub fn median_disp(&self) -> f64 {
        let mut v: Vec<f64> = self.matches.iter().map(|m| m.disp).collect();
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        median(&v)
    }

    /// One-line summary.
    pub fn summary(&self) -> String {
        format!(
            "{} matches of {} candidates: median dy {:+.2} px, MAD {:.2}, |dy|<=1px {:.0}%, <=2px {:.0}%, median disparity {:.1} px",
            self.matches.len(),
            self.candidates,
            self.median_dy(),
            self.mad_dy(),
            self.frac_within(1.0) * 100.0,
            self.frac_within(2.0) * 100.0,
            self.median_disp()
        )
    }
}

fn median(v: &[f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

struct PatchStats {
    mean: f64,
    norm: f64,
}

fn patch_stats(img: &[u8], w: usize, x: usize, y: usize, half: usize) -> PatchStats {
    let (mut s, mut s2) = (0.0, 0.0);
    let n = ((2 * half + 1) * (2 * half + 1)) as f64;
    for yy in y - half..=y + half {
        for &p in &img[yy * w + x - half..=yy * w + x + half] {
            let p = p as f64;
            s += p;
            s2 += p * p;
        }
    }
    let mean = s / n;
    PatchStats {
        mean,
        norm: (s2 - n * mean * mean).max(0.0).sqrt(),
    }
}

#[allow(clippy::too_many_arguments)]
fn zncc(
    a: &[u8],
    b: &[u8],
    w: usize,
    ax: usize,
    ay: usize,
    bx: usize,
    by: usize,
    half: usize,
    sa: &PatchStats,
) -> f64 {
    let sb = patch_stats(b, w, bx, by, half);
    if sa.norm < 1e-6 || sb.norm < 1e-6 {
        return -1.0;
    }
    let mut acc = 0.0;
    for d in 0..=2 * half {
        let ra = &a[(ay - half + d) * w + ax - half..=(ay - half + d) * w + ax + half];
        let rb = &b[(by - half + d) * w + bx - half..=(by - half + d) * w + bx + half];
        for (&p, &q) in ra.iter().zip(rb.iter()) {
            acc += (p as f64 - sa.mean) * (q as f64 - sb.mean);
        }
    }
    acc / (sa.norm * sb.norm)
}

/// Minimum eigenvalue of the gradient structure tensor over the patch (Shi-Tomasi corner score).
fn corner_score(img: &[u8], w: usize, x: usize, y: usize, half: usize) -> f64 {
    let (mut gxx, mut gyy, mut gxy) = (0.0, 0.0, 0.0);
    for yy in y - half..=y + half {
        for xx in x - half..=x + half {
            let gx = img[yy * w + xx + 1] as f64 - img[yy * w + xx - 1] as f64;
            let gy = img[(yy + 1) * w + xx] as f64 - img[(yy - 1) * w + xx] as f64;
            gxx += gx * gx;
            gyy += gy * gy;
            gxy += gx * gy;
        }
    }
    let n = ((2 * half + 1) * (2 * half + 1)) as f64;
    let (a, b, c) = (gxx / n, gyy / n, gxy / n);
    (a + b) / 2.0 - (((a - b) / 2.0).powi(2) + c * c).sqrt()
}

struct Best {
    x: i64,
    y: i64,
    score: f64,
    second: f64,
}

#[allow(clippy::too_many_arguments)]
fn search(
    a: &[u8],
    b: &[u8],
    valid_b: &[bool],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    sign: i64,
    mp: &MatchParams,
) -> Option<Best> {
    let half = mp.half as i64;
    let sa = patch_stats(a, w, x, y, mp.half);
    let mut scores: Vec<(i64, i64, f64)> = Vec::new();
    for dy in -mp.max_dy as i64..=mp.max_dy as i64 {
        let by = y as i64 + dy;
        if by < half + 1 || by >= h as i64 - half - 1 {
            continue;
        }
        for d in mp.min_disp as i64..=mp.max_disp as i64 {
            let bx = x as i64 - sign * d;
            if bx < half + 1 || bx >= w as i64 - half - 1 || !valid_b[by as usize * w + bx as usize]
            {
                continue;
            }
            let s = zncc(a, b, w, x, y, bx as usize, by as usize, mp.half, &sa);
            scores.push((bx, by, s));
        }
    }
    let best = scores
        .iter()
        .copied()
        .max_by(|p, q| p.2.partial_cmp(&q.2).unwrap())?;
    let second = scores
        .iter()
        .filter(|s| (s.0 - best.0).abs() > 2 || (s.1 - best.1).abs() > 2)
        .map(|s| s.2)
        .fold(-1.0, f64::max);
    Some(Best {
        x: best.0,
        y: best.1,
        score: best.2,
        second,
    })
}

/// Match textured points of `left` along `right` (both rectified, `w` x `h`). `valid` = pixels valid in both.
pub fn epipolar_check(
    left: &[u8],
    right: &[u8],
    valid: &[bool],
    w: usize,
    h: usize,
    mp: &MatchParams,
) -> EpipolarReport {
    let margin = mp.half + 2 + mp.max_dy as usize;
    let mut rep = EpipolarReport::default();
    let patch_valid = |x: usize, y: usize| -> bool {
        (y - mp.half..=y + mp.half)
            .all(|yy| (x - mp.half..=x + mp.half).all(|xx| valid[yy * w + xx]))
    };
    let mut cy = margin;
    while cy + mp.cell < h - margin {
        let mut cx = margin;
        while cx + mp.cell < w - margin {
            // best corner in this cell
            let mut best: Option<(usize, usize, f64)> = None;
            for y in (cy..cy + mp.cell).step_by(2) {
                for x in (cx..cx + mp.cell).step_by(2) {
                    if !valid[y * w + x] {
                        continue;
                    }
                    let s = corner_score(left, w, x, y, mp.half);
                    if best.is_none_or(|b| s > b.2) {
                        best = Some((x, y, s));
                    }
                }
            }
            if let Some((x, y, s)) = best {
                if s > 4.0
                    && patch_valid(x, y)
                    && patch_stats(left, w, x, y, mp.half).norm / ((2 * mp.half + 1) as f64)
                        >= mp.min_std
                {
                    rep.candidates += 1;
                    if let Some(m) = match_point(left, right, valid, w, h, x, y, mp) {
                        rep.matches.push(m);
                    }
                }
            }
            cx += mp.cell;
        }
        cy += mp.cell;
    }
    rep
}

#[allow(clippy::too_many_arguments)]
fn match_point(
    left: &[u8],
    right: &[u8],
    valid: &[bool],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    mp: &MatchParams,
) -> Option<Match> {
    let f = search(left, right, valid, w, h, x, y, 1, mp)?;
    if f.score < mp.min_score || f.score - f.second < mp.min_margin {
        return None;
    }
    // left-right consistency: search back from the right match
    let (bx, by) = (f.x as usize, f.y as usize);
    let back = search(right, left, valid, w, h, bx, by, -1, mp)?;
    if (back.x - x as i64).abs() > 1 || (back.y - y as i64).abs() > 1 {
        return None;
    }
    // sub-pixel dy and disparity by parabola fits around the best position
    let sa = patch_stats(left, w, x, y, mp.half);
    let s = |xx: usize, yy: usize| zncc(left, right, w, x, y, xx, yy, mp.half, &sa);
    let para = |m: f64, c: f64, p: f64| {
        let den = m - 2.0 * c + p;
        if den.abs() < 1e-9 {
            0.0
        } else {
            (0.5 * (m - p) / den).clamp(-0.5, 0.5)
        }
    };
    let lim = |v: usize, n: usize| v > mp.half + 1 && v + mp.half + 2 < n;
    let sub_y = if lim(by, h) {
        para(s(bx, by - 1), f.score, s(bx, by + 1))
    } else {
        0.0
    };
    let sub_x = if lim(bx, w) {
        para(s(bx - 1, by), f.score, s(bx + 1, by))
    } else {
        0.0
    };
    Some(Match {
        x,
        y,
        disp: x as f64 - (bx as f64 + sub_x),
        dy: by as f64 + sub_y - y as f64,
        score: f.score,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Deterministic textured image.
    fn texture(w: usize, h: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        let mut noise = vec![0u8; w * h];
        for p in noise.iter_mut() {
            s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            *p = (s >> 24) as u8;
        }
        // blur 3x3 so that patches are smooth enough for sub-pixel fits
        let mut out = vec![0u8; w * h];
        for y in 1..h - 1 {
            for x in 1..w - 1 {
                let mut a = 0u32;
                for dy in 0..3 {
                    for dx in 0..3 {
                        a += noise[(y + dy - 1) * w + x + dx - 1] as u32;
                    }
                }
                out[y * w + x] = (a / 9) as u8;
            }
        }
        out
    }

    fn shifted(src: &[u8], w: usize, h: usize, dx: i64, dy: i64) -> Vec<u8> {
        let mut out = vec![0u8; w * h];
        for y in 0..h as i64 {
            for x in 0..w as i64 {
                let (sx, sy) = (x + dx, y - dy);
                if sx >= 0 && sy >= 0 && sx < w as i64 && sy < h as i64 {
                    out[(y * w as i64 + x) as usize] = src[(sy * w as i64 + sx) as usize];
                }
            }
        }
        out
    }

    #[test]
    fn finds_known_shift() {
        let (w, h) = (160usize, 120usize);
        let left = texture(w, h, 7);
        // right image: content moved left by 9 px (disparity 9) and down by 3 px (dy +3)
        let right = shifted(&left, w, h, 9, 3);
        let valid = vec![true; w * h];
        let mp = MatchParams {
            max_disp: 20,
            min_disp: -5,
            max_dy: 6,
            cell: 20,
            ..Default::default()
        };
        let rep = epipolar_check(&left, &right, &valid, w, h, &mp);
        assert!(rep.matches.len() >= 5, "{}", rep.summary());
        assert!((rep.median_dy() - 3.0).abs() < 0.3, "{}", rep.summary());
        assert!((rep.median_disp() - 9.0).abs() < 0.3, "{}", rep.summary());
        assert!(rep.mad_dy() < 0.3);
        // and zero offset when aligned
        let right0 = shifted(&left, w, h, 4, 0);
        let rep0 = epipolar_check(&left, &right0, &valid, w, h, &mp);
        assert!(
            rep0.median_dy().abs() < 0.2 && rep0.frac_within(1.0) > 0.9,
            "{}",
            rep0.summary()
        );
    }

    #[test]
    fn rectifier_maps_a_symmetric_rig() {
        // two identical fisheyes 0.1 m apart along x, toed out by +-20 deg: a scene point straight ahead at 2 m must
        // land on the same rectified row in both images.
        let k = Kb4::new(
            640,
            480,
            230.0,
            230.0,
            320.0,
            240.0,
            [0.06, -0.06, 0.03, -0.008],
            vec![],
        );
        let ry = |deg: f64| {
            let (s, c) = deg.to_radians().sin_cos();
            [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
        };
        // a at x = -0.05 looking 20 deg left, b at x = +0.05 looking 20 deg right (p_cam = R (p_w - centre))
        let a_from_w = Pose {
            r: ry(20.0),
            t: geom::mat_vec(&ry(20.0), [0.05, 0.0, 0.0]),
        };
        let b_from_w = Pose {
            r: ry(-20.0),
            t: geom::mat_vec(&ry(-20.0), [-0.05, 0.0, 0.0]),
        };
        let b_from_a = b_from_w.compose(&a_from_w.inverse());
        let rect = StereoRectifier::new(&k, &k, &b_from_a, RectifyParams::default());
        assert!((rect.baseline - 0.1).abs() < 1e-9);
        for p in [[0.0, 0.0, 2.0], [0.3, -0.2, 1.5], [-0.4, 0.3, 3.0]] {
            let pa = a_from_w.apply(p);
            let pb = b_from_w.apply(p);
            let (ua, va) = k.project(pa).unwrap();
            let (ub, vb) = k.project(pb).unwrap();
            // find the rectified pixels that sample these source points (nearest map entry)
            let find = |which: usize, su: f64, sv: f64| {
                let mut best = (0usize, f64::MAX);
                for (i, &(mu, mv)) in rect.map[which].iter().enumerate() {
                    if mu.is_nan() {
                        continue;
                    }
                    let d = (mu as f64 - su).powi(2) + (mv as f64 - sv).powi(2);
                    if d < best.1 {
                        best = (i, d);
                    }
                }
                (best.0 % rect.p.width, best.0 / rect.p.width)
            };
            let (xa, ya) = find(0, ua, va);
            let (xb, yb) = find(1, ub, vb);
            assert!(
                (ya as i64 - yb as i64).abs() <= 1,
                "rows {ya} vs {yb} for {p:?}"
            );
            assert!(
                xa as i64 >= xb as i64 - 1,
                "disparity must be >= 0: {xa} {xb}"
            );
        }
    }
}
