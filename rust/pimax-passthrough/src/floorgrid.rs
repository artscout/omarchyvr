//! A subtle floor grid under the user: one OpenXR quad layer in STAGE space at y = 0 (Monado's floor; the room
//! tools put the real floor there through the world offset, see `analysis/floor.md`).
//!
//! GPU cost is one textured quad in the compositor: the texture (grid lines with a radial fade, premultiplied
//! alpha) is made on the CPU once and uploaded once; per frame only the quad's pose changes. The quad follows the
//! head in whole grid cells, so the lines stay fixed in the room while the fade stays around the user.
//!
//! Toggle: `--floor` at start, the `o` key, or `$XDG_RUNTIME_DIR/pimax-floor-grid` containing `1` / `0` (checked
//! once a second; the Pimax app's Room page writes it).

use std::path::PathBuf;

/// Grid geometry.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Grid {
    /// Texture size (px, square).
    pub px: usize,
    /// Quad side (m).
    pub side_m: f64,
    /// Line spacing (m).
    pub cell_m: f64,
    /// Line width (m).
    pub line_m: f64,
    /// Line opacity at the centre.
    pub alpha: f64,
    /// Colour (linear-ish 0..1, before premultiplication).
    pub rgb: [f64; 3],
}

impl Default for Grid {
    fn default() -> Self {
        Grid { px: 1024, side_m: 6.0, cell_m: 0.5, line_m: 0.012, alpha: 0.32, rgb: [0.45, 0.8, 1.0] }
    }
}

/// Opacity of the grid at quad-local `(x, z)` metres from its centre: line coverage x radial fade. The fade is
/// 1 up to 0.8 m, then falls smoothly to 0 at 2.8 m, and a soft disc of radius 0.35 m under the user is cleared so
/// the lines do not sit right at the feet.
pub fn opacity(g: &Grid, x: f64, z: f64) -> f64 {
    let d = |v: f64| {
        let m = v.rem_euclid(g.cell_m);
        m.min(g.cell_m - m)
    };
    let px_m = g.side_m / g.px as f64;
    // anti-aliased line: coverage over one pixel
    let cov = |dist: f64| (((g.line_m * 0.5 + px_m * 0.5) - dist) / px_m).clamp(0.0, 1.0);
    let line = cov(d(x)).max(cov(d(z)));
    let r = (x * x + z * z).sqrt();
    let smooth = |e0: f64, e1: f64, v: f64| {
        let t = ((v - e0) / (e1 - e0)).clamp(0.0, 1.0);
        t * t * (3.0 - 2.0 * t)
    };
    let fade = (1.0 - smooth(0.8, 2.8, r)) * smooth(0.2, 0.45, r);
    (line * fade * g.alpha).clamp(0.0, 1.0)
}

/// Premultiplied RGBA (or BGRA with `bgra`) texture of the grid, row 0 = quad top (-z after the floor rotation).
pub fn texture(g: &Grid, bgra: bool) -> Vec<u8> {
    let mut out = vec![0u8; g.px * g.px * 4];
    let half = g.side_m / 2.0;
    for j in 0..g.px {
        let z = -half + (j as f64 + 0.5) * g.side_m / g.px as f64;
        for i in 0..g.px {
            let x = -half + (i as f64 + 0.5) * g.side_m / g.px as f64;
            let a = opacity(g, x, z);
            if a <= 0.0 {
                continue;
            }
            let c = [g.rgb[0] * a, g.rgb[1] * a, g.rgb[2] * a];
            let o = (j * g.px + i) * 4;
            let (r, b) = if bgra { (c[2], c[0]) } else { (c[0], c[2]) };
            out[o] = (r * 255.0).round() as u8;
            out[o + 1] = (c[1] * 255.0).round() as u8;
            out[o + 2] = (b * 255.0).round() as u8;
            out[o + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

/// Quad centre for a head at `(x, z)` (STAGE): snapped to whole cells so the lines stay put in the room.
pub fn centre(g: &Grid, head_x: f64, head_z: f64) -> (f64, f64) {
    ((head_x / g.cell_m).round() * g.cell_m, (head_z / g.cell_m).round() * g.cell_m)
}

/// Orientation of a quad lying on the floor facing up: -90 deg about X (x, y, z, w).
pub const FLAT: [f32; 4] = [-std::f32::consts::FRAC_1_SQRT_2, 0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2];

/// The toggle file.
pub fn toggle_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp")).join("pimax-floor-grid")
}

/// `Some(on)` from the toggle file's text, `None` when it says nothing usable.
pub fn parse_toggle(s: &str) -> Option<bool> {
    match s.trim() {
        "1" | "on" | "true" => Some(true),
        "0" | "off" | "false" => Some(false),
        _ => None,
    }
}

/// Watches the toggle file (mtime, at most once a second).
#[derive(Debug, Default)]
pub struct ToggleWatch {
    last_check: Option<std::time::Instant>,
    mtime: Option<std::time::SystemTime>,
}

impl ToggleWatch {
    /// A new state when the file changed since the last look.
    pub fn poll(&mut self) -> Option<bool> {
        if self.last_check.is_some_and(|t| t.elapsed().as_secs_f64() < 1.0) {
            return None;
        }
        self.last_check = Some(std::time::Instant::now());
        let p = toggle_path();
        let m = std::fs::metadata(&p).and_then(|m| m.modified()).ok()?;
        if self.mtime == Some(m) {
            return None;
        }
        self.mtime = Some(m);
        parse_toggle(&std::fs::read_to_string(p).ok()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines_fade_and_clear_feet() {
        let g = Grid::default();
        assert!(opacity(&g, 0.5, 0.21) > 0.25, "a line near the user is visible");
        assert_eq!(opacity(&g, 0.25, 0.75), 0.0, "between lines: nothing");
        assert_eq!(opacity(&g, 0.0, 0.0), 0.0, "cleared under the feet");
        assert_eq!(opacity(&g, 2.9, 0.0), 0.0, "faded out at the edge");
        assert!(opacity(&g, 2.0, 0.0) < opacity(&g, 1.0, 0.0));
        let t = texture(&Grid { px: 256, ..g }, false);
        let lit = t.chunks(4).filter(|p| p[3] > 0).count();
        assert!(lit > 1000 && lit < 256 * 256 / 4, "{lit}");
        assert!(t.chunks(4).all(|p| p[0] <= p[3] && p[1] <= p[3] && p[2] <= p[3]), "premultiplied");
    }

    #[test]
    fn centre_snaps_to_cells_and_toggle_parses() {
        let g = Grid::default();
        assert_eq!(centre(&g, 0.74, -0.26), (0.5, -0.5));
        assert_eq!(parse_toggle("1\n"), Some(true));
        assert_eq!(parse_toggle("off"), Some(false));
        assert_eq!(parse_toggle("x"), None);
    }
}
