//! Controller LED model from Pimax's `controller_params.json` (Windows `ProgramData/Pimax/slam/H1/<serial>/`).
//!
//! 15 `ModelPoints` per controller, each `[x, y, z, nx, ny, nz]` in metres in the controller ("device") frame, the
//! unit LED normal, plus `ImuToDevice` (row-major 4x4) and `Hand`. Left and right are mirror images in x.

use pimax_cam::calib::reference_allowed;
use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::json;
use std::path::{Path, PathBuf};

/// The REFERENCE controller LED models of the Pimax Dream Air, shipped for everyone (user decision 2026-10-17,
/// "approve shipping calibrations"): Pimax's own `controller_params.json` of the project's Windows install
/// (`ProgramData/Pimax/slam/H1/<serial>/`), the serial folders renamed to left/right in the order the tracker always
/// used them. The per-serial files equal Pimax's default `test_0`/`test_1` models (rounding only, no per-unit LED
/// calibration; analysis/controller-6dof.md), so the reference fits any Dream Air controller pair.
pub const REFERENCE_LEFT: &str = include_str!("../../pimax-device/devices/dream-air/reference/controllers/left.json");
/// See [`REFERENCE_LEFT`].
pub const REFERENCE_RIGHT: &str = include_str!("../../pimax-device/devices/dream-air/reference/controllers/right.json");

/// Where the LED models in use came from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModelSource {
    /// The user's own files (imported from Pimax's Windows install or a previous install, or given with --left/--right).
    Files([PathBuf; 2]),
    /// The shipped Dream Air reference ([`REFERENCE_LEFT`], [`REFERENCE_RIGHT`]).
    Reference,
}

impl ModelSource {
    /// One plain line for the logs and the app.
    pub fn describe(&self) -> String {
        match self {
            ModelSource::Files([l, r]) => format!("your own controller models ({} / {})", l.display(), r.display()),
            ModelSource::Reference => "the shipped Pimax Dream Air reference controller models (no own copy found; the app imports yours from a Pimax Windows install)".into(),
        }
    }
}

#[derive(Clone, Debug)]
pub struct Led {
    pub p: Vec3,
    pub n: Vec3,
}

#[derive(Clone, Debug)]
pub struct LedModel {
    pub hand: String,
    pub serial: String,
    pub leds: Vec<Led>,
    /// `device_from_imu` (`ImuToDevice`).
    pub imu_to_device: Pose,
}

impl LedModel {
    pub fn parse(text: &str) -> Result<LedModel, String> {
        let v = json::parse(text).map_err(|e| format!("json: {e:?}"))?;
        let mp = v.get("ModelPoints").ok_or("no ModelPoints")?;
        let mut leds = Vec::new();
        for i in 0.. {
            let Some(p) = mp.get(&format!("Point{i}")) else {
                break;
            };
            let a: [f64; 6] = p.as_f64_array().ok_or("ModelPoint is not 6 numbers")?;
            leds.push(Led {
                p: [a[0], a[1], a[2]],
                n: geom::normalize([a[3], a[4], a[5]]),
            });
        }
        if leds.len() < 4 {
            return Err(format!("only {} LEDs", leds.len()));
        }
        let m: [f64; 16] = v
            .get("ImuToDevice")
            .and_then(|m| m.as_f64_array())
            .ok_or("no ImuToDevice")?;
        let r: Mat3 = [[m[0], m[1], m[2]], [m[4], m[5], m[6]], [m[8], m[9], m[10]]];
        Ok(LedModel {
            hand: v
                .get("Hand")
                .and_then(|h| h.as_str())
                .unwrap_or("?")
                .to_string(),
            serial: v
                .get("SN")
                .and_then(|h| h.as_str())
                .unwrap_or("?")
                .to_string(),
            leds,
            imu_to_device: Pose {
                r,
                t: [m[3], m[7], m[11]],
            },
        })
    }

    pub fn load(path: &Path) -> Result<LedModel, String> {
        let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        LedModel::parse(&t).map_err(|e| format!("{}: {e}", path.display()))
    }

