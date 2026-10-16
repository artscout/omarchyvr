//! Sharpness measurements on crops of the final compositor image.
//!
//! * [`dominant_frequency`] / [`amplitude_at`]: windowed single-frequency DFT, used for line-pair ladders. The ratio of the
//!   modulation depth in the final image to the one of the source ladder is the end-to-end MTF at that period.
//! * [`edge_profile`]: slanted-edge style edge spread function of an arbitrarily oriented straight edge: 10-90 % rise
//!   distance and MTF50 along the edge normal, both in pixels of the analysed image.

use crate::dump::Gray;
use std::f64::consts::PI;

/// Result of a single-frequency estimate.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Peak {
    /// Spatial frequency in cycles per pixel, x and y components.
    pub fx: f64,
    pub fy: f64,
    /// Amplitude of the sinusoid at that frequency, in the units of the image (0..1 luma).
    pub amp: f64,
    /// Mean of the analysed window (Hann weighted).
    pub mean: f64,
}

impl Peak {
    /// Frequency magnitude, cycles per pixel.
    pub fn freq(&self) -> f64 {
        self.fx.hypot(self.fy)
    }
    /// Modulation depth `amp / mean`.
    pub fn depth(&self) -> f64 {
        if self.mean.abs() < 1e-12 {
            0.0
        } else {
            self.amp / self.mean
        }
    }
}

fn hann(i: usize, n: usize) -> f64 {
    if n <= 1 {
        1.0
    } else {
        0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos()
    }
}

/// Hann-windowed amplitude of `g` (mean removed) at spatial frequency `(fx, fy)` cycles/pixel.
/// For `I = m + a cos(2 pi (fx x + fy y) + phi)` this returns `(a, m)`.
pub fn amplitude_at(g: &Gray, fx: f64, fy: f64) -> (f64, f64) {
    let wx: Vec<f64> = (0..g.w).map(|i| hann(i, g.w)).collect();
    let wy: Vec<f64> = (0..g.h).map(|i| hann(i, g.h)).collect();
    let mut sw = 0.0;
    let mut sm = 0.0;
    for (y, wyv) in wy.iter().enumerate() {
        for (x, wxv) in wx.iter().enumerate() {
            let w = wxv * wyv;
            sw += w;
            sm += w * f64::from(g.at(x, y));
        }
    }
    if sw == 0.0 {
        return (0.0, 0.0);
    }
    let mean = sm / sw;
    let (mut re, mut im) = (0.0, 0.0);
    // x and y phases factor; evaluate the exponentials per row/column once
    let ex: Vec<(f64, f64)> = (0..g.w).map(|x| (2.0 * PI * fx * x as f64).sin_cos()).collect();
    let ey: Vec<(f64, f64)> = (0..g.h).map(|y| (2.0 * PI * fy * y as f64).sin_cos()).collect();
    for (y, (&(sy, cy), wyv)) in ey.iter().zip(&wy).enumerate() {
        for (x, (&(sx, cx), wxv)) in ex.iter().zip(&wx).enumerate() {
            let w = wxv * wyv;
            let v = w * (f64::from(g.at(x, y)) - mean);
            // e^{-i(px+py)} = (cx - i sx)(cy - i sy)
            re += v * (cx * cy - sx * sy);
            im -= v * (sx * cy + cx * sy);
        }
    }
    (2.0 * re.hypot(im) / sw, mean)
}

/// Finds the strongest sinusoid with frequency magnitude in `[f_lo, f_hi]` cycles/pixel (any orientation).
pub fn dominant_frequency(g: &Gray, f_lo: f64, f_hi: f64) -> Option<Peak> {
    if g.w < 8 || g.h < 8 || !(f_hi > f_lo && f_lo > 0.0) {
        return None;
    }
    let mut best: Option<Peak> = None;
    let consider = |best: &mut Option<Peak>, fx: f64, fy: f64| {
        let (amp, mean) = amplitude_at(g, fx, fy);
        let better = match best {
            None => true,
            Some(b) => amp > b.amp,
        };
        if better {
            *best = Some(Peak { fx, fy, amp, mean });
        }
    };
    // coarse polar scan; step fine enough for windows of ~100 px (resolution ~ 1/size)
    let size = g.w.max(g.h) as f64;
    let df = (0.5 / size).max(1e-4);
    let nr = (((f_hi - f_lo) / df).ceil() as usize).max(2);
    let nth = 120usize;
    for ir in 0..=nr {
        let r = f_lo + (f_hi - f_lo) * ir as f64 / nr as f64;
        for it in 0..nth {
            let th = PI * it as f64 / nth as f64;
            consider(&mut best, r * th.cos(), r * th.sin());
        }
    }
    // refine around the best with shrinking steps
    let mut step = df;
    for _ in 0..24 {
        let b = best?;
        for dx in [-1.0, 0.0, 1.0] {
            for dy in [-1.0, 0.0, 1.0] {
                if dx != 0.0 || dy != 0.0 {
                    consider(&mut best, b.fx + dx * step, b.fy + dy * step);
                }
            }
        }
        step *= 0.6;
    }
    best
}

