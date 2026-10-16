//! Host-side accuracy correction of the combined gaze direction.
//!
//! The EyeChip's own calibration is never touched (no calibration opcode is ever sent, see [`crate::frames`]). The
//! error of the factory gaze is instead corrected on the host, in the head frame, by a smooth map fitted from a
//! measurement (`pimax-lenstest --patterns gaze`, the accuracy grid): a polynomial on the gaze *tangent*
//! `(u, v) = (x / -z, y / -z)` of a head-frame direction (OpenXR axes: x right, y up, -z forward):
//!
//! ```text
//! u' = cx . f(u, v)        v' = cy . f(u, v)        f(u, v) = (1, u, v, u*u, u*v, v*v)
//! ```
//!
//! An affine map has zero coefficients in the last three places. The corrected direction is `(u', v', -1)`
//! normalised. Directions that do not point forward (z >= 0) and tangents beyond [`MAX_TANGENT`] are left alone.
//!
//! # File (`~/.config/omarchyvr/eye-correction.toml`)
//!
//! ```toml
//! model = "affine"                       # or "quadratic" (informational)
//! x = [0.0, 1.0, 0.0, 0.0, 0.0, 0.0]     # cx; 3 numbers are padded with zeros
//! y = [0.0, 0.0, 1.0, 0.0, 0.0, 0.0]     # cy
//! flip = "0,0,1"                         # PIMAX_EYE_FLIP_X,Y,Z of the measurement (a warning if they differ later)
//! ```
//!
//! # Selecting
//!
//! `PIMAX_EYE_CORRECTION=<path>` loads that file, `none` (or `off`, `0`) disables it, unset loads the default path
//! when the file exists. See [`Correction::from_env`].

use crate::track::{FrameConv, Sample};
use std::path::{Path, PathBuf};

/// Tangents with a larger magnitude are not corrected (a polynomial is not trusted that far out: about 56 degrees).
pub const MAX_TANGENT: f64 = 1.5;

/// The feature vector `f(u, v)`.
pub fn features(u: f64, v: f64) -> [f64; 6] {
    [1.0, u, v, u * u, u * v, v * v]
}

/// Tangent of a forward-pointing direction (x right, y up, -z forward); `None` for a direction that does not point
/// forward or is not finite.
pub fn tangent_of(d: [f64; 3]) -> Option<(f64, f64)> {
    (d[2] < -1e-9 && d.iter().all(|c| c.is_finite())).then(|| (d[0] / -d[2], d[1] / -d[2]))
}

/// Unit direction of tangent (u, v).
pub fn dir_of(u: f64, v: f64) -> [f64; 3] {
    let n = (u * u + v * v + 1.0).sqrt();
    [u / n, v / n, -1.0 / n]
}

/// A fitted correction map.
#[derive(Debug, Clone, PartialEq)]
pub struct Correction {
    /// "affine" or "quadratic" (informational; the coefficients decide).
    pub model: String,
    /// Coefficients of the corrected u.
    pub cx: [f64; 6],
    /// Coefficients of the corrected v.
    pub cy: [f64; 6],
    /// `PIMAX_EYE_FLIP_X/Y/Z` in force when the measurement was taken, when recorded.
    pub flips: Option<[bool; 3]>,
}

impl Correction {
    /// The map that changes nothing.
    pub fn identity() -> Correction {
        Correction {
            model: "affine".into(),
            cx: [0.0, 1.0, 0.0, 0.0, 0.0, 0.0],
            cy: [0.0, 0.0, 1.0, 0.0, 0.0, 0.0],
            flips: None,
        }
    }

    /// True when the map is the identity.
    pub fn is_identity(&self) -> bool {
        let id = Correction::identity();
        self.cx == id.cx && self.cy == id.cy
    }

    /// The corrected tangent of tangent (u, v).
    pub fn map_tangent(&self, u: f64, v: f64) -> (f64, f64) {
        let f = features(u, v);
        let dot = |c: &[f64; 6]| c.iter().zip(f.iter()).map(|(a, b)| a * b).sum::<f64>();
        (dot(&self.cx), dot(&self.cy))
    }

