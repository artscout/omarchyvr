//! `lens.conf` parsing.
//!
//! The grammar is the one of the C parser this crate replaces (`pimax_lens.c` of the Monado `omarchyvr` branch):
//! INI-like, `#` and `;` start comments, `[common]`/`[both]` (or no section) apply to both eyes, `[left]`/`[right]`
//! override one eye, later lines win, bad lines are reported and skipped. Every key of the old file format keeps its
//! meaning; the new keys are optional and default to "no change", so an old file gives bit-identical results.
//!
//! Keys (per eye or common):
//!
//! | key | values | meaning |
//! |---|---|---|
//! | `distortion` | on/off | global switch (identity mesh when off) |
//! | `cant_deg` | deg | global (any section): each display is canted outward by this angle; the driver yaws the left view +, the right view - (default 0, `PIMAX_CANT_DEG` overrides) |
//! | `fov` | L R U D | rendered half-angles in degrees |
//! | `center` | x y | optical centre on the panel, half-display-heights, +x right, +y up |
//! | `base`, `base_r/g/b` | f | tangent per unit radius at the centre (all channels / one channel) |
//! | `scale` | f | overall multiplier of the ray tangent |
//! | `k`, `k_r/g/b` | k1 k2 k3 [k4 k5] | radial polynomial `1 + k1 r^2 + k2 r^4 + ...` (all channels / one channel) |
//! | `ca_r/g/b` | f | radial divisor per channel (lateral chromatic aberration) |
//! | `shift_r/g/b` | dx dy | channel optical centre shift relative to `center` |
//! | `fit_range` | R | beyond radius R the radial curve is continued with its tangent line (0 = off) |
//! | `blackout` | w or L R U D | black margin at the panel edges, half-display-heights |
//! | `mask` | off / ellipse a b / rect a b / superellipse a b n | black outside this shape around the optical centre |
//! | `mask_feather` | w | width of the transition band outside the black-out boundary |

use std::path::PathBuf;

/// Colour channels: 0 = red, 1 = green, 2 = blue.
pub const CHANNELS: usize = 3;
/// Largest accepted |cant_deg|.
pub const MAX_CANT_DEG: f32 = 20.0;
/// Number of polynomial coefficients per channel (k1 .. k5, multiplying r^2, r^4, .. r^10).
pub const K_TERMS: usize = 5;

/// Shape of the visibility mask.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MaskShape {
    Off,
    Ellipse,
    Rect,
    Superellipse,
}

/// Visibility mask around the optical centre: `(|x/a|^n + |y/b|^n)^(1/n) <= 1` is visible.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Mask {
    pub shape: MaskShape,
    pub a: f32,
    pub b: f32,
    pub n: f32,
}

impl Mask {
    pub const OFF: Mask = Mask {
        shape: MaskShape::Off,
        a: 0.0,
        b: 0.0,
        n: 2.0,
    };
}

/// Lens parameters of one eye.
#[derive(Clone, Debug, PartialEq)]
pub struct EyeConfig {
    /// Half-angles in degrees of the rendered (client) FOV: left, right, up, down (all positive).
    pub fov: [f32; 4],
    /// Optical centre on the panel in units of half the display height, +x right, +y up (as the client sees it).
    pub cx: f32,
    pub cy: f32,
    /// Tangent of the ray angle per unit panel radius at the centre, per channel.
    pub base: [f32; CHANNELS],
    /// Overall multiplier on ray tangents.
    pub scale: f32,
    /// Polynomial per channel: tan = base * scale * r * (1 + k1 r^2 + k2 r^4 + ...).
    pub k: [[f32; K_TERMS]; CHANNELS],
    /// Per-channel radial divisor (lateral chromatic aberration).
    pub ca: [f32; CHANNELS],
    /// Per-channel optical centre shift relative to (cx, cy).
    pub shift: [[f32; 2]; CHANNELS],
    /// Radius beyond which the radial curve is replaced by its tangent line; 0 = never.
    pub fit_range: f32,
    /// Black margins at the panel edges: left, right, top, bottom (half-display-heights).
    pub blackout: [f32; 4],
    pub mask: Mask,
    /// Width of the transition band outside the black-out boundary.
    pub mask_feather: f32,
    /// `lut = FILE`: exact distortion from a `pimax-lens make-lut` file instead of the polynomial. Relative paths are
    /// relative to the config file. `None` = radial model.
    pub lut: Option<String>,
    /// `lut_margin = DEG`: widen the rendered FOV by this much on every side of the LUT's FOV port.
    pub lut_margin: f32,
    /// `lut_interp = bilinear|bicubic`.
    pub lut_interp: crate::lut::Interp,
}

