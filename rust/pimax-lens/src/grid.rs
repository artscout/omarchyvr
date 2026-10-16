//! Debug rendering of the UV grid of a lens model, without a compositor or a headset.
//!
//! The picture is the user-facing panel of one eye (3840x3552 scaled down). It shows where the model puts
//! iso-lines of the client image UV (every 1/n of the rendered FOV, one colour per channel: red lines move
//! only the red channel, so a coloured fringe means the channels disagree), the rings at known angles off the
//! optical axis, the optical axis itself, the edge of the rendered FOV and the visibility mask. This is the
//! panel picture that the test pattern `pimax-lenstest` draws in tangent space should produce.

use crate::font;
use crate::model::Lens;

/// Half of the user-facing display height in pixels (3552 / 2); radii in the model are in these units.
pub const HALF_HEIGHT_PX: f32 = 1776.0;

/// Colours of the angle rings: 10, 20, 30, 40, 50 degrees (the same as the test pattern uses).
pub const RING_COLORS: [[u8; 3]; 5] = [
    [255, 255, 255],
    [255, 255, 0],
    [0, 255, 0],
    [0, 255, 255],
    [255, 0, 255],
];

#[derive(Clone, Debug)]
pub struct GridOptions {
    pub width: u32,
    pub height: u32,
    /// UV iso-lines per axis (10: one every 0.1 of the rendered FOV).
    pub divisions: u32,
    /// Ring angles in degrees, drawn with [`RING_COLORS`] in order.
    pub rings_deg: Vec<f32>,
}

impl GridOptions {
    /// Panel picture of `3840/div x 3552/div` pixels with a UV line every 0.1 and rings at 10..50 degrees.
    pub fn for_panel(div: u32) -> GridOptions {
        let div = div.max(1);
        GridOptions {
            width: 3840 / div,
            height: 3552 / div,
            divisions: 10,
            rings_deg: vec![10.0, 20.0, 30.0, 40.0, 50.0],
        }
    }
}

struct Field {
    w: usize,
    h: usize,
    v: Vec<f32>,
}

impl Field {
    fn new(w: usize, h: usize) -> Field {
        Field {
            w,
            h,
            v: vec![0.0; w * h],
        }
    }
    #[inline]
    fn at(&self, x: usize, y: usize) -> f32 {
        self.v[y * self.w + x]
    }
    /// Central-difference gradient magnitude per pixel.
    fn grad(&self, x: usize, y: usize) -> f32 {
        let x0 = x.saturating_sub(1);
        let x1 = (x + 1).min(self.w - 1);
        let y0 = y.saturating_sub(1);
        let y1 = (y + 1).min(self.h - 1);
        let gx = (self.at(x1, y) - self.at(x0, y)) / (x1 - x0).max(1) as f32;
        let gy = (self.at(x, y1) - self.at(x, y0)) / (y1 - y0).max(1) as f32;
        (gx * gx + gy * gy).sqrt()
    }
}

/// Coverage (0..1) of a line of half-width `halfw` pixels where `value` crosses a multiple of `step`.
fn iso_cover(value: f32, grad: f32, step: f32, halfw: f32) -> f32 {
    let q = value / step;
    let frac = (q - q.round()).abs() * step;
    let d = frac / grad.max(1e-9);
    (halfw + 0.5 - d).clamp(0.0, 1.0)
}

fn blend(px: &mut [u8], col: [u8; 3], a: f32) {
    for i in 0..3 {
        px[i] = (f32::from(px[i]) + (f32::from(col[i]) - f32::from(px[i])) * a)
            .round()
            .clamp(0.0, 255.0) as u8;
    }
}

fn add_channel(px: &mut [u8], c: usize, a: f32) {
    let v = f32::from(px[c]) + 255.0 * a;
    px[c] = v.round().clamp(0.0, 255.0) as u8;
}

