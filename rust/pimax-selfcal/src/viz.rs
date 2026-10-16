//! Pictures for humans: rectified pairs with the matches and their vertical errors drawn in, before/after
//! stacks, zoomed anaglyph details. Plain RGB buffers written as PNG with `pimax_cam::png`.

use crate::pipeline::PairMatches;
use std::path::Path;

/// An RGB image.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Rgb {
    /// Width.
    pub w: usize,
    /// Height.
    pub h: usize,
    /// `w * h * 3` bytes.
    pub px: Vec<u8>,
}

/// Matches within 1 px of the epipolar line.
pub const GOOD: [u8; 3] = [0, 210, 255];
/// Matches further than 1 px from it.
pub const BAD: [u8; 3] = [255, 140, 0];
/// Guide lines.
pub const GUIDE: [u8; 3] = [150, 60, 60];

impl Rgb {
    /// A filled image.
    pub fn new(w: usize, h: usize, c: [u8; 3]) -> Rgb {
        Rgb {
            w,
            h,
            px: c.iter().copied().cycle().take(w * h * 3).collect(),
        }
    }

    /// From a grey image.
    pub fn from_grey(g: &[u8], w: usize, h: usize) -> Rgb {
        assert!(g.len() >= w * h);
        Rgb {
            w,
            h,
            px: g[..w * h].iter().flat_map(|&v| [v, v, v]).collect(),
        }
    }

    /// Set one pixel (ignored outside).
    pub fn set(&mut self, x: i64, y: i64, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let o = (y as usize * self.w + x as usize) * 3;
            self.px[o..o + 3].copy_from_slice(&c);
        }
    }

    /// Pixel value (black outside).
    pub fn get(&self, x: i64, y: i64) -> [u8; 3] {
        if x >= 0 && y >= 0 && (x as usize) < self.w && (y as usize) < self.h {
            let o = (y as usize * self.w + x as usize) * 3;
            [self.px[o], self.px[o + 1], self.px[o + 2]]
        } else {
            [0; 3]
        }
    }

    /// Line between two points (inclusive).
    pub fn line(&mut self, x0: f64, y0: f64, x1: f64, y1: f64, c: [u8; 3]) {
        let n = (x1 - x0).abs().max((y1 - y0).abs()).ceil().max(1.0) as usize;
        for i in 0..=n {
            let t = i as f64 / n as f64;
            self.set(
                (x0 + t * (x1 - x0)).round() as i64,
                (y0 + t * (y1 - y0)).round() as i64,
                c,
            );
        }
    }

    /// Small square outline of half size `r` around a point.
    pub fn square(&mut self, x: f64, y: f64, r: f64, c: [u8; 3]) {
        self.line(x - r, y - r, x + r, y - r, c);
        self.line(x + r, y - r, x + r, y + r, c);
        self.line(x + r, y + r, x - r, y + r, c);
        self.line(x - r, y + r, x - r, y - r, c);
    }

    /// Copy `src` with its top-left corner at `(ox, oy)`.
    pub fn blit(&mut self, src: &Rgb, ox: usize, oy: usize) {
        for y in 0..src.h.min(self.h.saturating_sub(oy)) {
            let n = src.w.min(self.w.saturating_sub(ox)) * 3;
            let d = ((oy + y) * self.w + ox) * 3;
            let s = y * src.w * 3;
            self.px[d..d + n].copy_from_slice(&src.px[s..s + n]);
        }
    }

    /// Nearest-neighbour enlargement by an integer factor.
    pub fn scaled(&self, k: usize) -> Rgb {
        let mut o = Rgb::new(self.w * k, self.h * k, [0; 3]);
        for y in 0..o.h {
            for x in 0..o.w {
                let s = ((y / k) * self.w + x / k) * 3;
                let d = (y * o.w + x) * 3;
                o.px[d..d + 3].copy_from_slice(&self.px[s..s + 3]);
            }
        }
        o
    }

    /// Crop (clipped to the image).
    pub fn crop(&self, x0: usize, y0: usize, w: usize, h: usize) -> Rgb {
        let (x0, y0) = (x0.min(self.w), y0.min(self.h));
        let (w, h) = (w.min(self.w - x0), h.min(self.h - y0));
        let mut o = Rgb::new(w, h, [0; 3]);
        for y in 0..h {
            let s = ((y0 + y) * self.w + x0) * 3;
            o.px[y * w * 3..(y + 1) * w * 3].copy_from_slice(&self.px[s..s + w * 3]);
        }
        o
    }

    /// PNG bytes.
    pub fn png(&self) -> Vec<u8> {
        pimax_cam::png::encode(self.w, self.h, 3, &self.px)
    }

    /// Write a PNG file.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        std::fs::write(path, self.png()).map_err(|e| format!("{}: {e}", path.display()))
    }
}

