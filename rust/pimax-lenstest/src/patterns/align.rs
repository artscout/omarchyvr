//! Stereo alignment target: per eye a thin line on the optical axis (left red, right green) with a short
//! horizontal tick, on black, over a faint static grid. The user shifts the two targets against each other in the
//! RENDERED image until the two lines fuse into one; the shift needed is the error of the lens `center`.
//!
//! # Conversion to `lens.conf` `center` (derived from `pimax-lens/src/model.rs`)
//!
//! The model maps a panel point `p` (half display heights, +x right, +y up, measured from `center`) to the ray
//! tangent `t = base * scale * (p - shift) / ca * (1 + k1 r^2 + ...)`. At the centre the polynomial is 1, so
//! `t = gain * (pos - center)` with `gain = base_g * scale / ca_g` (tangent per half-height). A target drawn at
//! tangent `T = tan(angle)` therefore lands on the panel at `pos = center + T / gain`: shifting the target by
//! `angle` is exactly the same as moving `center` by `tan(angle) / gain`, in the same direction (+x right, +y up).
//! The k terms change this by well under 1 % for the sub-degree shifts used here (k1 r^2 with r < 0.1).
//!
//! # Sign convention
//!
//! Vergence value `v` (degrees, shown per eye): the LEFT eye target moves by `+v` (right, toward the nose), the
//! RIGHT eye target by `-v` (left, toward the nose): increasing `v` moves the two targets toward each other, the
//! total separation changes by `2 v`. Equivalent `center`: `[left] x += tan(v)/gain_L`, `[right] x -= tan(v)/gain_R`.
//! Vertical value `d` (degrees, the offset BETWEEN the eyes): left target up by `d/2`, right down by `d/2`;
//! `[left] y += tan(d/2)/gain_L`, `[right] y -= tan(d/2)/gain_R`.

use super::{cover, Params};
use crate::color::{gray, mix, Rgb, BLACK};

/// One key press changes a value by this many degrees (fine step).
pub const UNIT_DEG: f32 = 0.05;
/// Coarse step in units of [`UNIT_DEG`] (0.25 deg).
pub const COARSE_UNITS: i32 = 5;
/// Limit of each value in units (about 10 deg), so a stuck key cannot run away.
pub const MAX_UNITS: i32 = 200;

const LEFT_COLOR: Rgb = [1.0, 0.1, 0.1];
const RIGHT_COLOR: Rgb = [0.1, 1.0, 0.1];
/// Half length of the horizontal tick in tangent units.
const TICK_HALF: f32 = 0.05;
const GRID_STEP: f32 = 0.1;
const GRID_COLOR: f32 = 0.10;

/// The alignment offsets, in units of 0.05 deg (integers, so the state is exact and hashable).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Align {
    /// Vergence value (see the module docs for the sign).
    pub verg: i32,
    /// Vertical offset between the eyes.
    pub vert: i32,
}

/// What the model says about one eye's optical centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeCal {
    /// Tangent per half-height at the centre: `base_g * scale / ca_g`.
    pub gain: f32,
    /// Current `center` of lens.conf.
    pub cx: f32,
    pub cy: f32,
}

impl EyeCal {
    /// The values of `[common]` defaults and the shipped lens.conf, used until the real config is loaded.
    pub const DEFAULT: [EyeCal; 2] = [
        EyeCal {
            gain: 0.65307,
            cx: 0.0271,
            cy: 0.009,
        },
        EyeCal {
            gain: 0.65307,
            cx: -0.0271,
            cy: 0.009,
        },
    ];
}

impl Align {
    pub fn verg_deg(&self) -> f32 {
        self.verg as f32 * UNIT_DEG
    }

    pub fn vert_deg(&self) -> f32 {
        self.vert as f32 * UNIT_DEG
    }

    pub fn add_verg(&mut self, units: i32) {
        self.verg = (self.verg + units).clamp(-MAX_UNITS, MAX_UNITS);
    }

    pub fn add_vert(&mut self, units: i32) {
        self.vert = (self.vert + units).clamp(-MAX_UNITS, MAX_UNITS);
    }

    /// Shift of the target of `eye` (0 left, 1 right) in degrees: (right, up).
    pub fn eye_shift_deg(&self, eye: usize) -> [f32; 2] {
        let s = if eye == 0 { 1.0 } else { -1.0 };
        [s * self.verg_deg(), s * self.vert_deg() * 0.5]
    }

    /// The `center` of lens.conf that makes the axis land where the shifted target is.
    pub fn new_center(&self, eye: usize, cal: &EyeCal) -> [f32; 2] {
        let d = self.eye_shift_deg(eye);
        [
            cal.cx + centre_delta(cal.gain, d[0]),
            cal.cy + centre_delta(cal.gain, d[1]),
        ]
    }
}