/// Render the grid picture of `view` (RGB8, `opt.width * opt.height * 3` bytes).
pub fn render(lens: &Lens, view: usize, opt: &GridOptions) -> Vec<u8> {
    let (w, h) = (opt.width as usize, opt.height as usize);
    let n = opt.divisions.max(1) as f32;
    let aspect = lens.config().aspect;
    let mut uf: Vec<Field> = (0..3).map(|_| Field::new(w, h)).collect();
    let mut vf: Vec<Field> = (0..3).map(|_| Field::new(w, h)).collect();
    let (mut tx, mut ty) = (Field::new(w, h), Field::new(w, h));
    let mut ang = Field::new(w, h);
    let mut hide = vec![0.0f32; w * h];
    let mut sdist = Field::new(w, h);
    let has_mask = lens.eye(view).has_mask();
    for y in 0..h {
        for x in 0..w {
            let (u, v) = ((x as f32 + 0.5) / w as f32, (y as f32 + 0.5) / h as f32);
            let uv = lens.compute(view, u, v);
            let i = y * w + x;
            for c in 0..3 {
                uf[c].v[i] = uv[c][0];
                vf[c].v[i] = uv[c][1];
            }
            // tangent / angle from the unmasked green mapping (the mask pushes UV out of range)
            let t = lens.tangent(view, 1, u, v);
            tx.v[i] = t[0];
            ty.v[i] = t[1];
            ang.v[i] = t[0].hypot(t[1]).atan().to_degrees();
            if has_mask {
                hide[i] = lens.hide_amount(view, u, v);
                let s = lens.mask_distance(view, u, v);
                sdist.v[i] = if s.is_finite() { s } else { 1e3 };
            }
        }
    }

    let mut img = vec![0u8; w * h * 3];
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let px = &mut img[i * 3..i * 3 + 3];
            let g_in = (0.0..=1.0).contains(&uf[1].v[i]) && (0.0..=1.0).contains(&vf[1].v[i]);
            let base: [u8; 3] = if hide[i] >= 0.5 {
                [5, 5, 6]
            } else if g_in {
                [26, 26, 32]
            } else {
                [12, 12, 16]
            };
            px.copy_from_slice(&base);
            if hide[i] >= 0.5 {
                continue;
            }
            // UV iso-lines, one colour channel each
            for c in 0..3 {
                for (f, which) in [(&uf[c], 0), (&vf[c], 1)] {
                    let val = f.at(x, y);
                    if !(-0.0005..=1.0005).contains(&val) {
                        continue;
                    }
                    let g = f.grad(x, y);
                    let a = iso_cover(val, g, 1.0 / n, 0.5);
                    if a > 0.0 {
                        let edge = (val < 0.01 || val > 0.99) && which < 2;
                        add_channel(px, c, a * if edge { 1.0 } else { 0.6 });
                    }
                }
            }
            // rings at known angles
            for (ri, &deg) in opt.rings_deg.iter().enumerate() {
                let a = iso_cover(ang.at(x, y) - deg, ang.grad(x, y), 1e6, 1.0);
                if a > 0.0 {
                    blend(px, RING_COLORS[ri % RING_COLORS.len()], a);
                }
            }
            // optical axis
            let ax = iso_cover(tx.at(x, y), tx.grad(x, y), 1e6, 1.0).max(iso_cover(
                ty.at(x, y),
                ty.grad(x, y),
                1e6,
                1.0,
            ));
            if ax > 0.0 && ang.at(x, y) < 70.0 {
                blend(px, [255, 128, 0], ax * 0.9);
            }
            // visibility mask boundary
            if has_mask {
                let a = iso_cover(sdist.at(x, y), sdist.grad(x, y), 1e6, 0.75);
                if a > 0.0 {
                    blend(px, [255, 90, 160], a);
                }
            }
        }
    }
    // panel outline
    for x in 0..w {
        for y in [0, 1, h - 1, h.saturating_sub(2)] {
            img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[150, 150, 150]);
        }
    }
    for y in 0..h {
        for x in [0, 1, w - 1, w.saturating_sub(2)] {
            img[(y * w + x) * 3..(y * w + x) * 3 + 3].copy_from_slice(&[150, 150, 150]);
        }
    }
    // labels: ring angle on the horizontal axis through the optical centre, title in the corner
    let scale = ((h / 300).max(1)) as u32;
    let e = lens.eye(view);
    let mut put = |text: &str, x0: i32, y0: i32, col: [u8; 3]| {
        font::draw_text(text, x0, y0, scale, |px, py| {
            if px >= 0 && py >= 0 && (px as usize) < w && (py as usize) < h {
                let i = (py as usize * w + px as usize) * 3;
                img[i..i + 3].copy_from_slice(&col);
            }
        });
    };
    let ys = ((0.5 - e.cy / 2.0) * h as f32) as i32;
    for (ri, &deg) in opt.rings_deg.iter().enumerate() {
        if let Some(r) = lens.radius_for_angle(view, 1, deg) {
            let xp = e.cx + r;
            let xs = ((0.5 + xp / (2.0 * aspect)) * w as f32) as i32;
            if xs > 0 && (xs as usize) < w {
                let label = format!("{deg:.0}\u{b0}");
                let (tw, _) = font::text_size(&label, scale);
                put(
                    &label,
                    xs - tw as i32 / 2,
                    ys - 9 * scale as i32,
                    RING_COLORS[ri % RING_COLORS.len()],
                );
            }
        }
    }
    let title = format!(
        "{} EYE  UV LINES EVERY {:.2}",
        if view == 0 { "LEFT" } else { "RIGHT" },
        1.0 / n
    );
    put(&title, 6, 6, [220, 220, 220]);
    put(
        "R G B LINES = CHANNELS  ORANGE = AXIS",
        6,
        6 + 9 * scale as i32,
        [180, 180, 180],
    );
    img
}

