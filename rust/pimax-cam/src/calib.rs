//! Factory calibration (`<DeviceConfiguration>` XML served by the headset over IF0, also
//! `analysis/headset-calibration.xml`): four OV7251 fisheye cameras plus the camera/IMU extrinsic.
//!
//! Conventions (see `analysis/overnight/cam.md` for the evidence):
//! * Intrinsics: `FISHEYE_4_PARAMETERS` = OpenCV/Kannala-Brandt KB4 on the full 640x480 sensor image, OpenCV
//!   camera axes (x right, y down, z forward). `radial_distortion` has 6 numbers; the first four are k1..k4.
//! * `<Rig>` of camera i: **`X_i = R * X_0 + t`** (it maps camera-0 coordinates into camera i, "convention 2"
//!   of `camgeom.py`). Camera 0 has the identity.
//! * `<SFConfig><Stateinit ombc tbc>`: `ombc` is the rotation vector of `R_bc`, the rotation from camera-0
//!   coordinates into the body frame (`X_b = R_bc X_c + tbc`). With these two conventions the A-B baseline lies
//!   on body +X to 1e-2 and the optical axes are symmetric about the body YZ plane, looking along -Z and 29 deg
//!   down, with the image upside down. The body frame is therefore the OpenXR head frame (x right, y up, z back),
//!   i.e. the raw IMU axes with X and Z negated, which is the Monado driver's `PIMAX_AXIS_MODE=1`.

use crate::geom::{self, Mat3, Pose, Vec3};
use crate::kb4::Kb4;
use crate::xml::{self, Element};
use std::path::{Path, PathBuf};

/// The REFERENCE factory calibration of the Pimax Dream Air, shipped for everyone (user decision 2026-10-17,
/// "approve shipping calibrations"): the project's own headset's Windows calibration with the serial scrubbed
/// (`deviceUID="reference"`). It is used only when no calibration of the user's own headset is found (read from the
/// headset in Room setup, or kept by the uninstaller); the app then says "Using the reference calibration".
pub const REFERENCE_XML: &str = include_str!("../../pimax-device/devices/dream-air/reference/headset-calibration.xml");

/// `deviceUID` of [`REFERENCE_XML`].
pub const REFERENCE_UID: &str = "reference";

/// Where the reference is installed with the data copy, relative to `$HOME` (`./install.sh` copies the device
/// descriptors there).
pub const REFERENCE_INSTALLED: &str = ".local/share/omarchyvr/data/rust/pimax-device/devices/dream-air/reference/headset-calibration.xml";

/// The reference fallback is on unless `PIMAX_NO_REFERENCE` is set to something other than `0` (tests, and anyone
/// who wants a start to fail without the headset's own calibration).
pub fn reference_allowed(env: &dyn Fn(&str) -> Option<String>) -> bool {
    !env("PIMAX_NO_REFERENCE").is_some_and(|v| !v.is_empty() && v != "0")
}

/// The path reported for the reference: the installed copy when it exists, else a name that says it is built in.
pub fn reference_path(home: Option<&Path>) -> PathBuf {
    match home.map(|h| h.join(REFERENCE_INSTALLED)) {
        Some(p) if p.is_file() => p,
        _ => PathBuf::from("(built-in reference) dream-air/reference/headset-calibration.xml"),
    }
}

/// Calibration of one camera.
#[derive(Clone, Debug, PartialEq)]
pub struct CameraCalib {
    /// `id` attribute (0..3).
    pub id: u32,
    /// `name` attribute (trackingA, trackingB, Pi-controllerA, Pi-controllerB).
    pub name: String,
    /// `model` attribute of `<Calibration>`.
    pub model: String,
    /// Intrinsics.
    pub intr: Kb4,
    /// `<Rig rowMajorRotationMat>`.
    pub rig_r: Mat3,
    /// `<Rig translation>` (metres).
    pub rig_t: Vec3,
    /// `<TimeAlignment delta>` (seconds).
    pub time_delta: f64,
}