/// Standard normal CDF (Abramowitz & Stegun 7.1.26 based erf, error < 1.5e-7).
pub fn norm_cdf(x: f64) -> f64 {
    let z = x / std::f64::consts::SQRT_2;
    let t = 1.0 / (1.0 + 0.327_591_1 * z.abs());
    let poly = t
        * (0.254_829_592 + t * (-0.284_496_736 + t * (1.421_413_741 + t * (-1.453_152_027 + t * 1.061_405_429))));
    let erf = 1.0 - poly * (-z * z).exp();
    0.5 * (1.0 + if z >= 0.0 { erf } else { -erf })
}

/// Edge spread analysis result.
#[derive(Debug, Clone, PartialEq)]
pub struct Edge {
    /// Direction of the edge normal, degrees from +x towards +y (range -90..90).
    pub normal_deg: f64,
    /// Distance between the 10 % and 90 % points of the edge spread function, pixels.
    pub rise_10_90: f64,
    /// Spatial frequency where the MTF drops to 50 %, cycles/pixel (None if it never does below 2 c/px).
    pub mtf50: Option<f64>,
    /// Plateau levels (the two sides of the edge).
    pub levels: (f64, f64),
    /// Oversampled edge spread function: (distance along the normal in px, level).
    pub esf: Vec<(f64, f64)>,
}

const BIN: f64 = 0.25;

fn bin_esf(g: &Gray, theta: f64, margin: usize) -> (Vec<(f64, f64)>, f64) {
    let (c, s) = (theta.cos(), theta.sin());
    let (cx, cy) = (g.w as f64 / 2.0, g.h as f64 / 2.0);
    let mut dmin = f64::MAX;
    let mut dmax = f64::MIN;
    for y in margin..g.h - margin {
        for x in margin..g.w - margin {
            let d = (x as f64 - cx) * c + (y as f64 - cy) * s;
            dmin = dmin.min(d);
            dmax = dmax.max(d);
        }
    }
    let nb = (((dmax - dmin) / BIN).floor() as usize) + 1;
    let mut sum = vec![0.0f64; nb];
    let mut sum2 = vec![0.0f64; nb];
    let mut cnt = vec![0u32; nb];
    for y in margin..g.h - margin {
        for x in margin..g.w - margin {
            let d = (x as f64 - cx) * c + (y as f64 - cy) * s;
            let b = (((d - dmin) / BIN) as usize).min(nb - 1);
            let v = f64::from(g.at(x, y));
            sum[b] += v;
            sum2[b] += v * v;
            cnt[b] += 1;
        }
    }
    let mut resid = 0.0;
    let mut esf = Vec::new();
    for b in 0..nb {
        if cnt[b] > 0 {
            let n = f64::from(cnt[b]);
            let m = sum[b] / n;
            resid += sum2[b] - n * m * m;
            esf.push((dmin + (b as f64 + 0.5) * BIN, m));
        }
    }
    (esf, resid)
}

fn crossing(esf: &[(f64, f64)], level: f64) -> Option<f64> {
    for w in esf.windows(2) {
        let (a, b) = (w[0], w[1]);
        if (a.1 - level) * (b.1 - level) <= 0.0 && a.1 != b.1 {
            return Some(a.0 + (level - a.1) * (b.0 - a.0) / (b.1 - a.1));
        }
    }
    None
}

