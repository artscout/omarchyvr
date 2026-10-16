//! Camera rig in the OpenXR head frame.
//!
//! Frames (all right-handed, metres):
//! * **body**: the calibration's IMU frame with OpenXR head axes (x right, y up, z back), see `pimax_cam::calib`.
//! * **head**: what Monado calls the head / VIEW space: same axes as body, origin at the eye centre `E`
//!   (the midpoint between the eyes; the runtime puts the eyes at `±IPD/2` along x). `head = body - E`.
//! * **camera i**: OpenCV axes (x right, y down, z forward) of the raw sensor image, which is what the factory
//!   KB4 intrinsics use. Cameras 0 and 2 are mounted upside down; that is part of `body_from_cam`, so no image flip
//!   is needed anywhere.
//!
//! `E` is not in the calibration. The default ([`DEFAULT_HEAD_OFFSET_MM`]) puts the eye centre midway between the
//! left and right cameras (x), midway between the lower (tracking) and upper (controller) cameras (y), and 5 cm
//! behind the camera plate (z). It only matters for near objects (2 cm error = 0.6 deg at 2 m).

use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::kb4::Kb4;

/// Default eye centre in the body (IMU) frame, millimetres. Override with `PIMAX_PT_HEAD_OFFSET=x,y,z` (mm).
pub const DEFAULT_HEAD_OFFSET_MM: Vec3 = [49.5, 4.5, 30.0];

/// One camera of the rig.
#[derive(Clone, Debug)]
pub struct RigCamera {
    /// Camera id (= tile index).
    pub id: usize,
    /// Calibration name (trackingA, ...).
    pub name: String,
    /// Fisheye intrinsics on the raw 640x480 tile.
    pub intr: Kb4,
    /// Camera (OpenCV axes) -> head.
    pub head_from_cam: Pose,
    /// Head -> camera.
    pub cam_from_head: Pose,
}

/// All cameras plus the eye-centre offset used to build them.
#[derive(Clone, Debug)]
pub struct Rig {
    /// Cameras sorted by id.
    pub cameras: Vec<RigCamera>,
    /// Eye centre in the body frame (metres).
    pub head_offset_m: Vec3,
}

/// Parse `x,y,z` millimetres.
pub fn parse_mm3(s: &str) -> Option<Vec3> {
    let v: Vec<f64> = s
        .split(',')
        .map(|t| t.trim().parse::<f64>().ok().filter(|x| x.is_finite()))
        .collect::<Option<Vec<_>>>()?;
    (v.len() == 3).then(|| [v[0], v[1], v[2]])
}

