//! Dense, accurate stereo correspondences on a rectified pair, for calibration rather than depth:
//!
//! 1. corners: the strongest Shi-Tomasi corner per grid cell of the left image (a corner has texture in both
//!    directions, so it pins the *vertical* offset too, which is what calibration needs);
//! 2. integer ZNCC search in a band around the epipolar line (`±dy_search` rows, disparity range) with a
//!    uniqueness margin and a left-right consistency check;
//! 3. sub-pixel refinement by Lucas-Kanade (translation + gain + bias, Gaussian window) instead of a parabola fit,
//!    which avoids the "pixel locking" bias of parabola peaks (it matters when the effect is ~1 px).
//!
//! Output: matches in rectified pixels (`dy` = right row - left row) and the two unit rays in the camera
//! frames, which are what the bundle adjustment uses (they do not depend on the extrinsics used to rectify).

use crate::rect::{RectGeom, RectImage};
use pimax_cam::geom::Vec3;

/// Matching parameters.
#[derive(Clone, Copy, Debug)]
pub struct MatchConfig {
    /// Grid cell (pixels) for picking at most one corner.
    pub cell: usize,
    /// ZNCC patch half size for the integer search.
    pub half: usize,
    /// Lucas-Kanade patch half size.
    pub lk_half: usize,
    /// Vertical search range around the left row (± pixels).
    pub dy_search: i32,
    /// Lowest disparity (left x - right x).
    pub min_disp: i32,
    /// Highest disparity.
    pub max_disp: i32,
    /// Minimum ZNCC of the integer search peak.
    pub min_zncc_search: f64,
    /// Minimum ZNCC after the sub-pixel refinement.
    pub min_zncc: f64,
    /// Minimum gap between best and second-best score (positions > 2 px apart).
    pub min_margin: f64,
    /// Minimum smaller eigenvalue of the gradient structure tensor (grey levels^2 per pixel).
    pub min_eig: f64,
    /// Grey level from which a pixel counts as clipped. Clipped pixels and their neighbours (2 px) get no weight
    /// in the sub-pixel refinement: a clipped edge sits at a different radiance level in each camera and would
    /// bias the position. The fixed exposure overexposes a sunny room, so this matters on real frames.
    pub max_level: f32,
    /// Smallest fraction of the refinement window's weight that must remain after removing clipped pixels.
    pub min_usable: f64,
}

impl Default for MatchConfig {
    fn default() -> Self {
        MatchConfig {
            cell: 10,
            half: 6,
            lk_half: 7,
            dy_search: 12,
            min_disp: -8,
            max_disp: 130,
            min_zncc_search: 0.75,
            min_zncc: 0.85,
            min_margin: 0.03,
            min_eig: 4.0,
            max_level: 250.0,
            min_usable: 0.6,
        }
    }
}

/// One match.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StereoMatch {
    /// Left x (rectified pixels, integer position of the corner).
    pub x: f64,
    /// Left y.
    pub y: f64,
    /// Disparity = left x - right x (sub-pixel).
    pub disp: f64,
    /// Vertical offset = right y - left y (sub-pixel). Zero for a perfect calibration.
    pub dy: f64,
    /// ZNCC after refinement.
    pub zncc: f64,
    /// Unit ray in camera a.
    pub ray_a: Vec3,
    /// Unit ray in camera b.
    pub ray_b: Vec3,
}

/// Matches plus how many corners were tried.
#[derive(Clone, Debug, Default)]
pub struct MatchSet {
    /// Corners tried.
    pub candidates: usize,
    /// Accepted matches.
    pub matches: Vec<StereoMatch>,
    /// Rejected corners by reason: `[no valid window, low score, ambiguous, left-right check, sub-pixel]`.
    pub rejects: [usize; 5],
}

impl MatchSet {
    /// One-line summary of candidates, matches and rejections.
    pub fn summary(&self) -> String {
        let r = self.rejects;
        format!(
            "{} matches of {} corners (rejected: {} no window, {} low score, {} ambiguous, {} left-right, {} sub-pixel)",
            self.matches.len(),
            self.candidates,
            r[0],
            r[1],
            r[2],
            r[3],
            r[4]
        )
    }
}

