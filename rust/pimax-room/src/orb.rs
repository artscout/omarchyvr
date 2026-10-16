//! ORB features for 8-bit grey images: FAST-9 corners on an image pyramid, intensity-centroid orientation, steered
//! BRIEF (256-bit) descriptors and brute-force Hamming matching.
//!
//! Built for the 640x480 grey fisheye tracking cameras of the headset (relocalisation / room-map layer), but it works
//! on any grey image. Pure safe Rust, `std` only, single-threaded and deterministic: the same input gives the same
//! output (across platforms only the last bit of `sin`/`cos`/`atan2` of the steering angle may differ, which can flip
//! the odd descriptor bit whose two samples are all but equal).
//!
//! Written from the published descriptions (Rosten and Drummond: FAST; Calonder et al.: BRIEF; Rublee et al.: ORB).
//! No code from OpenCV or ORB-SLAM (GPL) was used.
//!
//! # Pipeline
//!
//! 1. **Pyramid.** `n_levels` levels; level `l` is the previous level resampled bilinearly by `scale`, sampled on a
//!    corner-aligned grid: pixel `(i, j)` of level `l` is the image value at `(i * scale^l, j * scale^l)` in level-0
//!    pixel coordinates, so a level-`l` position converts to level 0 by a plain multiplication with `scale^l`.
//!    Levels whose valid area would be tiny are dropped.
//! 2. **Detection.** FAST-9 on the 16-pixel circle of radius 3; the score of a corner is the largest threshold at which
//!    it is still a corner; 3x3 non-maximum suppression. Each level's valid area (the image minus a 22 pixel border,
//!    so that the rotated 31x31 patch always fits) is cut into `grid_cols x grid_rows` cells. Every cell is detected at
//!    `fast_threshold`, and retried at `min_fast_threshold` if it came up empty. The `mask` is applied to every corner.
//!    The strongest corners of each cell by FAST score are ranked by Harris response (7x7 block of Sobel gradients,
//!    `k = 0.04`).
//! 3. **Spreading.** Each level has a budget proportional to its area. The best corner of every cell is taken first
//!    (strongest cells first), then the budget is filled by response. Two keypoints of a level are never closer than
//!    3 level pixels (they would be the same corner twice). A level that cannot fill its budget leaves the shortfall
//!    to the strongest unused corners of the other levels, so the total reaches `n_features` whenever enough corners
//!    exist.
//! 4. **Orientation.** Intensity centroid over the disc of radius 15 at the keypoint's level, on the smoothed image.
//! 5. **Descriptor.** Steered BRIEF: 256 intensity comparisons between point pairs of a fixed pattern inside the
//!    31x31 patch. The pattern is rotated by the keypoint angle and sampled bilinearly at the exact rotated
//!    positions, on the level image smoothed by a 7x7 Gaussian (sigma 2).
//!
//! # Example
//!
//! ```ignore
//! use pimax_room::orb::{extract, match_bf, Image, OrbParams};
//!
//! let (w, h) = (640, 480);
//! let (frame_a, frame_b): (Vec<u8>, Vec<u8>) = /* two 8-bit grey frames, w * h bytes each */;
//! let params = OrbParams::default();
//! // Keep keypoints off the black rim of the fisheye circle, with room for their patches (22 to 38 pixels around them).
//! let inside = |x: f32, y: f32| (x - 319.5).hypot(y - 239.5) < 250.0;
//! let a = extract(&Image::new(w, h, &frame_a).unwrap(), Some(&inside), &params);
//! let b = extract(&Image::new(w, h, &frame_b).unwrap(), Some(&inside), &params);
//! let (da, db): (Vec<_>, Vec<_>) = (a.iter().map(|k| k.1).collect(), b.iter().map(|k| k.1).collect());
//! for (i, j, bits) in match_bf(&da, &db, 64, 0.8) {
//!     println!("{:?} -> {:?} ({bits} bits apart)", a[i].0, b[j].0);
//! }
//! ```
//!
//! # Tuning
//!
//! The FAST thresholds are absolute grey levels (`fast_threshold` 20, retry at 7 for empty cells). Dark or low-contrast
//! frames (a dim room seen by an IR camera, say) starve the detector with those defaults: scale the thresholds down
//! with the contrast (about 8 and 3 for a frame whose texture spans 40 grey levels) or stretch the frame first.
//!
//! # Conventions
//!
//! * Pixel `(x, y)` has its centre at integer coordinates; `x` grows to the right, `y` downwards.
//! * [`Keypoint::angle`] is in radians in `(-pi, pi]`, measured from +x towards +y (so clockwise on screen).
//! * [`Keypoint::x`] / [`Keypoint::y`] are level-0 pixel coordinates. They are integers at level 0 and multiples of
//!   `scale^level` above; there is no sub-pixel refinement.
//! * [`Keypoint::response`] is the Harris response at the keypoint's own level, with intensities scaled to `[0, 1]`
//!   and the structure tensor averaged over the 7x7 block (a corner of full contrast scores about `1e-3`). Comparable
//!   within a level, only roughly across levels.
//! * Descriptor bit `k` (byte `k / 8`, bit `k % 8`, least significant first) is set when the smoothed intensity at
//!   the first point of pair `k` is lower than at the second.
//!
//! # Compatibility
//!
//! The BRIEF pattern is **not** OpenCV's learned rBRIEF pattern: it is generated at compile time by a fixed-seed
//! splitmix64 generator (integer arithmetic only, so it is bit-identical on every platform): isotropic Gaussian with
//! sigma = 31 / 5 per axis, rounded and clamped to the 31x31 patch, pairs closer than 4 pixels redrawn. Descriptors
//! from this module only compare with descriptors from this module. Store [`DESCRIPTOR_VERSION`] next to any
//! persisted descriptors and discard them when it changes.
#![forbid(unsafe_code)]
#![warn(missing_docs)]

use std::cmp::Ordering;
use std::fmt;

/// Version of the descriptor definition (pattern, smoothing, steering, bit order). Bumped whenever descriptors computed
/// by an older build would no longer compare with new ones; persist it next to stored descriptors.
pub const DESCRIPTOR_VERSION: u32 = 1;

// ---------------------------------------------------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------------------------------------------------

/// Borrowed grey image (row-major, `stride` bytes per row).
#[derive(Clone, Copy)]
pub struct Image<'a> {
    /// Width in pixels.
    pub w: usize,
    /// Height in pixels.
    pub h: usize,
    /// Bytes from the start of one row to the start of the next (`>= w`).
    pub stride: usize,
    /// Pixels: `data[y * stride + x]`. Must hold at least `(h - 1) * stride + w` bytes.
    pub data: &'a [u8],
}

impl<'a> Image<'a> {
    /// A tightly packed image (`stride == w`). `None` if `w` or `h` is zero or `data` is too short.
    pub fn new(w: usize, h: usize, data: &'a [u8]) -> Option<Self> {
        let img = Image {
            w,
            h,
            stride: w,
            data,
        };
        img.is_valid().then_some(img)
    }

    /// True if the geometry is consistent (`w, h > 0`, `stride >= w`) and `data` is long enough for it.
    pub fn is_valid(&self) -> bool {
        if self.w == 0 || self.h == 0 || self.stride < self.w {
            return false;
        }
        match (self.h - 1)
            .checked_mul(self.stride)
            .and_then(|v| v.checked_add(self.w))
        {
            Some(need) => self.data.len() >= need,
            None => false,
        }
    }
}

impl fmt::Debug for Image<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Image")
            .field("w", &self.w)
            .field("h", &self.h)
            .field("stride", &self.stride)
            .field("data_len", &self.data.len())
            .finish()
    }
}

/// Keypoint in level-0 pixel coordinates.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Keypoint {
    /// Column in level-0 pixels (`x_level * scale^level`).
    pub x: f32,
    /// Row in level-0 pixels (`y_level * scale^level`).
    pub y: f32,
    /// Pyramid level the keypoint was detected at (0 = full resolution).
    pub level: u8,
    /// Orientation in radians, `(-pi, pi]`, from +x towards +y (intensity centroid direction).
    pub angle: f32,
    /// Harris response at the keypoint's level (see the module docs for the scaling).
    pub response: f32,
}

/// 256-bit binary descriptor.
pub type Descriptor = [u8; 32];

/// Extraction parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct OrbParams {
    /// Total number of keypoints wanted (upper bound of what [`extract`] returns; clamped to 1048576). Default 500.
    pub n_features: usize,
    /// Number of pyramid levels (clamped to 1..=32; levels that become too small are dropped). Default 4.
    pub n_levels: usize,
    /// Scale factor between consecutive levels (clamped to 1..=8). Default 1.2.
    pub scale: f32,
    /// FAST threshold used first in every grid cell. Default 20.
    pub fast_threshold: u8,
    /// FAST threshold of the retry in cells that found no corner at `fast_threshold`. Default 7.
    pub min_fast_threshold: u8,
    /// Grid columns per level for the spatial spreading (clamped to 1..=64). Default 8.
    pub grid_cols: usize,
    /// Grid rows per level for the spatial spreading (clamped to 1..=64). Default 6.
    pub grid_rows: usize,
}

impl Default for OrbParams {
    fn default() -> Self {
        OrbParams {
            n_features: 500,
            n_levels: 4,
            scale: 1.2,
            fast_threshold: 20,
            min_fast_threshold: 7,
            grid_cols: 8,
            grid_rows: 6,
        }
    }
}