/// Apply a learned calibration overlay (`pimax-selfcal`, see `pimax_cam::overlay`) to the factory calibration.
/// `spec` is a path or `default` (`~/.config/pimax/cam-calib-overlay.json`). Returns the corrected calibration
/// and one line per corrected camera for the log. Errors (missing file, other headset, angle above the limit)
/// are returned, not ignored: the user asked for the overlay explicitly.
pub fn apply_overlay(cal: &DeviceCalib, spec: &str) -> Result<(DeviceCalib, String), String> {
    use pimax_cam::overlay::{self, CalibOverlay};
    let path = if spec == "default" {
        overlay::default_path().ok_or("no default overlay path (HOME unset)")?
    } else {
        std::path::PathBuf::from(spec)
    };
    let ov = CalibOverlay::load(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let out = cal
        .with_overlay(&ov, overlay::DEFAULT_MAX_ANGLE_DEG)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    let mut log = format!("calibration overlay {}:", path.display());
    if ov.rotations.is_empty() {
        log += " no corrections";
    }
    for r in &ov.rotations {
        log += &format!(" camera {} rotated {:.3} deg;", r.id, r.angle_deg());
    }
    Ok((out, log))
}

impl Rig {
    /// Build from the factory calibration (rig convention `X_i = R X_0 + t`, the verified one) and the eye centre
    /// `head_offset_m` in the body frame.
    pub fn from_calib(c: &DeviceCalib, head_offset_m: Vec3) -> Result<Rig, String> {
        if c.cameras.len() < 2 {
            return Err(format!(
                "calibration has {} cameras, need 2..4",
                c.cameras.len()
            ));
        }
        let head_from_body = Pose {
            r: geom::IDENTITY,
            t: geom::scale(head_offset_m, -1.0),
        };
        let mut cameras = Vec::new();
        for (i, cam) in c.cameras.iter().enumerate() {
            let body_from_cam = c.body_from_cam(i, RigConvention::CamFromCam0).ok_or(
                "calibration lacks the camera-IMU extrinsic (<SFConfig><Stateinit ombc tbc>)",
            )?;
            let head_from_cam = head_from_body.compose(&body_from_cam);
            cameras.push(RigCamera {
                id: i,
                name: cam.name.clone(),
                intr: cam.intr.clone(),
                head_from_cam,
                cam_from_head: head_from_cam.inverse(),
            });
        }
        Ok(Rig {
            cameras,
            head_offset_m,
        })
    }

    /// Camera preference for an eye (0 = left, 1 = right): the tracking camera on the eye's side, the other
    /// tracking camera, then the controller camera on the eye's side and the other one. Cameras are classified by
    /// their head-frame x position, so this does not depend on the id order.
    pub fn preference(&self, eye: usize) -> Vec<usize> {
        let n = self.cameras.len();
        // tracking cameras look down (image "up" points to body -y... use the optical axis pitch instead)
        let pitch = |i: usize| -> f64 {
            let z = geom::col(&self.cameras[i].head_from_cam.r, 2);
            z[1]
        };
        let x = |i: usize| self.cameras[i].head_from_cam.t[0];
        let mut ids: Vec<usize> = (0..n).collect();
        // lower-looking cameras first (tracking), then same side as the eye
        ids.sort_by(|&a, &b| {
            let low_a = pitch(a) < 0.0;
            let low_b = pitch(b) < 0.0;
            let side = |i: usize| {
                let left = x(i) < 0.0;
                (left == (eye == 0)) as u8
            };
            low_b
                .cmp(&low_a)
                .then(side(b).cmp(&side(a)))
                .then(a.cmp(&b))
        });
        ids
    }

    /// Short text: camera positions / look directions in the head frame.
    pub fn report(&self) -> String {
        use std::fmt::Write;
        let mut s = String::new();
        let e = self.head_offset_m;
        let _ = writeln!(
            s,
            "eye centre in the IMU/body frame: ({:.1}, {:.1}, {:.1}) mm",
            e[0] * 1e3,
            e[1] * 1e3,
            e[2] * 1e3
        );
        for c in &self.cameras {
            let p = c.head_from_cam.t;
            let z = geom::col(&c.head_from_cam.r, 2);
            let yaw = (-z[0]).atan2(-z[2]).to_degrees();
            let pitch = z[1].asin().to_degrees();
            let _ = writeln!(
                s,
                "cam {} {:<15} head pos ({:+6.1}, {:+6.1}, {:+6.1}) mm  looks yaw {:+5.1} (+ left) pitch {:+5.1} deg",
                c.id,
                c.name,
                p[0] * 1e3,
                p[1] * 1e3,
                p[2] * 1e3,
                yaw,
                pitch
            );
        }
        s
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::path::PathBuf;

    pub fn calib_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../pimax-cam/tests/fixtures/headset-calibration.xml")
    }

    pub fn rig() -> Rig {
        let c = DeviceCalib::load(&calib_path()).unwrap();
        Rig::from_calib(&c, geom::scale(DEFAULT_HEAD_OFFSET_MM, 1e-3)).unwrap()
    }

    #[test]
    fn parse_offsets() {
        assert_eq!(parse_mm3("1, 2,3.5"), Some([1.0, 2.0, 3.5]));
        assert_eq!(parse_mm3("1,2"), None);
        assert_eq!(parse_mm3("1,x,2"), None);
        assert_eq!(parse_mm3("1,nan,2"), None);
    }

    #[test]
    fn cameras_sit_where_the_report_says() {
        let r = rig();
        // camera 0 (trackingA) left and in front of the eyes, camera 1 right; both look down and forward
        let c0 = &r.cameras[0].head_from_cam;
        let c1 = &r.cameras[1].head_from_cam;
        assert!(c0.t[0] < -0.04 && c1.t[0] > 0.04, "{:?} {:?}", c0.t, c1.t);
        assert!(c0.t[2] < -0.03 && c1.t[2] < -0.03);
        for c in &r.cameras {
            let z = geom::col(&c.head_from_cam.r, 2);
            assert!(z[2] < -0.5, "camera {} looks forward (-z): {z:?}", c.id);
            assert!(geom::orthonormality_error(&c.head_from_cam.r) < 1e-9);
            // inverse really is the inverse
            let p = [0.3, -0.2, -1.0];
            let q = c.head_from_cam.apply(c.cam_from_head.apply(p));
            assert!(geom::norm(geom::sub(p, q)) < 1e-8);
        }
    }

    #[test]
    fn overlay_rotates_one_camera_and_leaves_the_others() {
        use pimax_cam::overlay::{CalibOverlay, CameraRotation};
        let c = DeviceCalib::load(&calib_path()).unwrap();
        let mut ov = CalibOverlay::for_calib(&c, "test");
        ov.rotations.push(CameraRotation {
            id: 1,
            rotation_vector: [0.0, 0.0, 0.01],
            sigma: None,
        });
        let dir = std::env::temp_dir().join(format!("pt-overlay-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("ov.json");
        std::fs::write(&path, ov.to_json()).unwrap();
        let (c2, log) = apply_overlay(&c, path.to_str().unwrap()).unwrap();
        assert!(log.contains("camera 1 rotated 0.573 deg"), "{log}");
        let off = geom::scale(DEFAULT_HEAD_OFFSET_MM, 1e-3);
        let (r1, r2) = (
            Rig::from_calib(&c, off).unwrap(),
            Rig::from_calib(&c2, off).unwrap(),
        );
        assert_eq!(r1.cameras[0].head_from_cam, r2.cameras[0].head_from_cam);
        // camera 1 keeps its position and turns by 0.573 deg
        let (a, b) = (&r1.cameras[1].head_from_cam, &r2.cameras[1].head_from_cam);
        assert!(geom::norm(geom::sub(a.t, b.t)) < 1e-9);
        let ang = geom::angle_deg(geom::col(&a.r, 2), geom::col(&b.r, 2));
        assert!(ang < 0.6, "{ang}");
        let d = geom::mat_mul(&geom::transpose(&a.r), &b.r);
        let tr = (d[0][0] + d[1][1] + d[2][2] - 1.0) / 2.0;
        assert!((tr.clamp(-1.0, 1.0).acos().to_degrees() - 0.573).abs() < 1e-3);
        // another headset's overlay is refused
        ov.device_uid = "OTHER".into();
        std::fs::write(&path, ov.to_json()).unwrap();
        assert!(apply_overlay(&c, path.to_str().unwrap()).is_err());
        assert!(apply_overlay(&c, "/nonexistent/x.json").is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn preference_puts_the_eye_side_tracking_camera_first() {
        let r = rig();
        assert_eq!(r.preference(0), vec![0, 1, 2, 3]);
        assert_eq!(r.preference(1), vec![1, 0, 3, 2]);
        assert!(r.report().contains("trackingA"));
    }
}