struct Integral {
    w: usize,
    s: Vec<f64>,
    s2: Vec<f64>,
    bad: Vec<u32>,
}

impl Integral {
    fn new(img: &RectImage, w: usize, h: usize) -> Integral {
        let mut s = vec![0f64; (w + 1) * (h + 1)];
        let mut s2 = vec![0f64; (w + 1) * (h + 1)];
        let mut bad = vec![0u32; (w + 1) * (h + 1)];
        for y in 0..h {
            let (mut rs, mut rs2, mut rb) = (0f64, 0f64, 0u32);
            for x in 0..w {
                let i = y * w + x;
                let p = img.px[i] as f64;
                rs += p;
                rs2 += p * p;
                rb += (!img.valid[i]) as u32;
                let o = (y + 1) * (w + 1) + x + 1;
                s[o] = s[o - w - 1] + rs;
                s2[o] = s2[o - w - 1] + rs2;
                bad[o] = bad[o - w - 1] + rb;
            }
        }
        Integral { w, s, s2, bad }
    }

    /// (sum, sum of squares, invalid count) over the box centred at (x, y) with half size `h`.
    fn boxsum(&self, x: usize, y: usize, h: usize) -> (f64, f64, u32) {
        let (x0, y0, x1, y1) = (x - h, y - h, x + h + 1, y + h + 1);
        let w = self.w + 1;
        let f = |a: &[f64]| a[y1 * w + x1] - a[y0 * w + x1] - a[y1 * w + x0] + a[y0 * w + x0];
        let b = self.bad[y1 * w + x1] + self.bad[y0 * w + x0]
            - self.bad[y0 * w + x1]
            - self.bad[y1 * w + x0];
        (f(&self.s), f(&self.s2), b)
    }
}

/// Smaller eigenvalue of the gradient structure tensor over a (2h+1)^2 box at every pixel (0 near borders).
fn min_eig_map(img: &[f32], w: usize, h: usize, half: usize) -> Vec<f32> {
    let mut gxx = vec![0f64; (w + 1) * (h + 1)];
    let mut gyy = vec![0f64; (w + 1) * (h + 1)];
    let mut gxy = vec![0f64; (w + 1) * (h + 1)];
    for y in 0..h {
        let (mut a, mut b, mut c) = (0f64, 0f64, 0f64);
        for x in 0..w {
            let (gx, gy) = if x > 0 && x + 1 < w && y > 0 && y + 1 < h {
                (
                    (img[y * w + x + 1] - img[y * w + x - 1]) as f64 * 0.5,
                    (img[(y + 1) * w + x] - img[(y - 1) * w + x]) as f64 * 0.5,
                )
            } else {
                (0.0, 0.0)
            };
            a += gx * gx;
            b += gy * gy;
            c += gx * gy;
            let o = (y + 1) * (w + 1) + x + 1;
            gxx[o] = gxx[o - w - 1] + a;
            gyy[o] = gyy[o - w - 1] + b;
            gxy[o] = gxy[o - w - 1] + c;
        }
    }
    let mut out = vec![0f32; w * h];
    let n = ((2 * half + 1) * (2 * half + 1)) as f64;
    for y in half + 1..h.saturating_sub(half + 1) {
        for x in half + 1..w.saturating_sub(half + 1) {
            let (x0, y0, x1, y1) = (x - half, y - half, x + half + 1, y + half + 1);
            let ww = w + 1;
            let f = |q: &[f64]| {
                (q[y1 * ww + x1] - q[y0 * ww + x1] - q[y1 * ww + x0] + q[y0 * ww + x0]) / n
            };
            let (a, b, c) = (f(&gxx), f(&gyy), f(&gxy));
            out[y * w + x] = ((a + b) / 2.0 - (((a - b) / 2.0).powi(2) + c * c).sqrt()) as f32;
        }
    }
    out
}

struct Template {
    vals: Vec<f32>,
    norm: f64,
}