/// Analyses the dominant straight edge in `g` (the crop should contain one clean edge and nothing else).
pub fn edge_profile(g: &Gray) -> Option<Edge> {
    if g.w < 16 || g.h < 16 {
        return None;
    }
    // structure tensor of strong gradients -> initial normal direction
    let (mut jxx, mut jyy, mut jxy) = (0.0, 0.0, 0.0);
    let mut gmax = 0.0f64;
    let mut grads = Vec::new();
    for y in 1..g.h - 1 {
        for x in 1..g.w - 1 {
            let gx = f64::from(g.at(x + 1, y) - g.at(x - 1, y)) / 2.0;
            let gy = f64::from(g.at(x, y + 1) - g.at(x, y - 1)) / 2.0;
            gmax = gmax.max(gx.hypot(gy));
            grads.push((gx, gy));
        }
    }
    if gmax < 1e-4 {
        return None;
    }
    for &(gx, gy) in &grads {
        if gx.hypot(gy) > 0.2 * gmax {
            jxx += gx * gx;
            jyy += gy * gy;
            jxy += gx * gy;
        }
    }
    let theta0 = 0.5 * (2.0 * jxy).atan2(jxx - jyy);
    // refine: the angle with the least scatter around the binned ESF
    let margin = 2usize;
    let mut best = (f64::MAX, theta0);
    let span = 4.0f64.to_radians();
    let mut step = 0.5f64.to_radians();
    let mut centre = theta0;
    for round in 0..4 {
        let n = if round == 0 { (span / step) as i32 } else { 5 };
        for k in -n..=n {
            let th = centre + f64::from(k) * step;
            let (_, resid) = bin_esf(g, th, margin);
            if resid < best.0 {
                best = (resid, th);
            }
        }
        centre = best.1;
        step /= 4.0;
    }
    let theta = best.1;
    let (esf, _) = bin_esf(g, theta, margin);
    if esf.len() < 16 {
        return None;
    }
    let k = (esf.len() as f64 * 0.15).ceil() as usize;
    let a: f64 = esf[..k].iter().map(|p| p.1).sum::<f64>() / k as f64;
    let b: f64 = esf[esf.len() - k..].iter().map(|p| p.1).sum::<f64>() / k as f64;
    if (b - a).abs() < 0.02 {
        return None;
    }
    // light smoothing before locating crossings
    let sm: Vec<(f64, f64)> = (0..esf.len())
        .map(|i| {
            let lo = i.saturating_sub(1);
            let hi = (i + 1).min(esf.len() - 1);
            let v = (esf[lo].1 + esf[i].1 + esf[hi].1) / 3.0;
            (esf[i].0, v)
        })
        .collect();
    let l10 = a + 0.1 * (b - a);
    let l90 = a + 0.9 * (b - a);
    let d10 = crossing(&sm, l10)?;
    let d90 = crossing(&sm, l90)?;
    let rise = (d90 - d10).abs();
    // MTF from the derivative of the ESF (LSF) at its actual (possibly non-uniform) sample positions, Hann windowed
    // around the edge. Axis-aligned edges give 1 px spacing, slanted ones the full 0.25 px oversampling.
    let lsf: Vec<(f64, f64, f64)> = sm
        .windows(2)
        .map(|w| {
            let dx = w[1].0 - w[0].0;
            ((w[0].0 + w[1].0) / 2.0, (w[1].1 - w[0].1) / dx, dx)
        })
        .collect();
    let x0 = lsf.iter().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).map_or(0.0, |p| p.0);
    let half = (3.0 * rise).max(6.0);
    let mtf = |f: f64| -> f64 {
        let (mut re, mut im) = (0.0, 0.0);
        for &(x, v, dx) in &lsf {
            let u = (x - x0) / half;
            if u.abs() >= 1.0 {
                continue;
            }
            let w = 0.5 + 0.5 * (PI * u).cos();
            let ph = 2.0 * PI * f * (x - x0);
            re += v * w * dx * ph.cos();
            im -= v * w * dx * ph.sin();
        }
        re.hypot(im)
    };
    let dc = mtf(0.0);
    let mut mtf50 = None;
    if dc > 0.0 {
        let mut prev = (0.0, 1.0);
        let mut f = 0.005;
        while f < 2.0 {
            let m = mtf(f) / dc;
            if m < 0.5 {
                let t = (prev.1 - 0.5) / (prev.1 - m);
                mtf50 = Some(prev.0 + t * (f - prev.0));
                break;
            }
            prev = (f, m);
            f += 0.005;
        }
    }
    Some(Edge { normal_deg: theta.to_degrees(), rise_10_90: rise, mtf50, levels: (a, b), esf: sm })
}