/// Camera-IMU terms from `<SFConfig><Stateinit>`.
#[derive(Clone, Debug, PartialEq)]
pub struct ImuExtrinsic {
    /// Rotation vector of `R_bc` (camera 0 -> body).
    pub ombc: Vec3,
    /// Camera 0 origin in the body frame (metres).
    pub tbc: Vec3,
    /// `delta`: camera-IMU time offset (seconds). Not applied by this crate.
    pub delta_s: f64,
    /// All Stateinit attributes as raw strings (biases, scales, ...).
    pub raw: Vec<(String, String)>,
}

/// The whole document.
#[derive(Clone, Debug, PartialEq)]
pub struct DeviceCalib {
    /// `deviceUID`.
    pub uid: String,
    /// `cameraModel`.
    pub sensor: String,
    /// `calibration_date`.
    pub date: String,
    /// `pimax_calibration_version`.
    pub version: String,
    /// Cameras sorted by id.
    pub cameras: Vec<CameraCalib>,
    /// Camera-IMU extrinsic if present.
    pub imu: Option<ImuExtrinsic>,
}

/// How to read `<Rig>`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RigConvention {
    /// `X_i = R X_0 + t` (default, verified).
    CamFromCam0,
    /// `X_0 = R X_i + t` (alternative, for hypothesis tests).
    Cam0FromCam,
}

/// Error while reading the calibration.
#[derive(Debug)]
pub enum CalibError {
    /// File could not be read.
    Io(PathBuf, std::io::Error),
    /// Malformed XML.
    Xml(xml::XmlError),
    /// Structure or values not as expected.
    Format(String),
    /// No file found in the default locations.
    NotFound(Vec<PathBuf>),
}

impl std::fmt::Display for CalibError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CalibError::Io(p, e) => write!(f, "{}: {e}", p.display()),
            CalibError::Xml(e) => write!(f, "{e}"),
            CalibError::Format(s) => write!(f, "calibration: {s}"),
            CalibError::NotFound(c) => {
                write!(f, "no calibration file found (tried")?;
                for p in c {
                    write!(f, " {}", p.display())?;
                }
                write!(f, "); read one with `omarchyvr calib`")
            }
        }
    }
}

impl std::error::Error for CalibError {}

fn fmt_err<T>(s: impl Into<String>) -> Result<T, CalibError> {
    Err(CalibError::Format(s.into()))
}

fn nums(el: &Element, attr: &str) -> Result<Vec<f64>, CalibError> {
    let v = el
        .attr(attr)
        .ok_or_else(|| CalibError::Format(format!("<{}> lacks {attr}", el.name)))?;
    v.split_whitespace()
        .map(|t| {
            t.parse::<f64>()
                .ok()
                .filter(|x| x.is_finite())
                .ok_or_else(|| CalibError::Format(format!("{attr}: bad number {t:?}")))
        })
        .collect()
}

fn nums_n<const N: usize>(el: &Element, attr: &str) -> Result<[f64; N], CalibError> {
    let v = nums(el, attr)?;
    if v.len() < N {
        return fmt_err(format!("{attr}: {} numbers, need {N}", v.len()));
    }
    let mut a = [0.0; N];
    a.copy_from_slice(&v[..N]);
    Ok(a)
}