    /// The corrected unit direction of the head-frame direction `d` (returned unchanged when it does not point
    /// forward or lies beyond [`MAX_TANGENT`]).
    pub fn apply(&self, d: [f64; 3]) -> [f64; 3] {
        match tangent_of(d) {
            Some((u, v)) if u.abs() <= MAX_TANGENT && v.abs() <= MAX_TANGENT => {
                let (u2, v2) = self.map_tangent(u, v);
                dir_of(u2, v2)
            }
            _ => d,
        }
    }

    /// [`Correction::apply`] on f32 directions.
    pub fn apply_f32(&self, d: [f32; 3]) -> [f32; 3] {
        let o = self.apply([f64::from(d[0]), f64::from(d[1]), f64::from(d[2])]);
        [o[0] as f32, o[1] as f32, o[2] as f32]
    }

    /// Correct the combined gaze direction and the valid per-eye directions of `s` (the origins stay).
    pub fn apply_sample(&self, s: &mut Sample) {
        if let Some(g) = s.gaze.as_mut() {
            g.direction = self.apply_f32(g.direction);
        }
        for e in s.eyes.iter_mut().filter(|e| e.direction_valid) {
            e.direction = self.apply_f32(e.direction);
        }
    }

    /// The file text; `notes` become leading comment lines.
    pub fn to_toml(&self, notes: &[String]) -> String {
        let mut s = String::from(
            "# pimax eye gaze correction (host side; the eye chip's calibration is not touched)\n",
        );
        for n in notes {
            s.push_str("# ");
            s.push_str(n);
            s.push('\n');
        }
        s.push_str("# u' = x . (1, u, v, u*u, u*v, v*v), v' = y . (...) with (u, v) = (x / -z, y / -z) of the head-frame gaze\n");
        s.push_str(&format!("model = \"{}\"\n", self.model));
        let arr = |c: &[f64; 6]| {
            let v: Vec<String> = c.iter().map(|x| format!("{x:.9}")).collect();
            format!("[{}]", v.join(", "))
        };
        s.push_str(&format!("x = {}\ny = {}\n", arr(&self.cx), arr(&self.cy)));
        if let Some(f) = self.flips {
            s.push_str(&format!(
                "flip = \"{},{},{}\"\n",
                u8::from(f[0]),
                u8::from(f[1]),
                u8::from(f[2])
            ));
        }
        s
    }

    /// Parse the file text.
    pub fn parse(text: &str) -> Result<Correction, String> {
        let (mut model, mut cx, mut cy, mut flips) = (None, None, None, None);
        for (n, raw) in text.lines().enumerate() {
            let line = raw.split('#').next().unwrap_or("").trim();
            if line.is_empty() {
                continue;
            }
            let (k, v) = line
                .split_once('=')
                .ok_or(format!("line {}: expected key = value", n + 1))?;
            let (k, v) = (k.trim(), v.trim());
            let unquote = |s: &str| s.trim_matches('"').trim().to_string();
            let nums = |s: &str| -> Result<[f64; 6], String> {
                let inner = s
                    .strip_prefix('[')
                    .and_then(|s| s.strip_suffix(']'))
                    .ok_or(format!("line {}: {k} must be [a, b, ...]", n + 1))?;
                let v: Vec<f64> = inner
                    .split(',')
                    .filter(|p| !p.trim().is_empty())
                    .map(|p| p.trim().parse::<f64>())
                    .collect::<Result<_, _>>()
                    .map_err(|e| format!("line {}: {k}: {e}", n + 1))?;
                if !(v.len() == 3 || v.len() == 6) || v.iter().any(|x| !x.is_finite()) {
                    return Err(format!("line {}: {k} needs 3 or 6 finite numbers", n + 1));
                }
                let mut o = [0.0; 6];
                o[..v.len()].copy_from_slice(&v);
                Ok(o)
            };
            match k {
                "model" => model = Some(unquote(v)),
                "x" => cx = Some(nums(v)?),
                "y" => cy = Some(nums(v)?),
                "flip" => {
                    let p: Vec<bool> = unquote(v)
                        .split(',')
                        .map(|t| matches!(t.trim(), "1" | "true"))
                        .collect();
                    if p.len() == 3 {
                        flips = Some([p[0], p[1], p[2]]);
                    }
                }
                _ => {}
            }
        }
        let (cx, cy) = (
            cx.ok_or("missing x = [...]")?,
            cy.ok_or("missing y = [...]")?,
        );
        Ok(Correction {
            model: model.unwrap_or_else(|| "affine".into()),
            cx,
            cy,
            flips,
        })
    }