/// Stack images vertically with a gap (left aligned, dark background).
pub fn stack(parts: &[&Rgb], gap: usize) -> Rgb {
    let w = parts.iter().map(|p| p.w).max().unwrap_or(0);
    let h = parts.iter().map(|p| p.h).sum::<usize>() + gap * parts.len().saturating_sub(1);
    let mut o = Rgb::new(w, h, [24, 24, 28]);
    let mut y = 0;
    for p in parts {
        o.blit(p, 0, y);
        y += p.h + gap;
    }
    o
}

/// The rectified pair of `pm` side by side (left image | right image) with every match drawn:
/// a square at the corner in the left image, a square at the matched position in the right image, and in the
/// left image a vertical needle whose length is the vertical disparity times `needle` (so 1 px of error becomes
/// `needle` pixels: up = the right image's point is higher). Cyan: within 1 px; orange: worse. A faint
/// horizontal line every 40 rows helps to compare rows. `pm.images` must be present.
pub fn match_overlay(pm: &PairMatches, needle: f64) -> Option<Rgb> {
    let (l, r) = pm.images.as_ref()?;
    let (w, h) = (pm.geom.width, pm.geom.height);
    let mut o = Rgb::new(2 * w, h, [0; 3]);
    o.blit(&Rgb::from_grey(&pimax_cam::png::stretch(l), w, h), 0, 0);
    o.blit(&Rgb::from_grey(&pimax_cam::png::stretch(r), w, h), w, 0);
    for y in (40..h).step_by(40) {
        for x in (0..2 * w).step_by(4) {
            o.set(x as i64, y as i64, GUIDE);
        }
    }
    for m in &pm.set.matches {
        let c = if m.dy.abs() <= 1.0 { GOOD } else { BAD };
        o.square(m.x, m.y, 2.0, c);
        o.line(m.x, m.y, m.x, m.y + m.dy * needle, c);
        o.square(w as f64 + m.x - m.disp, m.y + m.dy, 2.0, c);
    }
    Some(o)
}

/// "Before" above "after" for the same scene and pair (see [`match_overlay`]).
pub fn before_after(before: &PairMatches, after: &PairMatches, needle: f64) -> Option<Rgb> {
    let (b, a) = (
        match_overlay(before, needle)?,
        match_overlay(after, needle)?,
    );
    Some(stack(&[&b, &a], 6))
}

/// Red/cyan anaglyph of two grey images of the same size (red = `a`, cyan = `b`); where they agree the picture
/// is grey, where they are displaced coloured fringes appear.
pub fn anaglyph(a: &[u8], b: &[u8], w: usize, h: usize) -> Rgb {
    Rgb {
        w,
        h,
        px: a[..w * h]
            .iter()
            .zip(&b[..w * h])
            .flat_map(|(&x, &y)| [x, y, y])
            .collect(),
    }
}

/// Crop of a grey image centred at `(cx, cy)` (black outside), contrast-stretched on its own (2nd to 98th
/// percentile) so that two cameras with different brightness give a neutral grey where they agree.
fn crop_norm(img: &[u8], w: usize, h: usize, cx: i64, cy: i64, size: usize) -> Vec<u8> {
    let half = (size / 2) as i64;
    let mut v = Vec::with_capacity(size * size);
    for yy in 0..size as i64 {
        for xx in 0..size as i64 {
            let (x, y) = (cx - half + xx, cy - half + yy);
            v.push(if x >= 0 && y >= 0 && x < w as i64 && y < h as i64 {
                img[y as usize * w + x as usize]
            } else {
                0
            });
        }
    }
    let mut s = v.clone();
    s.sort_unstable();
    let (lo, hi) = (s[s.len() / 50] as f64, s[s.len() * 49 / 50] as f64);
    if hi - lo < 4.0 {
        return v;
    }
    v.iter()
        .map(|&p| ((p as f64 - lo) / (hi - lo) * 255.0).clamp(0.0, 255.0) as u8)
        .collect()
}

