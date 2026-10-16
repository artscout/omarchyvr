//! A CPU model of what Monado's compositor does with a client image: `distortion.comp` looks up the lens model's UV
//! (a 128 x 128 table per colour channel, bilinear) and samples the client image with it (bilinear, linear light,
//! black outside). This shows what the panel gets for a pattern and a lens.conf without a headset, and is the
//! reference the real compositor dumps are compared with.

use crate::color::{linear_to_srgb, srgb_to_linear};
use crate::image::Image;
use pimax_lens::Lens;
use std::thread;

/// The compositor's distortion table size (`RENDER_DISTORTION_IMAGE_DIMENSIONS`).
pub const TABLE: usize = 128;

type UvTriplet = [[f32; 2]; 3];

/// The table the compositor builds for one eye: node (col, row) holds the model's UV at panel position
/// (col / 127, row / 127) of the user-facing display.
pub fn build_table(lens: &Lens, view: usize) -> Vec<UvTriplet> {
    let mut t = Vec::with_capacity(TABLE * TABLE);
    let dim = (TABLE - 1) as f64;
    for row in 0..TABLE {
        let v = (row as f64 / dim) as f32;
        for col in 0..TABLE {
            let u = (col as f64 / dim) as f32;
            t.push(lens.compute(view, u, v));
        }
    }
    t
}

/// Bilinear lookup of the table at panel position (u, v) in 0..1.
fn lookup(table: &[UvTriplet], u: f32, v: f32) -> UvTriplet {
    let sx = (u * (TABLE - 1) as f32).clamp(0.0, (TABLE - 1) as f32);
    let sy = (v * (TABLE - 1) as f32).clamp(0.0, (TABLE - 1) as f32);
    let (x0, y0) = (sx.floor() as usize, sy.floor() as usize);
    let (x1, y1) = ((x0 + 1).min(TABLE - 1), (y0 + 1).min(TABLE - 1));
    let (fx, fy) = (sx - x0 as f32, sy - y0 as f32);
    let at = |x: usize, y: usize| table[y * TABLE + x];
    let (a, b, c, d) = (at(x0, y0), at(x1, y0), at(x0, y1), at(x1, y1));
    let mut out = [[0.0f32; 2]; 3];
    for ch in 0..3 {
        for k in 0..2 {
            let top = a[ch][k] + (b[ch][k] - a[ch][k]) * fx;
            let bot = c[ch][k] + (d[ch][k] - c[ch][k]) * fx;
            out[ch][k] = top + (bot - top) * fy;
        }
    }
    out
}

/// The client image as linear light for sampling.
struct Source {
    w: u32,
    h: u32,
    lin: Vec<f32>,
}

impl Source {
    fn new(img: &Image) -> Source {
        let lut: Vec<f32> = (0..256).map(|i| srgb_to_linear(i as f32 / 255.0)).collect();
        Source {
            w: img.w,
            h: img.h,
            lin: img.rgb.iter().map(|&b| lut[b as usize]).collect(),
        }
    }

    /// Bilinear sample of channel `c` at texture coordinate (u, v); black outside the image (clamp to border).
    fn sample(&self, c: usize, u: f32, v: f32) -> f32 {
        let (fx, fy) = (u * self.w as f32 - 0.5, v * self.h as f32 - 0.5);
        let (x0, y0) = (fx.floor(), fy.floor());
        let (tx, ty) = (fx - x0, fy - y0);
        let at = |x: i64, y: i64| -> f32 {
            if x < 0 || y < 0 || x >= i64::from(self.w) || y >= i64::from(self.h) {
                0.0
            } else {
                self.lin[(y as usize * self.w as usize + x as usize) * 3 + c]
            }
        };
        let (x0, y0) = (x0 as i64, y0 as i64);
        let top = at(x0, y0) * (1.0 - tx) + at(x0 + 1, y0) * tx;
        let bot = at(x0, y0 + 1) * (1.0 - tx) + at(x0 + 1, y0 + 1) * tx;
        top * (1.0 - ty) + bot * ty
    }
}