impl DeviceCalib {
    /// Parse the XML text.
    pub fn parse(doc: &str) -> Result<DeviceCalib, CalibError> {
        let root = xml::parse(doc).map_err(CalibError::Xml)?;
        if root.name != "DeviceConfiguration" {
            return fmt_err(format!(
                "root element <{}>, expected <DeviceConfiguration>",
                root.name
            ));
        }
        let s = |k: &str| root.attr(k).unwrap_or("").to_string();
        let mut cameras = Vec::new();
        for c in root.children_named("Camera") {
            let id: u32 = c
                .attr("id")
                .and_then(|v| v.trim().parse().ok())
                .ok_or_else(|| CalibError::Format("<Camera> without a numeric id".into()))?;
            let cal = c
                .child("Calibration")
                .ok_or_else(|| CalibError::Format(format!("camera {id}: no <Calibration>")))?;
            let rig = c
                .child("Rig")
                .ok_or_else(|| CalibError::Format(format!("camera {id}: no <Rig>")))?;
            let size = nums_n::<2>(cal, "size")?;
            let pp = nums_n::<2>(cal, "principal_point")?;
            let fl = nums_n::<2>(cal, "focal_length")?;
            let dist = nums(cal, "radial_distortion")?;
            if dist.len() < 4 {
                return fmt_err(format!(
                    "camera {id}: {} distortion terms, need 4",
                    dist.len()
                ));
            }
            if size[0] < 1.0 || size[1] < 1.0 || fl[0] <= 0.0 || fl[1] <= 0.0 {
                return fmt_err(format!("camera {id}: bad size or focal length"));
            }
            let r = nums_n::<9>(rig, "rowMajorRotationMat")?;
            let rig_r = [[r[0], r[1], r[2]], [r[3], r[4], r[5]], [r[6], r[7], r[8]]];
            if geom::orthonormality_error(&rig_r) > 1e-4 || geom::det(&rig_r) < 0.0 {
                return fmt_err(format!("camera {id}: rig rotation is not a rotation"));
            }
            let time_delta = c
                .child("TimeAlignment")
                .and_then(|t| t.attr("delta"))
                .and_then(|v| v.trim().parse().ok())
                .unwrap_or(0.0);
            cameras.push(CameraCalib {
                id,
                name: c
                    .attr("name")
                    .or(c.attr("cam_name"))
                    .unwrap_or("")
                    .to_string(),
                model: cal.attr("model").unwrap_or("").to_string(),
                intr: Kb4::new(
                    size[0] as u32,
                    size[1] as u32,
                    fl[0],
                    fl[1],
                    pp[0],
                    pp[1],
                    [dist[0], dist[1], dist[2], dist[3]],
                    dist[4..].to_vec(),
                ),
                rig_r,
                rig_t: nums_n::<3>(rig, "translation")?,
                time_delta,
            });
        }
        if cameras.is_empty() {
            return fmt_err("no <Camera> elements");
        }
        cameras.sort_by_key(|c| c.id);
        for (i, c) in cameras.iter().enumerate() {
            if c.id as usize != i {
                return fmt_err(format!("camera ids are not 0..{}", cameras.len() - 1));
            }
        }
        let imu = match root.child("SFConfig").and_then(|s| s.child("Stateinit")) {
            Some(st) => Some(ImuExtrinsic {
                ombc: nums_n::<3>(st, "ombc")?,
                tbc: nums_n::<3>(st, "tbc")?,
                delta_s: st
                    .attr("delta")
                    .and_then(|v| v.trim().parse().ok())
                    .unwrap_or(0.0),
                raw: st.attrs.clone(),
            }),
            None => None,
        };
        Ok(DeviceCalib {
            uid: s("deviceUID"),
            sensor: s("cameraModel"),
            date: s("calibration_date"),
            version: s("pimax_calibration_version"),
            cameras,
            imu,
        })
    }

    /// Read and parse a file.
    pub fn load(path: &Path) -> Result<DeviceCalib, CalibError> {
        let s = std::fs::read_to_string(path).map_err(|e| CalibError::Io(path.to_path_buf(), e))?;
        DeviceCalib::parse(&s)
    }

