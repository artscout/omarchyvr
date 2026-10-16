//! Blob detection on short-exposure LED tiles (640x480 grey).
//!
//! Threshold + 8-connected components (two-pass union-find), intensity-weighted centroid above the background.

/// One detected blob in tile pixel coordinates (x right, y down).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Blob {
    pub x: f64,
    pub y: f64,
    /// Pixel count above the threshold.
    pub area: u32,
    /// Brightest pixel value.
    pub peak: u8,
    /// Bounding box width and height.
    pub w: u16,
    pub h: u16,
}

/// Detection parameters.
#[derive(Clone, Copy, Debug)]
pub struct BlobParams {
    /// Pixel threshold to be part of a blob.
    pub threshold: u8,
    /// A blob must contain at least one pixel at or above this value.
    pub required_peak: u8,
    pub min_area: u32,
    pub max_area: u32,
    /// Maximum bounding box side (rejects lamps and windows).
    pub max_side: u16,
}

impl Default for BlobParams {
    fn default() -> Self {
        BlobParams {
            threshold: 60,
            required_peak: 120,
            min_area: 2,
            max_area: 600,
            max_side: 40,
        }
    }
}

impl BlobParams {
    /// The live tracker's detection (`live`, `lockrate`): blob pixels from 80 instead of 60. At the LED-frame
    /// exposure (40 / gain 62) the 60-level halo merges neighbouring LEDs and shifts centroids; 80 gives more
    /// separate LEDs per controller (ctrl-test.pxcam, in-use part: brute-force finds both controllers in 54.5 %
    /// of LED frames instead of 46.8 %; analysis/ctrl-lock-rate.md). The required peak stays 120, so dim LEDs are
    /// detected as before.
    pub fn live() -> Self {
        BlobParams {
            threshold: 80,
            ..Default::default()
        }
    }

    /// `live()` with `--thr T` / `--peak P` overrides.
    pub fn from_args(a: &[String]) -> Self {
        let mut bp = Self::live();
        let get = |k: &str| {
            a.iter()
                .position(|s| s == k)
                .and_then(|i| a.get(i + 1))
                .and_then(|v| v.parse().ok())
        };
        if let Some(v) = get("--thr") {
            bp.threshold = v;
        }
        if let Some(v) = get("--peak") {
            bp.required_peak = v;
        }
        bp
    }
}

fn find(p: &mut [u32], mut a: u32) -> u32 {
    while p[a as usize] != a {
        p[a as usize] = p[p[a as usize] as usize];
        a = p[a as usize];
    }
    a
}

/// Detect blobs in a `w`x`h` image with row stride `stride`.
pub fn detect(img: &[u8], w: usize, h: usize, stride: usize, prm: &BlobParams) -> Vec<Blob> {
    let mut labels = vec![0u32; w * h];
    let mut parent: Vec<u32> = vec![0];
    for y in 0..h {
        for x in 0..w {
            if img[y * stride + x] < prm.threshold {
                continue;
            }
            let mut nb = [0u32; 4];
            let mut n = 0;
            if x > 0 && labels[y * w + x - 1] != 0 {
                nb[n] = labels[y * w + x - 1];
                n += 1;
            }
            if y > 0 {
                for dx in [-1i64, 0, 1] {
                    let xx = x as i64 + dx;
                    if xx >= 0 && (xx as usize) < w {
                        let l = labels[(y - 1) * w + xx as usize];
                        if l != 0 {
                            nb[n] = l;
                            n += 1;
                        }
                    }
                }
            }
            if n == 0 {
                let l = parent.len() as u32;
                parent.push(l);
                labels[y * w + x] = l;
            } else {
                let mut m = find(&mut parent, nb[0]);
                for &l in &nb[1..n] {
                    let r = find(&mut parent, l);
                    if r != m {
                        let (a, b) = if r < m { (r, m) } else { (m, r) };
                        parent[b as usize] = a;
                        m = a;
                    }
                }
                labels[y * w + x] = m;
            }
        }
    }
    #[derive(Clone, Copy, Default)]
    struct Acc {
        sw: f64,
        sx: f64,
        sy: f64,
        n: u32,
        peak: u8,
        x0: usize,
        x1: usize,
        y0: usize,
        y1: usize,
    }
    let mut acc = vec![
        Acc {
            x0: usize::MAX,
            y0: usize::MAX,
            ..Default::default()
        };
        parent.len()
    ];
    let bg = prm.threshold as f64 - 1.0;
    for y in 0..h {
        for x in 0..w {
            let l = labels[y * w + x];
            if l == 0 {
                continue;
            }
            let r = find(&mut parent, l) as usize;
            let v = img[y * stride + x];
            let a = &mut acc[r];
            let wt = v as f64 - bg;
            a.sw += wt;
            a.sx += wt * x as f64;
            a.sy += wt * y as f64;
            a.n += 1;
            a.peak = a.peak.max(v);
            a.x0 = a.x0.min(x);
            a.x1 = a.x1.max(x);
            a.y0 = a.y0.min(y);
            a.y1 = a.y1.max(y);
        }
    }
    acc.iter()
        .filter(|a| a.n > 0)
        .filter_map(|a| {
            let (bw, bh) = ((a.x1 - a.x0 + 1) as u16, (a.y1 - a.y0 + 1) as u16);
            (a.n >= prm.min_area
                && a.n <= prm.max_area
                && a.peak >= prm.required_peak
                && bw <= prm.max_side
                && bh <= prm.max_side)
                .then(|| Blob {
                    x: a.sx / a.sw,
                    y: a.sy / a.sw,
                    area: a.n,
                    peak: a.peak,
                    w: bw,
                    h: bh,
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_blobs_subpixel() {
        let (w, h) = (64, 48);
        let mut img = vec![0u8; w * h];
        // symmetric 3x3 blob centred at (10, 20)
        for (dy, dx, v) in [
            (0, 0, 250u8),
            (-1, 0, 150),
            (1, 0, 150),
            (0, -1, 150),
            (0, 1, 150),
        ] {
            img[(20 + dy) as usize * w + (10 + dx) as usize] = v;
        }
        // 2x1 blob: centre between 40 and 41 on row 5
        img[5 * w + 40] = 200;
        img[5 * w + 41] = 200;
        // a dim blob without a bright peak is rejected
        img[30 * w + 30] = 80;
        img[30 * w + 31] = 80;
        let b = detect(&img, w, h, w, &BlobParams::default());
        assert_eq!(b.len(), 2);
        let c = b.iter().find(|b| b.area == 5).unwrap();
        assert!((c.x - 10.0).abs() < 1e-9 && (c.y - 20.0).abs() < 1e-9);
        let d = b.iter().find(|b| b.area == 2).unwrap();
        assert!((d.x - 40.5).abs() < 1e-9 && (d.y - 5.0).abs() < 1e-9);
    }

    #[test]
    fn u_shape_merges() {
        let (w, h) = (16, 16);
        let mut img = vec![0u8; w * h];
        for y in 2..8 {
            img[y * w + 2] = 200;
            img[y * w + 6] = 200;
        }
        for x in 2..7 {
            img[8 * w + x] = 200;
        }
        let b = detect(&img, w, h, w, &BlobParams::default());
        assert_eq!(b.len(), 1);
        assert_eq!(b[0].area, 17);
    }
}