fn template(img: &[f32], w: usize, x: usize, y: usize, half: usize) -> Template {
    let mut vals = Vec::with_capacity((2 * half + 1).pow(2));
    for yy in y - half..=y + half {
        vals.extend_from_slice(&img[yy * w + x - half..=yy * w + x + half]);
    }
    let mean = vals.iter().map(|&v| v as f64).sum::<f64>() / vals.len() as f64;
    let mut norm = 0.0;
    for v in vals.iter_mut() {
        *v -= mean as f32;
        norm += (*v as f64) * (*v as f64);
    }
    Template {
        vals,
        norm: norm.sqrt(),
    }
}

/// ZNCC of a zero-mean template against the window of `img` centred at (x, y) (the window must be valid).
fn zncc_at(
    t: &Template,
    img: &[f32],
    ii: &Integral,
    w: usize,
    x: usize,
    y: usize,
    half: usize,
) -> f64 {
    let (s, s2, bad) = ii.boxsum(x, y, half);
    if bad > 0 {
        return -2.0;
    }
    let n = ((2 * half + 1) * (2 * half + 1)) as f64;
    let var = s2 - s * s / n;
    if var <= 1e-6 || t.norm <= 1e-6 {
        return -2.0;
    }
    let side = 2 * half + 1;
    let mut acc = 0f32;
    for r in 0..side {
        let row = &img[(y - half + r) * w + x - half..(y - half + r) * w + x + half + 1];
        let tv = &t.vals[r * side..(r + 1) * side];
        for (a, b) in tv.iter().zip(row) {
            acc += a * b;
        }
    }
    acc as f64 / (t.norm * var.sqrt())
}

struct Peak {
    x: usize,
    y: usize,
    score: f64,
    second: f64,
}

#[allow(clippy::too_many_arguments)]
fn band_search(
    t: &Template,
    img: &[f32],
    ii: &Integral,
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    sign: i64,
    cfg: &MatchConfig,
) -> Option<Peak> {
    let half = cfg.half as i64;
    let mut scores: Vec<(i64, i64, f64)> = Vec::new();
    for dy in -(cfg.dy_search as i64)..=cfg.dy_search as i64 {
        let by = y as i64 + dy;
        if by < half + 1 || by >= h as i64 - half - 1 {
            continue;
        }
        for d in cfg.min_disp as i64..=cfg.max_disp as i64 {
            let bx = x as i64 - sign * d;
            if bx < half + 1 || bx >= w as i64 - half - 1 {
                continue;
            }
            let s = zncc_at(t, img, ii, w, bx as usize, by as usize, cfg.half);
            if s > -1.5 {
                scores.push((bx, by, s));
            }
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
    Some(Peak {
        x: best.0 as usize,
        y: best.1 as usize,
        score: best.2,
        second,
    })
}

fn sample(img: &[f32], w: usize, x: f64, y: f64) -> f32 {
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let fx = (x - x0 as f64) as f32;
    let fy = (y - y0 as f64) as f32;
    let i = y0 * w + x0;
    img[i] * (1.0 - fx) * (1.0 - fy)
        + img[i + 1] * fx * (1.0 - fy)
        + img[i + w] * (1.0 - fx) * fy
        + img[i + w + 1] * fx * fy
}

fn gradients(img: &[f32], w: usize, h: usize) -> (Vec<f32>, Vec<f32>) {
    let mut gx = vec![0f32; w * h];
    let mut gy = vec![0f32; w * h];
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let i = y * w + x;
            gx[i] = (img[i + 1] - img[i - 1]) * 0.5;
            gy[i] = (img[i + w] - img[i - w]) * 0.5;
        }
    }
    (gx, gy)
}

fn solve4(h: &mut [[f64; 4]; 4], g: &mut [f64; 4]) -> Option<[f64; 4]> {
    // Gaussian elimination with partial pivoting
    for c in 0..4 {
        let p = (c..4).max_by(|&a, &b| h[a][c].abs().partial_cmp(&h[b][c].abs()).unwrap())?;
        if h[p][c].abs() < 1e-12 {
            return None;
        }
        h.swap(c, p);
        g.swap(c, p);
        for r in c + 1..4 {
            let f = h[r][c] / h[c][c];
            for k in c..4 {
                h[r][k] -= f * h[c][k];
            }
            g[r] -= f * g[c];
        }
    }
    let mut x = [0.0; 4];
    for c in (0..4).rev() {
        let mut s = g[c];
        for k in c + 1..4 {
            s -= h[c][k] * x[k];
        }
        x[c] = s / h[c][c];
    }
    Some(x)
}