/// Human readable summary of what the model does to one eye (numbers for the checklist).
pub fn summary(lens: &Lens, view: usize) -> String {
    use std::fmt::Write;
    let e = lens.eye(view);
    let cfg = lens.config();
    let mut s = String::new();
    let _ = writeln!(
        s,
        "eye {view} ({}): distortion {}, rendered FOV L{:.1} R{:.1} U{:.1} D{:.1} deg, optical centre ({:+.4}, {:+.4}) half-heights",
        if view == 0 { "left" } else { "right" },
        if cfg.enabled { "on" } else { "OFF" },
        e.fov[0], e.fov[1], e.fov[2], e.fov[3], e.cx, e.cy
    );
    let _ = writeln!(s, "  panel radius where the green channel shows a ray at angle off the optical axis (half-heights, px of 3552 tall):");
    for deg in [10.0f32, 20.0, 30.0, 40.0, 50.0] {
        match lens.radius_for_angle(view, 1, deg) {
            Some(r) => {
                let rr = lens.radius_for_angle(view, 0, deg).unwrap_or(r);
                let rb = lens.radius_for_angle(view, 2, deg).unwrap_or(r);
                let _ = writeln!(
                    s,
                    "    {deg:>4.0} deg: r = {r:.4} ({:.0} px)   red {:+.1} px, blue {:+.1} px vs green",
                    r * HALF_HEIGHT_PX,
                    (rr - r) * HALF_HEIGHT_PX,
                    (rb - r) * HALF_HEIGHT_PX
                );
            }
            None => {
                let _ = writeln!(s, "    {deg:>4.0} deg: not reached");
            }
        }
    }
    let a = cfg.aspect;
    let _ = writeln!(
        s,
        "  tangent (green) at the panel edges and corners; 'in FOV' = inside the rendered image:"
    );
    for (name, u, v) in [
        ("left  edge ", 0.0f32, 0.5f32),
        ("right edge ", 1.0, 0.5),
        ("top   edge ", 0.5, 0.0),
        ("bottom edge", 0.5, 1.0),
        ("corner TL  ", 0.0, 0.0),
        ("corner TR  ", 1.0, 0.0),
        ("corner BL  ", 0.0, 1.0),
        ("corner BR  ", 1.0, 1.0),
    ] {
        let t = lens.tangent(view, 1, u, v);
        let uv = lens.tangent_to_uv(view, t);
        let ang = t[0].hypot(t[1]).atan().to_degrees();
        let inside = (0.0..=1.0).contains(&uv[0]) && (0.0..=1.0).contains(&uv[1]);
        let _ =
            writeln!(
            s,
            "    {name} panel ({:+.3},{:+.3}): {ang:5.1} deg off axis, image uv ({:.3},{:.3}) {}",
            (u - 0.5) * 2.0 * a - e.cx,
            (0.5 - v) * 2.0 - e.cy,
            uv[0],
            uv[1],
            if inside { "in FOV" } else { "OUTSIDE FOV (black)" }
        );
    }
    let (mut in_fov, mut masked, total) = (0u32, 0u32, 200u32 * 200);
    for j in 0..200 {
        for i in 0..200 {
            let (u, v) = ((i as f32 + 0.5) / 200.0, (j as f32 + 0.5) / 200.0);
            let g = lens.compute(view, u, v)[1];
            if (0.0..=1.0).contains(&g[0]) && (0.0..=1.0).contains(&g[1]) {
                in_fov += 1;
            }
            if lens.hide_amount(view, u, v) >= 0.5 {
                masked += 1;
            }
        }
    }
    let _ = writeln!(
        s,
        "  {:.1}% of the panel shows image content (rest is black: outside the rendered FOV or masked); {:.1}% hidden by the visibility mask",
        100.0 * f64::from(in_fov) / f64::from(total),
        100.0 * f64::from(masked) / f64::from(total)
    );
    let folds: Vec<String> = (0..3)
        .filter_map(|c| {
            lens.first_fold(view, c, 1.6)
                .map(|r| format!("{}: r={r:.3}", ["red", "green", "blue"][c]))
        })
        .collect();
    if folds.is_empty() {
        let _ = writeln!(s, "  radial curve is monotonic out to r = 1.6 for all channels (panel corner is r = {:.3})", (a * a + 1.0).sqrt());
    } else {
        let _ = writeln!(
            s,
            "  WARNING: radial curve folds back (image mirrors) at {}",
            folds.join(", ")
        );
    }
    if e.fit_range > 0.0 {
        let _ = writeln!(
            s,
            "  fit_range {:.3}: beyond this radius the curve continues as its tangent line",
            e.fit_range
        );
    }
    if e.has_mask() {
        let _ = writeln!(
            s,
            "  visibility mask: {:?} a={} b={} n={} blackout L{} R{} T{} B{} feather {}",
            e.mask.shape,
            e.mask.a,
            e.mask.b,
            e.mask.n,
            e.blackout[0],
            e.blackout[1],
            e.blackout[2],
            e.blackout[3],
            e.mask_feather
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{parse, LensConfig};

    const ASPECT: f32 = 3840.0 / 3552.0;

    fn small() -> GridOptions {
        GridOptions {
            width: 192,
            height: 178,
            divisions: 10,
            rings_deg: vec![10.0, 20.0, 30.0, 40.0, 50.0],
        }
    }

    #[test]
    fn render_has_the_right_size_and_content() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        let img = render(&l, 0, &small());
        assert_eq!(img.len(), 192 * 178 * 3);
        let distinct: std::collections::HashSet<_> =
            img.chunks(3).map(|p| (p[0], p[1], p[2])).collect();
        assert!(distinct.len() > 20);
        // deterministic
        assert_eq!(img, render(&l, 0, &small()));
        // left and right eye differ (optical centre is mirrored)
        assert_ne!(img, render(&l, 1, &small()));
    }

    #[test]
    fn ring_colours_and_axis_colour_appear() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        let img = render(
            &l,
            0,
            &GridOptions {
                width: 480,
                height: 444,
                ..small()
            },
        );
        let has = |c: [u8; 3]| img.chunks(3).any(|p| p == c);
        for c in RING_COLORS.iter().take(5) {
            // pure ring colours exist at the centre of a line (some may be overdrawn by labels of the same colour)
            assert!(
                has(*c)
                    || img.chunks(3).any(|p| {
                        (i32::from(p[0]) - i32::from(c[0])).abs() < 40
                            && (i32::from(p[1]) - i32::from(c[1])).abs() < 40
                            && (i32::from(p[2]) - i32::from(c[2])).abs() < 40
                    }),
                "ring colour {c:?} missing"
            );
        }
        assert!(
            img.chunks(3)
                .any(|p| p[0] > 200 && p[1] > 90 && p[1] < 170 && p[2] < 60),
            "orange axis missing"
        );
    }

    #[test]
    fn mask_darkens_and_outlines() {
        let (cfg, w) = parse("mask = ellipse 0.9 0.8\n", ASPECT);
        assert!(w.is_empty());
        let l = Lens::new(cfg);
        let opt = GridOptions {
            width: 240,
            height: 222,
            ..small()
        };
        let img = render(&l, 0, &opt);
        // the panel corner is hidden: very dark
        let corner = &img[(8 * 240 + 8) * 3..(8 * 240 + 8) * 3 + 3];
        assert!(corner.iter().all(|&c| c < 12), "{corner:?}");
        assert!(
            img.chunks(3)
                .any(|p| p == [255, 90, 160] || (p[0] > 200 && p[1] < 140 && p[2] > 100)),
            "mask outline missing"
        );
        let plain = render(&Lens::new(LensConfig::defaults(ASPECT)), 0, &opt);
        assert_ne!(img, plain);
    }

    #[test]
    fn summary_reports_rings_corners_and_monotonicity() {
        let l = Lens::new(LensConfig::defaults(ASPECT));
        let s = summary(&l, 0);
        assert!(s.contains("eye 0 (left): distortion on"), "{s}");
        assert!(s.contains("30 deg: r = 0.7"), "{s}");
        assert!(s.contains("corner TL"), "{s}");
        assert!(s.contains("OUTSIDE FOV"), "{s}");
        assert!(s.contains("monotonic out to r = 1.6"), "{s}");
        let bad = Lens::new(parse("k = 0 -3 0\n", ASPECT).0);
        assert!(summary(&bad, 0).contains("WARNING: radial curve folds back"));
        let masked = Lens::new(parse("blackout = 0.1\nfit_range = 1.5\n", ASPECT).0);
        let ms = summary(&masked, 1);
        assert!(
            ms.contains("visibility mask") && ms.contains("fit_range 1.500"),
            "{ms}"
        );
    }

    #[test]
    fn iso_cover_is_one_on_the_line_and_zero_far_away() {
        assert_eq!(iso_cover(0.5, 0.01, 0.1, 0.5), 1.0);
        assert_eq!(iso_cover(0.55, 0.01, 0.1, 0.5), 0.0);
        let half = iso_cover(0.5 + 0.008, 0.01, 0.1, 0.5);
        assert!(half > 0.0 && half < 1.0);
    }
}
