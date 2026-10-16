//! The head-locked text (legend / instructions) as an `XrCompositionLayerQuad` in VIEW space.
//!
//! Text baked into each eye's image lands at the same image position in both eyes, which with canted displays are
//! different directions: the wearer sees it double. A quad layer is placed by the compositor in both eyes
//! consistently. This module is the pure part: where the quad hangs, how big it is, and the texture (the text on a
//! transparent background). The OpenXR/Vulkan plumbing is in `xr`.

use crate::color::{self, BLACK};
use crate::image::Image;
use crate::patterns::{HUD_BG, HUD_FG};

/// Quad texture size in pixels (1 mm per pixel at the default quad size).
pub const TEX_W: u32 = 2048;
pub const TEX_H: u32 = 1024;
/// Largest text scale (pixels per font pixel); smaller when the text does not fit.
pub const MAX_SCALE: u32 = 5;

/// Placement of the quad in VIEW space (metres; -z is forward).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QuadSpec {
    /// Distance straight ahead of the head.
    pub distance: f32,
    pub width: f32,
    pub height: f32,
    /// How far below the view centre the top edge of the quad is.
    pub top_below: f32,
}

/// 2 m ahead, 2.048 x 1.024 m, top edge 0.25 m (7 degrees) below the centre of view.
pub const SPEC: QuadSpec = QuadSpec {
    distance: 2.0,
    width: 2.048,
    height: 1.024,
    top_below: 0.25,
};

impl QuadSpec {
    /// Centre of the quad in VIEW space (x right, y up, z backwards).
    pub fn position(&self) -> [f32; 3] {
        [0.0, -(self.top_below + 0.5 * self.height), -self.distance]
    }

    /// Visual angle in degrees of `metres` on the quad, in the middle.
    pub fn angle_deg(&self, metres: f32) -> f32 {
        2.0 * (0.5 * metres / self.distance).atan().to_degrees()
    }

    /// Visual angle of one font pixel at text `scale` for a texture of `tex_w` pixels across the quad.
    pub fn font_pixel_deg(&self, scale: u32, tex_w: u32) -> f32 {
        self.angle_deg(self.width / tex_w as f32 * scale as f32)
    }
}

/// Largest scale (at most [`MAX_SCALE`]) at which all `lines` fit in a `w` x `h` texture.
pub fn fit_scale(lines: &[String], w: u32, h: u32) -> u32 {
    for scale in (1..=MAX_SCALE).rev() {
        let widest = lines
            .iter()
            .map(|l| pimax_lens::font::text_size(l, scale).0)
            .max()
            .unwrap_or(0);
        let rows = lines.len() as u32 * 10 * scale;
        if widest + 4 * scale <= w && rows + 4 * scale <= h {
            return scale;
        }
    }
    1
}

/// The texture: the lines centred, top-anchored, on near-black backdrops; everything else exactly black (made
/// transparent by [`alpha_from_black`]).
pub fn render(lines: &[String]) -> Image {
    let mut img = Image::new(TEX_W, TEX_H, BLACK);
    let scale = fit_scale(lines, TEX_W, TEX_H);
    for (i, line) in lines.iter().enumerate() {
        let (tw, _) = pimax_lens::font::text_size(line, scale);
        let x = (TEX_W as i32 - tw as i32) / 2;
        img.label(
            line,
            x,
            2 * scale as i32 + i as i32 * 10 * scale as i32,
            scale,
            color::gray(HUD_FG),
            HUD_BG,
        );
    }
    img
}

/// Set the alpha of 4-byte pixels: 0 where the colour is exactly black, 255 elsewhere (works for RGBA and BGRA).
pub fn alpha_from_black(px: &mut [u8]) {
    for p in px.chunks_exact_mut(4) {
        p[3] = if p[0] == 0 && p[1] == 0 && p[2] == 0 {
            0
        } else {
            255
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quad_hangs_ahead_and_slightly_below_the_centre() {
        let [x, y, z] = SPEC.position();
        assert_eq!(x, 0.0);
        assert_eq!(z, -2.0);
        assert!((y - -(0.25 + 0.512)).abs() < 1e-6);
        // the top edge is 0.25 m below the centre of view, the bottom edge well inside the lens FOV
        let top = (y + 0.5 * SPEC.height).atan2(-z).to_degrees();
        let bottom = (y - 0.5 * SPEC.height).atan2(-z).to_degrees();
        assert!((top - -7.125).abs() < 0.01, "{top}");
        assert!(bottom > -35.0 && bottom < top);
    }

    #[test]
    fn quad_size_is_legible_and_not_too_wide() {
        let width = SPEC.angle_deg(SPEC.width);
        assert!(width > 30.0 && width < 60.0, "{width}");
        // text at the largest scale: a character cell about 1 degree tall (7 font pixels)
        let ch = 7.0 * SPEC.font_pixel_deg(MAX_SCALE, TEX_W);
        assert!(ch > 0.8 && ch < 1.5, "{ch}");
        // texture and quad have the same aspect ratio: square pixels
        assert!((SPEC.width / TEX_W as f32 - SPEC.height / TEX_H as f32).abs() < 1e-6);
    }

    #[test]
    fn scale_shrinks_to_fit() {
        let short = vec!["HI".to_string()];
        assert_eq!(fit_scale(&short, TEX_W, TEX_H), MAX_SCALE);
        assert_eq!(fit_scale(&[], TEX_W, TEX_H), MAX_SCALE);
        // 64 characters (the wrap width) fit at scale 5 in 2048 px
        assert_eq!(fit_scale(&["A".repeat(64)], TEX_W, TEX_H), MAX_SCALE);
        let wide = vec!["A".repeat(80)];
        let s = fit_scale(&wide, TEX_W, TEX_H);
        assert!((1..MAX_SCALE).contains(&s));
        let tall: Vec<String> = (0..30).map(|i| format!("L{i}")).collect();
        let s = fit_scale(&tall, TEX_W, TEX_H);
        assert!(s * 10 * 30 + 4 * s <= TEX_H);
        // absurdly much text still returns the minimum
        assert_eq!(fit_scale(&vec!["A".repeat(500); 200], TEX_W, TEX_H), 1);
    }

    #[test]
    fn texture_has_text_centred_and_a_transparent_rest() {
        let img = render(&["HEAD LOCKED".to_string(), "SECOND".to_string()]);
        assert_eq!((img.w, img.h), (TEX_W, TEX_H));
        let lit = img.count(|c| c != [0, 0, 0]);
        assert!(lit > 100 && lit < (TEX_W * TEX_H / 10) as usize);
        assert_eq!(img.get(0, 0), [0, 0, 0]);
        assert_eq!(img.get(TEX_W - 1, TEX_H - 1), [0, 0, 0]);
        let xs: Vec<u32> = (0..TEX_W)
            .filter(|&x| (0..TEX_H).any(|y| img.get(x, y) != [0, 0, 0]))
            .collect();
        let mid = (xs[0] + xs[xs.len() - 1]) as i32 / 2;
        assert!((mid - TEX_W as i32 / 2).abs() < 8, "{mid}");
        assert!(render(&[]).count(|c| c != [0, 0, 0]) == 0);
    }

    #[test]
    fn alpha_is_zero_only_on_pure_black() {
        let mut px = [0, 0, 0, 255, 3, 0, 0, 255, 0, 0, 9, 255, 40, 40, 40, 0];
        alpha_from_black(&mut px);
        assert_eq!([px[3], px[7], px[11], px[15]], [0, 255, 255, 255]);
    }
}