impl EyeConfig {
    /// Built-in defaults: fitted to the "Dream Air like Pimax" angle table, see `analysis/lens-distortion.md`.
    pub fn defaults(eye: usize) -> EyeConfig {
        let k = [0.60779_f32, -0.72489, 0.58858, 0.0, 0.0];
        EyeConfig {
            fov: [50.0, 50.0, 44.5, 44.5],
            cx: if eye == 0 { 0.0271 } else { -0.0271 },
            cy: 0.009,
            base: [0.65307; CHANNELS],
            scale: 1.0,
            k: [k; CHANNELS],
            ca: [1.0045, 1.0, 0.995],
            shift: [[0.0; 2]; CHANNELS],
            fit_range: 0.0,
            blackout: [0.0; 4],
            mask: Mask::OFF,
            mask_feather: 0.03,
            lut: None,
            lut_margin: 0.0,
            lut_interp: crate::lut::Interp::Bicubic,
        }
    }

    /// True when any of the optional visibility features is set.
    pub fn has_mask(&self) -> bool {
        self.mask.shape != MaskShape::Off || self.blackout.iter().any(|&b| b > 0.0)
    }
}

/// Parsed configuration for both eyes.
#[derive(Clone, Debug, PartialEq)]
pub struct LensConfig {
    /// false: identity mesh (no distortion); the FOV still applies.
    pub enabled: bool,
    /// Display width / height as the client sees it (3840/3552).
    pub aspect: f32,
    /// Outward cant of each display in degrees (view rotation about +Y: left +, right -). 0 = parallel views.
    pub cant_deg: f32,
    pub eye: [EyeConfig; 2],
}

impl LensConfig {
    pub fn defaults(aspect: f32) -> LensConfig {
        LensConfig {
            enabled: true,
            aspect,
            cant_deg: 0.0,
            eye: [EyeConfig::defaults(0), EyeConfig::defaults(1)],
        }
    }
}

/// A problem found while parsing; parsing continues after it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Warning {
    pub line: usize,
    pub message: String,
}

fn is_space(c: char) -> bool {
    matches!(c, ' ' | '\t' | '\n' | '\x0b' | '\x0c' | '\r')
}

/// `strtof`-like scan of a decimal float at the start of `s` (leading white space allowed).
/// Returns the value and the number of bytes consumed. Non-finite results count as a failure.
fn scan_float(s: &str) -> Option<(f32, usize)> {
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && is_space(b[i] as char) {
        i += 1;
    }
    let start = i;
    if i < b.len() && (b[i] == b'+' || b[i] == b'-') {
        i += 1;
    }
    let int_start = i;
    while i < b.len() && b[i].is_ascii_digit() {
        i += 1;
    }
    let mut digits = i - int_start;
    if i < b.len() && b[i] == b'.' {
        i += 1;
        let frac_start = i;
        while i < b.len() && b[i].is_ascii_digit() {
            i += 1;
        }
        digits += i - frac_start;
    }
    if digits == 0 {
        return None;
    }
    if i < b.len() && (b[i] == b'e' || b[i] == b'E') {
        let mut j = i + 1;
        if j < b.len() && (b[j] == b'+' || b[j] == b'-') {
            j += 1;
        }
        let exp_start = j;
        while j < b.len() && b[j].is_ascii_digit() {
            j += 1;
        }
        if j > exp_start {
            i = j;
        }
    }
    let v: f32 = s[start..i].parse().ok()?;
    v.is_finite().then_some((v, i))
}