/// Lucas-Kanade refinement of the right-image position `(bx, by)` of the left patch at integer `(x, y)`:
/// minimises `sum g(p) [alpha R(b + p) + beta - L(x + p)]^2` over translation, gain and bias, with a Gaussian
/// window `g`. Samples marked in `sat_l` / `sat_r` (clipped pixels and their neighbourhood) get no weight: a
/// clipped edge sits at a different radiance level in each camera (vignetting, angle), so it would bias the
/// position, while unclipped pixels follow the gain/bias model exactly. At least `min_usable` of the window
/// weight must remain. Returns the refined position and the ZNCC there (usable samples only).
#[allow(clippy::too_many_arguments)]
pub(crate) fn lk_refine(
    left: &[f32],
    right: &RectImage,
    rgx: &[f32],
    rgy: &[f32],
    sat_l: &[bool],
    sat_r: &[bool],
    w: usize,
    h: usize,
    x: usize,
    y: usize,
    bx: f64,
    by: f64,
    half: usize,
    min_usable: f64,
) -> Option<(f64, f64, f64)> {
    let hf = half as f64;
    let sigma = hf / 1.5;
    let side = 2 * half + 1;
    let mut wgt = Vec::with_capacity(side * side);
    for i in 0..side {
        for j in 0..side {
            let (di, dj) = (i as f64 - hf, j as f64 - hf);
            let lsat = sat_l[(y + i - half) * w + x + j - half];
            let g = (-(di * di + dj * dj) / (2.0 * sigma * sigma)).exp();
            wgt.push(if lsat { 0.0 } else { g });
        }
    }
    let total: f64 = (0..side * side)
        .map(|k| {
            let (di, dj) = ((k / side) as f64 - hf, (k % side) as f64 - hf);
            (-(di * di + dj * dj) / (2.0 * sigma * sigma)).exp()
        })
        .sum();
    let (mut px, mut py, mut alpha, mut beta) = (bx, by, 1.0f64, 0.0f64);
    let inside = |px: f64, py: f64| {
        px - hf >= 1.0 && py - hf >= 1.0 && px + hf < (w - 2) as f64 && py + hf < (h - 2) as f64
    };
    // weight of sample (i, j) at the current right position
    let usable = |k: usize, sx: f64, sy: f64| -> f64 {
        if wgt[k] == 0.0 || sat_r[sy.floor() as usize * w + sx.floor() as usize] {
            0.0
        } else {
            wgt[k]
        }
    };
    let mut converged = false;
    for _ in 0..30 {
        if !inside(px, py) {
            return None;
        }
        let mut hm = [[0f64; 4]; 4];
        let mut g = [0f64; 4];
        let mut used = 0.0;
        for i in 0..side {
            for j in 0..side {
                let (dx, dy) = (j as f64 - hf, i as f64 - hf);
                let (sx, sy) = (px + dx, py + dy);
                let ww = usable(i * side + j, sx, sy);
                if ww == 0.0 {
                    continue;
                }
                used += ww;
                let r = sample(&right.px, w, sx, sy) as f64;
                let rx = sample(rgx, w, sx, sy) as f64;
                let ry = sample(rgy, w, sx, sy) as f64;
                let l = left[(y + i - half) * w + x + j - half] as f64;
                let e = alpha * r + beta - l;
                let jac = [alpha * rx, alpha * ry, r, 1.0];
                for a in 0..4 {
                    g[a] += ww * jac[a] * e;
                    for b in 0..4 {
                        hm[a][b] += ww * jac[a] * jac[b];
                    }
                }
            }
        }
        if used < min_usable * total {
            return None;
        }
        let d = solve4(&mut hm, &mut g)?;
        px -= d[0];
        py -= d[1];
        alpha -= d[2];
        beta -= d[3];
        if !(0.2..5.0).contains(&alpha) {
            return None;
        }
        if d[0] * d[0] + d[1] * d[1] < 1e-8 {
            converged = true;
            break;
        }
    }
    if !converged || !inside(px, py) {
        return None;
    }
    // every sample must come from valid pixels (inside the tile and the lens circle)
    let (xi, yi) = (px.floor() as usize, py.floor() as usize);
    for yy in yi - half..=yi + half + 1 {
        for xx in xi - half..=xi + half + 1 {
            if !right.valid[yy * w + xx] {
                return None;
            }
        }
    }
    // ZNCC at the refined position over the usable samples
    let (mut sl, mut sr, mut sll, mut srr, mut slr, mut n) = (0f64, 0f64, 0f64, 0f64, 0f64, 0f64);
    for i in 0..side {
        for j in 0..side {
            let (sx, sy) = (px + j as f64 - hf, py + i as f64 - hf);
            if usable(i * side + j, sx, sy) == 0.0 {
                continue;
            }
            let l = left[(y + i - half) * w + x + j - half] as f64;
            let r = sample(&right.px, w, sx, sy) as f64;
            sl += l;
            sr += r;
            sll += l * l;
            srr += r * r;
            slr += l * r;
            n += 1.0;
        }
    }
    if n < 9.0 {
        return None;
    }
    let cov = slr - sl * sr / n;
    let vl = sll - sl * sl / n;
    let vr = srr - sr * sr / n;
    if vl <= 1e-9 || vr <= 1e-9 {
        return None;
    }
    Some((px, py, cov / (vl * vr).sqrt()))
}

