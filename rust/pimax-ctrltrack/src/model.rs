//! Controller LED model from Pimax's `controller_params.json` (Windows `ProgramData/Pimax/slam/H1/<serial>/`).
//!
//! 15 `ModelPoints` per controller, each `[x, y, z, nx, ny, nz]` in metres in the controller ("device") frame, the
//! unit LED normal, plus `ImuToDevice` (row-major 4x4) and `Hand`. Left and right are mirror images in x.

use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::json;
use std::path::Path;

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

    /// Default model files (left, right): `$PIMAX_CTRL_MODEL_DIR`, else `~/.config/pimax/controllers`, else the
    /// lab's Windows dump (`~/pimax-re/pimax-data/ProgramData-Pimax/slam/H1`). Each holds one folder per controller
    /// serial with a `controller_params.json` (the Pimax `slam/H1/<serial>/` layout); the two folders are taken in
    /// name order, as the tracker has always used them. Nothing per-unit is compiled in; with no folder found the
    /// paths point at `~/.config/pimax/controllers/{left,right}.json` so the error names where to put them.
    pub fn default_paths() -> [std::path::PathBuf; 2] {
        let home = std::env::var_os("HOME")
            .map(std::path::PathBuf::from)
            .unwrap_or_default();
        let mut dirs: Vec<std::path::PathBuf> = Vec::new();
        if let Some(d) = std::env::var_os("PIMAX_CTRL_MODEL_DIR").filter(|v| !v.is_empty()) {
            dirs.push(d.into());
        }
        dirs.push(home.join(".config/pimax/controllers"));
        dirs.push(home.join("pimax-re/pimax-data/ProgramData-Pimax/slam/H1"));
        for d in &dirs {
            if let Some(p) = Self::pair_in(d) {
                return p;
            }
        }
        let c = home.join(".config/pimax/controllers");
        [c.join("left.json"), c.join("right.json")]
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
        (v.len() >= 2).then(|| [v[0].clone(), v[1].clone()])
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
}
