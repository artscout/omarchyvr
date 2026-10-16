//! Learned calibration overlay: small corrections on top of the factory calibration, kept in a separate file
//! (default `~/.config/pimax/cam-calib-overlay.json`) so the factory blob is never modified.
//!
//! Version 1 holds per-camera **rotation corrections about the camera centre**, the output of `pimax-selfcal`.
//! Semantics (rig convention `X_i = R X_0 + t`, the verified one): for camera `i` with rotation vector `w_i`
//! (radians, OpenCV camera axes) the corrected rig is `R_i' = Exp(w_i) R_i`, `t_i' = Exp(w_i) t_i`, i.e. every
//! `cam_i_from_X` gains a left factor `Exp(w_i)` and the camera centre stays where it is. Camera-IMU terms
//! (`ombc`, `tbc`), intrinsics and translations are unchanged.
//!
//! Safety: [`DeviceCalib::with_overlay`] refuses an overlay made for another headset (device UID or factory
//! calibration date), with unknown camera ids or with a correction above `max_angle_deg`. Callers then fall back
//! to the factory calibration. [`save`] keeps the previous overlay as `<file>.prev` for [`rollback`].
//! Nothing loads the overlay implicitly: callers opt in (`pimax-cam ... --overlay FILE`).

use crate::calib::{CalibError, DeviceCalib};
use crate::geom::{self, Vec3};
use crate::json::{self, Value};
use std::path::{Path, PathBuf};

/// `format` field of the file.
pub const FORMAT: &str = "pimax-cam-calib-overlay";
/// Current version.
pub const VERSION: u32 = 1;
/// Default refusal limit for one camera's correction (degrees).
pub const DEFAULT_MAX_ANGLE_DEG: f64 = 3.0;

/// Rotation correction of one camera.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraRotation {
    /// Camera id (0..3).
    pub id: usize,
    /// Rotation vector `w` (radians, camera axes): `cam_from_X' = Exp(w) cam_from_X`.
    pub rotation_vector: Vec3,
    /// One-sigma uncertainty per component (radians), if the estimator gave one.
    pub sigma: Option<Vec3>,
}

impl CameraRotation {
    /// Correction angle in degrees.
    pub fn angle_deg(&self) -> f64 {
        geom::norm(self.rotation_vector).to_degrees()
    }
}

/// The overlay document.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibOverlay {
    /// Headset serial the overlay belongs to (`deviceUID` of the factory XML). Empty: any headset.
    pub device_uid: String,
    /// `calibration_date` of the factory XML it was estimated against. Empty: not checked.
    pub factory_date: String,
    /// Seconds since the Unix epoch when it was written.
    pub created_unix: u64,
    /// Producer (tool, version, command).
    pub tool: String,
    /// Camera held fixed by the estimator (the gauge).
    pub reference_camera: usize,
    /// Per-camera rotation corrections.
    pub rotations: Vec<CameraRotation>,
    /// Numeric quality figures (free-form, for humans and sanity checks).
    pub quality: Vec<(String, f64)>,
    /// Free text.
    pub note: String,
}

/// Overlay errors.
#[derive(Debug)]
pub enum OverlayError {
    /// File I/O.
    Io(PathBuf, std::io::Error),
    /// Not JSON.
    Json(json::JsonError),
    /// JSON but not a valid overlay.
    Format(String),
}

impl std::fmt::Display for OverlayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OverlayError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            OverlayError::Json(e) => write!(f, "overlay: {e}"),
            OverlayError::Format(s) => write!(f, "overlay: {s}"),
        }
    }
}

impl std::error::Error for OverlayError {}

fn ferr<T>(s: impl Into<String>) -> Result<T, OverlayError> {
    Err(OverlayError::Format(s.into()))
}

impl CalibOverlay {
    /// An empty overlay for `calib` (no corrections).
    pub fn for_calib(calib: &DeviceCalib, tool: &str) -> CalibOverlay {
        CalibOverlay {
            device_uid: calib.uid.clone(),
            factory_date: calib.date.clone(),
            created_unix: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
            tool: tool.to_string(),
            reference_camera: 0,
            rotations: Vec::new(),
            quality: Vec::new(),
            note: String::new(),
        }
    }

    /// Rotation of camera `id` (zero if absent).
    pub fn rotation(&self, id: usize) -> Vec3 {
        self.rotations
            .iter()
            .find(|r| r.id == id)
            .map(|r| r.rotation_vector)
            .unwrap_or([0.0; 3])
    }

    /// Largest correction angle (degrees).
    pub fn max_angle_deg(&self) -> f64 {
        self.rotations
            .iter()
            .map(|r| r.angle_deg())
            .fold(0.0, f64::max)
    }