/// A row of zoomed red/cyan details of two grey images of the same size: for each `(x, y, dx)` the window of
/// `a` around `(x, y)` in red and the window of `b` around `(x - dx, y)` in cyan, each contrast-stretched on its
/// own, enlarged `zoom` times. Where the two agree the detail is grey; a vertical misalignment shows as red and
/// cyan fringes above and below horizontal edges.
pub fn zoom_pair(
    a: &[u8],
    b: &[u8],
    w: usize,
    h: usize,
    centres: &[(i64, i64, i64)],
    size: usize,
    zoom: usize,
) -> Rgb {
    let mut row = Rgb::new(
        centres.len().max(1) * (size * zoom + 4) - 4,
        size * zoom,
        [24, 24, 28],
    );
    for (k, &(cx, cy, dx)) in centres.iter().enumerate() {
        let (ca, cb) = (
            crop_norm(a, w, h, cx, cy, size),
            crop_norm(b, w, h, cx - dx, cy, size),
        );
        row.blit(
            &anaglyph(&ca, &cb, size, size).scaled(zoom),
            k * (size * zoom + 4),
            0,
        );
    }
    row
}

/// Centres `(x, y, integer disparity)` of up to `count` strong matches of `pm`, spread over the image (no two
/// closer than `size`).
pub fn detail_centres(pm: &PairMatches, count: usize, size: usize) -> Vec<(i64, i64, i64)> {
    let mut ms: Vec<_> = pm.set.matches.iter().collect();
    ms.sort_by(|a, b| b.zncc.partial_cmp(&a.zncc).unwrap());
    let mut picked: Vec<&crate::matcher::StereoMatch> = Vec::new();
    for m in ms {
        if picked.len() >= count {
            break;
        }
        if picked
            .iter()
            .all(|p| (p.x - m.x).abs() > size as f64 || (p.y - m.y).abs() > size as f64)
        {
            picked.push(m);
        }
    }
    picked
        .iter()
        .map(|m| (m.x as i64, m.y as i64, m.disp.round() as i64))
        .collect()
}

/// Zoomed details of a rectified pair at its strongest matches (see [`zoom_pair`]).
pub fn details(pm: &PairMatches, count: usize, size: usize, zoom: usize) -> Option<Rgb> {
    let (l, r) = pm.images.as_ref()?;
    let c = detail_centres(pm, count, size);
    (!c.is_empty()).then(|| zoom_pair(l, r, pm.geom.width, pm.geom.height, &c, size, zoom))
}

/// The same details before (top row) and after (bottom row) a correction: the windows are those of the
/// strongest `before` matches, at the same places in both rows (the right image's window follows the disparity
/// of the nearest match in each row, since a correction can also shift the right image sideways).
pub fn details_before_after(
    before: &PairMatches,
    after: &PairMatches,
    count: usize,
    size: usize,
    zoom: usize,
) -> Option<Rgb> {
    let ((bl, br), (al, ar)) = (before.images.as_ref()?, after.images.as_ref()?);
    let c = detail_centres(before, count, size);
    if c.is_empty() {
        return None;
    }
    // the correction also moves the right image sideways: take the disparity of the nearest `after` match
    let ca: Vec<(i64, i64, i64)> = c
        .iter()
        .map(|&(x, y, d)| {
            let near = after
                .set
                .matches
                .iter()
                .filter(|m| (m.x - x as f64).abs() <= 4.0 && (m.y - y as f64).abs() <= 4.0)
                .min_by(|p, q| {
                    let dp = (p.x - x as f64).hypot(p.y - y as f64);
                    let dq = (q.x - x as f64).hypot(q.y - y as f64);
                    dp.partial_cmp(&dq).unwrap()
                });
            (x, y, near.map_or(d, |m| m.disp.round() as i64))
        })
        .collect();
    let (w, h) = (before.geom.width, before.geom.height);
    Some(stack(
        &[
            &zoom_pair(bl, br, w, h, &c, size, zoom),
            &zoom_pair(al, ar, w, h, &ca, size, zoom),
        ],
        6,
    ))
}

