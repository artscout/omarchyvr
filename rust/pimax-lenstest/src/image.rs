//! A small RGB8 image in panel-code space, rendered row-parallel from a pure per-pixel function, plus drawing
//! helpers for overlays and the conversion into the byte layout of a swapchain format.

use crate::color::{Encoding, Rgb};
use std::thread;

#[derive(Clone, Debug, PartialEq)]
pub struct Image {
    pub w: u32,
    pub h: u32,
    /// `w * h * 3` bytes, rows top to bottom, R G B.
    pub rgb: Vec<u8>,
}

fn quantize(c: Rgb) -> [u8; 3] {
    [
        crate::color::to_byte(c[0]),
        crate::color::to_byte(c[1]),
        crate::color::to_byte(c[2]),
    ]
}

impl Image {
    pub fn new(w: u32, h: u32, fill: Rgb) -> Image {
        let px = quantize(fill);
        let mut rgb = Vec::with_capacity(w as usize * h as usize * 3);
        for _ in 0..w as usize * h as usize {
            rgb.extend_from_slice(&px);
        }
        Image { w, h, rgb }
    }

    /// Evaluate `f(x, y)` for every pixel, in parallel over rows (`threads` = 0 or 1: on the calling thread).
    pub fn render(w: u32, h: u32, threads: usize, f: &(dyn Fn(u32, u32) -> Rgb + Sync)) -> Image {
        let stride = w as usize * 3;
        let mut rgb = vec![0u8; stride * h as usize];
        let fill = |first_row: usize, chunk: &mut [u8]| {
            for (ry, row) in chunk.chunks_mut(stride).enumerate() {
                let y = (first_row + ry) as u32;
                for (x, px) in row.chunks_exact_mut(3).enumerate() {
                    px.copy_from_slice(&quantize(f(x as u32, y)));
                }
            }
        };
        if threads <= 1 || h < 2 {
            fill(0, &mut rgb);
        } else {
            let rows_per = (h as usize).div_ceil(threads);
            thread::scope(|s| {
                for (i, chunk) in rgb.chunks_mut(rows_per * stride).enumerate() {
                    let fill = &fill;
                    s.spawn(move || fill(i * rows_per, chunk));
                }
            });
        }
        Image { w, h, rgb }
    }

    pub fn get(&self, x: u32, y: u32) -> [u8; 3] {
        let i = (y as usize * self.w as usize + x as usize) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }

    /// Set a pixel; out of range coordinates are ignored.
    pub fn set(&mut self, x: i32, y: i32, c: [u8; 3]) {
        if x >= 0 && y >= 0 && (x as u32) < self.w && (y as u32) < self.h {
            let i = (y as usize * self.w as usize + x as usize) * 3;
            self.rgb[i..i + 3].copy_from_slice(&c);
        }
    }

    pub fn fill_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32, c: Rgb) {
        let q = quantize(c);
        for y in y0.max(0)..y1.min(self.h as i32) {
            for x in x0.max(0)..x1.min(self.w as i32) {
                self.set(x, y, q);
            }
        }
    }

    /// Draw `text` with the 5x7 font, top-left at (x, y), pixel `scale`.
    pub fn text(&mut self, text: &str, x: i32, y: i32, scale: u32, c: Rgb) {
        let q = quantize(c);
        let (w, h) = (self.w as i32, self.h as i32);
        let rgb = &mut self.rgb;
        pimax_lens::font::draw_text(text, x, y, scale, |px, py| {
            if px >= 0 && py >= 0 && px < w && py < h {
                let i = (py as usize * w as usize + px as usize) * 3;
                rgb[i..i + 3].copy_from_slice(&q);
            }
        });
    }

    /// Text on a filled backdrop with `pad` pixels of margin; returns the rectangle (x0, y0, x1, y1) it covers.
    pub fn label(
        &mut self,
        text: &str,
        x: i32,
        y: i32,
        scale: u32,
        fg: Rgb,
        bg: Rgb,
    ) -> (i32, i32, i32, i32) {
        let (tw, th) = pimax_lens::font::text_size(text, scale);
        let pad = scale as i32;
        let rect = (x - pad, y - pad, x + tw as i32 + pad, y + th as i32 + pad);
        self.fill_rect(rect.0, rect.1, rect.2, rect.3, bg);
        self.text(text, x, y, scale, fg);
        rect
    }

    /// Fill `out` (`w * h * 4` bytes) with the swapchain bytes: panel codes through `enc`, optionally BGRA order,
    /// alpha 255.
    pub fn to_upload(&self, enc: Encoding, bgra: bool, threads: usize, out: &mut [u8]) {
        assert_eq!(out.len(), self.w as usize * self.h as usize * 4);
        let table = enc.table();
        let convert = |src: &[u8], dst: &mut [u8]| {
            for (s, d) in src.chunks_exact(3).zip(dst.chunks_exact_mut(4)) {
                let (r, g, b) = (
                    table[s[0] as usize],
                    table[s[1] as usize],
                    table[s[2] as usize],
                );
                if bgra {
                    d.copy_from_slice(&[b, g, r, 255]);
                } else {
                    d.copy_from_slice(&[r, g, b, 255]);
                }
            }
        };
        let px = self.w as usize * self.h as usize;
        if threads <= 1 || px < 4096 {
            convert(&self.rgb, out);
            return;
        }
        let per = px.div_ceil(threads);
        thread::scope(|s| {
            for (src, dst) in self.rgb.chunks(per * 3).zip(out.chunks_mut(per * 4)) {
                let convert = &convert;
                s.spawn(move || convert(src, dst));
            }
        });
    }

    /// Count pixels for which `pred` is true (test helper).
    pub fn count(&self, pred: impl Fn([u8; 3]) -> bool) -> usize {
        self.rgb
            .chunks_exact(3)
            .filter(|p| pred([p[0], p[1], p[2]]))
            .count()
    }
}