    /// The directories searched for the user's own model files, in order: `$PIMAX_CTRL_MODEL_DIR`, else
    /// `~/.config/omarchyvr/controllers` (where the app imports them), else `~/.local/share/omarchyvr/controller-models`,
    /// else the Windows dump in the repo (`<repo>/pimax-data/ProgramData-Pimax/slam/H1`; repo = `$PIMAX_REPO`, the
    /// installed data copy, or `~/omarchyvr`). Each holds `left.json` + `right.json` or one folder per controller serial
    /// with a `controller_params.json` (the Pimax `slam/H1/<serial>/` layout; the two folders are taken in name order,
    /// as the tracker has always used them).
    pub fn search_dirs(env: &dyn Fn(&str) -> Option<String>) -> Vec<PathBuf> {
        let home = env("HOME").map(PathBuf::from).unwrap_or_default();
        let mut dirs: Vec<PathBuf> = Vec::new();
        if let Some(d) = env("PIMAX_CTRL_MODEL_DIR").filter(|v| !v.is_empty()) {
            dirs.push(d.into());
        }
        dirs.push(home.join(".config/omarchyvr/controllers"));
        dirs.push(home.join(".local/share/omarchyvr/controller-models"));
        // the repo as omarchyvr_config::paths::repo_dir finds it (inline: this crate stays free of the app's crates)
        let repo = env("PIMAX_REPO").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
            let installed = home.join(".local/share/omarchyvr/data");
            if installed.join("docs/LESSONS.md").is_file() { installed } else { home.join("omarchyvr") }
        });
        dirs.push(repo.join("pimax-data/ProgramData-Pimax/slam/H1"));
        dirs
    }

    /// The user's own model files (left, right), if any: the first pair in [`LedModel::search_dirs`].
    pub fn own_paths(env: &dyn Fn(&str) -> Option<String>) -> Option<[PathBuf; 2]> {
        Self::search_dirs(env).iter().find_map(|d| Self::pair_in(d))
    }

    /// Default model files (left, right): the user's own ([`LedModel::own_paths`]); with none found the paths point
    /// at `~/.config/omarchyvr/controllers/{left,right}.json` so an error names where they go. [`LedModel::default_models`]
    /// falls back to the shipped reference instead.
    pub fn default_paths() -> [PathBuf; 2] {
        let env = |k: &str| std::env::var(k).ok();
        Self::own_paths(&env).unwrap_or_else(|| {
            let c = PathBuf::from(env("HOME").unwrap_or_default()).join(".config/omarchyvr/controllers");
            [c.join("left.json"), c.join("right.json")]
        })
    }

    /// The models the tracker uses: `left`/`right` when given, else the user's own ([`LedModel::own_paths`]), else the
    /// shipped reference (unless `PIMAX_NO_REFERENCE=1`). An unreadable or broken own file is an error (never a panic).
    pub fn resolve(
        left: Option<&Path>,
        right: Option<&Path>,
        env: &dyn Fn(&str) -> Option<String>,
    ) -> Result<([LedModel; 2], ModelSource), String> {
        let own = Self::own_paths(env);
        let pick = |given: Option<&Path>, i: usize| given.map(Path::to_path_buf).or_else(|| own.as_ref().map(|p| p[i].clone()));
        match (pick(left, 0), pick(right, 1)) {
            (Some(l), Some(r)) => {
                let models = [LedModel::load(&l)?, LedModel::load(&r)?];
                Ok((models, ModelSource::Files([l, r])))
            }
            (None, None) if reference_allowed(env) => Ok(([LedModel::parse(REFERENCE_LEFT)?, LedModel::parse(REFERENCE_RIGHT)?], ModelSource::Reference)),
            (l, r) => {
                let c = PathBuf::from(env("HOME").unwrap_or_default()).join(".config/omarchyvr/controllers");
                let missing = if l.is_none() { c.join("left.json") } else { r.unwrap_or_else(|| c.join("right.json")) };
                Err(format!(
                    "controller models missing ({}: no such file); import them in the app (Room setup) or from a Pimax Windows install",
                    missing.display()
                ))
            }
        }
    }

    /// The first two `<dir>/<serial>/controller_params.json` in folder-name order (or `left.json`/`right.json`).
    pub fn pair_in(dir: &Path) -> Option<[std::path::PathBuf; 2]> {
        let (l, r) = (dir.join("left.json"), dir.join("right.json"));
        if l.is_file() && r.is_file() {
            return Some([l, r]);
        }
        let mut v: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
            .ok()?
            .filter_map(|e| e.ok().map(|e| e.path().join("controller_params.json")))
            .filter(|p| p.is_file())
            .collect();
        v.sort();
        if v.len() < 2 {
            return None;
        }
        // folder-name order is what the tracker always used; the files' own `Hand` field wins when it says otherwise
        let hand = |p: &Path| std::fs::read_to_string(p).ok().and_then(|t| LedModel::parse(&t).ok()).map(|m| m.hand.to_ascii_lowercase());
        if hand(&v[0]).as_deref() == Some("right") && hand(&v[1]).as_deref() == Some("left") {
            v.swap(0, 1);
        }
        Some([v[0].clone(), v[1].clone()])
    }

    /// Ordered LED triples that can be seen together and are close: used as P3P hypotheses.
    /// `max_dist` metres between any two, normals pairwise within `max_normal_deg`.
    pub fn neighbour_triples(&self, max_dist: f64, max_normal_deg: f64) -> Vec<[usize; 3]> {
        let n = self.leds.len();
        let ok = |a: usize, b: usize| {
            let la = &self.leds[a];
            let lb = &self.leds[b];
            geom::norm(geom::sub(la.p, lb.p)) <= max_dist
                && geom::angle_deg(la.n, lb.n) <= max_normal_deg
        };
        let mut out = Vec::new();
        for i in 0..n {
            for j in 0..n {
                for k in 0..n {
                    if i != j && j != k && i != k && ok(i, j) && ok(j, k) && ok(i, k) {
                        out.push([i, j, k]);
                    }
                }
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_windows_file() {
        let t = r#"{"Hand":"Left","ModelPoints":{"Point0":[0.01,0,0,1,0,0],"Point1":[0,0.01,0,0,2,0],
            "Point2":[0,0,0.01,0,0,1],"Point3":[0.01,0.01,0,1,0,0]},
            "ImuToDevice":[1,0,0,0.5, 0,1,0,0, 0,0,1,0, 0,0,0,1],"SN":"x"}"#;
        let m = LedModel::parse(t).unwrap();
        assert_eq!(m.leds.len(), 4);
        assert_eq!(m.hand, "Left");
        assert!((m.leds[1].n[1] - 1.0).abs() < 1e-12);
        assert_eq!(m.imu_to_device.t, [0.5, 0.0, 0.0]);
        assert_eq!(m.neighbour_triples(1.0, 180.0).len(), 24);
    }

    #[test]
    fn the_shipped_reference_models_are_a_mirrored_left_right_pair() {
        let l = LedModel::parse(REFERENCE_LEFT).unwrap();
        let r = LedModel::parse(REFERENCE_RIGHT).unwrap();
        assert_eq!((l.hand.as_str(), r.hand.as_str()), ("Left", "Right"));
        assert_eq!((l.leds.len(), r.leds.len()), (15, 15));
        // left and right are mirror images in x (analysis/controller-6dof.md)
        for (a, b) in l.leds.iter().zip(&r.leds) {
            assert!((a.p[0] + b.p[0]).abs() < 1e-6 && (a.p[1] - b.p[1]).abs() < 1e-6 && (a.p[2] - b.p[2]).abs() < 1e-6);
        }
    }

    #[test]
    fn the_users_own_models_win_and_the_reference_is_the_fallback() {
        let home = std::env::temp_dir().join(format!("pimax-ctrl-models-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let h = home.display().to_string();
        let env = |k: &str| (k == "HOME").then(|| h.clone());
        // nothing of the user's own: the shipped reference
        let (m, src) = LedModel::resolve(None, None, &env).unwrap();
        assert_eq!(src, ModelSource::Reference);
        assert_eq!((m[0].hand.as_str(), m[1].hand.as_str()), ("Left", "Right"));
        assert!(src.describe().contains("reference"));
        // reference switched off: a plain error that names where the files go (never a panic)
        let off = |k: &str| match k {
            "HOME" => Some(h.clone()),
            "PIMAX_NO_REFERENCE" => Some("1".to_string()),
            _ => None,
        };
        let e = LedModel::resolve(None, None, &off).unwrap_err();
        assert!(e.contains("controller models missing") && e.contains(".config/omarchyvr/controllers/left.json"), "{e}");
        // the user's own, in Pimax's layout (one folder per controller serial; name order = left, right)
        let dir = home.join(".config/omarchyvr/controllers");
        for (sn, text) in [("A1", REFERENCE_LEFT), ("B2", REFERENCE_RIGHT)] {
            std::fs::create_dir_all(dir.join(sn)).unwrap();
            std::fs::write(dir.join(sn).join("controller_params.json"), text).unwrap();
        }
        let (m, src) = LedModel::resolve(None, None, &env).unwrap();
        assert_eq!(src, ModelSource::Files([dir.join("A1/controller_params.json"), dir.join("B2/controller_params.json")]));
        assert_eq!(m[1].hand, "Right");
        // an explicit --left wins over the folder
        let l = home.join("given-left.json");
        std::fs::write(&l, REFERENCE_LEFT).unwrap();
        let (_, src) = LedModel::resolve(Some(&l), None, &env).unwrap();
        assert_eq!(src, ModelSource::Files([l.clone(), dir.join("B2/controller_params.json")]));
        // a broken own file is an error, not a silent fallback
        std::fs::write(dir.join("A1/controller_params.json"), "{").unwrap();
        assert!(LedModel::resolve(None, None, &env).is_err());
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_hand_field_wins_over_the_folder_order() {
        let dir = std::env::temp_dir().join(format!("pimax-ctrl-hand-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        for (sn, text) in [("A", REFERENCE_RIGHT), ("B", REFERENCE_LEFT)] {
            std::fs::create_dir_all(dir.join(sn)).unwrap();
            std::fs::write(dir.join(sn).join("controller_params.json"), text).unwrap();
        }
        let [l, r] = LedModel::pair_in(&dir).unwrap();
        assert_eq!((l, r), (dir.join("B/controller_params.json"), dir.join("A/controller_params.json")));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