    /// Load a file.
    pub fn load(path: &Path) -> Result<Correction, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        Correction::parse(&t).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// `~/.config/omarchyvr/eye-correction.toml` (`None` without `$HOME`).
    pub fn default_path() -> Option<PathBuf> {
        default_path_in(std::env::var_os("HOME").map(PathBuf::from))
    }

    /// Selection by the value of `PIMAX_EYE_CORRECTION` (see the module docs): `Ok(None)` for "no correction".
    pub fn select(var: Option<&str>, home: Option<PathBuf>) -> Result<Option<Correction>, String> {
        match var.map(str::trim) {
            Some("none" | "off" | "0") => Ok(None),
            Some(p) if !p.is_empty() => Correction::load(Path::new(p)).map(Some),
            _ => match default_path_in(home) {
                Some(p) if p.exists() => Correction::load(&p).map(Some),
                _ => Ok(None),
            },
        }
    }

    /// [`Correction::select`] with the real environment.
    pub fn from_env() -> Result<Option<Correction>, String> {
        Correction::select(
            std::env::var("PIMAX_EYE_CORRECTION").ok().as_deref(),
            std::env::var_os("HOME").map(PathBuf::from),
        )
    }

    /// A warning when the file was measured with other `PIMAX_EYE_FLIP_*` settings than `conv`.
    pub fn flip_warning(&self, conv: &FrameConv) -> Option<String> {
        let f = self.flips?;
        let now = [conv.flip_x, conv.flip_y, conv.flip_z];
        (f != now).then(|| {
            format!(
                "eye correction was measured with PIMAX_EYE_FLIP_X/Y/Z = {}/{}/{} but they are {}/{}/{} now: the correction is probably wrong, measure again",
                u8::from(f[0]), u8::from(f[1]), u8::from(f[2]),
                u8::from(now[0]), u8::from(now[1]), u8::from(now[2])
            )
        })
    }
}