    /// Parse the JSON text.
    pub fn parse(text: &str) -> Result<CalibOverlay, OverlayError> {
        let v = json::parse(text).map_err(OverlayError::Json)?;
        if v.get("format").and_then(Value::as_str) != Some(FORMAT) {
            return ferr(format!("\"format\" is not \"{FORMAT}\""));
        }
        let ver = v.get("version").and_then(Value::as_f64).unwrap_or(0.0);
        if ver != VERSION as f64 {
            return ferr(format!("version {ver}, this reader knows {VERSION}"));
        }
        let s = |k: &str| v.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let mut rotations = Vec::new();
        for c in v
            .get("cameras")
            .and_then(Value::as_array)
            .unwrap_or_default()
        {
            let id = c
                .get("id")
                .and_then(Value::as_f64)
                .filter(|x| x.fract() == 0.0 && (0.0..16.0).contains(x))
                .ok_or_else(|| OverlayError::Format("camera without a valid id".into()))?
                as usize;
            let w = c
                .get("rotation_vector")
                .and_then(Value::as_f64_array::<3>)
                .ok_or_else(|| {
                    OverlayError::Format(format!("camera {id}: rotation_vector needs 3 numbers"))
                })?;
            if rotations.iter().any(|r: &CameraRotation| r.id == id) {
                return ferr(format!("camera {id} listed twice"));
            }
            rotations.push(CameraRotation {
                id,
                rotation_vector: w,
                sigma: c.get("sigma").and_then(Value::as_f64_array::<3>),
            });
        }
        let quality = match v.get("quality") {
            Some(Value::Obj(m)) => m
                .iter()
                .filter_map(|(k, x)| x.as_f64().map(|f| (k.clone(), f)))
                .collect(),
            _ => Vec::new(),
        };
        Ok(CalibOverlay {
            device_uid: s("device_uid"),
            factory_date: s("factory_calibration_date"),
            created_unix: v
                .get("created_unix")
                .and_then(Value::as_f64)
                .filter(|x| *x >= 0.0)
                .unwrap_or(0.0) as u64,
            tool: s("tool"),
            reference_camera: v
                .get("reference_camera")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as usize,
            rotations,
            quality,
            note: s("note"),
        })
    }

    /// Serialise (stable, human-readable layout).
    pub fn to_json(&self) -> String {
        let mut o = String::from("{\n");
        o += &format!("  \"format\": {},\n", json::quote(FORMAT));
        o += &format!("  \"version\": {VERSION},\n");
        o += &format!("  \"device_uid\": {},\n", json::quote(&self.device_uid));
        o += &format!(
            "  \"factory_calibration_date\": {},\n",
            json::quote(&self.factory_date)
        );
        o += &format!("  \"created_unix\": {},\n", self.created_unix);
        o += &format!("  \"tool\": {},\n", json::quote(&self.tool));
        o += &format!("  \"reference_camera\": {},\n", self.reference_camera);
        o += "  \"convention\": \"camera rotated about its own centre: cam_from_X' = Exp(rotation_vector) * cam_from_X; radians, OpenCV camera axes; rig X_i = R X_0 + t\",\n";
        o += "  \"cameras\": [";
        for (n, r) in self.rotations.iter().enumerate() {
            o += if n == 0 { "\n" } else { ",\n" };
            o += &format!(
                "    {{ \"id\": {}, \"rotation_vector\": {}, \"angle_deg\": {}",
                r.id,
                json::num_array(&r.rotation_vector),
                json::num((r.angle_deg() * 1e5).round() / 1e5)
            );
            if let Some(sg) = r.sigma {
                o += &format!(", \"sigma\": {}", json::num_array(&sg));
            }
            o += " }";
        }
        o += if self.rotations.is_empty() {
            "],\n"
        } else {
            "\n  ],\n"
        };
        o += "  \"quality\": {";
        for (n, (k, x)) in self.quality.iter().enumerate() {
            o += if n == 0 { "\n" } else { ",\n" };
            o += &format!("    {}: {}", json::quote(k), json::num(*x));
        }
        o += if self.quality.is_empty() {
            "},\n"
        } else {
            "\n  },\n"
        };
        o += &format!("  \"note\": {}\n}}\n", json::quote(&self.note));
        o
    }

    /// Read and parse a file.
    pub fn load(path: &Path) -> Result<CalibOverlay, OverlayError> {
        let t = std::fs::read_to_string(path).map_err(|e| OverlayError::Io(path.into(), e))?;
        CalibOverlay::parse(&t)
    }