/// Number of worker threads to use for rendering.
pub fn default_threads() -> usize {
    thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::{gray, BLACK, RED, WHITE};

    #[test]
    fn render_parallel_equals_serial() {
        let f = |x: u32, y: u32| {
            [
                (x % 7) as f32 / 7.0,
                (y % 5) as f32 / 5.0,
                ((x + y) % 3) as f32 / 3.0,
            ]
        };
        let a = Image::render(101, 67, 1, &f);
        for threads in [2, 3, 8, 64] {
            assert_eq!(Image::render(101, 67, threads, &f), a, "{threads} threads");
        }
        assert_eq!(a.rgb.len(), 101 * 67 * 3);
        assert_eq!(
            a.get(3, 4),
            [to(3.0 / 7.0), to(4.0 / 5.0), to(7.0 % 3.0 / 3.0)]
        );
    }

    fn to(c: f32) -> u8 {
        crate::color::to_byte(c)
    }

    #[test]
    fn new_set_and_clipping() {
        let mut im = Image::new(4, 3, gray(0.5));
        assert_eq!(im.get(3, 2), [128, 128, 128]);
        im.set(-1, 0, [1, 2, 3]);
        im.set(4, 0, [1, 2, 3]);
        im.set(0, 3, [1, 2, 3]);
        im.set(1, 1, [9, 8, 7]);
        assert_eq!(im.get(1, 1), [9, 8, 7]);
        assert_eq!(im.count(|p| p == [9, 8, 7]), 1);
        im.fill_rect(-5, -5, 2, 2, RED);
        assert_eq!(im.count(|p| p == [255, 0, 0]), 4);
    }

    #[test]
    fn text_and_label_draw_inside_the_image_only() {
        let mut im = Image::new(60, 20, BLACK);
        im.text("AB", 2, 2, 1, WHITE);
        assert!(im.count(|p| p == [255, 255, 255]) > 10);
        let before = im.clone();
        im.text("ZZZZZZZZZZ", 50, 15, 3, WHITE); // mostly outside: must not panic or wrap
        assert_eq!(im.get(0, 19), before.get(0, 19));
        let mut im2 = Image::new(100, 30, BLACK);
        let r = im2.label("HI", 10, 10, 2, WHITE, RED);
        assert_eq!(r, (8, 8, 34, 26));
        assert_eq!(im2.get(8, 8), [255, 0, 0]);
    }

    #[test]
    fn upload_layouts() {
        let mut im = Image::new(2, 1, BLACK);
        im.set(0, 0, [10, 100, 200]);
        im.set(1, 0, [255, 0, 128]);
        let mut out = [0u8; 8];
        im.to_upload(Encoding::Srgb, false, 1, &mut out);
        assert_eq!(out, [10, 100, 200, 255, 255, 0, 128, 255]);
        im.to_upload(Encoding::Srgb, true, 1, &mut out);
        assert_eq!(out, [200, 100, 10, 255, 128, 0, 255, 255]);
        im.to_upload(Encoding::Linear, false, 1, &mut out);
        assert!(out[0] < 10 && out[1] < 100 && out[2] < 200 && out[3] == 255);
        // parallel path equals the serial one on a larger image
        let big = Image::render(200, 100, 1, &|x, y| {
            gray(((x * 3 + y) % 256) as f32 / 255.0)
        });
        let (mut a, mut b) = (vec![0u8; 200 * 100 * 4], vec![0u8; 200 * 100 * 4]);
        big.to_upload(Encoding::Linear, true, 1, &mut a);
        big.to_upload(Encoding::Linear, true, 8, &mut b);
        assert_eq!(a, b);
    }
}