/// Pixels at or above `max_level`, dilated by `r` (clipped pixels contaminate bilinear neighbours, and at the
/// edge of the rectified view one source pixel covers more than one rectified pixel).
fn saturation_mask(img: &RectImage, w: usize, h: usize, max_level: f32, r: usize) -> Vec<bool> {
    let mut m = vec![false; w * h];
    for y in 0..h {
        for x in 0..w {
            if img.px[y * w + x] >= max_level {
                for yy in y.saturating_sub(r)..(y + r + 1).min(h) {
                    for xx in x.saturating_sub(r)..(x + r + 1).min(w) {
                        m[yy * w + xx] = true;
                    }
                }
            }
        }
    }
    m
}

/// Match the left image of a rectified pair against the right one.
pub fn match_pair(
    g: &RectGeom,
    left: &RectImage,
    right: &RectImage,
    cfg: &MatchConfig,
) -> MatchSet {
    let (w, h) = (g.width, g.height);
    let sat_l = saturation_mask(left, w, h, cfg.max_level, 2);
    let sat_r = saturation_mask(right, w, h, cfg.max_level, 2);
    let eig = min_eig_map(&left.px, w, h, cfg.half);
    let ii_r = Integral::new(right, w, h);
    let ii_l = Integral::new(left, w, h);
    let (rgx, rgy) = gradients(&right.px, w, h);
    let margin = cfg.half.max(cfg.lk_half) + 3;
    let mut out = MatchSet::default();
    let mut cy = margin;
    while cy + cfg.cell < h - margin {
        let mut cx = margin;
        while cx + cfg.cell < w - margin {
            let mut best: Option<(usize, usize, f32)> = None;
            for y in cy..cy + cfg.cell {
                for x in cx..cx + cfg.cell {
                    let e = eig[y * w + x];
                    if best.is_none_or(|b| e > b.2) {
                        best = Some((x, y, e));
                    }
                }
            }
            cx += cfg.cell;
            let Some((x, y, e)) = best else { continue };
            if (e as f64) < cfg.min_eig || ii_l.boxsum(x, y, cfg.lk_half + 1).2 > 0 {
                continue;
            }
            let k = cfg.lk_half;
            let clipped = (y - k..=y + k)
                .flat_map(|yy| (x - k..=x + k).map(move |xx| (xx, yy)))
                .filter(|&(xx, yy)| sat_l[yy * w + xx])
                .count();
            if clipped as f64 > (1.0 - cfg.min_usable) * ((2 * k + 1) * (2 * k + 1)) as f64 {
                continue;
            }
            out.candidates += 1;
            let sat = (&sat_l[..], &sat_r[..]);
            match match_one(g, left, right, &ii_l, &ii_r, &rgx, &rgy, sat, x, y, cfg) {
                Ok(m) => out.matches.push(m),
                Err(r) => out.rejects[r as usize] += 1,
            }
        }
        cy += cfg.cell;
    }
    out
}