impl OrbParams {
    /// Size of a level-`level` pixel in level-0 pixels (`scale^level`, with `scale` clamped like [`extract`] does);
    /// also the factor between a keypoint's patch size at its own level and in level-0 pixels.
    pub fn level_scale(&self, level: u8) -> f32 {
        sane_scale(self.scale).powi(i32::from(level)) as f32
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Constants and internal configuration
// ---------------------------------------------------------------------------------------------------------------------

/// Keypoints stay this many (level) pixels away from the image border: the 31x31 patch rotated by any angle reaches
/// 15 * sqrt(2) = 21.2 pixels from its centre.
const BORDER: usize = 22;
/// A level needs at least this many pixels per side to take part (border on both sides plus a usable area).
const MIN_LEVEL_DIM: usize = 2 * BORDER + 16;
/// Radius of the circular patch of the intensity centroid.
const IC_RADIUS: usize = 15;
/// Two keypoints of the same level are at least this far apart (squared, in level pixels).
const MIN_KP_DIST2: usize = 9;
/// Maximum number of pyramid levels (the level index must fit a `u8`).
const MAX_LEVELS: usize = 32;
/// Upper clamp of `n_features` (far beyond any sensible use; keeps the budget arithmetic far from overflow).
const MAX_FEATURES: usize = 1 << 20;
/// Harris `k`.
const HARRIS_K: f64 = 0.04;
/// Normalisation of the Harris response: 49 pixels of the block, Sobel gradient of 8 * 255 for a unit intensity step.
const HARRIS_NORM: f64 = 1.0 / (49.0 * 49.0 * 2040.0 * 2040.0 * 2040.0 * 2040.0);
/// Gaussian 7-tap kernel (sigma 2), integer weights summing to 256.
const GAUSS7: [u16; 7] = [18, 34, 49, 54, 49, 34, 18];

fn sane_scale(scale: f32) -> f64 {
    if scale >= 1.0 {
        f64::from(scale).min(8.0)
    } else {
        1.0
    }
}

/// Sanitised parameters.
struct Cfg {
    n_features: usize,
    n_levels: usize,
    scale: f64,
    thr: u8,
    min_thr: u8,
    cols: usize,
    rows: usize,
}

impl Cfg {
    fn new(p: &OrbParams) -> Self {
        Cfg {
            n_features: p.n_features.min(MAX_FEATURES),
            n_levels: p.n_levels.clamp(1, MAX_LEVELS),
            scale: sane_scale(p.scale),
            thr: p.fast_threshold,
            min_thr: p.min_fast_threshold.min(p.fast_threshold),
            cols: p.grid_cols.clamp(1, 64),
            rows: p.grid_rows.clamp(1, 64),
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// BRIEF pattern (compile-time, integer only)
// ---------------------------------------------------------------------------------------------------------------------

/// Seed of the pattern generator (arbitrary, fixed forever: changing it changes every descriptor).
const PATTERN_SEED: u64 = 0x0B5E_55ED_0123_4567;
/// Point pairs closer than this (squared) are redrawn: with the smoothing in use they would compare nearly equal values.
const MIN_PAIR_SEP2: i32 = 16;

/// One step of splitmix64 (Vigna, public domain): returns `(new_state, output)`.
const fn splitmix64(state: u64) -> (u64, u64) {
    let state = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    let mut z = state;
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (state, z ^ (z >> 31))
}

/// One pattern coordinate: a Gaussian with sigma 6.2 (= 31 / 5), rounded, clamped to -15..=15. The Gaussian is built
/// from the sum of twelve uniform 16-bit values (Irwin-Hall: mean 393210, standard deviation 65536), so no floating
/// point is needed and the result is identical on every platform.
const fn gauss_coord(state: u64) -> (u64, i32) {
    let mut st = state;
    let mut sum: i64 = 0;
    let mut n = 0;
    while n < 3 {
        let (next, z) = splitmix64(st);
        st = next;
        sum += (z & 0xFFFF) as i64
            + ((z >> 16) & 0xFFFF) as i64
            + ((z >> 32) & 0xFFFF) as i64
            + (z >> 48) as i64;
        n += 1;
    }
    // (sum - mean) / 65536 is N(0, 1); times 6.2 = 62 / 10, rounded to nearest.
    let num = (sum - 393_210) * 62;
    let v = (num + 327_680).div_euclid(655_360);
    let v = if v > 15 {
        15
    } else if v < -15 {
        -15
    } else {
        v
    };
    (st, v as i32)
}

/// `[ax, ay, bx, by]` of the 256 comparisons.
const fn make_pattern() -> [[i8; 4]; 256] {
    let mut pat = [[0i8; 4]; 256];
    let mut st = PATTERN_SEED;
    let mut k = 0;
    while k < 256 {
        let (s1, ax) = gauss_coord(st);
        let (s2, ay) = gauss_coord(s1);
        let (s3, bx) = gauss_coord(s2);
        let (s4, by) = gauss_coord(s3);
        st = s4;
        let (dx, dy) = (ax - bx, ay - by);
        if dx * dx + dy * dy < MIN_PAIR_SEP2 {
            continue;
        }
        pat[k] = [ax as i8, ay as i8, bx as i8, by as i8];
        k += 1;
    }
    pat
}

static PATTERN: [[i8; 4]; 256] = make_pattern();

/// Half-width of the intensity-centroid disc for each row offset `|v|`.
const UMAX: [usize; IC_RADIUS + 1] = make_umax();

const fn make_umax() -> [usize; IC_RADIUS + 1] {
    let mut t = [0usize; IC_RADIUS + 1];
    let mut v = 0;
    while v <= IC_RADIUS {
        let mut u = IC_RADIUS;
        while u * u + v * v > IC_RADIUS * IC_RADIUS {
            u -= 1;
        }
        t[v] = u;
        v += 1;
    }
    t
}

// ---------------------------------------------------------------------------------------------------------------------
// Pyramid
// ---------------------------------------------------------------------------------------------------------------------

/// Geometry of one pyramid level.
#[derive(Clone, Copy)]
struct Geom {
    w: usize,
    h: usize,
    /// Size of a level pixel in level-0 pixels (`scale^level`).
    scale: f64,
}

/// Sizes of the levels that take part. Empty if level 0 is too small. `img_w, img_h > 0`.
fn level_geometry(img_w: usize, img_h: usize, cfg: &Cfg) -> Vec<Geom> {
    let mut v = Vec::with_capacity(cfg.n_levels);
    for l in 0..cfg.n_levels {
        let s = cfg.scale.powi(l as i32);
        let w = ((img_w - 1) as f64 / s).floor() as usize + 1;
        let h = ((img_h - 1) as f64 / s).floor() as usize + 1;
        if w < MIN_LEVEL_DIM || h < MIN_LEVEL_DIM {
            break;
        }
        v.push(Geom { w, h, scale: s });
    }
    v
}

/// Bilinear resampling on a corner-aligned grid: `dst(x, y) = src(x * scale, y * scale)`, 8-bit fixed-point weights.
fn resize(src: &Image, dw: usize, dh: usize, scale: f64) -> Vec<u8> {
    let (sw, sh) = (src.w, src.h);
    // Per output column: left and right source column and their weights (summing to 256).
    let cols: Vec<(usize, usize, u32, u32)> = (0..dw)
        .map(|x| {
            let pos = x as f64 * scale;
            let x0 = (pos as usize).min(sw - 1);
            let x1 = (x0 + 1).min(sw - 1);
            let f = (((pos - x0 as f64) * 256.0 + 0.5) as u32).min(256);
            (x0, x1, 256 - f, f)
        })
        .collect();
    let mut dst = vec![0u8; dw * dh];
    let mut vrow = vec![0u16; sw];
    for (y, out) in dst.chunks_exact_mut(dw).enumerate() {
        let pos = y as f64 * scale;
        let y0 = (pos as usize).min(sh - 1);
        let y1 = (y0 + 1).min(sh - 1);
        let f = (((pos - y0 as f64) * 256.0 + 0.5) as u32).min(256);
        let r0 = &src.data[y0 * src.stride..y0 * src.stride + sw];
        let r1 = &src.data[y1 * src.stride..y1 * src.stride + sw];
        // Vertical blend first (vectorises), scaled by 256: at most 255 * 256.
        for ((v, &a), &b) in vrow.iter_mut().zip(r0).zip(r1) {
            *v = (u32::from(a) * (256 - f) + u32::from(b) * f) as u16;
        }
        for (o, &(x0, x1, wl, wr)) in out.iter_mut().zip(&cols) {
            *o = ((u32::from(vrow[x0]) * wl + u32::from(vrow[x1]) * wr + 32768) >> 16) as u8;
        }
    }
    dst
}

/// The images of levels `1..` (level 0 is the caller's image).
fn build_pyramid(img: &Image, geom: &[Geom], scale: f64) -> Vec<Vec<u8>> {
    let mut bufs: Vec<Vec<u8>> = Vec::with_capacity(geom.len().saturating_sub(1));
    for l in 1..geom.len() {
        let next = {
            let prev = if l == 1 {
                *img
            } else {
                Image {
                    w: geom[l - 1].w,
                    h: geom[l - 1].h,
                    stride: geom[l - 1].w,
                    data: &bufs[l - 2],
                }
            };
            resize(&prev, geom[l].w, geom[l].h, scale)
        };
        bufs.push(next);
    }
    bufs
}

/// All level images: level 0 borrowed from the caller, the rest from `bufs`.
fn level_images<'a>(img: Image<'a>, bufs: &'a [Vec<u8>], geom: &[Geom]) -> Vec<Image<'a>> {
    let mut v = Vec::with_capacity(geom.len());
    v.push(img);
    for (b, g) in bufs.iter().zip(&geom[1..]) {
        v.push(Image {
            w: g.w,
            h: g.h,
            stride: g.w,
            data: b,
        });
    }
    v
}

/// 7x7 Gaussian (sigma 2) smoothing with replicated borders; output is tightly packed.
fn gaussian_blur(img: &Image) -> Vec<u8> {
    let (w, h) = (img.w, img.h);
    let k = GAUSS7;
    // Horizontal pass, scaled by 256 (at most 255 * 256, fits u16).
    let mut tmp = vec![0u16; w * h];
    let mut padded = vec![0u8; w + 6];
    for (y, out) in tmp.chunks_exact_mut(w).enumerate() {
        let row = &img.data[y * img.stride..y * img.stride + w];
        padded[..3].fill(row[0]);
        padded[3..3 + w].copy_from_slice(row);
        padded[3 + w..].fill(row[w - 1]);
        for (o, p) in out.iter_mut().zip(padded.windows(7)) {
            let p = |i: usize| u16::from(p[i]);
            *o = k[3] * p(3) + k[2] * (p(2) + p(4)) + k[1] * (p(1) + p(5)) + k[0] * (p(0) + p(6));
        }
    }
    // Vertical pass.
    let mut dst = vec![0u8; w * h];
    for (y, out) in dst.chunks_exact_mut(w).enumerate() {
        let row = |d: isize| {
            let yy = (y as isize + d).clamp(0, h as isize - 1) as usize;
            &tmp[yy * w..yy * w + w]
        };
        let (r0, r1, r2, r3, r4, r5, r6) =
            (row(-3), row(-2), row(-1), row(0), row(1), row(2), row(3));
        for (x, o) in out.iter_mut().enumerate() {
            let s = u32::from(k[3]) * u32::from(r3[x])
                + u32::from(k[2]) * (u32::from(r2[x]) + u32::from(r4[x]))
                + u32::from(k[1]) * (u32::from(r1[x]) + u32::from(r5[x]))
                + u32::from(k[0]) * (u32::from(r0[x]) + u32::from(r6[x]));
            *o = ((s + 32768) >> 16) as u8;
        }
    }
    dst
}

// ---------------------------------------------------------------------------------------------------------------------
// FAST-9
// ---------------------------------------------------------------------------------------------------------------------

/// True if the 16-bit circular mask has nine consecutive set bits.
#[inline(always)]
fn has_run9(m: u32) -> bool {
    let m2 = m | (m << 16);
    let mut r = m2 & (m2 >> 1); // runs of 2 starting at each bit
    r &= r >> 2; // 4
    r &= r >> 4; // 8
    r &= m2 >> 8; // 9
    r & 0xFFFF != 0
}

/// FAST-9 decision and score for the 16-pixel ring around a pixel of value `center`: 0 if there is no arc of nine
/// consecutive ring pixels all brighter than `center + thr` or all darker than `center - thr`, otherwise the largest
/// threshold for which such an arc still exists, plus one (so the result is `> thr`, at most 255).
#[inline(always)]
fn ring_score(ring: &[u8; 16], center: u8, thr: u8) -> u8 {
    let c = i16::from(center);
    let t = i16::from(thr);
    let d: [i16; 16] = std::array::from_fn(|k| i16::from(ring[k]) - c);
    let mut bright = 0u32;
    let mut dark = 0u32;
    for (k, &v) in d.iter().enumerate() {
        bright |= u32::from(v > t) << k;
        dark |= u32::from(v < -t) << k;
    }
    let sign: i16 = if has_run9(bright) {
        1
    } else if has_run9(dark) {
        -1
    } else {
        return 0;
    };
    // Largest value v such that some 9-arc has all of sign * d >= v: sliding-window minimum by doubling.
    let s: [i16; 24] = std::array::from_fn(|k| sign * d[k % 16]);
    let m2: [i16; 23] = std::array::from_fn(|i| s[i].min(s[i + 1]));
    let m4: [i16; 21] = std::array::from_fn(|i| m2[i].min(m2[i + 2]));
    let m8: [i16; 17] = std::array::from_fn(|i| m4[i].min(m4[i + 4]));
    let best = (0..16).map(|i| m8[i].min(s[i + 8])).max().unwrap_or(0);
    best.clamp(0, 255) as u8
}

#[derive(Clone, Copy)]
struct Corner {
    x: usize,
    y: usize,
    score: u8,
}

/// Rectangle `[x0, x1) x [y0, y1)` of level pixels.
#[derive(Clone, Copy)]
struct Rect {
    x0: usize,
    y0: usize,
    x1: usize,
    y1: usize,
}

/// FAST-9 corners with 3x3 non-maximum suppression inside `r` (corners are only reported inside `r`; scores are
/// computed one pixel further out so the suppression sees the neighbours). Requires `r.x0, r.y0 >= 4` and `r.x1 + 4`,
/// `r.y1 + 4` inside the image. Appends to `out`.
fn fast_cell(img: &Image, r: Rect, thr: u8, scratch: &mut Vec<u8>, out: &mut Vec<Corner>) {
    let rw = r.x1 - r.x0 + 2;
    let rh = r.y1 - r.y0 + 2;
    scratch.clear();
    scratch.resize(rw * rh, 0);
    let lo = r.x0 - 1 - 3; // leftmost column read
    let t = i16::from(thr);
    for (j, srow) in scratch.chunks_exact_mut(rw).enumerate() {
        let y = r.y0 - 1 + j;
        let row = |k: usize| &img.data[(y + k - 3) * img.stride + lo..][..rw + 6];
        let (r0, r1, r2, r3, r4, r5, r6) = (row(0), row(1), row(2), row(3), row(4), row(5), row(6));
        for (i, s) in srow.iter_mut().enumerate() {
            let c = i16::from(r3[i + 3]);
            let (hi, lo_t) = (c + t, c - t);
            // Compass pixels 0, 4, 8, 12. An arc of nine contains two adjacent compass pixels, both brighter or both
            // darker, so most pixels are rejected after four loads.
            let (p0, p4, p8, p12) = (
                i16::from(r0[i + 3]),
                i16::from(r3[i + 6]),
                i16::from(r6[i + 3]),
                i16::from(r3[i]),
            );
            let b = [p0 > hi, p4 > hi, p8 > hi, p12 > hi];
            let d = [p0 < lo_t, p4 < lo_t, p8 < lo_t, p12 < lo_t];
            let any_b = (b[0] & b[1]) | (b[1] & b[2]) | (b[2] & b[3]) | (b[3] & b[0]);
            let any_d = (d[0] & d[1]) | (d[1] & d[2]) | (d[2] & d[3]) | (d[3] & d[0]);
            if !(any_b | any_d) {
                continue;
            }
            let ring = [
                r0[i + 3],
                r0[i + 4],
                r1[i + 5],
                r2[i + 6],
                r3[i + 6],
                r4[i + 6],
                r5[i + 5],
                r6[i + 4],
                r6[i + 3],
                r6[i + 2],
                r5[i + 1],
                r4[i],
                r3[i],
                r2[i],
                r1[i + 1],
                r0[i + 2],
            ];
            *s = ring_score(&ring, r3[i + 3], thr);
        }
    }
    // 3x3 non-maximum suppression; ties are broken by raster order (the first of equal neighbours survives).
    for y in 1..rh - 1 {
        for x in 1..rw - 1 {
            let s = scratch[y * rw + x];
            if s == 0 {
                continue;
            }
            let up = &scratch[(y - 1) * rw + x - 1..(y - 1) * rw + x + 2];
            let mid = &scratch[y * rw + x - 1..y * rw + x + 2];
            let dn = &scratch[(y + 1) * rw + x - 1..(y + 1) * rw + x + 2];
            let earlier = up.iter().all(|&n| n < s) && mid[0] < s;
            let later = mid[2] <= s && dn.iter().all(|&n| n <= s);
            if earlier && later {
                out.push(Corner {
                    x: r.x0 + x - 1,
                    y: r.y0 + y - 1,
                    score: s,
                });
            }
        }
    }
}

// ---------------------------------------------------------------------------------------------------------------------
// Harris, orientation, descriptor
// ---------------------------------------------------------------------------------------------------------------------

/// Harris response of the 7x7 block centred on `(x, y)`: Sobel gradients (each needs its 3x3 neighbourhood, so the block
/// reads a 9x9 area), structure tensor summed over the block, `det - k * trace^2`, normalised (see the module docs).
/// Needs a 4 pixel margin.
fn harris(img: &Image, x: usize, y: usize) -> f32 {
    let (mut sxx, mut syy, mut sxy) = (0i64, 0i64, 0i64);
    for yy in y - 3..=y + 3 {
        let row = |dy: usize| &img.data[(yy + dy - 1) * img.stride + x - 4..][..9];
        let (top, mid, bot) = (row(0), row(1), row(2));
        for ((t, m), b) in top.windows(3).zip(mid.windows(3)).zip(bot.windows(3)) {
            let (t0, t1, t2) = (i32::from(t[0]), i32::from(t[1]), i32::from(t[2]));
            let (m0, m2) = (i32::from(m[0]), i32::from(m[2]));
            let (b0, b1, b2) = (i32::from(b[0]), i32::from(b[1]), i32::from(b[2]));
            let ix = (t2 - t0) + 2 * (m2 - m0) + (b2 - b0);
            let iy = (b0 - t0) + 2 * (b1 - t1) + (b2 - t2);
            sxx += i64::from(ix * ix);
            syy += i64::from(iy * iy);
            sxy += i64::from(ix * iy);
        }
    }
    let det = (sxx * syy - sxy * sxy) as f64;
    let tr = (sxx + syy) as f64;
    ((det - HARRIS_K * tr * tr) * HARRIS_NORM) as f32
}

/// Intensity-centroid orientation over the disc of radius 15 around `(x, y)` of the tightly packed image `img`.
fn ic_angle(img: &[u8], stride: usize, x: usize, y: usize) -> f32 {
    let (mut m10, mut m01) = (0i32, 0i32);
    for v in -(IC_RADIUS as isize)..=IC_RADIUS as isize {
        let um = UMAX[v.unsigned_abs()];
        let start = (y as isize + v) as usize * stride + x - um;
        let row = &img[start..=start + 2 * um];
        let mut s = 0i32;
        let mut mu = 0i32;
        for (k, &p) in row.iter().enumerate() {
            let p = i32::from(p);
            s += p;
            mu += (k as i32 - um as i32) * p;
        }
        m10 += mu;
        m01 += v as i32 * s;
    }
    (m01 as f32).atan2(m10 as f32)
}

/// Bilinear sample of the tightly packed image `sm` at `(px, py)`. Both coordinates must be at least 0 (so that
/// truncation is a floor) and at most one pixel inside the last column and row.
#[inline(always)]
fn sample(sm: &[u8], stride: usize, px: f32, py: f32) -> f32 {
    let (ix, iy) = (px as usize, py as usize);
    let (tx, ty) = (px - ix as f32, py - iy as f32);
    let i = iy * stride + ix;
    let (p00, p01) = (f32::from(sm[i]), f32::from(sm[i + 1]));
    let (p10, p11) = (f32::from(sm[i + stride]), f32::from(sm[i + stride + 1]));
    let top = p00 + (p01 - p00) * tx;
    let bot = p10 + (p11 - p10) * tx;
    top + (bot - top) * ty
}

/// Steered BRIEF descriptor at `(x, y)` of the smoothed, tightly packed level image `sm`: the pattern is rotated by
/// `angle` and sampled bilinearly at the exact rotated positions. Needs the 22 pixel margin around `(x, y)`.
fn describe(sm: &[u8], stride: usize, x: usize, y: usize, angle: f32) -> Descriptor {
    let (s, c) = angle.sin_cos();
    let (x, y) = (x as f32, y as f32);
    let mut out = [0u8; 32];
    for (k, p) in PATTERN.iter().enumerate() {
        let [ax, ay, bx, by] = p.map(f32::from);
        let a = sample(sm, stride, x + (c * ax - s * ay), y + (s * ax + c * ay));
        let b = sample(sm, stride, x + (c * bx - s * by), y + (s * bx + c * by));
        out[k >> 3] |= u8::from(a < b) << (k & 7);
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// Detection per level and spatial selection
// ---------------------------------------------------------------------------------------------------------------------

type Mask<'m> = Option<&'m dyn Fn(f32, f32) -> bool>;

/// A corner candidate of some level, with its Harris response.
#[derive(Clone, Copy)]
struct Cand {
    x: usize,
    y: usize,
    resp: f32,
    level: u8,
    cell: u16,
}

/// Strongest first; ties broken by position so the order is total and deterministic.
fn cand_cmp(a: &Cand, b: &Cand) -> Ordering {
    b.resp
        .total_cmp(&a.resp)
        .then(a.level.cmp(&b.level))
        .then(a.y.cmp(&b.y))
        .then(a.x.cmp(&b.x))
}

fn far_enough(picked: &[Cand], c: &Cand) -> bool {
    picked.iter().all(|p| {
        let dx = p.x.abs_diff(c.x);
        let dy = p.y.abs_diff(c.y);
        dx * dx + dy * dy >= MIN_KP_DIST2
    })
}

/// Per-level corner detection state (reused scratch buffers).
struct Detector<'a> {
    cfg: &'a Cfg,
    mask: Mask<'a>,
    scratch: Vec<u8>,
    corners: Vec<Corner>,
}

impl Detector<'_> {
    /// Candidates of one level, one batch per grid cell: FAST (with the retry), mask, the strongest few by FAST
    /// score, Harris response. `budget` sizes how many per cell are worth ranking.
    fn detect_level(&mut self, img: &Image, level: u8, lscale: f32, budget: usize) -> Vec<Cand> {
        let cfg = self.cfg;
        let n_cells = cfg.cols * cfg.rows;
        let cap = (budget.div_ceil(n_cells) * 4).max(16);
        let (rx0, ry0, rx1, ry1) = (BORDER, BORDER, img.w - BORDER, img.h - BORDER);
        let (rw, rh) = (rx1 - rx0, ry1 - ry0);
        let mut out = Vec::with_capacity(n_cells * cap.min(32));
        for cy in 0..cfg.rows {
            let (y0, y1) = (ry0 + cy * rh / cfg.rows, ry0 + (cy + 1) * rh / cfg.rows);
            for cx in 0..cfg.cols {
                let (x0, x1) = (rx0 + cx * rw / cfg.cols, rx0 + (cx + 1) * rw / cfg.cols);
                if x1 <= x0 || y1 <= y0 {
                    continue;
                }
                let rect = Rect { x0, y0, x1, y1 };
                self.corners.clear();
                self.run_fast(img, rect, cfg.thr, lscale);
                if self.corners.is_empty() && cfg.min_thr < cfg.thr {
                    self.run_fast(img, rect, cfg.min_thr, lscale);
                }
                if self.corners.len() > cap {
                    self.corners.sort_unstable_by(|a, b| {
                        b.score
                            .cmp(&a.score)
                            .then(a.y.cmp(&b.y))
                            .then(a.x.cmp(&b.x))
                    });
                    self.corners.truncate(cap);
                }
                let cell = (cy * cfg.cols + cx) as u16;
                out.extend(self.corners.iter().map(|c| Cand {
                    x: c.x,
                    y: c.y,
                    resp: harris(img, c.x, c.y),
                    level,
                    cell,
                }));
            }
        }
        out
    }

    /// FAST in `rect` at `thr`, keeping the corners the mask lets through (it sees level-0 coordinates).
    fn run_fast(&mut self, img: &Image, rect: Rect, thr: u8, lscale: f32) {
        fast_cell(img, rect, thr, &mut self.scratch, &mut self.corners);
        if let Some(mask) = self.mask {
            self.corners
                .retain(|c| mask(c.x as f32 * lscale, c.y as f32 * lscale));
        }
    }
}

/// Picks up to `budget` of one level's candidates: the best candidate of every cell first (strongest cells first when
/// there are more cells than budget), then the strongest of the rest. Returns `(picked, not picked)`.
fn select_level(cands: &[Cand], n_cells: usize, budget: usize) -> (Vec<Cand>, Vec<Cand>) {
    let mut order: Vec<usize> = (0..cands.len()).collect();
    order.sort_unstable_by(|&a, &b| cand_cmp(&cands[a], &cands[b]));
    let mut taken = vec![false; cands.len()];
    let mut cell_done = vec![false; n_cells];
    let mut picked: Vec<Cand> = Vec::with_capacity(budget.min(cands.len()));
    for &i in &order {
        if picked.len() >= budget {
            break;
        }
        let c = &cands[i];
        if !cell_done[usize::from(c.cell)] && far_enough(&picked, c) {
            picked.push(*c);
            taken[i] = true;
            cell_done[usize::from(c.cell)] = true;
        }
    }
    for &i in &order {
        if picked.len() >= budget {
            break;
        }
        if !taken[i] && far_enough(&picked, &cands[i]) {
            picked.push(cands[i]);
            taken[i] = true;
        }
    }
    let rest = order
        .iter()
        .filter(|&&i| !taken[i])
        .map(|&i| cands[i])
        .collect();
    (picked, rest)
}

/// Levels that came up short of their budget leave the shortfall to the strongest unused candidates of any level: adds
/// them to `picked` (respecting the spacing rule) until `n_features` are picked or the candidates run out.
fn top_up(picked: &mut [Vec<Cand>], mut unused: Vec<Cand>, n_features: usize) {
    let mut total: usize = picked.iter().map(Vec::len).sum();
    if total >= n_features {
        return;
    }
    unused.sort_unstable_by(cand_cmp);
    for c in unused {
        if total >= n_features {
            break;
        }
        let lv = &mut picked[usize::from(c.level)];
        if far_enough(lv, &c) {
            lv.push(c);
            total += 1;
        }
    }
}

/// Per-level budgets proportional to the level areas, summing to exactly `n` (largest remainders).
fn level_budgets(n: usize, geom: &[Geom]) -> Vec<usize> {
    let areas: Vec<f64> = geom.iter().map(|g| (g.w * g.h) as f64).collect();
    let total: f64 = areas.iter().sum();
    let exact: Vec<f64> = areas.iter().map(|a| n as f64 * a / total).collect();
    let mut out: Vec<usize> = exact.iter().map(|e| e.floor() as usize).collect();
    let rem = n.saturating_sub(out.iter().sum::<usize>());
    let mut idx: Vec<usize> = (0..out.len()).collect();
    idx.sort_by(|&a, &b| {
        let (fa, fb) = (exact[a] - exact[a].floor(), exact[b] - exact[b].floor());
        fb.total_cmp(&fa).then(a.cmp(&b))
    });
    for &i in idx.iter().take(rem) {
        out[i] += 1;
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// extract
// ---------------------------------------------------------------------------------------------------------------------

/// Detect + describe. `mask(x, y)` (level-0 pixel coords) = false excludes a location (fisheye border); None = whole
/// image.
///
/// Returns at most `p.n_features` keypoints with their descriptors, ordered by level, then row, then column; fewer if
/// the image does not hold enough corners. The result is empty for an invalid image (see [`Image::is_valid`]) or one
/// smaller than about 60 pixels per side. A keypoint is never closer than 22 pixels to the image border (at its own
/// level).
///
/// `mask` is evaluated at the keypoint position only, not over its patch. A keypoint's descriptor reads up to
/// 22 level pixels around it (22 * scale^level in level-0 pixels, 38 at the default coarsest level), so a mask that
/// should keep descriptors clear of a fisheye image-circle edge has to stay that far inside it.
pub fn extract(
    img: &Image,
    mask: Option<&dyn Fn(f32, f32) -> bool>,
    p: &OrbParams,
) -> Vec<(Keypoint, Descriptor)> {
    let cfg = Cfg::new(p);
    if cfg.n_features == 0 || !img.is_valid() {
        return Vec::new();
    }
    let geom = level_geometry(img.w, img.h, &cfg);
    if geom.is_empty() {
        return Vec::new();
    }
    let bufs = build_pyramid(img, &geom, cfg.scale);
    let levels = level_images(*img, &bufs, &geom);
    let budgets = level_budgets(cfg.n_features, &geom);
    let n_cells = cfg.cols * cfg.rows;

    // Detect and select per level.
    let mut det = Detector {
        cfg: &cfg,
        mask,
        scratch: Vec::new(),
        corners: Vec::new(),
    };
    let mut picked: Vec<Vec<Cand>> = Vec::with_capacity(levels.len());
    let mut unused: Vec<Cand> = Vec::new();
    for (l, (im, g)) in levels.iter().zip(&geom).enumerate() {
        if budgets[l] == 0 {
            picked.push(Vec::new());
            continue;
        }
        let cands = det.detect_level(im, l as u8, g.scale as f32, budgets[l]);
        let (sel, rest) = select_level(&cands, n_cells, budgets[l]);
        picked.push(sel);
        unused.extend(rest);
    }

    top_up(&mut picked, unused, cfg.n_features);
    let total: usize = picked.iter().map(Vec::len).sum();

    // Orientation and descriptors on the smoothed level images.
    let mut out = Vec::with_capacity(total);
    for (l, (sel, (im, g))) in picked.iter_mut().zip(levels.iter().zip(&geom)).enumerate() {
        if sel.is_empty() {
            continue;
        }
        sel.sort_unstable_by_key(|c| (c.y, c.x));
        let sm = gaussian_blur(im);
        let lscale = g.scale as f32;
        for c in sel.iter() {
            let angle = ic_angle(&sm, im.w, c.x, c.y);
            let desc = describe(&sm, im.w, c.x, c.y, angle);
            let kp = Keypoint {
                x: c.x as f32 * lscale,
                y: c.y as f32 * lscale,
                level: l as u8,
                angle,
                response: c.resp,
            };
            out.push((kp, desc));
        }
    }
    out
}

// ---------------------------------------------------------------------------------------------------------------------
// Matching
// ---------------------------------------------------------------------------------------------------------------------

#[inline(always)]
fn le64(b: &[u8]) -> u64 {
    let mut a = [0u8; 8];
    a.copy_from_slice(&b[..8]);
    u64::from_le_bytes(a)
}

#[inline(always)]
fn words(d: &Descriptor) -> [u64; 4] {
    [le64(&d[0..]), le64(&d[8..]), le64(&d[16..]), le64(&d[24..])]
}

#[inline(always)]
fn hamming_words(a: &[u64; 4], b: &[u64; 4]) -> u32 {
    (a[0] ^ b[0]).count_ones()
        + (a[1] ^ b[1]).count_ones()
        + (a[2] ^ b[2]).count_ones()
        + (a[3] ^ b[3]).count_ones()
}

/// Hamming distance (number of differing bits) of two descriptors, 0..=256.
#[inline]
pub fn hamming(a: &Descriptor, b: &Descriptor) -> u32 {
    hamming_words(&words(a), &words(b))
}

/// Mutual nearest neighbours with the ratio test over the pairs `allow` lets through. One pass over all pairs: the
/// forward best and second best of every row, the backward best of every column.
fn match_core<F: Fn(usize, usize) -> bool>(
    a: &[Descriptor],
    b: &[Descriptor],
    max_dist: u32,
    ratio: f32,
    allow: F,
) -> Vec<(usize, usize, u32)> {
    if a.is_empty() || b.is_empty() {
        return Vec::new();
    }
    let aw: Vec<[u64; 4]> = a.iter().map(words).collect();
    let bw: Vec<[u64; 4]> = b.iter().map(words).collect();
    let mut col_d = vec![u32::MAX; b.len()];
    let mut col_i = vec![usize::MAX; b.len()];
    let mut fwd: Vec<(u32, usize, u32)> = Vec::with_capacity(a.len());
    for (i, wa) in aw.iter().enumerate() {
        let (mut d1, mut j1, mut d2) = (u32::MAX, usize::MAX, u32::MAX);
        for (j, wb) in bw.iter().enumerate() {
            if !allow(i, j) {
                continue;
            }
            let d = hamming_words(wa, wb);
            if d < d1 {
                d2 = d1;
                d1 = d;
                j1 = j;
            } else if d < d2 {
                d2 = d;
            }
            if d < col_d[j] {
                col_d[j] = d;
                col_i[j] = i;
            }
        }
        fwd.push((d1, j1, d2));
    }
    let mut out = Vec::new();
    for (i, &(d1, j1, d2)) in fwd.iter().enumerate() {
        if j1 == usize::MAX || d1 > max_dist || col_i[j1] != i {
            continue;
        }
        // No second candidate (d2 == MAX): nothing to be ambiguous with.
        if d2 == u32::MAX || (d1 as f32) < ratio * d2 as f32 {
            out.push((i, j1, d1));
        }
    }
    out
}

/// Brute-force matching a -> b: mutual nearest neighbours, distance <= max_dist, Lowe ratio best/second < ratio.
/// Returns (index in a, index in b, distance), ordered by the index in `a`.
///
/// Ties are broken towards the lower index, so the result is deterministic; a descriptor with two equally close
/// candidates in `b` is never matched (best/second = 1). The ratio test is applied on the `a -> b` side only.
pub fn match_bf(
    a: &[Descriptor],
    b: &[Descriptor],
    max_dist: u32,
    ratio: f32,
) -> Vec<(usize, usize, u32)> {
    match_core(a, b, max_dist, ratio, |_, _| true)
}

/// Matching restricted by a caller predicate (e.g. epipolar or projected-position gate): only pairs with
/// allow(i, j) == true are considered.
///
/// Same rules as [`match_bf`], but the nearest neighbour, the second nearest (ratio test) and the mutual check all
/// range over the allowed pairs only. `allow` is called exactly once per pair `(i, j)` (a pair the caller forbids is
/// never looked at again), in row-major order.
pub fn match_gated(
    a: &[Descriptor],
    b: &[Descriptor],
    max_dist: u32,
    ratio: f32,
    allow: &dyn Fn(usize, usize) -> bool,
) -> Vec<(usize, usize, u32)> {
    match_core(a, b, max_dist, ratio, allow)
}

// ---------------------------------------------------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::f32::consts::{PI, TAU};
    use std::time::Instant;

    // ---------------------------------------------------------------------------------------------------------------
    // Helpers: deterministic RNG and synthetic textured images (no files needed)
    // ---------------------------------------------------------------------------------------------------------------

    /// Small deterministic generator (splitmix64) for the synthetic images and the random descriptors.
    struct Rng(u64);

    impl Rng {
        fn new(seed: u64) -> Self {
            Rng(seed)
        }

        fn next_u64(&mut self) -> u64 {
            let (s, z) = splitmix64(self.0);
            self.0 = s;
            z
        }

        /// Uniform in `[0, 1)`.
        fn unit(&mut self) -> f32 {
            (self.next_u64() >> 40) as f32 / (1u64 << 24) as f32
        }

        fn range(&mut self, lo: f32, hi: f32) -> f32 {
            lo + (hi - lo) * self.unit()
        }

        /// "Gaussian-ish" with sigma 1: twelve uniforms summed, minus six.
        fn gauss(&mut self) -> f32 {
            (0..12).map(|_| self.unit()).sum::<f32>() - 6.0
        }

        fn descriptor(&mut self) -> Descriptor {
            let words: [u64; 4] = std::array::from_fn(|_| self.next_u64());
            let mut d = [0u8; 32];
            for (k, w) in words.iter().enumerate() {
                d[8 * k..8 * k + 8].copy_from_slice(&w.to_le_bytes());
            }
            d
        }
    }

    /// True if there is no mask or it lets `(x, y)` through.
    fn mask_allows(mask: Option<&dyn Fn(f32, f32) -> bool>, x: f32, y: f32) -> bool {
        match mask {
            Some(f) => f(x, y),
            None => true,
        }
    }

    fn in_triangle(u: f32, v: f32, a: f32, b: f32) -> bool {
        let p = [(a, 0.0), (-a * 0.5, b), (-a * 0.5, -b)];
        let e = |i: usize| {
            let (x0, y0) = p[i];
            let (x1, y1) = p[(i + 1) % 3];
            (x1 - x0) * (v - y0) - (y1 - y0) * (u - x0)
        };
        let (e0, e1, e2) = (e(0), e(1), e(2));
        (e0 >= 0.0 && e1 >= 0.0 && e2 >= 0.0) || (e0 <= 0.0 && e1 <= 0.0 && e2 <= 0.0)
    }

    /// Separable `[1 2 1] / 4` blur with clamped edges.
    fn blur3(src: &[f32], w: usize, h: usize) -> Vec<f32> {
        let mut tmp = vec![0f32; w * h];
        for y in 0..h {
            for x in 0..w {
                let (l, r) = (x.saturating_sub(1), (x + 1).min(w - 1));
                tmp[y * w + x] =
                    0.25 * src[y * w + l] + 0.5 * src[y * w + x] + 0.25 * src[y * w + r];
            }
        }
        let mut out = vec![0f32; w * h];
        for y in 0..h {
            let (u, d) = (y.saturating_sub(1), (y + 1).min(h - 1));
            for x in 0..w {
                out[y * w + x] =
                    0.25 * tmp[u * w + x] + 0.5 * tmp[y * w + x] + 0.25 * tmp[d * w + x];
            }
        }
        out
    }

    /// Textured grey canvas: smooth illumination, many random rotated rectangles, triangles and ellipses (sharp corners
    /// at every orientation, contrasts from faint to strong), a little grain, then one 3x3 binomial blur.
    fn canvas(w: usize, h: usize, seed: u64) -> Vec<f32> {
        let mut rng = Rng::new(seed);
        let (gx, gy, base) = (
            rng.range(-0.06, 0.06),
            rng.range(-0.06, 0.06),
            rng.range(90.0, 130.0),
        );
        let mut img: Vec<f32> = (0..w * h)
            .map(|i| base + gx * (i % w) as f32 + gy * (i / w) as f32)
            .collect();
        for _ in 0..w * h / 600 {
            let val = rng.range(20.0, 240.0);
            let (cx, cy) = (rng.range(0.0, w as f32), rng.range(0.0, h as f32));
            let size = rng.range(5.0, 26.0);
            let (a, b) = (size * rng.range(0.5, 1.4), size * rng.range(0.5, 1.4));
            let kind = rng.next_u64() % 3;
            let (s, c) = rng.range(0.0, TAU).sin_cos();
            let r = (a.max(b) * 1.5 + 2.0).ceil();
            let (x0, x1) = ((cx - r).max(0.0) as usize, ((cx + r) as usize).min(w - 1));
            let (y0, y1) = ((cy - r).max(0.0) as usize, ((cy + r) as usize).min(h - 1));
            for y in y0..=y1 {
                for x in x0..=x1 {
                    let (dx, dy) = (x as f32 - cx, y as f32 - cy);
                    let (u, v) = (c * dx + s * dy, -s * dx + c * dy);
                    let inside = match kind {
                        0 => u.abs() <= a && v.abs() <= b,
                        1 => in_triangle(u, v, a, b),
                        _ => (u / a).powi(2) + (v / b).powi(2) <= 1.0,
                    };
                    if inside {
                        img[y * w + x] = val;
                    }
                }
            }
        }
        for p in img.iter_mut() {
            *p += 4.0 * rng.gauss();
        }
        blur3(&img, w, h)
    }

    fn to_u8(v: &[f32]) -> Vec<u8> {
        v.iter()
            .map(|&p| p.round().clamp(0.0, 255.0) as u8)
            .collect()
    }

    /// Bilinear sample with clamped edges.
    fn bilinear(img: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
        let x = x.clamp(0.0, (w - 1) as f32 - 1e-3);
        let y = y.clamp(0.0, (h - 1) as f32 - 1e-3);
        let (x0, y0) = (x as usize, y as usize);
        let (fx, fy) = (x - x0 as f32, y - y0 as f32);
        let a = img[y0 * w + x0] * (1.0 - fx) + img[y0 * w + x0 + 1] * fx;
        let b = img[(y0 + 1) * w + x0] * (1.0 - fx) + img[(y0 + 1) * w + x0 + 1] * fx;
        a * (1.0 - fy) + b * fy
    }

    /// A 640x480 view of a canvas (the middle of it) and a second view related to it by a known similarity transform
    /// (rotation by `ang` about the image centre, then translation by `shift`, magnification `zoom`) with a brightness
    /// change and noise. Both are produced by inverse mapping with bilinear sampling from the same larger canvas, so
    /// both frames are fully textured. Returns `(a, b)`.
    struct Pair {
        a: Vec<u8>,
        b: Vec<u8>,
        ang: f32,
        zoom: f32,
        shift: (f32, f32),
    }

    const W: usize = 640;
    const H: usize = 480;
    /// Centre of a 640x480 frame in pixel-centre coordinates.
    const CENTRE: (f32, f32) = ((W - 1) as f32 / 2.0, (H - 1) as f32 / 2.0);

    impl Pair {
        fn new(
            seed: u64,
            ang: f32,
            zoom: f32,
            shift: (f32, f32),
            gain: f32,
            offset: f32,
            sigma: f32,
        ) -> Pair {
            let m = 120usize;
            let (cw, ch) = (W + 2 * m, H + 2 * m);
            let cv = canvas(cw, ch, seed);
            let mut rng = Rng::new(seed ^ 0x5EED);
            let (s, c) = ang.sin_cos();
            let mut a = vec![0u8; W * H];
            let mut b = vec![0u8; W * H];
            for y in 0..H {
                for x in 0..W {
                    a[y * W + x] = cv[(y + m) * cw + x + m].round().clamp(0.0, 255.0) as u8;
                    // Inverse map of the pixel of B to the pixel of A it shows: p_A = c + R^-1 (q - c - t) / zoom.
                    let (qx, qy) = (x as f32 - CENTRE.0 - shift.0, y as f32 - CENTRE.1 - shift.1);
                    let (ux, uy) = (c * qx + s * qy, -s * qx + c * qy);
                    let (px, py) = (CENTRE.0 + ux / zoom, CENTRE.1 + uy / zoom);
                    let v = bilinear(&cv, cw, ch, px + m as f32, py + m as f32);
                    b[y * W + x] = (v * gain + offset + sigma * rng.gauss())
                        .round()
                        .clamp(0.0, 255.0) as u8;
                }
            }
            Pair {
                a,
                b,
                ang,
                zoom,
                shift,
            }
        }

        /// Where the point `(x, y)` of A is in B.
        fn forward(&self, x: f32, y: f32) -> (f32, f32) {
            let (s, c) = self.ang.sin_cos();
            let (dx, dy) = (x - CENTRE.0, y - CENTRE.1);
            (
                CENTRE.0 + self.zoom * (c * dx - s * dy) + self.shift.0,
                CENTRE.1 + self.zoom * (s * dx + c * dy) + self.shift.1,
            )
        }
    }

    /// Extracts both views, matches them (`max_dist` 64, ratio 0.8) and returns
    /// `(keypoints A, keypoints B, matches, correct matches)`, a match being correct when B's keypoint lies within
    /// `tol` pixels of where the known warp puts A's.
    fn run_pair(p: &Pair, tol: f32) -> (usize, usize, usize, usize) {
        let params = OrbParams::default();
        let ka = extract(&Image::new(W, H, &p.a).unwrap(), None, &params);
        let kb = extract(&Image::new(W, H, &p.b).unwrap(), None, &params);
        let da: Vec<Descriptor> = ka.iter().map(|k| k.1).collect();
        let db: Vec<Descriptor> = kb.iter().map(|k| k.1).collect();
        let m = match_bf(&da, &db, 64, 0.8);
        let correct = m
            .iter()
            .filter(|&&(i, j, _)| {
                let (ex, ey) = p.forward(ka[i].0.x, ka[i].0.y);
                (kb[j].0.x - ex).hypot(kb[j].0.y - ey) <= tol
            })
            .count();
        (ka.len(), kb.len(), m.len(), correct)
    }

    fn textured_640x480(seed: u64) -> Vec<u8> {
        to_u8(&canvas(W, H, seed))
    }

    // ---------------------------------------------------------------------------------------------------------------
    // 1. Count, margins, spread
    // ---------------------------------------------------------------------------------------------------------------

    #[test]
    fn extract_fills_the_budget_inside_the_margins_and_spreads_over_the_grid() {
        let data = textured_640x480(1);
        let img = Image::new(W, H, &data).unwrap();
        let p = OrbParams::default();
        let kps = extract(&img, None, &p);
        let n = p.n_features as f32;
        println!("keypoints: {} of {}", kps.len(), p.n_features);
        assert!(
            kps.len() as f32 >= 0.8 * n && kps.len() <= p.n_features,
            "got {}",
            kps.len()
        );

        let mut per_level = vec![0usize; p.n_levels];
        let mut cells = [false; 48];
        for (k, _) in &kps {
            assert!(usize::from(k.level) < p.n_levels);
            per_level[usize::from(k.level)] += 1;
            // Inside the 22 pixel border (level-0 coordinates; coarser levels have a wider border in these units).
            assert!(
                k.x >= BORDER as f32 && k.x <= (W - 1 - BORDER) as f32,
                "x = {}",
                k.x
            );
            assert!(
                k.y >= BORDER as f32 && k.y <= (H - 1 - BORDER) as f32,
                "y = {}",
                k.y
            );
            assert!(
                k.angle > -PI - 1e-6 && k.angle <= PI + 1e-6,
                "angle = {}",
                k.angle
            );
            assert!(k.response.is_finite());
            let cx = ((k.x / (W as f32 / 8.0)) as usize).min(7);
            let cy = ((k.y / (H as f32 / 6.0)) as usize).min(5);
            cells[cy * 8 + cx] = true;
        }
        let covered = cells.iter().filter(|&&c| c).count();
        println!("per level: {per_level:?}, grid cells covered: {covered}/48");
        assert!(
            covered as f32 >= 0.7 * 48.0,
            "only {covered}/48 cells have a keypoint"
        );
        // Every level contributes, finest first (budgets are proportional to the level area).
        assert!(per_level.iter().all(|&c| c > 0));
        assert!(per_level.windows(2).all(|w| w[0] >= w[1]), "{per_level:?}");
    }

    // ---------------------------------------------------------------------------------------------------------------
    // 2. Repeatability under a known similarity warp
    // ---------------------------------------------------------------------------------------------------------------

    #[test]
    fn repeatable_under_rotation_shift_brightness_and_noise() {
        // Rotate 15 degrees about the centre, shift by 7.3 px (both axes), gain 0.7, offset +20, noise sigma 3.
        let pair = Pair::new(2, 15f32.to_radians(), 1.0, (7.3, 7.3), 0.7, 20.0, 3.0);
        let (na, nb, matches, correct) = run_pair(&pair, 2.5);
        let pct = 100.0 * correct as f32 / matches.max(1) as f32;
        println!("keypoints {na}/{nb}, matches {matches}, correct {correct} ({pct:.1}%)");
        assert!(correct >= 100, "only {correct} correct matches");
        assert!(
            correct * 10 >= matches * 6,
            "{correct} of {matches} matches agree with the warp ({pct:.1}%)"
        );
    }

    #[test]
    fn identical_images_match_themselves_exactly() {
        let data = textured_640x480(3);
        let img = Image::new(W, H, &data).unwrap();
        let kps = extract(&img, None, &OrbParams::default());
        let d: Vec<Descriptor> = kps.iter().map(|k| k.1).collect();
        let m = match_bf(&d, &d, 0, 0.8);
        // A keypoint whose descriptor equals another's (duplicates) cannot pass the ratio test, everything else does.
        assert!(m.len() + 5 >= d.len(), "{} of {}", m.len(), d.len());
        assert!(m.iter().all(|&(i, j, dist)| i == j && dist == 0));
    }

    #[test]
    fn repeatable_across_a_scale_change() {
        // The second view shows the scene magnified by 1.2 (one pyramid step), turned by 20 degrees.
        let pair = Pair::new(4, 20f32.to_radians(), 1.2, (-5.1, 3.7), 0.8, 10.0, 2.0);
        let (na, nb, matches, correct) = run_pair(&pair, 3.0);
        println!("scale 1.2: keypoints {na}/{nb}, matches {matches}, correct {correct}");
        assert!(correct >= 50, "only {correct} correct matches");
        assert!(correct * 10 >= matches * 7, "{correct} of {matches}");
    }

    // ---------------------------------------------------------------------------------------------------------------
    // 3. Hamming and matching
    // ---------------------------------------------------------------------------------------------------------------

    type Matches = Vec<(usize, usize, u32)>;

    fn naive_hamming(a: &Descriptor, b: &Descriptor) -> u32 {
        let mut n = 0;
        for (x, y) in a.iter().zip(b) {
            for bit in 0..8 {
                n += u32::from(((x >> bit) & 1) != ((y >> bit) & 1));
            }
        }
        n
    }

    #[test]
    fn hamming_agrees_with_a_naive_bit_count() {
        let mut rng = Rng::new(10);
        for _ in 0..2000 {
            let (a, b) = (rng.descriptor(), rng.descriptor());
            assert_eq!(hamming(&a, &b), naive_hamming(&a, &b));
        }
        let zero = [0u8; 32];
        let ones = [0xFFu8; 32];
        assert_eq!(hamming(&zero, &zero), 0);
        assert_eq!(hamming(&zero, &ones), 256);
        assert_eq!(hamming(&ones, &zero), 256);
        let mut one_bit = zero;
        one_bit[31] = 0x80;
        assert_eq!(hamming(&zero, &one_bit), 1);
        assert_eq!(hamming(&one_bit, &ones), 255);
    }

    /// Descriptor with the first `k` bits set: `hamming(nested(k), nested(m)) == |k - m|`.
    fn nested(k: usize) -> Descriptor {
        let mut d = [0u8; 32];
        for bit in 0..k {
            d[bit / 8] |= 1 << (bit % 8);
        }
        d
    }

    #[test]
    fn match_bf_applies_the_mutual_distance_and_ratio_rules() {
        let a = [nested(0), nested(8), nested(100)];
        let b = [nested(10), nested(12), nested(120)];
        // a0's nearest is b0 (10) but b0's nearest is a1 (2): not mutual. a1 -> b0 (2, second 4), a2 -> b2 (20, second 88).
        assert_eq!(match_bf(&a, &b, 64, 0.8), vec![(1, 0, 2), (2, 2, 20)]);
        // max_dist cuts the 20; a distance equal to max_dist passes.
        assert_eq!(match_bf(&a, &b, 10, 0.8), vec![(1, 0, 2)]);
        assert_eq!(match_bf(&a, &b, 20, 0.8), vec![(1, 0, 2), (2, 2, 20)]);
        assert_eq!(match_bf(&a, &b, 1, 0.8), Matches::new());

        // Ratio: 10 / 12 = 0.83.
        let one = [nested(0)];
        let two = [nested(10), nested(12)];
        assert_eq!(match_bf(&one, &two, 64, 0.8), Matches::new());
        assert_eq!(match_bf(&one, &two, 64, 0.9), vec![(0, 0, 10)]);
        // The comparison is strict: best / second < ratio.
        assert_eq!(match_bf(&one, &two, 64, 10.0 / 12.0), Matches::new());
        // A single candidate has no second best to be confused with.
        assert_eq!(match_bf(&one, &[nested(10)], 64, 0.5), vec![(0, 0, 10)]);
        // Two equally close candidates are ambiguous, even at distance zero.
        assert_eq!(
            match_bf(&one, &[nested(10), nested(10)], 64, 0.99),
            Matches::new()
        );
        assert_eq!(
            match_bf(&one, &[nested(0), nested(0)], 64, 0.8),
            Matches::new()
        );
        // Empty inputs.
        assert_eq!(match_bf(&[], &two, 64, 0.8), Matches::new());
        assert_eq!(match_bf(&one, &[], 64, 0.8), Matches::new());
    }

    /// Straightforward O(n^2) statement of the matching rules, to check the single-pass implementation against.
    fn naive_match(
        a: &[Descriptor],
        b: &[Descriptor],
        max_dist: u32,
        ratio: f32,
        allow: &dyn Fn(usize, usize) -> bool,
    ) -> Matches {
        let mut out = Vec::new();
        for (i, da) in a.iter().enumerate() {
            let cand: Vec<(u32, usize)> = (0..b.len())
                .filter(|&j| allow(i, j))
                .map(|j| (hamming(da, &b[j]), j))
                .collect();
            let Some(&(d1, j1)) = cand.iter().min() else {
                continue;
            };
            let second = cand.iter().filter(|c| c.1 != j1).map(|c| c.0).min();
            let back = (0..a.len())
                .filter(|&k| allow(k, j1))
                .map(|k| (hamming(&a[k], &b[j1]), k))
                .min();
            let mutual = back.map(|(_, k)| k) == Some(i);
            let ratio_ok = match second {
                Some(d2) => (d1 as f32) < ratio * d2 as f32,
                None => true,
            };
            if mutual && d1 <= max_dist && ratio_ok {
                out.push((i, j1, d1));
            }
        }
        out
    }

    #[test]
    fn matching_agrees_with_a_naive_implementation_on_random_data_with_ties() {
        let mut rng = Rng::new(11);
        let mut total = 0;
        for trial in 0..12usize {
            let nb = 20 + trial * 7;
            let mut b: Vec<Descriptor> = (0..nb).map(|_| rng.descriptor()).collect();
            let mut a: Vec<Descriptor> = Vec::new();
            for _ in 0..15 + trial * 5 {
                // A noisy copy of a random descriptor of b: near matches at all distances.
                let mut d = b[(rng.next_u64() % nb as u64) as usize];
                for _ in 0..rng.next_u64() % 60 {
                    let bit = (rng.next_u64() % 256) as usize;
                    d[bit / 8] ^= 1 << (bit % 8);
                }
                a.push(d);
            }
            for _ in 0..5 {
                a.push(rng.descriptor());
            }
            // Exact duplicates on both sides: ties everywhere.
            a.push(a[0]);
            a.push(b[1]);
            b.push(b[2]);
            b.push(a[3]);
            for &(max_dist, ratio) in &[
                (64, 0.8),
                (100, 0.9),
                (256, 1.0),
                (30, 0.6),
                (256, 2.0),
                (0, 0.8),
            ] {
                let all = |_: usize, _: usize| true;
                let expect = naive_match(&a, &b, max_dist, ratio, &all);
                total += expect.len();
                assert_eq!(
                    match_bf(&a, &b, max_dist, ratio),
                    expect,
                    "trial {trial}, {max_dist}, {ratio}"
                );
                let allow = |i: usize, j: usize| (i * 7 + j * 3) % 5 < 4;
                let expect = naive_match(&a, &b, max_dist, ratio, &allow);
                assert_eq!(
                    match_gated(&a, &b, max_dist, ratio, &allow),
                    expect,
                    "gated, trial {trial}"
                );
            }
        }
        assert!(
            total > 100,
            "the random data should produce plenty of matches to compare ({total})"
        );
    }

    #[test]
    fn match_gated_respects_the_predicate_and_asks_once_per_pair() {
        let mut rng = Rng::new(12);
        let b: Vec<Descriptor> = (0..40).map(|_| rng.descriptor()).collect();
        let a: Vec<Descriptor> = b[..30].to_vec(); // a[i] == b[i]: the diagonal is the true match
        let (na, nb) = (a.len(), b.len());

        let calls: Vec<Cell<u32>> = (0..na * nb).map(|_| Cell::new(0)).collect();
        let same_parity = |i: usize, j: usize| {
            calls[i * nb + j].set(calls[i * nb + j].get() + 1);
            i % 2 == j % 2
        };
        let m = match_gated(&a, &b, 0, 0.8, &same_parity);
        assert!(
            calls.iter().all(|c| c.get() == 1),
            "the predicate must be asked exactly once per pair"
        );
        assert_eq!(
            m.len(),
            na,
            "the diagonal is allowed, so every row matches its twin"
        );
        assert!(m.iter().all(|&(i, j, d)| i == j && d == 0));

        // Forbid the diagonal: no twin to find, and nothing forbidden may come back.
        let off_diagonal = |i: usize, j: usize| i != j;
        let m = match_gated(&a, &b, 256, 1.0, &off_diagonal);
        assert!(m.iter().all(|&(i, j, _)| i != j));
        let free = match_bf(&a, &b, 256, 1.0);
        assert!(free.iter().all(|&(i, j, _)| i == j) && free.len() == na);
        // Allow only a single column per row: the single candidate always wins the ratio test, but must also be
        // the nearest of its column among the allowed rows.
        let only_next = |i: usize, j: usize| j == (i + 1) % nb;
        let m = match_gated(&a, &b, 256, 0.5, &only_next);
        assert_eq!(m.len(), na);
        assert!(m.iter().all(|&(i, j, _)| j == i + 1));
        // Nothing allowed: nothing found.
        assert!(match_gated(&a, &b, 256, 1.0, &|_, _| false).is_empty());
    }

    // ---------------------------------------------------------------------------------------------------------------
    // 4. Mask
    // ---------------------------------------------------------------------------------------------------------------

    #[test]
    fn mask_excludes_locations() {
        let data = textured_640x480(5);
        let img = Image::new(W, H, &data).unwrap();
        let p = OrbParams::default();

        // Left half excluded: nothing there, and the right half still fills (nearly) the whole budget.
        let right_half = |x: f32, _y: f32| x >= W as f32 / 2.0;
        let kps = extract(&img, Some(&right_half), &p);
        println!("left half masked: {} keypoints", kps.len());
        assert!(kps.iter().all(|(k, _)| k.x >= W as f32 / 2.0));
        assert!(
            kps.len() as f32 >= 0.8 * p.n_features as f32,
            "{} keypoints",
            kps.len()
        );

        // A disc (fisheye image circle): everything inside.
        let (cx, cy, r) = (CENTRE.0, CENTRE.1, 200.0f32);
        let disc = move |x: f32, y: f32| (x - cx).hypot(y - cy) <= r;
        let kps = extract(&img, Some(&disc), &p);
        assert!(
            kps.len() as f32 >= 0.8 * p.n_features as f32,
            "{} keypoints",
            kps.len()
        );
        assert!(kps.iter().all(|(k, _)| (k.x - cx).hypot(k.y - cy) <= r));

        // Everything excluded; nothing, and no panic.
        assert!(extract(&img, Some(&|_, _| false), &p).is_empty());
        // A mask that lets everything through changes nothing.
        let all = extract(&img, Some(&|_, _| true), &p);
        assert_eq!(all, extract(&img, None, &p));
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Internals: pyramid, orientation, steering, pattern
    // ---------------------------------------------------------------------------------------------------------------

    #[test]
    fn pyramid_samples_the_image_on_a_corner_aligned_grid() {
        // A linear ramp is reproduced exactly by bilinear resampling (up to 8-bit rounding), so every level pixel must
        // equal the ramp at (i * scale^l, j * scale^l).
        let data: Vec<u8> = (0..H)
            .flat_map(|y| (0..W).map(move |x| (0.2 * x as f32 + 0.1 * y as f32).round() as u8))
            .collect();
        let img = Image::new(W, H, &data).unwrap();
        let cfg = Cfg::new(&OrbParams::default());
        let geom = level_geometry(W, H, &cfg);
        assert_eq!(geom.len(), 4);
        let bufs = build_pyramid(&img, &geom, cfg.scale);
        let levels = level_images(img, &bufs, &geom);
        assert_eq!((levels[1].w, levels[1].h), (533, 400));
        for (l, (im, g)) in levels.iter().zip(&geom).enumerate() {
            // The last column is inside the source, the one after it would not be.
            assert!((im.w - 1) as f64 * g.scale <= (W - 1) as f64 + 1e-6);
            assert!(im.w as f64 * g.scale > (W - 1) as f64);
            assert!((im.h - 1) as f64 * g.scale <= (H - 1) as f64 + 1e-6);
            assert!(im.h as f64 * g.scale > (H - 1) as f64);
            for (x, y) in [
                (0, 0),
                (im.w - 1, 0),
                (0, im.h - 1),
                (im.w - 1, im.h - 1),
                (im.w / 2, im.h / 3),
                (37, 91),
            ] {
                let expect = 0.2 * (x as f64 * g.scale) + 0.1 * (y as f64 * g.scale);
                let got = f64::from(im.data[y * im.stride + x]);
                assert!(
                    (got - expect).abs() <= 1.3,
                    "level {l} ({x}, {y}): {got} vs {expect}"
                );
            }
        }
    }

    #[test]
    fn intensity_centroid_points_towards_the_bright_side() {
        let n = 61usize;
        let (cx, cy) = (30usize, 30usize);
        for deg in [0.0f32, 30.0, 90.0, 135.0, 180.0, -60.0, -150.0, 7.0] {
            let phi = deg.to_radians();
            let (bx, by) = (cx as f32 + 7.0 * phi.cos(), cy as f32 + 7.0 * phi.sin());
            let img: Vec<u8> = (0..n * n)
                .map(|i| {
                    let (x, y) = ((i % n) as f32, (i / n) as f32);
                    let d2 = (x - bx).powi(2) + (y - by).powi(2);
                    (20.0 + 200.0 * (-d2 / 18.0).exp()).round() as u8
                })
                .collect();
            let a = ic_angle(&img, n, cx, cy);
            let diff = ((a - phi + PI).rem_euclid(TAU) - PI).abs();
            assert!(
                diff < 3f32.to_radians(),
                "blob at {deg} deg: got {} deg",
                a.to_degrees()
            );
        }
        // Brightness offset and gain do not change the direction.
        let img: Vec<u8> = (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as f32, (i / n) as f32);
                (60.0 + 100.0 * (-((x - 36.0).powi(2) + (y - 25.0).powi(2)) / 18.0).exp()).round()
                    as u8
            })
            .collect();
        let a = ic_angle(&img, n, cx, cy);
        assert!(
            (a - (-5.0f32).atan2(6.0)).abs() < 3f32.to_radians(),
            "{}",
            a.to_degrees()
        );
    }

    #[test]
    fn descriptors_are_covariant_with_exact_quarter_turns() {
        // Rotating the image by 90 degrees about a pixel turns the intensity-centroid angle by 90 degrees and, since the
        // steered pattern turns with it, leaves the descriptor unchanged (the smoothing is symmetric).
        let n = 101usize;
        let mut cur = to_u8(&canvas(n, n, 6));
        let (mut x, mut y) = (47usize, 52usize);
        let sm = gaussian_blur(&Image::new(n, n, &cur).unwrap());
        let a0 = ic_angle(&sm, n, x, y);
        let d0 = describe(&sm, n, x, y, a0);
        for quarter in 1..4 {
            // Pixel (x, y) moves to (n - 1 - y, x): a +90 degree turn in the x-right, y-down frame.
            let mut next = vec![0u8; n * n];
            for yy in 0..n {
                for xx in 0..n {
                    next[xx * n + (n - 1 - yy)] = cur[yy * n + xx];
                }
            }
            cur = next;
            (x, y) = (n - 1 - y, x);
            let sm = gaussian_blur(&Image::new(n, n, &cur).unwrap());
            let a = ic_angle(&sm, n, x, y);
            let expect = a0 + quarter as f32 * PI / 2.0;
            let diff = ((a - expect + PI).rem_euclid(TAU) - PI).abs();
            assert!(
                diff < 1e-3,
                "quarter {quarter}: angle {} vs {}",
                a.to_degrees(),
                expect.to_degrees()
            );
            let d = describe(&sm, n, x, y, a);
            // Identical unless some rotated sample sits within float noise of a rounding tie.
            assert!(
                hamming(&d0, &d) <= 4,
                "quarter {quarter}: {} bits differ",
                hamming(&d0, &d)
            );
        }
    }

    #[test]
    fn descriptors_separate_true_matches_from_different_places() {
        let pair = Pair::new(13, 12f32.to_radians(), 1.0, (3.4, -2.2), 0.8, 10.0, 2.0);
        let params = OrbParams::default();
        let ka = extract(&Image::new(W, H, &pair.a).unwrap(), None, &params);
        let kb = extract(&Image::new(W, H, &pair.b).unwrap(), None, &params);

        // Bits are roughly balanced, and keypoints at different places are far apart. (Random BRIEF pairs on corner
        // patches are correlated, so the mean distance between unrelated keypoints sits below the 128 of random
        // vectors: about 75 to 100 on natural photographs and on this image.)
        let ones: u32 = ka
            .iter()
            .map(|k| k.1.iter().map(|b| b.count_ones()).sum::<u32>())
            .sum();
        let density = ones as f64 / (ka.len() * 256) as f64;
        let (mut sum, mut n) = (0u64, 0u64);
        for (i, a) in ka.iter().enumerate() {
            for b in &ka[i + 1..] {
                sum += u64::from(hamming(&a.1, &b.1));
                n += 1;
            }
        }
        let unrelated = sum as f64 / n as f64;

        // True correspondences (known warp, within 1.5 px) are much closer than that.
        let mut dists = Vec::new();
        for (a, da) in &ka {
            let (ex, ey) = pair.forward(a.x, a.y);
            let best = kb
                .iter()
                .filter(|(b, _)| (b.x - ex).hypot(b.y - ey) <= 1.5 && b.level == a.level)
                .map(|(_, db)| hamming(da, db))
                .min();
            dists.extend(best);
        }
        let related = dists.iter().map(|&d| f64::from(d)).sum::<f64>() / dists.len() as f64;
        println!("bit density {density:.3}; unrelated pairs {unrelated:.1} bits, true correspondences {related:.1} bits ({} found)", dists.len());
        assert!((0.35..0.65).contains(&density), "bit density {density}");
        assert!(
            (60.0..150.0).contains(&unrelated),
            "mean distance of unrelated keypoints {unrelated}"
        );
        assert!(dists.len() >= 100);
        assert!(
            related < 0.55 * unrelated,
            "true correspondences {related} bits vs unrelated {unrelated}"
        );
    }

    #[test]
    fn pattern_is_well_formed_and_pinned() {
        let (mut sum, mut sum2) = (0.0f64, 0.0f64);
        for p in PATTERN.iter() {
            assert!(
                p.iter().all(|v| (-15..=15).contains(v)),
                "{p:?} leaves the 31x31 patch"
            );
            let (dx, dy) = (
                i32::from(p[0]) - i32::from(p[2]),
                i32::from(p[1]) - i32::from(p[3]),
            );
            assert!(dx * dx + dy * dy >= MIN_PAIR_SEP2, "{p:?} too short");
            for &v in p {
                sum += f64::from(v);
                sum2 += f64::from(v) * f64::from(v);
            }
        }
        let n = (PATTERN.len() * 4) as f64;
        let mean = sum / n;
        let std = (sum2 / n - mean * mean).sqrt();
        println!("pattern: mean {mean:.2}, std {std:.2}");
        assert!(mean.abs() < 0.7, "mean {mean}");
        assert!((5.0..7.0).contains(&std), "std {std}");
        // FNV-1a of the table: any change of the pattern (and so of every stored descriptor) must be deliberate and
        // come with a new DESCRIPTOR_VERSION.
        let hash = PATTERN
            .iter()
            .flatten()
            .fold(0xCBF2_9CE4_8422_2325u64, |h, &v| {
                (h ^ u64::from(v as u8)).wrapping_mul(0x0100_0000_01B3)
            });
        assert_eq!(DESCRIPTOR_VERSION, 1);
        assert_eq!(
            hash, 0x0645_acfa_93b0_82e8,
            "pattern changed: FNV-1a is now {hash:#018x}"
        );
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Robustness and bookkeeping
    // ---------------------------------------------------------------------------------------------------------------

    #[test]
    fn row_padding_is_ignored() {
        let data = textured_640x480(7);
        let stride = W + 13;
        let mut padded = vec![0u8; stride * H];
        let mut rng = Rng::new(99);
        for (y, row) in padded.chunks_exact_mut(stride).enumerate() {
            row[..W].copy_from_slice(&data[y * W..(y + 1) * W]);
            for p in &mut row[W..] {
                *p = (rng.next_u64() & 0xFF) as u8;
            }
        }
        let p = OrbParams::default();
        let tight = extract(&Image::new(W, H, &data).unwrap(), None, &p);
        assert!(!tight.is_empty());
        let strided = Image {
            w: W,
            h: H,
            stride,
            data: &padded,
        };
        assert_eq!(extract(&strided, None, &p), tight);
        // The buffer may end right after the last pixel of the last row.
        let short = Image {
            w: W,
            h: H,
            stride,
            data: &padded[..stride * (H - 1) + W],
        };
        assert!(short.is_valid());
        assert_eq!(extract(&short, None, &p), tight);
    }

    #[test]
    fn invalid_or_tiny_inputs_give_no_keypoints() {
        let p = OrbParams::default();
        let data = textured_640x480(8);
        let short = Image {
            w: W,
            h: H,
            stride: W,
            data: &data[..W * H - 1],
        };
        let narrow_stride = Image {
            w: W,
            h: H,
            stride: W - 1,
            data: &data,
        };
        let no_width = Image {
            w: 0,
            h: H,
            stride: 0,
            data: &data,
        };
        let no_height = Image {
            w: W,
            h: 0,
            stride: W,
            data: &data,
        };
        let overflow = Image {
            w: usize::MAX,
            h: 2,
            stride: usize::MAX,
            data: &data,
        };
        for bad in [short, narrow_stride, no_width, no_height, overflow] {
            assert!(!bad.is_valid(), "{bad:?}");
            assert!(extract(&bad, None, &p).is_empty(), "{bad:?}");
        }
        assert!(Image::new(W, H, &data[..10]).is_none());
        assert!(Image::new(W, H, &data).is_some());
        // Too small to hold a keypoint with its patch.
        let tiny = vec![128u8; 50 * 50];
        assert!(extract(&Image::new(50, 50, &tiny).unwrap(), None, &p).is_empty());
        let flat = vec![128u8; W * H];
        assert!(
            extract(&Image::new(W, H, &flat).unwrap(), None, &p).is_empty(),
            "no corners in a flat image"
        );
        let zero_budget = OrbParams {
            n_features: 0,
            ..OrbParams::default()
        };
        assert!(extract(&Image::new(W, H, &data).unwrap(), None, &zero_budget).is_empty());
    }

    #[test]
    fn odd_sizes_extreme_pixels_and_random_parameters_never_panic() {
        let mut rng = Rng::new(77);
        let sizes = [
            (1, 1),
            (1, 500),
            (500, 1),
            (7, 7),
            (43, 43),
            (59, 61),
            (60, 60),
            (61, 61),
            (65, 300),
            (300, 65),
            (131, 97),
            (254, 255),
            (641, 479),
        ];
        let mut total = 0;
        for &(w, h) in &sizes {
            for variant in 0..4 {
                let data: Vec<u8> = match variant {
                    0 => (0..w * h).map(|_| rng.next_u64() as u8).collect(),
                    // Full-contrast noise: the largest gradients, scores and sums the arithmetic can see.
                    1 => (0..w * h)
                        .map(|_| if rng.next_u64() & 1 == 0 { 0 } else { 255 })
                        .collect(),
                    2 => (0..w * h)
                        .map(|i| {
                            if (i % w / 2 + i / w / 2) & 1 == 0 {
                                0
                            } else {
                                255
                            }
                        })
                        .collect(),
                    _ if w >= 8 && h >= 8 => to_u8(&canvas(w, h, rng.next_u64())),
                    _ => vec![255; w * h],
                };
                let img = Image::new(w, h, &data).unwrap();
                let p = OrbParams {
                    n_features: (rng.next_u64() % 1500) as usize,
                    n_levels: (rng.next_u64() % 12) as usize,
                    scale: rng.range(0.5, 3.0),
                    fast_threshold: (rng.next_u64() % 60) as u8,
                    min_fast_threshold: (rng.next_u64() % 60) as u8,
                    grid_cols: (rng.next_u64() % 20) as usize,
                    grid_rows: (rng.next_u64() % 20) as usize,
                };
                let stripes = |x: f32, y: f32| ((x as u32 + y as u32) / 7) & 1 == 0;
                let mask: Option<&dyn Fn(f32, f32) -> bool> = if variant % 2 == 0 {
                    Some(&stripes)
                } else {
                    None
                };
                let kps = extract(&img, mask, &p);
                total += kps.len();
                assert!(
                    kps.len() <= p.n_features,
                    "{w}x{h} variant {variant}: {} keypoints",
                    kps.len()
                );
                for (k, _) in &kps {
                    assert!(
                        k.x >= BORDER as f32 && k.x <= (w - 1 - BORDER) as f32,
                        "{w}x{h}: x = {}",
                        k.x
                    );
                    assert!(
                        k.y >= BORDER as f32 && k.y <= (h - 1 - BORDER) as f32,
                        "{w}x{h}: y = {}",
                        k.y
                    );
                    assert!(k.angle.is_finite() && k.response.is_finite());
                    assert!(usize::from(k.level) < MAX_LEVELS);
                    assert!(
                        mask_allows(mask, k.x, k.y),
                        "the mask must hold at every keypoint"
                    );
                }
            }
        }
        assert!(
            total > 500,
            "some of these must produce keypoints ({total})"
        );
    }

    #[test]
    fn extraction_is_deterministic() {
        let data = textured_640x480(14);
        let img = Image::new(W, H, &data).unwrap();
        let p = OrbParams::default();
        assert_eq!(extract(&img, None, &p), extract(&img, None, &p));
    }

    #[test]
    fn absurd_parameters_are_sanitised() {
        let data = textured_640x480(9);
        let img = Image::new(W, H, &data).unwrap();
        let weird = OrbParams {
            n_features: 100,
            n_levels: 0,
            scale: f32::NAN,
            fast_threshold: 5,
            min_fast_threshold: 50,
            grid_cols: 0,
            grid_rows: 0,
        };
        let k = extract(&img, None, &weird);
        assert!(!k.is_empty() && k.len() <= 100, "{}", k.len());
        assert!(
            k.iter().all(|(k, _)| k.level == 0),
            "no levels asked for means the full-resolution one"
        );

        let many = OrbParams {
            n_levels: 100,
            scale: 1.5,
            ..OrbParams::default()
        };
        let k = extract(&img, None, &many);
        let top = k.iter().map(|(k, _)| k.level).max().unwrap();
        // 480 / 1.5^5 = 63 pixels is the last level with room for the 22 pixel borders and a usable area.
        assert_eq!(top, 5, "levels that become too small are dropped");
        assert!(k.len() as f32 >= 0.8 * 500.0);

        let huge_scale = OrbParams {
            scale: 1000.0,
            ..OrbParams::default()
        };
        let k = extract(&img, None, &huge_scale);
        assert!(
            k.iter().all(|(k, _)| k.level <= 1),
            "the scale is clamped to 8"
        );
        assert!(
            (OrbParams {
                scale: 1000.0,
                ..OrbParams::default()
            })
            .level_scale(1)
                == 8.0
        );

        let one_level = OrbParams {
            n_levels: 1,
            ..OrbParams::default()
        };
        let k = extract(&img, None, &one_level);
        assert!(k.len() as f32 >= 0.8 * 500.0 && k.iter().all(|(k, _)| k.level == 0));
    }

    #[test]
    fn keypoints_are_ordered_spaced_and_on_the_level_grid() {
        let data = textured_640x480(15);
        let p = OrbParams::default();
        let kps = extract(&Image::new(W, H, &data).unwrap(), None, &p);
        assert!(
            kps.windows(2).all(|w| w[0].0.level <= w[1].0.level),
            "ordered by level first"
        );
        for l in 0..p.n_levels as u8 {
            let s = p.level_scale(l);
            let pts: Vec<(f32, f32)> = kps
                .iter()
                .filter(|(k, _)| k.level == l)
                .map(|(k, _)| (k.x / s, k.y / s))
                .collect();
            assert!(
                pts.windows(2).all(|w| (w[0].1, w[0].0) < (w[1].1, w[1].0)),
                "level {l}: row-major order"
            );
            for &(x, y) in &pts {
                assert!(
                    (x - x.round()).abs() < 2e-3 && (y - y.round()).abs() < 2e-3,
                    "level {l}: ({x}, {y})"
                );
            }
            for (i, a) in pts.iter().enumerate() {
                for b in &pts[i + 1..] {
                    let d2 = (a.0 - b.0).powi(2) + (a.1 - b.1).powi(2);
                    assert!(
                        d2 >= MIN_KP_DIST2 as f32 - 0.1,
                        "level {l}: {a:?} and {b:?} are too close"
                    );
                }
            }
        }
    }

    #[test]
    fn level_budgets_are_proportional_to_the_area_and_exact() {
        let cfg = Cfg::new(&OrbParams::default());
        let geom = level_geometry(W, H, &cfg);
        let total: f64 = geom.iter().map(|g| (g.w * g.h) as f64).sum();
        for n in [1usize, 7, 100, 500, 1000, 1234] {
            let b = level_budgets(n, &geom);
            assert_eq!(b.iter().sum::<usize>(), n);
            for (bi, g) in b.iter().zip(&geom) {
                let exact = n as f64 * (g.w * g.h) as f64 / total;
                assert!((*bi as f64 - exact).abs() < 1.0, "{n}: {b:?}");
            }
        }
        assert_eq!(level_budgets(500, &geom), vec![199, 138, 96, 67]);
    }

    // ---------------------------------------------------------------------------------------------------------------
    // Internals: FAST, suppression, Harris, smoothing, selection
    // ---------------------------------------------------------------------------------------------------------------

    /// The FAST-9 score straight from the definition: the largest `t` such that nine consecutive ring pixels are all
    /// brighter than `c + t` or all darker than `c - t`, plus one (0 if there is none even at `t = 0`).
    fn naive_fast_score(ring: &[u8; 16], c: u8) -> i32 {
        let d: Vec<i32> = ring.iter().map(|&p| i32::from(p) - i32::from(c)).collect();
        let mut best = 0;
        for start in 0..16 {
            let arc: Vec<i32> = (0..9).map(|k| d[(start + k) % 16]).collect();
            let all_brighter_by = arc.iter().copied().min().unwrap_or(0);
            let all_darker_by = -arc.iter().copied().max().unwrap_or(0);
            best = best.max(all_brighter_by).max(all_darker_by);
        }
        best
    }

    #[test]
    fn ring_score_matches_the_textbook_definition() {
        let mut rng = Rng::new(21);
        let mut corners = 0;
        for n in 0..200_000 {
            let c = (rng.next_u64() & 0xFF) as u8;
            let mut ring = [0u8; 16];
            for p in &mut ring {
                *p = (rng.next_u64() & 0xFF) as u8;
            }
            if n % 2 == 0 {
                // A bright or dark arc of 7 to 11 pixels with some jitter: the interesting cases around the 9 limit.
                let start = (rng.next_u64() % 16) as usize;
                let len = 7 + (rng.next_u64() % 5) as usize;
                let sign = if rng.next_u64() & 1 == 0 { 1 } else { -1 };
                let margin = (rng.next_u64() % 60) as i32 + 1;
                for k in 0..len {
                    let jitter = (rng.next_u64() % 9) as i32 - 4;
                    ring[(start + k) % 16] =
                        (i32::from(c) + sign * margin + jitter).clamp(0, 255) as u8;
                }
            }
            let thr = (rng.next_u64() % 64) as u8;
            let naive = naive_fast_score(&ring, c);
            let expect = if naive > i32::from(thr) { naive } else { 0 };
            assert_eq!(
                i32::from(ring_score(&ring, c, thr)),
                expect,
                "ring {ring:?}, centre {c}, threshold {thr}"
            );
            corners += usize::from(expect > 0);
        }
        assert!(
            corners > 10_000,
            "the generator should hit plenty of corners ({corners})"
        );

        // Hand-made: 8 bright pixels are not enough, 9 are (also across the wrap-around), the score is the weakest arc pixel.
        let mut ring = [100u8; 16];
        for k in 0..8 {
            ring[(12 + k) % 16] = 160;
        }
        assert_eq!(ring_score(&ring, 100, 20), 0);
        ring[(12 + 8) % 16] = 140;
        assert_eq!(ring_score(&ring, 100, 20), 40);
        assert_eq!(ring_score(&ring, 100, 39), 40);
        assert_eq!(ring_score(&ring, 100, 40), 0);
        // Dark arc.
        let mut ring = [200u8; 16];
        for r in ring.iter_mut().take(9) {
            *r = 120;
        }
        assert_eq!(ring_score(&ring, 200, 7), 80);
    }

    fn ring_at(img: &Image, x: usize, y: usize) -> [u8; 16] {
        const OFFSETS: [(isize, isize); 16] = [
            (0, -3),
            (1, -3),
            (2, -2),
            (3, -1),
            (3, 0),
            (3, 1),
            (2, 2),
            (1, 3),
            (0, 3),
            (-1, 3),
            (-2, 2),
            (-3, 1),
            (-3, 0),
            (-3, -1),
            (-2, -2),
            (-1, -3),
        ];
        std::array::from_fn(|k| {
            let (dx, dy) = OFFSETS[k];
            img.data[(y as isize + dy) as usize * img.stride + (x as isize + dx) as usize]
        })
    }

    #[test]
    fn fast_cell_reports_exactly_the_local_maxima_of_the_score_map() {
        let data = textured_640x480(16);
        let img = Image::new(W, H, &data).unwrap();
        let rect = Rect {
            x0: 100,
            y0: 90,
            x1: 180,
            y1: 150,
        };
        for thr in [20u8, 7] {
            let mut found = Vec::new();
            fast_cell(&img, rect, thr, &mut Vec::new(), &mut found);
            // Score map by the definition, one pixel beyond the rectangle.
            let score = |x: usize, y: usize| -> u8 {
                let s = naive_fast_score(&ring_at(&img, x, y), img.data[y * img.stride + x]);
                if s > i32::from(thr) {
                    s.min(255) as u8
                } else {
                    0
                }
            };
            let mut expect = Vec::new();
            for y in rect.y0..rect.y1 {
                for x in rect.x0..rect.x1 {
                    let s = score(x, y);
                    // Strictly above the neighbours that come earlier in raster order, at least equal to the later ones.
                    let earlier = [(x - 1, y - 1), (x, y - 1), (x + 1, y - 1), (x - 1, y)];
                    let later = [(x + 1, y), (x - 1, y + 1), (x, y + 1), (x + 1, y + 1)];
                    if s > 0
                        && earlier.iter().all(|&(a, b)| score(a, b) < s)
                        && later.iter().all(|&(a, b)| score(a, b) <= s)
                    {
                        expect.push((x, y, s));
                    }
                }
            }
            let got: Vec<(usize, usize, u8)> = found.iter().map(|c| (c.x, c.y, c.score)).collect();
            println!("threshold {thr}: {} corners", got.len());
            assert!(got.len() > 20, "the test region should hold corners");
            assert_eq!(got, expect, "threshold {thr}");
            // No two reported corners touch.
            for (i, a) in got.iter().enumerate() {
                for b in &got[i + 1..] {
                    assert!(
                        a.0.abs_diff(b.0) > 1 || a.1.abs_diff(b.1) > 1,
                        "{a:?} and {b:?} touch"
                    );
                }
            }
        }
    }

    #[test]
    fn harris_prefers_corners_to_edges_and_is_invariant_to_quarter_turns() {
        let n = 41usize;
        let quadrant: Vec<u8> = (0..n * n)
            .map(|i| if i % n >= 20 && i / n >= 20 { 220 } else { 30 })
            .collect();
        let edge: Vec<u8> = (0..n * n)
            .map(|i| if i % n >= 20 { 220 } else { 30 })
            .collect();
        let flat = vec![90u8; n * n];
        let at = |d: &[u8]| harris(&Image::new(n, n, d).unwrap(), 20, 20);
        let (corner, along_edge, nothing) = (at(&quadrant), at(&edge), at(&flat));
        println!("harris: corner {corner:.4}, edge {along_edge:.4}, flat {nothing}");
        assert!(corner > 0.0, "corner {corner}");
        assert!(along_edge < 0.0, "edge {along_edge}");
        assert_eq!(nothing, 0.0);
        // Full contrast (0 to 255) scales the response by (255 / 190)^4.
        let strong: Vec<u8> = quadrant
            .iter()
            .map(|&p| if p > 100 { 255 } else { 0 })
            .collect();
        println!("harris, full-contrast corner: {:.5}", at(&strong));
        assert!(
            (at(&strong) / corner - (255.0f32 / 190.0).powi(4)).abs()
                < 0.05 * (255.0f32 / 190.0).powi(4)
        );
        // Turned by 90 degrees about the centre pixel: the same response.
        let mut turned = vec![0u8; n * n];
        for y in 0..n {
            for x in 0..n {
                turned[x * n + (n - 1 - y)] = quadrant[y * n + x];
            }
        }
        let t = at(&turned);
        assert!((t - corner).abs() <= 1e-6 * corner.abs(), "{t} vs {corner}");
    }

    #[test]
    fn gaussian_blur_keeps_constants_spreads_an_impulse_symmetrically_and_respects_the_stride() {
        let (w, h) = (40usize, 30usize);
        let flat = vec![137u8; w * h];
        assert!(gaussian_blur(&Image::new(w, h, &flat).unwrap())
            .iter()
            .all(|&p| p == 137));
        let mut impulse = vec![0u8; w * h];
        impulse[15 * w + 20] = 255;
        let out = gaussian_blur(&Image::new(w, h, &impulse).unwrap());
        let at = |x: usize, y: usize| i32::from(out[y * w + x]);
        for d in 1..=3usize {
            assert_eq!(at(20 + d, 15), at(20 - d, 15));
            assert_eq!(at(20, 15 + d), at(20, 15 - d));
            assert_eq!(
                at(20 + d, 15),
                at(20, 15 + d),
                "the kernel is separable and symmetric"
            );
            assert!(
                at(20 + d, 15) < at(20 + d - 1, 15),
                "the response falls off with the distance"
            );
        }
        assert_eq!(at(20 + 4, 15), 0);
        // The total is conserved up to rounding of the 49 non-zero taps.
        let sum: i32 = out.iter().map(|&p| i32::from(p)).sum();
        assert!((255 - 24..=255 + 24).contains(&sum), "sum {sum}");
        // Stride: a padded copy with garbage in the padding gives the same result as the tight image.
        let textured = textured_640x480(18);
        let (tw, th, stride) = (64usize, 48usize, 64 + 5);
        let mut padded = vec![0xA5u8; stride * th];
        for y in 0..th {
            padded[y * stride..y * stride + tw].copy_from_slice(&textured[y * W..y * W + tw]);
        }
        let tight: Vec<u8> = (0..th)
            .flat_map(|y| textured[y * W..y * W + tw].to_vec())
            .collect();
        let a = gaussian_blur(&Image::new(tw, th, &tight).unwrap());
        let b = gaussian_blur(&Image {
            w: tw,
            h: th,
            stride,
            data: &padded,
        });
        assert_eq!(a, b);
    }

    fn cand(x: usize, y: usize, resp: f32, cell: u16) -> Cand {
        Cand {
            x,
            y,
            resp,
            level: 0,
            cell,
        }
    }

    #[test]
    fn selection_takes_the_best_of_every_cell_first_then_the_strongest() {
        // Cell 0 holds three strong candidates, cell 1 a weak one, cell 2 a medium one.
        let cands = vec![
            cand(30, 30, 9.0, 0),
            cand(60, 30, 8.0, 0),
            cand(90, 30, 7.0, 0),
            cand(30, 60, 1.0, 1),
            cand(60, 60, 4.0, 2),
        ];
        let resp = |v: &[Cand]| {
            let mut r: Vec<f32> = v.iter().map(|c| c.resp).collect();
            r.sort_by(|a, b| b.total_cmp(a));
            r
        };
        // Budget 3: one per cell, although 8 and 7 are stronger than 1 and 4.
        let (picked, rest) = select_level(&cands, 3, 3);
        assert_eq!(resp(&picked), vec![9.0, 4.0, 1.0]);
        assert_eq!(resp(&rest), vec![8.0, 7.0]);
        // Budget 5 takes everything; budget 4 adds the strongest remaining one.
        assert_eq!(
            resp(&select_level(&cands, 3, 5).0),
            vec![9.0, 8.0, 7.0, 4.0, 1.0]
        );
        assert_eq!(
            resp(&select_level(&cands, 3, 4).0),
            vec![9.0, 8.0, 4.0, 1.0]
        );
        // Fewer slots than cells: the strongest cells win.
        assert_eq!(resp(&select_level(&cands, 3, 2).0), vec![9.0, 4.0]);
        assert_eq!(resp(&select_level(&cands, 3, 1).0), vec![9.0]);
        assert!(select_level(&cands, 3, 0).0.is_empty());
        // Spacing: a weaker candidate 2 pixels from a stronger one is dropped even with budget to spare; one 3 pixels
        // away is kept.
        let close = vec![
            cand(50, 50, 5.0, 0),
            cand(52, 50, 4.0, 0),
            cand(50, 53, 3.0, 0),
            cand(60, 60, 2.0, 1),
        ];
        let (picked, rest) = select_level(&close, 2, 10);
        assert_eq!(resp(&picked), vec![5.0, 3.0, 2.0]);
        assert_eq!(resp(&rest), vec![4.0]);
        // Equal responses: the order is still deterministic (by position).
        let tied = vec![
            cand(40, 40, 1.0, 0),
            cand(30, 30, 1.0, 0),
            cand(30, 40, 1.0, 0),
        ];
        let (p1, _) = select_level(&tied, 1, 1);
        let mut shuffled = tied.clone();
        shuffled.reverse();
        let (p2, _) = select_level(&shuffled, 1, 1);
        assert_eq!((p1[0].x, p1[0].y), (30, 30));
        assert_eq!((p2[0].x, p2[0].y), (30, 30));
    }

    #[test]
    fn short_levels_are_topped_up_with_the_strongest_unused_candidates() {
        let at_level = |level: u8, x: usize, resp: f32| Cand {
            x,
            y: 40,
            resp,
            level,
            cell: 0,
        };
        // Level 0 has already picked two, level 1 one; level 2 has nothing at all.
        let mut picked = vec![
            vec![at_level(0, 30, 9.0), at_level(0, 60, 8.0)],
            vec![at_level(1, 30, 3.0)],
            vec![],
        ];
        let unused = vec![
            at_level(0, 90, 6.0),
            at_level(1, 60, 5.0),
            at_level(1, 31, 5.5), // too close to the pick at x = 30
            at_level(0, 120, 2.0),
        ];
        top_up(&mut picked, unused.clone(), 5);
        assert_eq!(
            picked.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![3, 2, 0]
        );
        // The strongest usable ones were taken: 6.0 (level 0) and 5.0 (level 1); 5.5 is within 3 pixels of a pick.
        assert!(picked[0].iter().any(|c| c.resp == 6.0));
        assert!(picked[1].iter().any(|c| c.resp == 5.0));
        // Nothing to add when the target is met, and everything usable is taken when it is not.
        let mut full = vec![vec![at_level(0, 30, 9.0)], vec![]];
        top_up(&mut full, unused.clone(), 1);
        assert_eq!(full.iter().map(Vec::len).sum::<usize>(), 1);
        let mut greedy = vec![vec![], vec![], vec![]];
        top_up(&mut greedy, unused, 100);
        assert_eq!(
            greedy.iter().map(Vec::len).collect::<Vec<_>>(),
            vec![2, 2, 0],
            "all four are usable when nothing is picked"
        );
    }

    #[test]
    fn a_large_request_is_met_by_redistributing_between_levels() {
        let data = textured_640x480(19);
        let img = Image::new(W, H, &data).unwrap();
        let p = OrbParams {
            n_features: 4000,
            ..OrbParams::default()
        };
        let kps = extract(&img, None, &p);
        let mut per_level = vec![0usize; p.n_levels];
        for (k, _) in &kps {
            per_level[usize::from(k.level)] += 1;
        }
        println!(
            "request 4000: {} keypoints, per level {per_level:?}",
            kps.len()
        );
        assert!(kps.len() <= 4000);
        assert!(kps.len() >= 3200, "{} keypoints", kps.len());
    }

    // ---------------------------------------------------------------------------------------------------------------
    // 5. Timing (not run by default)
    // ---------------------------------------------------------------------------------------------------------------

    fn median_ms(mut v: Vec<f64>) -> f64 {
        v.sort_by(|a, b| a.total_cmp(b));
        v[v.len() / 2]
    }

    #[test]
    #[ignore = "timing report: cargo test --release -- --ignored --nocapture"]
    fn timing() {
        let pair = Pair::new(2, 15f32.to_radians(), 1.0, (7.3, 7.3), 0.7, 20.0, 3.0);
        let params = OrbParams::default();
        let (ia, ib) = (
            Image::new(W, H, &pair.a).unwrap(),
            Image::new(W, H, &pair.b).unwrap(),
        );
        let time = |f: &mut dyn FnMut()| {
            for _ in 0..5 {
                f();
            }
            let mut t = Vec::new();
            for _ in 0..50 {
                let s = Instant::now();
                f();
                t.push(s.elapsed().as_secs_f64() * 1e3);
            }
            (
                median_ms(t.clone()),
                t.iter().copied().fold(f64::INFINITY, f64::min),
            )
        };

        let (t_ext, t_ext_min) = time(&mut || {
            std::hint::black_box(extract(std::hint::black_box(&ia), None, &params));
        });
        let ka = extract(&ia, None, &params);
        let kb = extract(&ib, None, &params);
        let da: Vec<Descriptor> = ka.iter().map(|k| k.1).collect();
        let db: Vec<Descriptor> = kb.iter().map(|k| k.1).collect();
        let (t_match, t_match_min) = time(&mut || {
            std::hint::black_box(match_bf(
                std::hint::black_box(&da),
                std::hint::black_box(&db),
                64,
                0.8,
            ));
        });
        let gate = |i: usize, j: usize| (ka[i].0.x - kb[j].0.x).hypot(ka[i].0.y - kb[j].0.y) < 80.0;
        let (t_gated, _) = time(&mut || {
            std::hint::black_box(match_gated(&da, &db, 64, 0.8, &gate));
        });

        // Where the extraction time goes.
        let cfg = Cfg::new(&params);
        let geom = level_geometry(W, H, &cfg);
        let (t_pyr, _) = time(&mut || {
            std::hint::black_box(build_pyramid(&ia, &geom, cfg.scale));
        });
        let bufs = build_pyramid(&ia, &geom, cfg.scale);
        let levels = level_images(ia, &bufs, &geom);
        let (t_blur, _) = time(&mut || {
            for im in &levels {
                std::hint::black_box(gaussian_blur(im));
            }
        });
        let budgets = level_budgets(cfg.n_features, &geom);
        let (t_det, _) = time(&mut || {
            let mut det = Detector {
                cfg: &cfg,
                mask: None,
                scratch: Vec::new(),
                corners: Vec::new(),
            };
            for (l, (im, g)) in levels.iter().zip(&geom).enumerate() {
                let c = det.detect_level(im, l as u8, g.scale as f32, budgets[l]);
                std::hint::black_box(select_level(&c, cfg.cols * cfg.rows, budgets[l]));
            }
        });

        println!(
            "keypoints {} / {}, matches {}",
            ka.len(),
            kb.len(),
            match_bf(&da, &db, 64, 0.8).len()
        );
        println!("extract 640x480, {} features: median {t_ext:.2} ms (min {t_ext_min:.2}); target < 20 ms", params.n_features);
        println!("  of which pyramid {t_pyr:.2} ms, smoothing of all levels {t_blur:.2} ms, detection+selection {t_det:.2} ms");
        println!(
            "match_bf {}x{}: median {t_match:.3} ms (min {t_match_min:.3}); match_gated (80 px gate): {t_gated:.3} ms; target < 5 ms",
            da.len(),
            db.len()
        );
        // The best of 50 runs measures the code rather than whatever else the machine was doing.
        if !cfg!(debug_assertions) {
            assert!(t_ext_min < 20.0, "extract took {t_ext_min:.2} ms at best");
            assert!(
                t_match_min < 5.0,
                "match_bf took {t_match_min:.3} ms at best"
            );
        }
    }
}