/// Change of the lens `center` (half-heights, same direction as the shift) equivalent to shifting the image
/// by `angle_deg` on the optical axis, for a centre gain `gain` (tangent per half-height).
pub fn centre_delta(gain: f32, angle_deg: f32) -> f32 {
    angle_deg.to_radians().tan() / gain
}

pub fn shade(p: &Params, x: u32, y: u32) -> Rgb {
    let (tx, ty) = p.tangent(x, y);
    let pix = p.fov.tangent_per_pixel(p.w);
    let mut c = BLACK;
    // faint static reference grid (does not move with the offsets)
    let gx = tx - (tx / GRID_STEP).round() * GRID_STEP;
    let gy = ty - (ty / GRID_STEP).round() * GRID_STEP;
    let g = gray(GRID_COLOR);
    c = mix(c, g, cover(gx, pix * 0.7, pix));
    c = mix(c, g, cover(gy, pix * 0.7, pix));
    let col = if p.eye == 0 { LEFT_COLOR } else { RIGHT_COLOR };
    let ox = p.align_shift_deg[0].to_radians().tan();
    let oy = p.align_shift_deg[1].to_radians().tan();
    c = mix(c, col, cover(tx - ox, pix * 1.0, pix));
    if (tx - ox).abs() < TICK_HALF {
        c = mix(c, col, cover(ty - oy, pix * 1.0, pix));
    }
    c
}

/// Terminal text for the current values.
pub fn report(a: &Align, cal: &[EyeCal; 2]) -> String {
    let l = a.new_center(0, &cal[0]);
    let r = a.new_center(1, &cal[1]);
    format!(
        "align: vergence {:+.2} deg per eye (left {:+.2}, right {:+.2}) | vertical {:+.2} deg between eyes | \
         [left] center = {:.4} {:.4} | [right] center = {:.4} {:.4}",
        a.verg_deg(),
        a.eye_shift_deg(0)[0],
        a.eye_shift_deg(1)[0],
        a.vert_deg(),
        l[0],
        l[1],
        r[0],
        r[1]
    )
}

/// The on-screen lines (upper case, font characters only).
pub fn hud_lines(a: &Align, cal: &[EyeCal; 2]) -> Vec<String> {
    let l = a.new_center(0, &cal[0]);
    let r = a.new_center(1, &cal[1]);
    vec![
        format!(
            "VERGENCE {:+.2} DEG/EYE   VERTICAL {:+.2} DEG",
            a.verg_deg(),
            a.vert_deg()
        ),
        format!("LEFT CENTER = {:.4} {:.4}", l[0], l[1]),
        format!("RIGHT CENTER = {:.4} {:.4}", r[0], r[1]),
        "[ ] VERGENCE 0.05  { } 0.25  I K VERTICAL  R RESET  Q QUIT".to_string(),
        "STICK L/R VERGENCE  STICK U/D VERTICAL  TRIGGER RESET".to_string(),
    ]
}

/// Ready-to-paste block printed on quit.
pub fn final_block(a: &Align, cal: &[EyeCal; 2], lens_path: &str) -> String {
    let l = a.new_center(0, &cal[0]);
    let r = a.new_center(1, &cal[1]);
    let lines = format!(
        "[left]\ncenter = {:.4} {:.4}\n[right]\ncenter = {:.4} {:.4}\n",
        l[0], l[1], r[0], r[1]
    );
    format!(
        "---- align result: vergence {:+.2} deg/eye, vertical {:+.2} deg ----\n{lines}\
         ---- write a variant file (lens.conf is not touched; later lines win) ----\n\
         mkdir -p ~/.config/omarchyvr/lens-ipd && {{ cat {lens_path}; cat <<'EOF'\n{lines}EOF\n}} > ~/.config/omarchyvr/lens-ipd/measured.conf\n",
        a.verg_deg(),
        a.vert_deg()
    )
}

#[cfg(test)]
mod tests {
    use super::super::tests::params;
    use super::*;
    use crate::image::Image;

    fn render(p: &Params) -> Image {
        Image::render(p.w, p.h, 1, &|x, y| shade(p, x, y))
    }

    #[test]
    fn centre_delta_matches_the_model_and_beats_the_rule_of_thumb() {
        // 0.01 half-heights at gain 0.65307 -> tangent 0.0065307 -> 0.3742 deg (not 0.45)
        let d = centre_delta(0.65307, 0.3742);
        assert!((d - 0.01).abs() < 2e-5, "{d}");
        assert_eq!(centre_delta(0.65307, 0.0), 0.0);
        assert!((centre_delta(0.5, -1.0) + centre_delta(0.5, 1.0)).abs() < 1e-7);
        // consistent with the lens model itself: the tangent at centre+delta is tan(angle)
        let (cfg, _) =
            pimax_lens::config::parse("base = 0.65307\nscale = 1.1\nca_g = 1.002\n", 1.08);
        let lens = pimax_lens::Lens::new(cfg);
        let e = lens.eye(0);
        let gain = e.base[1] * e.scale / e.ca[1];
        let delta = centre_delta(gain, 0.5);
        // panel u,v where p = (delta, 0) relative to the centre
        let u = 0.5 + (e.cx + delta) / (2.0 * 1.08);
        let v = 0.5 - e.cy / 2.0;
        let t = lens.tangent(0, 1, u, v);
        assert!((t[0] - 0.5f32.to_radians().tan()).abs() < 1e-5, "{t:?}");
        assert!(t[1].abs() < 1e-6);
    }