/// Why a corner was not matched (index into [`MatchSet::rejects`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Reject {
    NoWindow = 0,
    Score = 1,
    Ambiguous = 2,
    LeftRight = 3,
    SubPixel = 4,
}

#[allow(clippy::too_many_arguments)]
fn match_one(
    g: &RectGeom,
    left: &RectImage,
    right: &RectImage,
    ii_l: &Integral,
    ii_r: &Integral,
    rgx: &[f32],
    rgy: &[f32],
    (sat_l, sat_r): (&[bool], &[bool]),
    x: usize,
    y: usize,
    cfg: &MatchConfig,
) -> Result<StereoMatch, Reject> {
    let (w, h) = (g.width, g.height);
    let t = template(&left.px, w, x, y, cfg.half);
    let f = band_search(&t, &right.px, ii_r, w, h, x, y, 1, cfg).ok_or(Reject::NoWindow)?;
    if f.score < cfg.min_zncc_search {
        return Err(Reject::Score);
    }
    if f.score - f.second < cfg.min_margin {
        return Err(Reject::Ambiguous);
    }
    // left-right consistency
    let tb = template(&right.px, w, f.x, f.y, cfg.half);
    let back =
        band_search(&tb, &left.px, ii_l, w, h, f.x, f.y, -1, cfg).ok_or(Reject::LeftRight)?;
    if (back.x as i64 - x as i64).abs() > 1 || (back.y as i64 - y as i64).abs() > 1 {
        return Err(Reject::LeftRight);
    }
    let (bx, by, z) = lk_refine(
        &left.px,
        right,
        rgx,
        rgy,
        sat_l,
        sat_r,
        w,
        h,
        x,
        y,
        f.x as f64,
        f.y as f64,
        cfg.lk_half,
        cfg.min_usable,
    )
    .ok_or(Reject::SubPixel)?;
    if z < cfg.min_zncc || (bx - f.x as f64).abs() > 1.5 || (by - f.y as f64).abs() > 1.5 {
        return Err(Reject::SubPixel);
    }
    let (xf, yf) = (x as f64, y as f64);
    Ok(StereoMatch {
        x: xf,
        y: yf,
        disp: xf - bx,
        dy: by - yf,
        zncc: z,
        ray_a: g.ray(0, xf, yf),
        ray_b: g.ray(1, bx, by),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::geom::{Pose, IDENTITY};

    /// Smooth random texture: sum of a few random sinusoid products.
    pub(crate) fn texture(w: usize, h: usize, seed: u64) -> impl Fn(f64, f64) -> f64 {
        let mut s = seed;
        let mut rnd = move || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 11) as f64) / ((1u64 << 53) as f64)
        };
        let waves: Vec<(f64, f64, f64, f64)> = (0..24)
            .map(|_| {
                let a = rnd() * std::f64::consts::TAU;
                let f = 0.08 + 0.35 * rnd();
                (f * a.cos(), f * a.sin(), rnd() * 6.3, 10.0 + 25.0 * rnd())
            })
            .collect();
        let _ = (w, h);
        move |x: f64, y: f64| {
            let mut v = 128.0;
            for &(kx, ky, ph, amp) in &waves {
                v += amp * (kx * x + ky * y + ph).sin() / 3.0;
            }
            v.clamp(0.0, 255.0)
        }
    }

    fn image(w: usize, h: usize, f: &dyn Fn(f64, f64) -> f64) -> RectImage {
        let mut px = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                px[y * w + x] = f(x as f64, y as f64) as f32;
            }
        }
        RectImage {
            px,
            valid: vec![true; w * h],
        }
    }

    #[test]
    fn subpixel_shift_is_recovered_without_pixel_locking() {
        let (w, h) = (200usize, 160usize);
        let tex = texture(w, h, 11);
        let left = image(w, h, &tex);
        let g = RectGeom::new(
            &Pose {
                r: IDENTITY,
                t: [-0.1, 0.0, 0.0],
            },
            w,
            h,
            100.0,
        );
        let cfg = MatchConfig {
            max_disp: 20,
            min_disp: -4,
            dy_search: 4,
            cell: 16,
            min_eig: 1.0,
            ..Default::default()
        };
        // several fractional shifts: the mean error must be tiny for each (no bias toward integers)
        for &(dx, dy) in &[(7.0, 0.0), (7.25, 0.4), (6.5, -0.5), (9.8, 1.3)] {
            let right = image(w, h, &|x, y| tex(x + dx, y - dy));
            let ms = match_pair(&g, &left, &right, &cfg);
            assert!(
                ms.matches.len() >= 20,
                "{} matches for {dx},{dy}",
                ms.matches.len()
            );
            let n = ms.matches.len() as f64;
            let mdy = ms.matches.iter().map(|m| m.dy).sum::<f64>() / n;
            let mdx = ms.matches.iter().map(|m| m.disp).sum::<f64>() / n;
            assert!((mdy - dy).abs() < 0.02, "dy {mdy} vs {dy}");
            assert!((mdx - dx).abs() < 0.02, "disp {mdx} vs {dx}");
            let worst = ms
                .matches
                .iter()
                .map(|m| (m.dy - dy).abs())
                .fold(0.0, f64::max);
            assert!(worst < 0.1, "worst dy error {worst}");
        }
    }

    #[test]
    fn clipped_texture_does_not_bias_the_position() {
        // The right camera sees the scene 25 % darker (vignetting, angle), so the bright texture clips at a
        // different radiance level in each image. Weighting the clipped pixels out keeps the sub-pixel
        // position unbiased.
        let (w, h) = (220usize, 170usize);
        let tex = texture(w, h, 5);
        let (dx, dy) = (7.3, 0.45);
        let left = image(w, h, &|x, y| (tex(x, y) * 2.2 - 100.0).clamp(0.0, 255.0));
        let right = image(w, h, &|x, y| {
            ((tex(x + dx, y - dy) * 2.2 - 100.0) * 0.75).clamp(0.0, 255.0)
        });
        let g = RectGeom::new(
            &Pose {
                r: IDENTITY,
                t: [-0.1, 0.0, 0.0],
            },
            w,
            h,
            80.0,
        );
        let cfg = MatchConfig {
            min_eig: 1.0,
            max_disp: 20,
            min_disp: -4,
            dy_search: 4,
            cell: 12,
            max_level: 250.0,
            ..Default::default()
        };
        let clipped = left.px.iter().filter(|&&v| v >= 250.0).count() as f64 / left.px.len() as f64;
        assert!(clipped > 0.05, "test texture must clip: {clipped}");
        let ms = match_pair(&g, &left, &right, &cfg);
        assert!(ms.matches.len() >= 30, "{}", ms.summary());
        let n = ms.matches.len() as f64;
        let mdy = ms.matches.iter().map(|m| m.dy).sum::<f64>() / n;
        let worst = ms
            .matches
            .iter()
            .map(|m| (m.dy - dy).abs())
            .fold(0.0, f64::max);
        assert!((mdy - dy).abs() < 0.02, "mean dy {mdy} vs {dy}");
        assert!(worst < 0.1, "worst dy error {worst}");
    }

    #[test]
    fn invalid_regions_are_not_matched() {
        let (w, h) = (160usize, 120usize);
        let tex = texture(w, h, 3);
        let left = image(w, h, &tex);
        let mut right = image(w, h, &|x, y| tex(x + 5.0, y));
        for v in right.valid.iter_mut() {
            *v = false;
        }
        let g = RectGeom::new(
            &Pose {
                r: IDENTITY,
                t: [-0.1, 0.0, 0.0],
            },
            w,
            h,
            80.0,
        );
        let ms = match_pair(&g, &left, &right, &MatchConfig::default());
        assert!(ms.matches.is_empty());
    }
}