/// Parse between `min` and `max` floats separated by white space and/or commas. Anything after the last
/// accepted number is ignored (as the C parser did). `None` if fewer than `min` numbers could be read.
fn floats(s: &str, min: usize, max: usize) -> Option<Vec<f32>> {
    let mut out = Vec::new();
    let mut rest = s;
    while out.len() < max {
        match scan_float(rest) {
            Some((v, used)) => {
                out.push(v);
                rest = &rest[used..];
                rest = rest.trim_start_matches(|c: char| c == ',' || is_space(c));
            }
            None => break,
        }
    }
    (out.len() >= min).then_some(out)
}

fn channel_of(suffix: &str) -> Option<usize> {
    match suffix {
        "r" => Some(0),
        "g" => Some(1),
        "b" => Some(2),
        _ => None,
    }
}

fn parse_mask(val: &str) -> Option<Mask> {
    let val = val.trim();
    let (word, rest) = match val.find(|c: char| is_space(c) || c == ',') {
        Some(p) => (&val[..p], &val[p..]),
        None => (val, ""),
    };
    match word.to_ascii_lowercase().as_str() {
        "off" | "none" | "0" => Some(Mask::OFF),
        "ellipse" | "circle" => {
            let f = floats(rest, 1, 2)?;
            let (a, b) = (f[0], *f.get(1).unwrap_or(&f[0]));
            (a > 0.0 && b > 0.0).then_some(Mask {
                shape: MaskShape::Ellipse,
                a,
                b,
                n: 2.0,
            })
        }
        "rect" | "box" => {
            let f = floats(rest, 1, 2)?;
            let (a, b) = (f[0], *f.get(1).unwrap_or(&f[0]));
            (a > 0.0 && b > 0.0).then_some(Mask {
                shape: MaskShape::Rect,
                a,
                b,
                n: 2.0,
            })
        }
        "superellipse" | "round" => {
            let f = floats(rest, 3, 3)?;
            (f[0] > 0.0 && f[1] > 0.0 && f[2] >= 1.0).then_some(Mask {
                shape: MaskShape::Superellipse,
                a: f[0],
                b: f[1],
                n: f[2],
            })
        }
        _ => None,
    }
}