    #[test]
    fn sign_convention_opposite_directions() {
        let mut a = Align::default();
        assert_eq!(a.eye_shift_deg(0), [0.0, 0.0]);
        a.add_verg(1);
        let (l, r) = (a.eye_shift_deg(0), a.eye_shift_deg(1));
        // increasing vergence: left target moves right (+x), right target moves left (-x)
        assert!(
            (l[0] - 0.05).abs() < 1e-6 && (r[0] + 0.05).abs() < 1e-6,
            "{l:?} {r:?}"
        );
        a.add_verg(1);
        assert!(a.eye_shift_deg(0)[0] > l[0] && a.eye_shift_deg(1)[0] < r[0]);
        assert_eq!(a.eye_shift_deg(0)[1], 0.0);
        let mut b = Align::default();
        b.add_vert(2);
        assert!((b.eye_shift_deg(0)[1] - 0.05).abs() < 1e-6);
        assert!((b.eye_shift_deg(1)[1] + 0.05).abs() < 1e-6);
        // clamp
        b.add_vert(10_000);
        assert_eq!(b.vert, MAX_UNITS);
    }

    #[test]
    fn centres_follow_the_shift_in_the_same_direction() {
        let cal = EyeCal::DEFAULT;
        let mut a = Align::default();
        assert_eq!(a.new_center(0, &cal[0]), [0.0271, 0.009]);
        a.add_verg(COARSE_UNITS); // 0.25 deg
        let l = a.new_center(0, &cal[0]);
        let r = a.new_center(1, &cal[1]);
        let d = centre_delta(0.65307, 0.25);
        assert!((l[0] - (0.0271 + d)).abs() < 1e-6 && (r[0] - (-0.0271 - d)).abs() < 1e-6);
        assert!((d - 0.0067).abs() < 1e-4, "{d}");
        assert_eq!((l[1], r[1]), (0.009, 0.009));
    }

    #[test]
    fn printed_conversion() {
        let mut a = Align::default();
        a.add_verg(5);
        a.add_vert(-2);
        let t = report(&a, &EyeCal::DEFAULT);
        assert!(
            t.contains("vergence +0.25 deg per eye (left +0.25, right -0.25)"),
            "{t}"
        );
        assert!(t.contains("vertical -0.10 deg"), "{t}");
        assert!(t.contains("[left] center = 0.0338 0.0077"), "{t}");
        assert!(t.contains("[right] center = -0.0338 0.0103"), "{t}");
        let b = final_block(&a, &EyeCal::DEFAULT, "~/.config/omarchyvr/lens.conf");
        assert!(
            b.contains("[left]\ncenter = 0.0338 0.0077\n[right]\ncenter = -0.0338 0.0103\n"),
            "{b}"
        );
        assert!(
            b.contains("lens-ipd/measured.conf") && b.contains("cat ~/.config/omarchyvr/lens.conf")
        );
        for l in hud_lines(&a, &EyeCal::DEFAULT) {
            assert!(l.len() <= 64, "{l}");
            assert!(
                l.chars().all(|c| c.is_ascii_uppercase()
                    || c.is_ascii_digit()
                    || " []{}(),./%:|-+=?".contains(c)),
                "{l}"
            );
        }
    }

    #[test]
    fn target_is_on_the_axis_and_moves_with_the_offset() {
        let mut p = params(1000, 900);
        let (ax, ay) = p.principal();
        let img = render(&p);
        let c = img.get(ax as u32, 100);
        assert!(c[0] > 200 && c[1] < 60, "left is red: {c:?}");
        // short tick, not a full cross
        assert!(img.get(ax as u32 + 150, ay as u32)[0] < 80);
        assert!(img.get(ax as u32 + 10, ay as u32)[0] > 200);
        p.eye = 1;
        let c = render(&p).get(ax as u32, 100);
        assert!(c[1] > 200 && c[0] < 60, "right is green: {c:?}");
        // shift 2 deg right: the line is at tan(2 deg) in tangent units
        p.align_shift_deg = [2.0, 0.0];
        let img = render(&p);
        let (px, _) = p.to_pixel(2f32.to_radians().tan(), 0.0);
        assert!(img.get(px as u32, 100)[1] > 200);
        assert!(img.get(ax as u32 + 1, 100)[1] < 60 || (px - ax).abs() < 3.0);
        // black background with a faint grid only
        let far = img.get(3, 3);
        assert!(far.iter().all(|&v| v < 60), "{far:?}");
    }
}