    /// Why this overlay must not be applied to `calib` (`None`: it may be applied).
    pub fn refusal(&self, calib: &DeviceCalib, max_angle_deg: f64) -> Option<String> {
        if !self.device_uid.is_empty() && self.device_uid != calib.uid {
            return Some(format!(
                "made for headset {}, this calibration is {}",
                self.device_uid, calib.uid
            ));
        }
        if !self.factory_date.is_empty() && self.factory_date != calib.date {
            return Some(format!(
                "made against the factory calibration of {}, this one is from {}",
                self.factory_date, calib.date
            ));
        }
        for r in &self.rotations {
            if r.id >= calib.cameras.len() {
                return Some(format!("camera {} does not exist", r.id));
            }
            if r.rotation_vector.iter().any(|x| !x.is_finite()) {
                return Some(format!("camera {}: not a number", r.id));
            }
            if r.angle_deg() > max_angle_deg {
                return Some(format!(
                    "camera {}: correction {:.2} deg exceeds the {max_angle_deg} deg limit",
                    r.id,
                    r.angle_deg()
                ));
            }
        }
        None
    }
}

impl DeviceCalib {
    /// Copy with the overlay's rotation corrections applied (see the module docs). Errors if
    /// [`CalibOverlay::refusal`] objects; the factory calibration (`self`) is never modified.
    pub fn with_overlay(
        &self,
        ov: &CalibOverlay,
        max_angle_deg: f64,
    ) -> Result<DeviceCalib, CalibError> {
        if let Some(why) = ov.refusal(self, max_angle_deg) {
            return Err(CalibError::Format(format!("overlay refused: {why}")));
        }
        let mut c = self.clone();
        for r in &ov.rotations {
            let e = geom::rodrigues(r.rotation_vector);
            let cam = &mut c.cameras[r.id];
            cam.rig_r = geom::mat_mul(&e, &cam.rig_r);
            cam.rig_t = geom::mat_vec(&e, cam.rig_t);
        }
        Ok(c)
    }
}

/// Default overlay path: `$PIMAX_CAM_OVERLAY`, else `$XDG_CONFIG_HOME/pimax/cam-calib-overlay.json`, else
/// `~/.config/pimax/cam-calib-overlay.json`.
pub fn default_path() -> Option<PathBuf> {
    if let Some(v) = std::env::var_os("PIMAX_CAM_OVERLAY").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(v));
    }
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("pimax/cam-calib-overlay.json"))
}

/// `<path>.prev`.
pub fn prev_path(path: &Path) -> PathBuf {
    let mut s = path.as_os_str().to_owned();
    s.push(".prev");
    PathBuf::from(s)
}

/// Write atomically (temp file + rename). An existing file is kept as `<path>.prev` (one level of rollback).
pub fn save(ov: &CalibOverlay, path: &Path) -> std::io::Result<()> {
    if let Some(dir) = path.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    std::fs::write(&tmp, ov.to_json())?;
    if path.exists() {
        std::fs::rename(path, prev_path(path))?;
    }
    std::fs::rename(&tmp, path)
}

/// Restore `<path>.prev` as the overlay (the current one becomes the new `.prev`, so a second rollback undoes the
/// first). Errors if there is no previous overlay.
pub fn rollback(path: &Path) -> std::io::Result<()> {
    let prev = prev_path(path);
    if !prev.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("no previous overlay {}", prev.display()),
        ));
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".swap");
    let tmp = PathBuf::from(tmp);
    let had_current = path.exists();
    if had_current {
        std::fs::rename(path, &tmp)?;
    }
    std::fs::rename(&prev, path)?;
    if had_current {
        std::fs::rename(&tmp, &prev)?;
    }
    Ok(())
}