/// Synthesises the source-resolution vertical-stripe ladder of `analysis/overnight/desk/make_chart.sh`:
/// `period` px per line pair, black bar `max(1, period / 2)` px wide on white.
pub fn synth_ladder(period: usize, w: usize, h: usize) -> Gray {
    let bar = (period / 2).max(1);
    let mut data = Vec::with_capacity(w * h);
    for _ in 0..h {
        for x in 0..w {
            data.push(if x % period < bar { 0.0 } else { 1.0 });
        }
    }
    Gray { w, h, data }
}

/// Result of [`ladder_transfer`].
#[derive(Debug, Clone, PartialEq)]
pub struct Ladder {
    /// Final-image pixels per source pixel along the ladder (local magnification; > 1 magnifies).
    pub k: f64,
    /// Modulation depth (fundamental amplitude / mean) in the final image and in the source ladder.
    pub depth_final: f64,
    pub depth_source: f64,
    /// `depth_final / depth_source`: end-to-end transfer at this line-pair period (1.0 = lossless).
    pub transfer: f64,
    pub peak: Peak,
}

/// Measures how well a line-pair ladder of `period` source pixels survives: finds the strongest sinusoid in `final_crop`
/// for magnifications `k_min..=k_max` (final px per source px) and compares its modulation depth with the source ladder's.
pub fn ladder_transfer(final_crop: &Gray, period: usize, k_min: f64, k_max: f64) -> Option<Ladder> {
    if period < 2 || !(k_max > k_min && k_min > 0.0) {
        return None;
    }
    let p = period as f64;
    let peak = dominant_frequency(final_crop, 1.0 / (p * k_max), 1.0 / (p * k_min))?;
    let src = synth_ladder(period, period * 40, period * 40);
    let (a, m) = amplitude_at(&src, 1.0 / p, 0.0);
    let depth_source = if m > 0.0 { a / m } else { 0.0 };
    let depth_final = peak.depth();
    Some(Ladder {
        k: 1.0 / (p * peak.freq()),
        depth_final,
        depth_source,
        transfer: if depth_source > 0.0 { depth_final / depth_source } else { 0.0 },
        peak,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sinusoid(w: usize, h: usize, fx: f64, fy: f64, a: f64, m: f64) -> Gray {
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                data.push((m + a * (2.0 * PI * (fx * x as f64 + fy * y as f64) + 0.7).cos()) as f32);
            }
        }
        Gray { w, h, data }
    }

    fn blurred_edge(w: usize, h: usize, normal_deg: f64, sigma: f64, offset: f64) -> Gray {
        let th = normal_deg.to_radians();
        let mut data = Vec::new();
        for y in 0..h {
            for x in 0..w {
                let d = (x as f64 - w as f64 / 2.0) * th.cos() + (y as f64 - h as f64 / 2.0) * th.sin() - offset;
                data.push((0.1 + 0.8 * norm_cdf(d / sigma)) as f32);
            }
        }
        Gray { w, h, data }
    }

    #[test]
    fn norm_cdf_values() {
        assert!((norm_cdf(0.0) - 0.5).abs() < 1e-7);
        assert!((norm_cdf(1.0) - 0.841_344_7).abs() < 1e-6);
        assert!((norm_cdf(-1.2816) - 0.1).abs() < 1e-4);
    }

    #[test]
    fn amplitude_of_sinusoid() {
        let g = sinusoid(96, 96, 0.2, 0.1, 0.3, 0.5);
        let (a, m) = amplitude_at(&g, 0.2, 0.1);
        assert!((a - 0.3).abs() < 0.01, "amp {a}");
        assert!((m - 0.5).abs() < 0.01, "mean {m}");
        // far from the frequency the response is small
        let (off, _) = amplitude_at(&g, 0.4, 0.3);
        assert!(off < 0.02);
    }

    #[test]
    fn dominant_frequency_any_orientation() {
        for (fx, fy) in [(0.25, 0.0), (0.0, 0.3), (0.18, 0.12), (-0.15, 0.22)] {
            let g = sinusoid(80, 80, fx, fy, 0.25, 0.5);
            let r = (fx * fx + fy * fy).sqrt();
            let p = dominant_frequency(&g, r * 0.7, r * 1.3).unwrap();
            // the DFT cannot tell +f from -f; compare up to sign
            let same = (p.fx - fx).abs() < 0.004 && (p.fy - fy).abs() < 0.004;
            let flip = (p.fx + fx).abs() < 0.004 && (p.fy + fy).abs() < 0.004;
            assert!(same || flip, "({fx},{fy}) -> ({},{})", p.fx, p.fy);
            assert!((p.amp - 0.25).abs() < 0.01);
            assert!((p.depth() - 0.5).abs() < 0.02);
        }
        assert!(dominant_frequency(&sinusoid(4, 4, 0.1, 0.1, 1.0, 0.0), 0.05, 0.2).is_none());
        assert!(dominant_frequency(&sinusoid(40, 40, 0.1, 0.1, 1.0, 0.0), 0.3, 0.2).is_none());
    }

    #[test]
    fn edge_rise_matches_gaussian_theory() {
        // Gaussian blur sigma: 10-90 % rise = 2.563 sigma, MTF50 = 0.1874 / sigma (cycles/px)
        for (angle, sigma) in [(5.0, 1.0), (37.0, 1.5), (-20.0, 0.8), (88.0, 1.2), (0.0, 2.0)] {
            let g = blurred_edge(64, 64, angle, sigma, 0.3);
            let e = edge_profile(&g).expect("edge");
            let want = 2.563 * sigma;
            assert!((e.rise_10_90 - want).abs() < 0.12 * want + 0.15, "angle {angle} sigma {sigma}: rise {} vs {want}", e.rise_10_90);
            let m = e.mtf50.expect("mtf50");
            let want_m = 0.1874 / sigma;
            assert!((m - want_m).abs() < 0.2 * want_m, "angle {angle} sigma {sigma}: mtf50 {m} vs {want_m}");
            let mut diff = (e.normal_deg - angle).abs();
            if diff > 90.0 {
                diff = 180.0 - diff;
            }
            assert!(diff < 1.0, "angle {angle}: {}", e.normal_deg);
        }
    }

    #[test]
    fn sharper_edge_has_shorter_rise_and_higher_mtf50() {
        let soft = edge_profile(&blurred_edge(64, 64, 12.0, 2.0, 0.0)).unwrap();
        let sharp = edge_profile(&blurred_edge(64, 64, 12.0, 0.7, 0.0)).unwrap();
        assert!(sharp.rise_10_90 < soft.rise_10_90);
        assert!(sharp.mtf50.unwrap() > soft.mtf50.unwrap());
    }

    #[test]
    fn ladder_transfer_recovers_scale_and_loss() {
        // final image: the 4 px ladder magnified by 1.25 and blurred by a box of 1.5 px
        let k = 1.25;
        let src = synth_ladder(4, 400, 100);
        let (w, h) = (96usize, 96usize);
        let mut data = Vec::new();
        for y in 0..h {
            let _ = y;
            for x in 0..w {
                // average source over a 1.5/k source-px wide box centred at x/k
                let c = (x as f64 + 0.5) / k;
                let half = 1.5 / k / 2.0;
                let mut acc = 0.0;
                let n = 24;
                for i in 0..n {
                    let sx = c - half + 2.0 * half * (i as f64 + 0.5) / f64::from(n);
                    let xi = sx.floor().rem_euclid(400.0) as usize;
                    acc += f64::from(src.at(xi, 0));
                }
                data.push((acc / f64::from(n)) as f32);
            }
        }
        let fin = Gray { w, h, data };
        let r = ladder_transfer(&fin, 4, 0.9, 1.8).unwrap();
        assert!((r.k - k).abs() < 0.03, "k {}", r.k);
        assert!(r.transfer < 1.0 && r.transfer > 0.5, "transfer {}", r.transfer);
        // an unblurred, unscaled copy transfers fully
        let same = synth_ladder(4, 96, 96);
        let r1 = ladder_transfer(&same, 4, 0.9, 1.1).unwrap();
        assert!((r1.transfer - 1.0).abs() < 0.08, "{}", r1.transfer);
        assert!(ladder_transfer(&same, 1, 0.9, 1.1).is_none());
    }

    #[test]
    fn synth_ladder_bar_width() {
        let g = synth_ladder(3, 6, 1);
        assert_eq!(g.data, vec![0.0, 1.0, 1.0, 0.0, 1.0, 1.0]);
        let g2 = synth_ladder(2, 4, 1);
        assert_eq!(g2.data, vec![0.0, 1.0, 0.0, 1.0]);
    }

    #[test]
    fn flat_or_tiny_images_have_no_edge() {
        let flat = Gray { w: 32, h: 32, data: vec![0.5; 32 * 32] };
        assert!(edge_profile(&flat).is_none());
        assert!(edge_profile(&Gray { w: 8, h: 8, data: vec![0.0; 64] }).is_none());
    }
}
