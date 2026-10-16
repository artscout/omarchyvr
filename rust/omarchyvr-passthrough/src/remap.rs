//! Per-frame work: bilinear lookups through a [`Lut`], then an optional tone curve and expansion to RGBA.

use crate::lut::{self, Lut, Tap, NONE};
use pimax_cam::frame::{TILE_H, TILE_W};

/// Four raw tiles (640x480 each, row pitch 640). Missing cameras may be empty slices; taps pointing at them
/// produce black.
pub type Tiles<'a> = [&'a [u8]; 4];

#[inline]
fn bilinear(tile: &[u8], u: u16, v: u16) -> u32 {
    let (x0, y0) = ((u >> 6) as usize, (v >> 6) as usize);
    let (fx, fy) = ((u & 63) as u32, (v & 63) as u32);
    // the map keeps samples >= 1 px from the border; clamp anyway so a bad map cannot read out of bounds
    let x0 = x0.min(TILE_W - 2);
    let y0 = y0.min(TILE_H - 2);
    let i = y0 * TILE_W + x0;
    if i + TILE_W + 1 >= tile.len() {
        return 0;
    }
    let p00 = tile[i] as u32;
    let p01 = tile[i + 1] as u32;
    let p10 = tile[i + TILE_W] as u32;
    let p11 = tile[i + TILE_W + 1] as u32;
    let top = p00 * (64 - fx) + p01 * fx;
    let bot = p10 * (64 - fx) + p11 * fx;
    (top * (64 - fy) + bot * fy + 2048) >> 12
}

/// Value of one eye pixel.
#[inline]
pub fn sample(t: &Tap, tiles: &Tiles) -> u8 {
    if t.tile_a == NONE {
        return 0;
    }
    let a = bilinear(tiles[t.tile_a as usize & 3], t.ua, t.va);
    let w = t.w as u32;
    let b = if t.tile_b == NONE {
        0
    } else {
        bilinear(tiles[t.tile_b as usize & 3], t.ub, t.vb)
    };
    ((a * w + b * (255 - w) + 127) / 255) as u8
}

/// Grey eye image (`lut.width * lut.height` bytes).
pub fn remap_grey(lut: &Lut, tiles: &Tiles, out: &mut [u8], threads: usize) {
    assert_eq!(out.len(), lut.taps.len());
    let w = lut.width;
    lut::par_rows(out, w, lut::threads(threads), |y0, rows| {
        let taps = &lut.taps[y0 * w..y0 * w + rows.len()];
        for (o, t) in rows.iter_mut().zip(taps) {
            *o = sample(t, tiles);
        }
    });
}

/// Tone curve `out = 255 · clamp(gain · (in/255)^gamma)`.
pub fn tone_table(gamma: f64, gain: f64) -> [u8; 256] {
    let mut t = [0u8; 256];
    for (i, e) in t.iter_mut().enumerate() {
        let x = (i as f64 / 255.0).powf(gamma.max(0.05)) * gain.max(0.0);
        *e = (x.clamp(0.0, 1.0) * 255.0).round() as u8;
    }
    t
}

/// Expand a grey image to 4-byte pixels (`R=G=B=tone[g]`, `A=255`; the same bytes for RGBA and BGRA) into `dst`.
/// The swapchain is sRGB, and so are the camera's 8-bit values as far as we know: the bytes go through unchanged.
pub fn expand_rgba(grey: &[u8], tone: &[u8; 256], dst: &mut [u8], threads: usize) {
    assert!(dst.len() >= grey.len() * 4);
    let dst = &mut dst[..grey.len() * 4];
    const ROW: usize = 4096;
    lut::par_rows(dst, ROW * 4, lut::threads(threads).min(4), |r0, chunk| {
        let g = &grey[r0 * ROW..r0 * ROW + chunk.len() / 4];
        for (i, &v) in g.iter().enumerate() {
            let t = tone[v as usize];
            chunk[i * 4..i * 4 + 4].copy_from_slice(&[t, t, t, 255]);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lut::{Lut, LutParams, FP};
    use crate::rig::tests::rig;
    use crate::view::{EyeView, Fov};

    fn tap(tile: u8, u: f64, v: f64) -> Tap {
        Tap {
            tile_a: tile,
            tile_b: NONE,
            w: 255,
            pad: 0,
            ua: (u * FP) as u16,
            va: (v * FP) as u16,
            ub: 0,
            vb: 0,
        }
    }

    #[test]
    fn bilinear_interpolates_and_blends() {
        // horizontal ramp: value = x / 4
        let ramp: Vec<u8> = (0..TILE_W * TILE_H)
            .map(|i| ((i % TILE_W) / 4) as u8)
            .collect();
        let flat = vec![200u8; TILE_W * TILE_H];
        let tiles: Tiles = [&ramp, &flat, &[], &[]];
        assert_eq!(sample(&tap(0, 400.0, 100.0), &tiles), 100);
        assert_eq!(sample(&tap(0, 402.0, 100.5), &tiles), 100); // 100.5 rounds to 100 or 101
        assert_eq!(sample(&tap(1, 3.25, 7.75), &tiles), 200);
        let mut t = tap(0, 400.0, 100.0);
        t.tile_b = 1;
        t.ub = (10.0 * FP) as u16;
        t.vb = (10.0 * FP) as u16;
        t.w = 128;
        // 100 * 128/255 + 200 * 127/255 = 149.8
        assert_eq!(sample(&t, &tiles), 150);
        // no B: fade to black
        t.tile_b = NONE;
        assert_eq!(sample(&t, &tiles), 50);
        // missing tile data: black, no panic
        assert_eq!(sample(&tap(2, 10.0, 10.0), &tiles), 0);
        let none = Tap {
            tile_a: NONE,
            ..Tap::default()
        };
        assert_eq!(sample(&none, &tiles), 0);
    }

    #[test]
    fn tone_and_expand() {
        let id = tone_table(1.0, 1.0);
        assert!(id.iter().enumerate().all(|(i, &v)| v as usize == i));
        let dark = tone_table(2.0, 1.0);
        assert_eq!(dark[255], 255);
        assert!(dark[128] < 70);
        let grey: Vec<u8> = (0..10_000).map(|i| (i % 256) as u8).collect();
        let mut dst = vec![0u8; grey.len() * 4];
        expand_rgba(&grey, &id, &mut dst, 3);
        for (i, &g) in grey.iter().enumerate() {
            assert_eq!(&dst[i * 4..i * 4 + 4], &[g, g, g, 255]);
        }
    }

    #[test]
    fn remap_of_a_constant_scene_is_constant_where_covered() {
        let r = rig();
        let ev = EyeView::default_pair(90, 96, 0.063, Fov::dream_air_default());
        let lut = Lut::build(&r, &ev[0], 0, &LutParams::default());
        let t = vec![77u8; TILE_W * TILE_H];
        let tiles: Tiles = [&t, &t, &t, &t];
        let mut out = vec![0u8; 90 * 96];
        remap_grey(&lut, &tiles, &mut out, 4);
        let full = lut
            .taps
            .iter()
            .zip(&out)
            .filter(|(t, _)| t.tile_a != NONE && (t.w == 255 || t.tile_b != NONE))
            .count();
        assert!(full > out.len() * 9 / 10);
        for (t, &o) in lut.taps.iter().zip(&out) {
            if t.tile_a != NONE && (t.w == 255 || t.tile_b != NONE) {
                assert!((76..=78).contains(&o), "{o}");
            }
        }
    }
}