/// Apply one `key = value` to one eye. `Err` carries nothing: the caller reports "bad key/value".
fn apply_eye_key(e: &mut EyeConfig, key: &str, val: &str) -> Result<(), ()> {
    match key {
        "fov" => {
            let f = floats(val, 4, 4).ok_or(())?;
            // tan() of 90 degrees is infinite, and a zero width image has no UV map
            if f.iter().any(|&a| !(0.5..=89.5).contains(&a)) {
                return Err(());
            }
            e.fov = [f[0], f[1], f[2], f[3]];
        }
        "center" => {
            let f = floats(val, 2, 2).ok_or(())?;
            if f.iter().any(|a| a.abs() > 2.0) {
                return Err(());
            }
            e.cx = f[0];
            e.cy = f[1];
        }
        "base" => {
            let b = floats(val, 1, 1).ok_or(())?[0];
            if !(0.01..=10.0).contains(&b) {
                return Err(());
            }
            e.base = [b; CHANNELS];
        }
        "scale" => {
            let v = floats(val, 1, 1).ok_or(())?[0];
            if !(0.05..=20.0).contains(&v) {
                return Err(());
            }
            e.scale = v;
        }
        "k" => {
            let f = floats(val, 3, K_TERMS).ok_or(())?;
            let mut k = [0.0; K_TERMS];
            k[..f.len()].copy_from_slice(&f);
            e.k = [k; CHANNELS];
        }
        "fit_range" => {
            let r = floats(val, 1, 1).ok_or(())?[0];
            if r < 0.0 {
                return Err(());
            }
            e.fit_range = r;
        }
        "blackout" => {
            let f = floats(val, 1, 4).ok_or(())?;
            let b = match f.len() {
                1 => [f[0]; 4],
                4 => [f[0], f[1], f[2], f[3]],
                _ => return Err(()),
            };
            if b.iter().any(|&x| x < 0.0) {
                return Err(());
            }
            e.blackout = b;
        }
        "mask" => e.mask = parse_mask(val).ok_or(())?,
        "lut" => {
            e.lut = match val.trim_matches(['"', '\'']) {
                "" => return Err(()),
                v if matches!(v.to_ascii_lowercase().as_str(), "none" | "off") => None,
                v => Some(v.to_string()),
            };
        }
        "lut_margin" => {
            let m = floats(val, 1, 1).ok_or(())?[0];
            if !(0.0..=10.0).contains(&m) {
                return Err(());
            }
            e.lut_margin = m;
        }
        "lut_interp" => {
            e.lut_interp = match val.to_ascii_lowercase().as_str() {
                "bilinear" | "linear" => crate::lut::Interp::Bilinear,
                "bicubic" | "cubic" => crate::lut::Interp::Bicubic,
                _ => return Err(()),
            };
        }
        "mask_feather" => {
            let w = floats(val, 1, 1).ok_or(())?[0];
            if w < 0.0 {
                return Err(());
            }
            e.mask_feather = w;
        }
        _ => {
            // Keys with a channel suffix: k_r, base_g, ca_b, shift_r ...
            let (stem, suffix) = key.rsplit_once('_').ok_or(())?;
            let c = channel_of(suffix).ok_or(())?;
            match stem {
                "k" => {
                    let f = floats(val, 3, K_TERMS).ok_or(())?;
                    let mut k = [0.0; K_TERMS];
                    k[..f.len()].copy_from_slice(&f);
                    e.k[c] = k;
                }
                "base" => {
                    let b = floats(val, 1, 1).ok_or(())?[0];
                    if !(0.01..=10.0).contains(&b) {
                        return Err(());
                    }
                    e.base[c] = b;
                }
                "ca" => {
                    // a divisor of the panel radius: zero or negative would turn the picture inside out
                    let v = floats(val, 1, 1).ok_or(())?[0];
                    if !(0.5..=2.0).contains(&v) {
                        return Err(());
                    }
                    e.ca[c] = v;
                }
                "shift" => {
                    let f = floats(val, 2, 2).ok_or(())?;
                    e.shift[c] = [f[0], f[1]];
                }
                _ => return Err(()),
            }
        }
    }
    Ok(())
}

/// Apply the text of a lens.conf to `cfg`. `name` is only used in the warnings (usually the file path).
pub fn parse_into(cfg: &mut LensConfig, text: &str, name: &str) -> Vec<Warning> {
    let mut warnings = Vec::new();
    let mut section = 0; // 0 = common, 1 = left, 2 = right
    for (idx, raw) in text.lines().enumerate() {
        let lineno = idx + 1;
        let line = match raw.find(['#', ';']) {
            Some(p) => &raw[..p],
            None => raw,
        };
        let s = line.trim_matches(is_space);
        if s.is_empty() {
            continue;
        }
        if s.starts_with('[') {
            if s.starts_with("[left]") {
                section = 1;
            } else if s.starts_with("[right]") {
                section = 2;
            } else if s.starts_with("[common]") || s.starts_with("[both]") {
                section = 0;
            } else {
                warnings.push(Warning {
                    line: lineno,
                    message: format!("{name}:{lineno} unknown section {s}"),
                });
            }
            continue;
        }
        let Some((key, val)) = s.split_once('=') else {
            warnings.push(Warning {
                line: lineno,
                message: format!("{name}:{lineno} expected key = value"),
            });
            continue;
        };
        let key = key.trim_matches(is_space).to_ascii_lowercase();
        let val = val.trim_matches(is_space);
        if key == "distortion" {
            let v = val.to_ascii_lowercase();
            cfg.enabled = !matches!(v.as_str(), "off" | "0" | "false" | "no");
            continue;
        }
        if key == "cant_deg" {
            match floats(val, 1, 1)
                .map(|f| f[0])
                .filter(|a| a.abs() <= MAX_CANT_DEG)
            {
                Some(a) => cfg.cant_deg = a,
                None => warnings.push(Warning {
                    line: lineno,
                    message: format!("{name}:{lineno} bad key/value '{key} = {val}'"),
                }),
            }
            continue;
        }
        let mut ok = true;
        for e in 0..2 {
            if section == 0 || section == e + 1 {
                ok = apply_eye_key(&mut cfg.eye[e], &key, val).is_ok() && ok;
            }
        }
        if !ok {
            warnings.push(Warning {
                line: lineno,
                message: format!("{name}:{lineno} bad key/value '{key} = {val}'"),
            });
        }
    }
    warnings
}