/// Delete the overlay and its `.prev` copy (back to the factory calibration). Returns the files removed.
pub fn clear(path: &Path) -> std::io::Result<Vec<PathBuf>> {
    let mut gone = Vec::new();
    for p in [path.to_path_buf(), prev_path(path)] {
        match std::fs::remove_file(&p) {
            Ok(()) => gone.push(p),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
    }
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calib::RigConvention;

    fn calib() -> DeviceCalib {
        DeviceCalib::parse(include_str!("../tests/fixtures/headset-calibration.xml")).unwrap()
    }

    fn sample(c: &DeviceCalib) -> CalibOverlay {
        let mut o = CalibOverlay::for_calib(c, "test");
        o.rotations.push(CameraRotation {
            id: 1,
            rotation_vector: [0.004, -0.001, 0.0005],
            sigma: Some([1e-4, 3e-4, 1e-4]),
        });
        o.quality.push(("vertical_rms_after_px".into(), 0.31));
        o.note = "unit \"test\"".into();
        o
    }

    #[test]
    fn json_roundtrip() {
        let c = calib();
        let o = sample(&c);
        let back = CalibOverlay::parse(&o.to_json()).unwrap();
        assert_eq!(back, o);
        assert!(
            (o.max_angle_deg() - 0.2381).abs() < 1e-3,
            "{}",
            o.max_angle_deg()
        );
        let empty = CalibOverlay::for_calib(&c, "t");
        assert_eq!(CalibOverlay::parse(&empty.to_json()).unwrap(), empty);
    }

    #[test]
    fn rejects_bad_files() {
        assert!(CalibOverlay::parse("{}").is_err());
        assert!(CalibOverlay::parse("not json").is_err());
        let o = sample(&calib()).to_json();
        assert!(CalibOverlay::parse(&o.replace("\"version\": 1", "\"version\": 2")).is_err());
        assert!(CalibOverlay::parse(&o.replace("\"id\": 1", "\"id\": 1.5")).is_err());
        assert!(CalibOverlay::parse(&o.replace("[0.004, -0.001, 0.0005]", "[0.004]")).is_err());
    }

    #[test]
    fn apply_rotates_about_the_camera_centre() {
        let c = calib();
        let o = sample(&c);
        let c2 = c.with_overlay(&o, DEFAULT_MAX_ANGLE_DEG).unwrap();
        let conv = RigConvention::CamFromCam0;
        // camera centre unchanged, orientation changed by exactly the correction
        let centre = |cal: &DeviceCalib| cal.cam_from_cam0(1, conv).inverse().t;
        assert!(geom::norm(geom::sub(centre(&c), centre(&c2))) < 1e-12);
        let d = geom::mat_mul(
            &c2.cam_from_cam0(1, conv).r,
            &geom::transpose(&c.cam_from_cam0(1, conv).r),
        );
        let e = geom::rodrigues(o.rotations[0].rotation_vector);
        for i in 0..3 {
            for j in 0..3 {
                // the XML rotations are orthonormal to ~1e-10 only
                assert!((d[i][j] - e[i][j]).abs() < 1e-8);
            }
        }
        // other cameras and the IMU extrinsic untouched
        assert_eq!(c2.cameras[0], c.cameras[0]);
        assert_eq!(c2.cameras[2], c.cameras[2]);
        assert_eq!(c2.imu, c.imu);
        // stereo pair follows: X_1' = E X_1
        let p = [0.3, -0.2, 1.5];
        let s = c.stereo(0, 1, conv).apply(p);
        let s2 = c2.stereo(0, 1, conv).apply(p);
        assert!(geom::norm(geom::sub(geom::mat_vec(&e, s), s2)) < 1e-12);
    }

    #[test]
    fn refusals() {
        let c = calib();
        let mut o = sample(&c);
        o.device_uid = "OTHER".into();
        assert!(c.with_overlay(&o, 3.0).is_err());
        let mut o = sample(&c);
        o.factory_date = "1999".into();
        assert!(c.with_overlay(&o, 3.0).is_err());
        let mut o = sample(&c);
        o.rotations[0].rotation_vector = [0.1, 0.0, 0.0]; // 5.7 deg
        assert!(c.with_overlay(&o, 3.0).is_err());
        assert!(c.with_overlay(&o, 6.0).is_ok());
        let mut o = sample(&c);
        o.rotations[0].id = 7;
        assert!(c.with_overlay(&o, 3.0).is_err());
        let mut o = sample(&c);
        o.device_uid.clear();
        o.factory_date.clear();
        assert!(c.with_overlay(&o, 3.0).is_ok());
    }

    #[test]
    fn save_rollback_clear() {
        let dir = std::env::temp_dir().join(format!("pimax-overlay-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("sub/cam-calib-overlay.json");
        let c = calib();
        let mut a = sample(&c);
        a.note = "first".into();
        let mut b = sample(&c);
        b.note = "second".into();
        save(&a, &path).unwrap();
        assert!(!prev_path(&path).exists());
        save(&b, &path).unwrap();
        assert_eq!(CalibOverlay::load(&path).unwrap().note, "second");
        assert_eq!(CalibOverlay::load(&prev_path(&path)).unwrap().note, "first");
        rollback(&path).unwrap();
        assert_eq!(CalibOverlay::load(&path).unwrap().note, "first");
        rollback(&path).unwrap(); // undo the rollback
        assert_eq!(CalibOverlay::load(&path).unwrap().note, "second");
        assert_eq!(clear(&path).unwrap().len(), 2);
        assert!(rollback(&path).is_err());
        assert!(clear(&path).unwrap().is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