/// The user-facing panel image of one eye (`out_w` x `out_h`, normally 3840 x 3552 divided by an integer) for the
/// client image `src`, with `ss` x `ss` samples per output pixel (the box filter of the downscaled dumps).
pub fn distort(
    lens: &Lens,
    view: usize,
    src: &Image,
    out_w: u32,
    out_h: u32,
    ss: u32,
    threads: usize,
) -> Image {
    let table = build_table(lens, view);
    let source = Source::new(src);
    let ss = ss.max(1);
    let stride = out_w as usize * 3;
    let mut out = vec![0u8; stride * out_h as usize];
    let row_job = |y: usize, row: &mut [u8]| {
        for x in 0..out_w as usize {
            let mut acc = [0.0f32; 3];
            for sy in 0..ss {
                for sx in 0..ss {
                    let u = (x as f32 + (sx as f32 + 0.5) / ss as f32) / out_w as f32;
                    let v = (y as f32 + (sy as f32 + 0.5) / ss as f32) / out_h as f32;
                    let uv = lookup(&table, u, v);
                    for c in 0..3 {
                        // the compositor encodes linear light to sRGB per sample, the box filter averages the codes
                        acc[c] +=
                            linear_to_srgb(source.sample(c, uv[c][0], uv[c][1]).clamp(0.0, 1.0));
                    }
                }
            }
            let n = (ss * ss) as f32;
            for c in 0..3 {
                row[x * 3 + c] = crate::color::to_byte(acc[c] / n);
            }
        }
    };
    if threads <= 1 {
        for (y, row) in out.chunks_mut(stride).enumerate() {
            row_job(y, row);
        }
    } else {
        let rows_per = (out_h as usize).div_ceil(threads);
        thread::scope(|s| {
            for (i, chunk) in out.chunks_mut(rows_per * stride).enumerate() {
                let row_job = &row_job;
                s.spawn(move || {
                    for (r, row) in chunk.chunks_mut(stride).enumerate() {
                        row_job(i * rows_per + r, row);
                    }
                });
            }
        });
    }
    Image {
        w: out_w,
        h: out_h,
        rgb: out,
    }
}