/// Defaults plus the text of a lens.conf.
pub fn parse(text: &str, aspect: f32) -> (LensConfig, Vec<Warning>) {
    let mut cfg = LensConfig::defaults(aspect);
    let w = parse_into(&mut cfg, text, "lens.conf");
    (cfg, w)
}

/// Where the config is read from: `PIMAX_LENS_CONF`, else `$XDG_CONFIG_HOME/omarchyvr/lens.conf`,
/// else `$HOME/.config/omarchyvr/lens.conf` (`./.config/...` without HOME), like the C driver did.
pub fn config_path(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("PIMAX_LENS_CONF").filter(|p| !p.is_empty()) {
        return PathBuf::from(p);
    }
    if let Some(x) = env("XDG_CONFIG_HOME").filter(|x| !x.is_empty()) {
        return PathBuf::from(x).join("omarchyvr/lens.conf");
    }
    let home = env("HOME").unwrap_or_else(|| ".".to_string());
    PathBuf::from(home).join(".config/omarchyvr/lens.conf")
}

/// `PIMAX_CANT_DEG=<deg>` overrides `cant_deg`; `None` if unset, `Err(text)` if unparsable or out of range.
pub fn cant_override(env: &dyn Fn(&str) -> Option<String>) -> Option<Result<f32, String>> {
    let v = env("PIMAX_CANT_DEG").filter(|v| !v.trim().is_empty())?;
    Some(
        match floats(v.trim(), 1, 1)
            .map(|f| f[0])
            .filter(|a| a.abs() <= MAX_CANT_DEG)
        {
            Some(a) => Ok(a),
            None => Err(v),
        },
    )
}