    /// Default search order: `$PIMAX_CAM_CALIB`, `$PIMAX_CALIB`, `~/.config/omarchyvr/headset-calibration.xml`,
    /// `<repo>/analysis/headset-calibration.xml`
    /// (the installed data copy `~/.local/share/omarchyvr/data`, else `~/omarchyvr`; `$PIMAX_REPO` overrides).
    /// When none exists, [`DeviceCalib::load_default`] falls back to the shipped [`REFERENCE_XML`].
    pub fn default_candidates() -> Vec<PathBuf> {
        let mut c = Vec::new();
        for var in ["PIMAX_CAM_CALIB", "PIMAX_CALIB"] {
            if let Some(v) = std::env::var_os(var).filter(|v| !v.is_empty()) {
                c.push(PathBuf::from(v));
            }
        }
        if let Some(h) = std::env::var_os("HOME") {
            let h = PathBuf::from(h);
            c.push(h.join(".config/omarchyvr/headset-calibration.xml"));
            // the repo the way omarchyvr_config::paths::repo_dir finds it (computed here: this crate is linked into Monado
            // as a staticlib and stays free of the app's crates): $PIMAX_REPO, the installed data copy, else ~/omarchyvr
            let repo = std::env::var_os("PIMAX_REPO")
                .filter(|v| !v.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    let installed = h.join(".local/share/omarchyvr/data");
                    if installed.join("docs/LESSONS.md").is_file() { installed } else { h.join("omarchyvr") }
                });
            c.push(repo.join("analysis/headset-calibration.xml"));
        }
        c
    }

    /// Load from `path`, or from the first existing default candidate, else the shipped reference ([`REFERENCE_XML`],
    /// `uid` "reference"; `$PIMAX_NO_REFERENCE=1` turns that fallback off). The headset's own file always wins.
    pub fn load_default(path: Option<&Path>) -> Result<(DeviceCalib, PathBuf), CalibError> {
        if let Some(p) = path {
            return DeviceCalib::load(p).map(|c| (c, p.to_path_buf()));
        }
        let cands = DeviceCalib::default_candidates();
        let home = std::env::var_os("HOME").map(PathBuf::from);
        DeviceCalib::load_first(&cands, reference_allowed(&|k| std::env::var(k).ok()), home.as_deref())
    }

    /// [`DeviceCalib::load_default`] with explicit candidates: the first existing file, else (when `reference`)
    /// the shipped reference, reported under [`reference_path`].
    pub fn load_first(cands: &[PathBuf], reference: bool, home: Option<&Path>) -> Result<(DeviceCalib, PathBuf), CalibError> {
        match cands.iter().find(|p| p.is_file()) {
            Some(p) => DeviceCalib::load(p).map(|c| (c, p.clone())),
            None if reference => DeviceCalib::parse(REFERENCE_XML).map(|c| (c, reference_path(home))),
            None => Err(CalibError::NotFound(cands.to_vec())),
        }
    }

    /// True for the shipped reference (its `deviceUID` is [`REFERENCE_UID`]).
    pub fn is_reference(&self) -> bool {
        self.uid == REFERENCE_UID
    }

    /// `cam_i_from_cam0` under a rig convention.
    pub fn cam_from_cam0(&self, i: usize, conv: RigConvention) -> Pose {
        let c = &self.cameras[i];
        let p = Pose {
            r: c.rig_r,
            t: c.rig_t,
        };
        match conv {
            RigConvention::CamFromCam0 => p,
            RigConvention::Cam0FromCam => p.inverse(),
        }
    }

    /// Stereo extrinsic `b_from_a`: `X_b = R X_a + t` (OpenCV `stereoCalibrate` R, T; Monado
    /// `t_stereo_camera_calibration::camera_rotation/translation`).
    pub fn stereo(&self, a: usize, b: usize, conv: RigConvention) -> Pose {
        self.cam_from_cam0(b, conv)
            .compose(&self.cam_from_cam0(a, conv).inverse())
    }

    /// `body_from_cam0` from `ombc`/`tbc` (body = OpenXR head axes, see the module docs).
    pub fn body_from_cam0(&self) -> Option<Pose> {
        self.imu.as_ref().map(|m| Pose {
            r: geom::rodrigues(m.ombc),
            t: m.tbc,
        })
    }

    /// `body_from_cam_i`.
    pub fn body_from_cam(&self, i: usize, conv: RigConvention) -> Option<Pose> {
        Some(
            self.body_from_cam0()?
                .compose(&self.cam_from_cam0(i, conv).inverse()),
        )
    }

    /// Human-readable geometry summary (centres, optical axes, baselines, FOV).
    pub fn geometry_report(&self, conv: RigConvention) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let _ = writeln!(
            s,
            "device {} sensor {} calibrated {} ({})",
            self.uid, self.sensor, self.date, self.version
        );
        let n = self.cameras.len();
        let mut centres = Vec::new();
        let mut axes = Vec::new();
        for (i, c) in self.cameras.iter().enumerate() {
            let k = &c.intr;
            let h = |u: f64, v: f64| k.angle_of_pixel_deg(u, v).unwrap_or(f64::NAN);
            let hfov = h(0.0, k.cy) + h(k.width as f64 - 1.0, k.cy);
            let vfov = h(k.cx, 0.0) + h(k.cx, k.height as f64 - 1.0);
            let p = self.cam_from_cam0(i, conv).inverse(); // cam0_from_cam_i
            centres.push(p.t);
            axes.push(geom::col(&p.r, 2));
            let _ = writeln!(
                s,
                "cam {i} {:15} {}x{} f=({:.1},{:.1}) c=({:.1},{:.1}) k={:?} extra={:?} FOV {:.0}x{:.0} deg, model limit {:.1} deg",
                c.name, k.width, k.height, k.fx, k.fy, k.cx, k.cy, k.k, k.extra, hfov, vfov, k.theta_limit.to_degrees()
            );
        }
        let _ = writeln!(s, "rig convention {conv:?}: camera centres / optical axes in the camera-0 frame (OpenCV axes)");
        for i in 0..n {
            let _ = writeln!(
                s,
                "  cam {i}: centre ({:7.1},{:7.1},{:7.1}) mm  axis ({:+.3},{:+.3},{:+.3})  {:.1} deg from cam 0",
                centres[i][0] * 1e3,
                centres[i][1] * 1e3,
                centres[i][2] * 1e3,
                axes[i][0],
                axes[i][1],
                axes[i][2],
                geom::angle_deg(axes[i], axes[0])
            );
        }
        for i in 0..n {
            for j in i + 1..n {
                let _ = writeln!(
                    s,
                    "  pair {i}-{j}: baseline {:.1} mm, axes {:.1} deg apart",
                    geom::norm(geom::sub(centres[j], centres[i])) * 1e3,
                    geom::angle_deg(axes[i], axes[j])
                );
            }
        }
        if self.body_from_cam0().is_some() {
            let _ = writeln!(
                s,
                "body frame (OpenXR head axes: x right, y up, -z forward) from ombc/tbc:"
            );
            for i in 0..n {
                let p = self.body_from_cam(i, conv).unwrap();
                let z = geom::col(&p.r, 2);
                let down = geom::col(&p.r, 1);
                let yaw = (-z[0]).atan2(-z[2]).to_degrees();
                let pitch = z[1].asin().to_degrees();
                let _ = writeln!(
                    s,
                    "  cam {i}: position ({:6.1},{:6.1},{:6.1}) mm, looks yaw {:+.1} deg (+ = left) pitch {:+.1} deg, image-down axis ({:+.2},{:+.2},{:+.2})",
                    p.t[0] * 1e3,
                    p.t[1] * 1e3,
                    p.t[2] * 1e3,
                    yaw,
                    pitch,
                    down[0],
                    down[1],
                    down[2]
                );
            }
        }
        s
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const XML: &str = include_str!("../tests/fixtures/headset-calibration.xml");

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn parses_factory_file() {
        let c = DeviceCalib::parse(XML).unwrap();
        assert_eq!(c.uid, "P511X0X000000000002");
        assert_eq!(c.sensor, "ov7251");
        assert_eq!(c.cameras.len(), 4);
        let names: Vec<_> = c.cameras.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            ["trackingA", "trackingB", "Pi-controllerA", "Pi-controllerB"]
        );
        let a = &c.cameras[0];
        assert_eq!(a.model, "FISHEYE_4_PARAMETERS");
        assert_eq!((a.intr.width, a.intr.height), (640, 480));
        assert_eq!(a.intr.cx, 325.3779138659);
        assert_eq!(a.intr.fy, 234.7063990438);
        assert_eq!(
            a.intr.k,
            [0.0589502951, -0.0603263194, 0.0320281449, -0.0082048556]
        );
        assert_eq!(a.intr.extra, vec![0.0011599741, -0.0001122173]);
        assert_eq!(a.rig_r, geom::IDENTITY);
        assert_eq!(
            c.cameras[1].rig_t,
            [-0.0996975113, 0.0070973858, -0.0497311326]
        );
        assert_eq!(c.cameras[3].rig_r[2][2], 0.1736576490);
        let imu = c.imu.as_ref().unwrap();
        assert_eq!(imu.ombc, [-0.07417106183, -2.573029401, 0.6280404502]);
        assert_eq!(imu.delta_s, 0.001637992156);
        assert!(imu.raw.iter().any(|(k, _)| k == "wBias"));
    }

    #[test]
    fn stereo_pair_a_b() {
        let c = DeviceCalib::parse(XML).unwrap();
        let ab = c.stereo(0, 1, RigConvention::CamFromCam0);
        // camera 0 is the reference: b_from_a is exactly the rig of camera 1.
        assert_eq!(ab.r, c.cameras[1].rig_r);
        assert_eq!(ab.t, c.cameras[1].rig_t);
        let baseline = geom::norm(ab.inverse().t);
        assert!((baseline - 0.1116).abs() < 0.001, "baseline {baseline}");
        // the other convention gives the same baseline length but a different centre
        let ab1 = c.stereo(0, 1, RigConvention::Cam0FromCam);
        assert!((geom::norm(ab1.inverse().t) - baseline).abs() < 1e-9);
        // pair 2-3 composes two rigs
        let p23 = c.stereo(2, 3, RigConvention::CamFromCam0);
        let x0 = [0.1, 0.2, 1.0];
        let via = c.cam_from_cam0(3, RigConvention::CamFromCam0).apply(x0);
        let direct = p23.apply(c.cam_from_cam0(2, RigConvention::CamFromCam0).apply(x0));
        // the XML rotations are orthonormal to ~1e-10 only
        assert!(geom::norm(geom::sub(via, direct)) < 1e-8);
    }

    #[test]
    fn body_frame_symmetry() {
        // The evidence for the conventions: A-B baseline along body +X, optical axes symmetric about YZ.
        let c = DeviceCalib::parse(XML).unwrap();
        let conv = RigConvention::CamFromCam0;
        let a = c.body_from_cam(0, conv).unwrap();
        let b = c.body_from_cam(1, conv).unwrap();
        let base = geom::normalize(geom::sub(b.t, a.t));
        assert!(base[0] > 0.999, "baseline dir {base:?}");
        let (za, zb) = (geom::col(&a.r, 2), geom::col(&b.r, 2));
        let bis = geom::normalize(geom::add(za, zb));
        assert!(
            bis[0].abs() < 0.03 && bis[2] < -0.85 && bis[1] < -0.4,
            "bisector {bis:?}"
        );
        // image "down" (+y_cam) of A points up in the body frame (A is mounted upside down); B is rolled 180 deg
        // relative to A (mirror-symmetric mounting), so B's picture is upright.
        assert!(
            geom::col(&a.r, 1)[1] > 0.85,
            "A down {:?}",
            geom::col(&a.r, 1)
        );
        assert!(
            geom::col(&b.r, 1)[1] < -0.85,
            "B down {:?}",
            geom::col(&b.r, 1)
        );
        // A is the left camera (x < B's x) and both are ~27 deg off the bisector.
        assert!(geom::angle_deg(za, bis) > 25.0 && geom::angle_deg(za, bis) < 29.0);
        assert!(!c.geometry_report(conv).is_empty());
    }

    #[test]
    #[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
    fn bad_documents() {
        assert!(matches!(
            DeviceCalib::parse("<Foo/>"),
            Err(CalibError::Format(_))
        ));
        assert!(matches!(
            DeviceCalib::parse("<DeviceConfiguration/>"),
            Err(CalibError::Format(_))
        ));
        assert!(matches!(
            DeviceCalib::parse("<DeviceConfiguration"),
            Err(CalibError::Xml(_))
        ));
        let broken = XML.replace("234.7133343909", "abc");
        assert!(matches!(
            DeviceCalib::parse(&broken),
            Err(CalibError::Format(_))
        ));
        let not_rot = XML.replace("-0.5777754392 -0.1162377714", "-0.9777754392 -0.1162377714");
        assert!(matches!(
            DeviceCalib::parse(&not_rot),
            Err(CalibError::Format(_))
        ));
        assert!(matches!(
            DeviceCalib::load(Path::new("/nonexistent/x.xml")),
            Err(CalibError::Io(..))
        ));
    }

    #[test]
    fn the_shipped_reference_parses_and_carries_no_serial() {
        let c = DeviceCalib::parse(REFERENCE_XML).unwrap();
        assert!(c.is_reference());
        assert_eq!(c.cameras.len(), 4);
        assert!(c.imu.is_some(), "the 6DoF export needs ombc/tbc");
        assert!(c.body_from_cam(0, RigConvention::CamFromCam0).is_some());
        assert!(!REFERENCE_XML.contains("deviceUID=\"P"));
    }

    #[test]
    fn the_headsets_own_file_wins_and_the_reference_is_the_fallback() {
        let d = std::env::temp_dir().join(format!("pimax-cam-ref-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let (own, missing) = (d.join("own.xml"), d.join("missing.xml"));
        // nothing on disk: the reference, reported as built in (no installed copy under this home)
        let (c, p) = DeviceCalib::load_first(std::slice::from_ref(&missing), true, Some(&d)).unwrap();
        assert!(c.is_reference());
        assert!(p.to_string_lossy().contains("built-in reference"), "{}", p.display());
        // the installed copy is reported by its path
        let inst = d.join(REFERENCE_INSTALLED);
        std::fs::create_dir_all(inst.parent().unwrap()).unwrap();
        std::fs::write(&inst, REFERENCE_XML).unwrap();
        assert_eq!(DeviceCalib::load_first(std::slice::from_ref(&missing), true, Some(&d)).unwrap().1, inst);
        // fallback off: not found, and the error names the places tried
        match DeviceCalib::load_first(std::slice::from_ref(&missing), false, Some(&d)) {
            Err(CalibError::NotFound(v)) => assert_eq!(v, vec![missing.clone()]),
            other => panic!("{other:?}"),
        }
        // the user's own file wins over the reference
        std::fs::write(&own, REFERENCE_XML.replace("deviceUID=\"reference\"", "deviceUID=\"mine\"")).unwrap();
        let (c, p) = DeviceCalib::load_first(&[missing, own.clone()], true, Some(&d)).unwrap();
        assert_eq!((c.uid.as_str(), p), ("mine", own));
        assert!(!c.is_reference());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_reference_fallback_can_be_switched_off() {
        assert!(reference_allowed(&|_| None));
        assert!(reference_allowed(&|k| (k == "PIMAX_NO_REFERENCE").then(|| "0".to_string())));
        assert!(!reference_allowed(&|k| (k == "PIMAX_NO_REFERENCE").then(|| "1".to_string())));
    }
}