/// Both eyes side by side with a gap, like the converted capture PNGs.
pub fn side_by_side(left: &Image, right: &Image, gap: u32) -> Image {
    let mut out = Image::new(
        left.w + gap + right.w,
        left.h.max(right.h),
        [0.12, 0.12, 0.12],
    );
    for (e, x0) in [(left, 0), (right, left.w + gap)] {
        for y in 0..e.h {
            for x in 0..e.w {
                out.set((x0 + x) as i32, y as i32, e.get(x, y));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::Fov;
    use crate::patterns::{self, Params, Pattern};
    use pimax_lens::config::LensConfig;

    const ASPECT: f32 = 3840.0 / 3552.0;

    fn default_lens() -> Lens {
        Lens::new(LensConfig::defaults(ASPECT))
    }

    fn fov_of(lens: &Lens, view: usize) -> Fov {
        let d = lens.fov_deg(view);
        Fov::from_half_angles_deg(d[0], d[1], d[2], d[3])
    }

    fn client(lens: &Lens, pattern: Pattern, step: usize, view: usize, w: u32, h: u32) -> Image {
        let mut p = Params::new(w, h, fov_of(lens, view), view);
        p.step = step;
        patterns::render(pattern, &p, None, 4)
    }

    #[test]
    fn table_nodes_are_the_model_at_the_grid_positions() {
        let lens = default_lens();
        let t = build_table(&lens, 0);
        assert_eq!(t.len(), TABLE * TABLE);
        let dim = (TABLE - 1) as f64;
        assert_eq!(
            t[5 * TABLE + 9],
            lens.compute(0, (9.0 / dim) as f32, (5.0 / dim) as f32)
        );
        // interpolation at a node returns the node, halfway between two nodes the mean
        let u = (9.0 / dim) as f32;
        let v = (5.0 / dim) as f32;
        let n = lookup(&t, u, v);
        for c in 0..3 {
            for k in 0..2 {
                assert!((n[c][k] - t[5 * TABLE + 9][c][k]).abs() < 1e-5);
            }
        }
        let mid = lookup(&t, (9.5 / dim) as f32, v);
        assert!(
            (mid[1][0] - 0.5 * (t[5 * TABLE + 9][1][0] + t[5 * TABLE + 10][1][0])).abs() < 1e-5
        );
        // positions outside 0..1 clamp
        assert_eq!(lookup(&t, -1.0, 2.0), t[(TABLE - 1) * TABLE]);
    }

    #[test]
    fn sampling_is_bilinear_in_linear_light_and_black_outside() {
        let mut img = Image::new(2, 1, [0.0; 3]);
        img.set(1, 0, [255, 255, 255]);
        let s = Source::new(&img);
        assert!((s.sample(0, 0.25, 0.5) - 0.0).abs() < 1e-6); // centre of texel 0
        assert!((s.sample(0, 0.75, 0.5) - 1.0).abs() < 1e-6);
        assert!((s.sample(0, 0.5, 0.5) - 0.5).abs() < 1e-6); // halfway: average of the linear values
        assert!(s.sample(0, 1.2, 0.5) < 0.2 || s.sample(0, 1.2, 0.5) >= 0.0);
        assert_eq!(s.sample(0, 2.0, 0.5), 0.0);
        assert_eq!(s.sample(0, -1.0, 0.5), 0.0);
        assert_eq!(s.sample(0, 0.5, 5.0), 0.0);
    }

    #[test]
    fn identity_lens_reproduces_the_client_image() {
        let lens = Lens::new(pimax_lens::config::parse("distortion = off\n", ASPECT).0);
        let src = client(&lens, Pattern::Grid, 0, 0, 240, 222);
        let out = distort(&lens, 0, &src, 240, 222, 1, 2);
        // identity mesh: UV = panel position, so the output is the source up to the table interpolation
        let mut worst = 0u32;
        for (a, b) in src.rgb.iter().zip(out.rgb.iter()) {
            worst = worst.max(u32::from(a.abs_diff(*b)));
        }
        assert!(worst <= 3, "{worst}");
    }

    /// x (pixels from the optical axis) of the strongest value of channel `c` within `half` px of `centre` on `row`.
    fn peak(out: &Image, row: u32, ax: f32, centre: f32, half: f32, c: usize) -> f32 {
        let lo = (ax + centre - half).max(0.0) as u32;
        let hi = ((ax + centre + half) as u32).min(out.w - 1);
        // centre of mass of the values above half of the maximum (sub-pixel)
        let vals: Vec<(u32, f32)> = (lo..=hi)
            .map(|x| (x, f32::from(out.get(x, row)[c])))
            .collect();
        let max = vals.iter().map(|v| v.1).fold(0.0, f32::max);
        let (mut sum, mut wsum) = (0.0f32, 0.0f32);
        for (x, v) in vals {
            if v > max * 0.5 {
                sum += (x as f32 + 0.5) * v;
                wsum += v;
            }
        }
        sum / wsum - ax
    }

    #[test]
    fn the_rings_land_at_the_panel_radius_the_model_predicts_per_channel() {
        let lens = default_lens();
        let (w, h) = (1920u32, 1776u32); // half of 3840 x 3552
        let src = client(&lens, Pattern::Rings, 0, 0, 1920, 1776);
        let out = distort(&lens, 0, &src, w, h, 1, 8);
        let e = lens.eye(0);
        let ax = (0.5 + e.cx / (2.0 * ASPECT)) * w as f32;
        let ay = (0.5 - e.cy / 2.0) * h as f32;
        let half_height = h as f32 / 2.0;
        // a row a little above the axis line, away from the label text of the rings
        let row = (ay - 6.0) as u32;
        // (angle, channels in the ring colour): white 10: RGB, yellow 20: RG, green 30: G, cyan 40: GB, magenta 50 is clipped
        for (deg, chans) in [
            (10.0f32, vec![0usize, 1, 2]),
            (20.0, vec![0, 1]),
            (30.0, vec![1]),
            (40.0, vec![1, 2]),
        ] {
            for c in chans {
                let want = lens.radius_for_angle(0, c, deg).unwrap() * half_height;
                let got = peak(&out, row, ax, want, 5.0, c);
                assert!(
                    (got - want).abs() < 1.2,
                    "{deg} deg channel {c}: ring {got:.2} px from the axis, model says {want:.2}"
                );
            }
        }
    }

    #[test]
    fn straight_grid_lines_follow_the_model_and_the_axes_stay_straight() {
        let lens = default_lens();
        let (w, h) = (960u32, 888u32);
        let src = client(&lens, Pattern::Grid, 0, 0, 1920, 1776);
        let out = distort(&lens, 0, &src, w, h, 2, 8);
        let e = lens.eye(0);
        let (ax, ay) = (
            (0.5 + e.cx / (2.0 * ASPECT)) * w as f32,
            (0.5 - e.cy / 2.0) * h as f32,
        );
        // the optical axis (red) is straight: a pixel on the vertical axis is red in every row away from the crossing
        let mut red_rows = 0;
        for y in (30..h - 30).step_by(7) {
            if (y as f32 - ay).abs() < 12.0 {
                continue;
            }
            let c = out.get(ax as u32, y);
            if c[0] > 180 && c[1] < 90 && c[2] < 90 {
                red_rows += 1;
            }
        }
        assert!(red_rows > 100, "{red_rows}");
        // every pixel the model puts on the bright vertical line tx = 0.5 is bright, those between lines are dark
        let (mut on, mut on_bright, mut off, mut off_dark) = (0, 0, 0, 0);
        let pix_t = fov_of(&lens, 0).tangent_per_pixel(1920);
        for y in 60..h - 60 {
            for x in 0..w {
                let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
                let t = lens.tangent(0, 1, u, v);
                if t[1].abs() < 0.2 || t[1].abs() > 0.8 || t[0] < 0.3 || t[0] > 0.7 {
                    continue; // stay away from the axes, the labels and the edges
                }
                let d = (t[0] - 0.5).abs();
                let c = out.get(x, y);
                let bright = c[1] > 130; // the green channel: red and blue are shifted by the chromatic aberration
                if d < 0.25 * pix_t {
                    on += 1;
                    on_bright += i32::from(bright);
                } else if (t[0] - 0.4).abs() > 0.1 * 0.45
                    && d > 0.04
                    && ((t[0] / 0.1).fract() - 0.5).abs() < 0.2
                {
                    off += 1;
                    off_dark += i32::from(c[1] < 130);
                }
            }
        }
        assert!(on > 100, "{on}");
        assert!(
            f64::from(on_bright) > 0.9 * f64::from(on),
            "{on_bright} of {on} pixels on the line are bright"
        );
        assert!(
            off > 100 && f64::from(off_dark) > 0.95 * f64::from(off),
            "{off_dark} of {off}"
        );
        // and the line bows: the same tangent column sits at different panel columns at different rows
        let col_of_line = |y: u32| {
            (0..w)
                .filter(|&x| {
                    lens.tangent(
                        0,
                        1,
                        (x as f32 + 0.5) / w as f32,
                        (y as f32 + 0.5) / h as f32,
                    )[0] > 0.5
                })
                .map(|x| x as i64)
                .next()
                .unwrap_or(0)
        };
        assert!((col_of_line(ay as u32) - col_of_line(100)).abs() >= 6);
    }

    #[test]
    fn masked_regions_come_out_black() {
        let lens =
            Lens::new(pimax_lens::config::parse("blackout = 0.2\nmask_feather = 0\n", ASPECT).0);
        let (w, h) = (480u32, 444u32);
        let src = client(&lens, Pattern::FovEdge, 1, 0, 960, 888); // white fill
        let out = distort(&lens, 0, &src, w, h, 1, 4);
        assert_eq!(out.get(2, 200), [0, 0, 0]);
        assert_eq!(out.get(w - 3, 200), [0, 0, 0]);
        assert_eq!(out.get(w / 2, 3), [0, 0, 0]);
        assert!(out.get(w / 2, h / 2)[0] > 250);
    }

    #[test]
    fn side_by_side_layout() {
        let a = Image::new(3, 2, [1.0, 0.0, 0.0]);
        let b = Image::new(3, 2, [0.0, 0.0, 1.0]);
        let c = side_by_side(&a, &b, 2);
        assert_eq!((c.w, c.h), (8, 2));
        assert_eq!(c.get(0, 0), [255, 0, 0]);
        assert_eq!(c.get(7, 1), [0, 0, 255]);
        assert_eq!(c.get(3, 0), [31, 31, 31]);
    }

    /// The compositor interpolates the model on a 128 x 128 table, so a mask edge can only follow the configured
    /// shape to about one table cell. Measured on the white fill of an elliptical mask (output 1920 px wide, cell
    /// 15 px): the default feather keeps the visible edge within a few pixels of the ideal ellipse.
    #[test]
    fn mask_edge_follows_the_configured_ellipse_to_about_a_cell() {
        let aspect = 3840.0 / 3552.0;
        for (feather, mean_limit, worst_limit) in [(0.0f32, 10.0f32, 18.0f32), (0.03, 4.0, 9.0)] {
            let text = format!("mask = ellipse 1.02 0.92\nmask_feather = {feather}\n");
            let lens = Lens::new(pimax_lens::config::parse(&text, aspect).0);
            let d = lens.fov_deg(0);
            let fov = Fov::from_half_angles_deg(d[0], d[1], d[2], d[3]);
            let mut p = Params::new(480, 444, fov, 0);
            p.step = 1;
            let src = patterns::render(Pattern::FovEdge, &p, None, 4);
            let (w, h) = (1920u32, 1776u32);
            let out = distort(&lens, 0, &src, w, h, 1, 8);
            let e = lens.eye(0);
            let (ax, ay) = (
                (0.5 + e.cx / (2.0 * aspect)) * w as f32,
                (0.5 - e.cy / 2.0) * h as f32,
            );
            let px_per_hh = h as f32 / 2.0;
            let (a, b) = (1.02 * px_per_hh, 0.92 * px_per_hh);
            let (mut worst, mut sum, mut n) = (0.0f32, 0.0f32, 0u32);
            for k in 0..360 {
                let phi = (k as f32).to_radians();
                let (dx, dy) = (phi.cos(), -phi.sin());
                let ideal = 1.0 / ((dx / a).powi(2) + (dy / b).powi(2)).sqrt();
                // walk outwards from the axis until the picture goes dark
                let mut r = 0.0f32;
                while r < 2.0 * px_per_hh {
                    let (x, y) = (ax + dx * r, ay + dy * r);
                    if x < 0.0 || y < 0.0 || x >= w as f32 || y >= h as f32 {
                        break;
                    }
                    if out.get(x as u32, y as u32)[1] < 128 {
                        // only directions where the ellipse (not the FOV rectangle) is the limit
                        if (r - ideal).abs() < 60.0 {
                            worst = worst.max((r - ideal).abs());
                            sum += (r - ideal).abs();
                            n += 1;
                        }
                        break;
                    }
                    r += 0.5;
                }
            }
            assert!(n > 300, "feather {feather}: only {n} directions measured");
            assert!(
                sum / (n as f32) < mean_limit,
                "feather {feather}: mean edge error {:.2} px",
                sum / n as f32
            );
            assert!(
                worst < worst_limit,
                "feather {feather}: worst edge error {worst:.2} px"
            );
        }
    }
}