fn default_path_in(home: Option<PathBuf>) -> Option<PathBuf> {
    home.map(|h| h.join(".config/omarchyvr/eye-correction.toml"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::track::{EyeRay, Gaze};

    fn shifted() -> Correction {
        Correction {
            model: "affine".into(),
            cx: [0.02, 1.05, 0.01, 0.0, 0.0, 0.0],
            cy: [-0.03, -0.02, 0.95, 0.0, 0.0, 0.0],
            flips: Some([false, true, true]),
        }
    }

    #[test]
    fn identity_changes_nothing_and_is_recognised() {
        let c = Correction::identity();
        assert!(c.is_identity() && !shifted().is_identity());
        for d in [[0.0, 0.0, -1.0], dir_of(0.4, -0.3), dir_of(-1.2, 0.9)] {
            let o = c.apply(d);
            for k in 0..3 {
                assert!((o[k] - d[k]).abs() < 1e-12, "{d:?} {o:?}");
            }
        }
    }

    #[test]
    fn affine_moves_tangents_as_written() {
        let c = shifted();
        let (u, v) = c.map_tangent(0.5, -0.25);
        assert!((u - (0.02 + 1.05 * 0.5 + 0.01 * -0.25)).abs() < 1e-12);
        assert!((v - (-0.03 - 0.02 * 0.5 + 0.95 * -0.25)).abs() < 1e-12);
        let o = c.apply(dir_of(0.5, -0.25));
        let (ou, ov) = tangent_of(o).unwrap();
        assert!((ou - u).abs() < 1e-12 && (ov - v).abs() < 1e-12);
        let n = o.iter().map(|x| x * x).sum::<f64>().sqrt();
        assert!((n - 1.0).abs() < 1e-12);
    }

    #[test]
    fn backward_and_far_out_directions_are_left_alone() {
        let c = shifted();
        let back = [0.0, 0.0, 1.0];
        assert_eq!(c.apply(back), back);
        let sideways = [1.0, 0.0, 0.0];
        assert_eq!(c.apply(sideways), sideways);
        let far = dir_of(2.0, 0.0);
        assert_eq!(c.apply(far), far);
        assert_eq!(c.apply([f64::NAN, 0.0, -1.0]).len(), 3);
    }

    #[test]
    fn toml_round_trip_and_padding() {
        let c = shifted();
        let t = c.to_toml(&["mean error 2.1 -> 0.7 deg".into()]);
        assert!(t.starts_with("# pimax eye gaze correction"));
        assert!(t.contains("# mean error 2.1 -> 0.7 deg"));
        let back = Correction::parse(&t).unwrap();
        for k in 0..6 {
            assert!((back.cx[k] - c.cx[k]).abs() < 1e-9 && (back.cy[k] - c.cy[k]).abs() < 1e-9);
        }
        assert_eq!(back.flips, Some([false, true, true]));
        assert_eq!(back.model, "affine");
        let short = Correction::parse("x = [0, 1, 0]\ny = [0, 0, 1]  # identity\n").unwrap();
        assert!(short.is_identity());
    }

    #[test]
    fn parse_errors_are_specific() {
        assert!(Correction::parse("x = [0, 1, 0]")
            .unwrap_err()
            .contains("missing y"));
        assert!(Correction::parse("x = [1, 2]\ny = [0, 0, 1]")
            .unwrap_err()
            .contains("3 or 6"));
        assert!(Correction::parse("x = 1\ny = [0,0,1]")
            .unwrap_err()
            .contains("[a, b"));
        assert!(Correction::parse("garbage")
            .unwrap_err()
            .contains("key = value"));
        assert!(Correction::parse("x = [a, b, c]\ny = [0,0,1]").is_err());
    }

    #[test]
    fn select_follows_the_variable_and_the_default_file() {
        let dir = std::env::temp_dir().join(format!("pimax-corr-{}", std::process::id()));
        let cfgdir = dir.join(".config/omarchyvr");
        std::fs::create_dir_all(&cfgdir).unwrap();
        let home = Some(dir.clone());
        // no file, no variable
        assert_eq!(Correction::select(None, home.clone()).unwrap(), None);
        std::fs::write(cfgdir.join("eye-correction.toml"), shifted().to_toml(&[])).unwrap();
        let d = Correction::select(None, home.clone()).unwrap().unwrap();
        assert!((d.cx[1] - 1.05).abs() < 1e-9);
        // "none" disables even when the file exists; an empty value counts as unset
        assert_eq!(
            Correction::select(Some("none"), home.clone()).unwrap(),
            None
        );
        assert_eq!(Correction::select(Some("off"), home.clone()).unwrap(), None);
        assert!(Correction::select(Some(""), home.clone())
            .unwrap()
            .is_some());
        // an explicit path wins, and a missing one is an error (not silently no correction)
        std::fs::write(dir.join("other.toml"), Correction::identity().to_toml(&[])).unwrap();
        let o = Correction::select(Some(dir.join("other.toml").to_str().unwrap()), home.clone())
            .unwrap()
            .unwrap();
        assert!(o.is_identity());
        assert!(Correction::select(Some("/nonexistent/x.toml"), home).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn samples_are_corrected_in_gaze_and_valid_eyes_only() {
        let c = shifted();
        let d = dir_of(0.3, 0.1);
        let df = [d[0] as f32, d[1] as f32, d[2] as f32];
        let mut s = Sample {
            gaze: Some(Gaze {
                origin: [0.0, 0.0, 0.01],
                direction: df,
                eyes: 3,
            }),
            ..Sample::default()
        };
        s.eyes[0] = EyeRay {
            origin: [-0.03, 0.0, 0.0],
            direction: df,
            origin_valid: true,
            direction_valid: true,
            pupil_mm: 4.0,
            ..EyeRay::default()
        };
        s.eyes[1] = EyeRay {
            direction: df,
            ..EyeRay::default()
        };
        c.apply_sample(&mut s);
        let want = c.apply_f32(df);
        assert_eq!(s.gaze.unwrap().direction, want);
        assert_eq!(s.gaze.unwrap().origin, [0.0, 0.0, 0.01]);
        assert_eq!(s.eyes[0].direction, want);
        assert_eq!(s.eyes[1].direction, df, "an invalid eye keeps what it had");
        assert_eq!(s.eyes[0].origin, [-0.03, 0.0, 0.0]);
    }

    #[test]
    fn flip_warning_only_when_they_differ() {
        let c = shifted();
        let mut conv = FrameConv {
            flip_y: true, // measured with 0,1,1
            ..FrameConv::default()
        };
        assert!(c.flip_warning(&conv).is_none());
        conv.flip_y = false;
        assert!(c.flip_warning(&conv).unwrap().contains("0/1/1"));
        assert!(Correction::identity().flip_warning(&conv).is_none());
    }
}