/// Up to `count` window centres where both grey images have strong texture (and neither is black or clipped):
/// where a misalignment between them is visible.
pub fn textured_centres(
    a: &[u8],
    b: &[u8],
    w: usize,
    h: usize,
    count: usize,
    size: usize,
) -> Vec<(i64, i64, i64)> {
    let energy = |img: &[u8], x0: usize, y0: usize| -> Option<f64> {
        let mut e = 0.0;
        for y in y0 + 1..y0 + size - 1 {
            for x in x0 + 1..x0 + size - 1 {
                let p = img[y * w + x];
                if p <= 4 || p >= 250 {
                    return None;
                }
                let gx = img[y * w + x + 1] as f64 - img[y * w + x - 1] as f64;
                let gy = img[(y + 1) * w + x] as f64 - img[(y - 1) * w + x] as f64;
                e += gx * gx + gy * gy;
            }
        }
        Some(e)
    };
    let mut cand = Vec::new();
    let step = (size / 2).max(1);
    let mut y0 = 0;
    while y0 + size <= h {
        let mut x0 = 0;
        while x0 + size <= w {
            if let (Some(ea), Some(eb)) = (energy(a, x0, y0), energy(b, x0, y0)) {
                cand.push((ea.min(eb), x0 + size / 2, y0 + size / 2));
            }
            x0 += step;
        }
        y0 += step;
    }
    cand.sort_by(|p, q| q.0.partial_cmp(&p.0).unwrap());
    let mut out: Vec<(i64, i64, i64)> = Vec::new();
    for (_, x, y) in cand {
        if out.len() >= count {
            break;
        }
        let (x, y) = (x as i64, y as i64);
        if out
            .iter()
            .all(|p| (p.0 - x).abs() >= size as i64 || (p.1 - y).abs() >= size as i64)
        {
            out.push((x, y, 0));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matcher::{MatchSet, StereoMatch};
    use crate::rect::RectGeom;
    use pimax_cam::geom::{Pose, IDENTITY};

    fn pm(dy: f64) -> PairMatches {
        let (w, h) = (64usize, 48usize);
        let img: Vec<u8> = (0..w * h)
            .map(|i| ((i % w) * 3 + (i / w) * 2) as u8)
            .collect();
        PairMatches {
            scene: 0,
            a: 0,
            b: 1,
            geom: RectGeom::new(
                &Pose {
                    r: IDENTITY,
                    t: [-0.1, 0.0, 0.0],
                },
                w,
                h,
                40.0,
            ),
            set: MatchSet {
                candidates: 1,
                matches: vec![StereoMatch {
                    x: 30.0,
                    y: 20.0,
                    disp: 4.0,
                    dy,
                    zncc: 0.9,
                    ray_a: [0.0, 0.0, 1.0],
                    ray_b: [0.0, 0.0, 1.0],
                }],
                rejects: [0; 5],
            },
            images: Some((img.clone(), img)),
        }
    }

    #[test]
    fn primitives() {
        let mut a = Rgb::new(8, 6, [1, 2, 3]);
        assert_eq!(a.get(7, 5), [1, 2, 3]);
        a.set(100, 100, [9; 3]); // ignored
        a.line(0.0, 0.0, 7.0, 5.0, [255, 0, 0]);
        assert_eq!(a.get(0, 0), [255, 0, 0]);
        assert_eq!(a.get(7, 5), [255, 0, 0]);
        let b = a.scaled(2);
        assert_eq!((b.w, b.h), (16, 12));
        assert_eq!(b.get(15, 11), [255, 0, 0]);
        assert_eq!(a.crop(6, 4, 10, 10).w, 2);
        let s = stack(&[&a, &b], 2);
        assert_eq!((s.w, s.h), (16, 20));
        assert_eq!(&a.png()[1..4], b"PNG");
        assert_eq!(
            anaglyph(&[10, 20], &[30, 40], 2, 1).px,
            vec![10, 30, 30, 20, 40, 40]
        );
    }

    #[test]
    fn overlay_marks_matches_and_colours_by_error() {
        let good = match_overlay(&pm(0.4), 8.0).unwrap();
        assert_eq!((good.w, good.h), (128, 48));
        // square corner in the left image, and at the matched position in the right image
        assert_eq!(good.get(28, 18), GOOD);
        assert_eq!(good.get(64 + 30 - 4 - 2, 18), GOOD);
        let bad = match_overlay(&pm(-2.0), 8.0).unwrap();
        // needle: 2 px up times 8
        assert_eq!(bad.get(30, 20 - 16), BAD);
        assert_ne!(bad.get(30, 20 - 17), BAD);
        let ba = before_after(&pm(-2.0), &pm(0.2), 8.0).unwrap();
        assert_eq!((ba.w, ba.h), (128, 102));
        let d = details(&pm(0.0), 3, 16, 4).unwrap();
        assert_eq!((d.w, d.h), (64, 64));
        let dd = details_before_after(&pm(-2.0), &pm(0.1), 3, 16, 4).unwrap();
        assert_eq!((dd.w, dd.h), (64, 134));
        // identical images give grey details (red = cyan)
        let img: Vec<u8> = (0..64 * 48)
            .map(|i| ((i % 64) * 3 + (i / 64) * 2) as u8)
            .collect();
        let z = zoom_pair(&img, &img, 64, 48, &[(30, 20, 0)], 16, 2);
        assert!(z.px.chunks(3).all(|c| c[0] == c[1] && c[1] == c[2]));
        let tc = textured_centres(&img, &img, 64, 48, 4, 16);
        assert!(!tc.is_empty() && tc.len() <= 4);
        let mut none = pm(0.0);
        none.images = None;
        assert!(match_overlay(&none, 8.0).is_none());
    }
}