/// The shipped reference profile is used when the user has no lens.conf, unless `PIMAX_NO_REFERENCE` is set to
/// something other than `0` (the same switch as the reference calibration in `pimax-cam`).
pub fn reference_allowed(env: &dyn Fn(&str) -> Option<String>) -> bool {
    !env("PIMAX_NO_REFERENCE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// `PIMAX_LENS_OFF=1` (anything non-empty not starting with '0') forces the identity mesh.
pub fn forced_off(env: &dyn Fn(&str) -> Option<String>) -> bool {
    env("PIMAX_LENS_OFF").is_some_and(|v| !v.is_empty() && !v.starts_with('0'))
}

/// The process environment as the `env` argument of [`config_path`] / [`forced_off`].
pub fn process_env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    const ASPECT: f32 = 3840.0 / 3552.0;

    #[test]
    fn defaults_match_the_old_c_defaults() {
        let c = LensConfig::defaults(ASPECT);
        let l = &c.eye[0];
        assert_eq!(l.fov, [50.0, 50.0, 44.5, 44.5]);
        assert_eq!((l.cx, l.cy), (0.0271, 0.009));
        assert_eq!(c.eye[1].cx, -0.0271);
        assert_eq!(l.base, [0.65307; 3]);
        assert_eq!(l.scale, 1.0);
        for ch in 0..3 {
            assert_eq!(l.k[ch], [0.60779, -0.72489, 0.58858, 0.0, 0.0]);
        }
        assert_eq!(l.ca, [1.0045, 1.0, 0.995]);
        assert_eq!(l.fit_range, 0.0);
        assert!(!l.has_mask());
        assert!(c.enabled);
    }

    #[test]
    fn the_old_example_file_parses_to_the_defaults() {
        let text = "[common]\nfov = 50 50 44.5 44.5\nbase = 0.65307\nscale = 1.0\n\
                    k_g = 0.60779 -0.72489 0.58858\nk_r = 0.60779 -0.72489 0.58858\nk_b = 0.60779 -0.72489 0.58858\n\
                    ca_r = 1.0045\nca_g = 1.0\nca_b = 0.995\n[left]\ncenter = 0.0271 0.009\n[right]\ncenter = -0.0271 0.009\n";
        let (c, w) = parse(text, ASPECT);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c, LensConfig::defaults(ASPECT));
    }

    #[test]
    fn sections_comments_and_override_order() {
        let text = "fov = 40 41 42 43 # trailing comment\n; whole line\n[left]\nfov = 1 2 3 4\n[both]\nscale = 1.5\n[right]\nscale = 2\n";
        let (c, w) = parse(text, ASPECT);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.eye[0].fov, [1.0, 2.0, 3.0, 4.0]);
        assert_eq!(c.eye[1].fov, [40.0, 41.0, 42.0, 43.0]);
        assert_eq!(c.eye[0].scale, 1.5);
        assert_eq!(c.eye[1].scale, 2.0);
    }

    #[test]
    fn k_accepts_three_to_five_numbers_and_zeroes_the_rest() {
        let (c, w) = parse("k = 1 2 3\nk_r = 1 2 3 4 5\nk_b = 9,8,7,6\n", ASPECT);
        assert!(w.is_empty());
        assert_eq!(c.eye[0].k[1], [1.0, 2.0, 3.0, 0.0, 0.0]);
        assert_eq!(c.eye[0].k[0], [1.0, 2.0, 3.0, 4.0, 5.0]);
        assert_eq!(c.eye[0].k[2], [9.0, 8.0, 7.0, 6.0, 0.0]);
        // fewer than three numbers is an error and changes nothing
        let (c2, w2) = parse("k = 1 2\n", ASPECT);
        assert_eq!(w2.len(), 1);
        assert_eq!(c2.eye[0].k[0], EyeConfig::defaults(0).k[0]);
    }

    #[test]
    fn bad_lines_are_reported_with_the_old_wording() {
        let (_, w) = parse("[nonsense]\njust words\nfov = 1 2\nunknown = 3\n", ASPECT);
        let msgs: Vec<_> = w.iter().map(|x| x.message.as_str()).collect();
        assert_eq!(
            msgs,
            vec![
                "lens.conf:1 unknown section [nonsense]",
                "lens.conf:2 expected key = value",
                "lens.conf:3 bad key/value 'fov = 1 2'",
                "lens.conf:4 bad key/value 'unknown = 3'",
            ]
        );
    }

    #[test]
    fn cant_deg_key_and_env() {
        let (c, w) = parse("cant_deg = 2.5\n[left]\ncant_deg = 3\n", ASPECT);
        assert!(w.is_empty());
        assert_eq!(c.cant_deg, 3.0);
        assert_eq!(parse("", ASPECT).0.cant_deg, 0.0);
        let (c, w) = parse("cant_deg = 2\ncant_deg = 99\ncant_deg = x\n", ASPECT);
        assert_eq!((c.cant_deg, w.len()), (2.0, 2));
        assert_eq!(cant_override(&|_| None), None);
        assert_eq!(cant_override(&|_| Some("2.95".into())), Some(Ok(2.95)));
        assert!(matches!(
            cant_override(&|_| Some("abc".into())),
            Some(Err(_))
        ));
        assert!(matches!(
            cant_override(&|_| Some("50".into())),
            Some(Err(_))
        ));
    }

    #[test]
    fn distortion_switch() {
        for off in ["off", "OFF", "0", "false", "no"] {
            let (c, _) = parse(&format!("distortion = {off}\n"), ASPECT);
            assert!(!c.enabled, "{off}");
        }
        for on in ["on", "1", "true", "yes", "anything"] {
            let (c, _) = parse(&format!("distortion = {on}\n"), ASPECT);
            assert!(c.enabled, "{on}");
        }
    }

    #[test]
    fn number_scanner_follows_strtof_for_decimals() {
        assert_eq!(scan_float("  -1.5e2 rest"), Some((-150.0, 8)));
        assert_eq!(scan_float(".5x"), Some((0.5, 2)));
        assert_eq!(scan_float("5."), Some((5.0, 2)));
        assert_eq!(scan_float("1e"), Some((1.0, 1)));
        assert_eq!(scan_float("1e+"), Some((1.0, 1)));
        assert_eq!(scan_float("+3"), Some((3.0, 2)));
        assert_eq!(scan_float("abc"), None);
        assert_eq!(scan_float("-"), None);
        assert_eq!(scan_float("nan"), None);
        assert_eq!(floats("1,2  3 ,4", 4, 4), Some(vec![1.0, 2.0, 3.0, 4.0]));
        assert_eq!(floats("1 2 x", 3, 3), None);
        assert_eq!(floats("1 2 3 4 5 6", 1, 3), Some(vec![1.0, 2.0, 3.0]));
    }

    #[test]
    fn new_per_channel_keys() {
        let (c, w) = parse(
            "base_r = 0.7\nbase_b = 0.6\nca_r = 1.01\nshift_b = 0.01 -0.02\nk_g = 0.1 0.2 0.3 0.4 0.5\n",
            ASPECT,
        );
        assert!(w.is_empty(), "{w:?}");
        let e = &c.eye[0];
        assert_eq!(e.base, [0.7, 0.65307, 0.6]);
        assert_eq!(e.ca[0], 1.01);
        assert_eq!(e.shift[2], [0.01, -0.02]);
        assert_eq!(e.k[1], [0.1, 0.2, 0.3, 0.4, 0.5]);
        // `base` after `base_r` resets all channels, later lines win
        let (c2, _) = parse("base_r = 0.7\nbase = 0.5\n", ASPECT);
        assert_eq!(c2.eye[0].base, [0.5; 3]);
    }

    #[test]
    fn mask_and_blackout_and_fit_range() {
        let (c, w) = parse(
            "fit_range = 1.5\nblackout = 0.02\nmask = ellipse 1.1 0.95\nmask_feather = 0.05\n[right]\nblackout = 0.01 0.02 0.03 0.04\nmask = rect 1 0.9\n",
            ASPECT,
        );
        assert!(w.is_empty(), "{w:?}");
        let l = &c.eye[0];
        assert_eq!(l.fit_range, 1.5);
        assert_eq!(l.blackout, [0.02; 4]);
        assert_eq!(
            l.mask,
            Mask {
                shape: MaskShape::Ellipse,
                a: 1.1,
                b: 0.95,
                n: 2.0
            }
        );
        assert_eq!(l.mask_feather, 0.05);
        assert!(l.has_mask());
        let r = &c.eye[1];
        assert_eq!(r.blackout, [0.01, 0.02, 0.03, 0.04]);
        assert_eq!(r.mask.shape, MaskShape::Rect);
        let (c3, w3) = parse("mask = superellipse 1.1 1.0 4\n", ASPECT);
        assert!(w3.is_empty());
        assert_eq!(
            c3.eye[0].mask,
            Mask {
                shape: MaskShape::Superellipse,
                a: 1.1,
                b: 1.0,
                n: 4.0
            }
        );
        let (c4, _) = parse("mask = ellipse 1\n", ASPECT);
        assert_eq!((c4.eye[0].mask.a, c4.eye[0].mask.b), (1.0, 1.0));
        let (c5, _) = parse("mask = ellipse 1\nmask = off\n", ASPECT);
        assert_eq!(c5.eye[0].mask, Mask::OFF);
        // invalid values are rejected and leave the setting alone
        for bad in [
            "mask = ellipse -1 1",
            "mask = blob 1 1",
            "blackout = -0.1",
            "blackout = 1 2",
            "fit_range = -1",
            "mask_feather = -1",
        ] {
            let (_, w) = parse(bad, ASPECT);
            assert_eq!(w.len(), 1, "{bad}");
        }
    }

    #[test]
    fn lut_keys() {
        let (c, w) = parse("lut = /a/b.bin\n[left]\nlut_margin = 0.3\nlut_interp = Bilinear\n[right]\nlut = none\n", 1.0);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.eye[0].lut.as_deref(), Some("/a/b.bin"));
        assert_eq!(c.eye[1].lut, None);
        assert_eq!(c.eye[0].lut_margin, 0.3);
        assert_eq!(c.eye[1].lut_margin, 0.0);
        assert_eq!(c.eye[0].lut_interp, crate::lut::Interp::Bilinear);
        assert_eq!(c.eye[1].lut_interp, crate::lut::Interp::Bicubic);
        let (_, w) = parse("lut =\nlut_margin = -1\nlut_interp = spline\n", 1.0);
        assert_eq!(w.len(), 3, "{w:?}");
    }

    #[test]
    fn implausible_values_are_rejected_and_leave_the_setting_alone() {
        for bad in [
            "fov = 0 50 44.5 44.5",
            "fov = 90 50 44.5 44.5",
            "fov = 50 50 44.5 -3",
            "center = 5 0",
            "base = 0",
            "base = -1",
            "base_r = 100",
            "scale = 0",
            "scale = 100",
            "ca_r = 0",
            "ca_g = -1",
            "ca_b = 3",
        ] {
            let (c, w) = parse(bad, ASPECT);
            assert_eq!(w.len(), 1, "{bad}");
            assert_eq!(c, LensConfig::defaults(ASPECT), "{bad}");
        }
        // the values of the old example and ordinary tunings still pass
        let (c, w) = parse("fov = 60 60 55 55\ncenter = -0.1 0.2\nbase = 0.7\nscale = 1.2\nca_r = 1.02\nca_b = 0.98\n", ASPECT);
        assert!(w.is_empty(), "{w:?}");
        assert_eq!(c.eye[0].fov, [60.0, 60.0, 55.0, 55.0]);
    }

    #[test]
    fn path_resolution_and_off_switch() {
        let env = |m: &'static [(&'static str, &'static str)]| {
            move |k: &str| m.iter().find(|(n, _)| *n == k).map(|(_, v)| v.to_string())
        };
        assert_eq!(
            config_path(&env(&[("PIMAX_LENS_CONF", "/x/y.conf"), ("HOME", "/h")])),
            PathBuf::from("/x/y.conf")
        );
        assert_eq!(
            config_path(&env(&[("XDG_CONFIG_HOME", "/xdg"), ("HOME", "/h")])),
            PathBuf::from("/xdg/omarchyvr/lens.conf")
        );
        assert_eq!(
            config_path(&env(&[("HOME", "/h")])),
            PathBuf::from("/h/.config/omarchyvr/lens.conf")
        );
        assert_eq!(
            config_path(&env(&[])),
            PathBuf::from("./.config/omarchyvr/lens.conf")
        );
        assert_eq!(
            config_path(&env(&[("PIMAX_LENS_CONF", ""), ("HOME", "/h")])),
            PathBuf::from("/h/.config/omarchyvr/lens.conf")
        );
        assert!(forced_off(&env(&[("PIMAX_LENS_OFF", "1")])));
        assert!(forced_off(&env(&[("PIMAX_LENS_OFF", "yes")])));
        assert!(!forced_off(&env(&[("PIMAX_LENS_OFF", "0")])));
        assert!(!forced_off(&env(&[("PIMAX_LENS_OFF", "")])));
        assert!(!forced_off(&env(&[])));
    }
}
