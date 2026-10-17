//! Camera extrinsic corrections fitted on controller poses (`track --fit-extrinsics --save-extrinsics P`).
//! Stores the full corrected `cam_from_body` of each refitted camera; `live`/`track` load them with `--extrinsics`
//! (default `~/.config/omarchyvr/ctrl-cam-extrinsics.json` when it exists).
//!
//! The shipped Dream Air reference has the fit of the reference headset ([`REFERENCE_JSON`]): cameras 1-3 corrected by
//! 0.46, 1.45 and 3.20 deg (mostly pitch; analysis/controller-6dof.md). Without it the controllers come out a few
//! degrees tilted (the 2026-10-17 clean reinstall lost the user's own copy and the controllers were "tilted forwards"
//! again). [`reference_for`] applies it only to a headset whose factory camera rig equals the reference's, i.e. the
//! same headset; the user's own file always wins.

use pimax_cam::calib::{DeviceCalib, RigConvention, REFERENCE_XML};
use pimax_cam::geom::Pose;
use pimax_cam::json;
use std::path::{Path, PathBuf};

/// The camera extrinsics refit of the reference headset (serial scrubbed, `device_uid` "reference").
pub const REFERENCE_JSON: &str = include_str!("../../pimax-device/devices/dream-air/reference/ctrl-cam-extrinsics.json");

pub fn default_path() -> PathBuf {
    let h = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    h.join(".config/omarchyvr/ctrl-cam-extrinsics.json")
}

/// `cams`: (camera id, cam_from_body).
pub fn save(
    path: &Path,
    device_uid: &str,
    cams: &[(usize, Pose)],
    note: &str,
) -> std::io::Result<()> {
    let mut s =
        String::from("{\n  \"format\": \"pimax-ctrltrack-extrinsics\",\n  \"version\": 1,\n");
    s += &format!(
        "  \"device_uid\": {},\n  \"note\": {},\n  \"cameras\": [\n",
        json::quote(device_uid),
        json::quote(note)
    );
    for (k, (id, p)) in cams.iter().enumerate() {
        let r: Vec<f64> = p.r.iter().flatten().copied().collect();
        s += &format!(
            "    {{ \"id\": {id}, \"cam_from_body_r\": {}, \"cam_from_body_t\": {} }}{}\n",
            json::num_array(&r),
            json::num_array(&p.t),
            if k + 1 < cams.len() { "," } else { "" }
        );
    }
    s += "  ]\n}\n";
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, s)?;
    std::fs::rename(tmp, path)
}

pub fn load(path: &Path) -> Result<(String, Vec<(usize, Pose)>), String> {
    let t = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&t).map_err(|e| format!("{}: {e}", path.display()))
}

/// Parse the file text: (device_uid, [(camera id, cam_from_body)]).
pub fn parse(t: &str) -> Result<(String, Vec<(usize, Pose)>), String> {
    let v = json::parse(t).map_err(|e| format!("{e:?}"))?;
    if v.get("format").and_then(|f| f.as_str()) != Some("pimax-ctrltrack-extrinsics") {
        return Err("not a pimax-ctrltrack-extrinsics file".into());
    }
    let uid = v
        .get("device_uid")
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    let mut out = Vec::new();
    for c in v
        .get("cameras")
        .and_then(|c| c.as_array())
        .ok_or("no cameras")?
    {
        let id = c
            .get("id")
            .and_then(|x| x.as_f64())
            .ok_or("camera without id")? as usize;
        let r: [f64; 9] = c
            .get("cam_from_body_r")
            .and_then(|x| x.as_f64_array())
            .ok_or("bad r")?;
        let t: [f64; 3] = c
            .get("cam_from_body_t")
            .and_then(|x| x.as_f64_array())
            .ok_or("bad t")?;
        out.push((
            id,
            Pose {
                r: [[r[0], r[1], r[2]], [r[3], r[4], r[5]], [r[6], r[7], r[8]]],
                t,
            },
        ));
    }
    Ok((uid, out))
}

/// True when `calib` has the same factory camera rig (every camera's body_from_cam: rig rotation and translation plus
/// the camera to IMU extrinsic) as the shipped reference: the reference headset itself, or the reference in use.
pub fn same_rig_as_reference(calib: &DeviceCalib) -> bool {
    let Ok(r) = DeviceCalib::parse(REFERENCE_XML) else {
        return false;
    };
    if calib.cameras.len() != r.cameras.len() {
        return false;
    }
    let close = |a: &Pose, b: &Pose| {
        (0..3).all(|i| (a.t[i] - b.t[i]).abs() < 1e-6 && (0..3).all(|j| (a.r[i][j] - b.r[i][j]).abs() < 1e-6))
    };
    (0..r.cameras.len()).all(|i| {
        match (calib.body_from_cam(i, RigConvention::CamFromCam0), r.body_from_cam(i, RigConvention::CamFromCam0)) {
            (Some(a), Some(b)) => close(&a, &b),
            _ => false,
        }
    })
}

/// The shipped reference extrinsics for `calib` when it is the reference headset (same factory rig) and the reference
/// is allowed (`PIMAX_NO_REFERENCE` unset or 0). None for any other headset: its own factory extrinsics are kept.
pub fn reference_for(calib: &DeviceCalib, env: &dyn Fn(&str) -> Option<String>) -> Option<Vec<(usize, Pose)>> {
    if !pimax_cam::calib::reference_allowed(env) || !same_rig_as_reference(calib) {
        return None;
    }
    parse(REFERENCE_JSON).ok().map(|(_, v)| v)
}

/// The corrections to use on top of the factory extrinsics of `calib`, and a note for the log: the user's own fit at
/// `path` (default [`default_path`]) when it exists and belongs to this headset, else the shipped reference fit for
/// the reference headset ([`reference_for`]), else none ("factory extrinsics").
pub fn resolve(
    calib: &DeviceCalib,
    path: Option<&Path>,
    env: &dyn Fn(&str) -> Option<String>,
) -> (Vec<(usize, Pose)>, String) {
    let p = path.map(Path::to_path_buf).unwrap_or_else(default_path);
    if p.exists() {
        match load(&p) {
            Ok((uid, v)) if uid.is_empty() || uid == calib.uid => {
                let note = format!("extrinsics of cams {:?} from {}", v.iter().map(|x| x.0).collect::<Vec<_>>(), p.display());
                return (v, note);
            }
            Ok((uid, _)) => eprintln!("{}: for headset {uid}, not {}: ignored", p.display(), calib.uid),
            Err(e) => eprintln!("extrinsics not loaded: {e}"),
        }
    }
    match reference_for(calib, env) {
        Some(v) => {
            let note = format!(
                "extrinsics of cams {:?} from the shipped Dream Air reference (same headset)",
                v.iter().map(|x| x.0).collect::<Vec<_>>()
            );
            (v, note)
        }
        None => (Vec::new(), "factory extrinsics".to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::geom;

    #[test]
    fn roundtrip() {
        let p =
            std::env::temp_dir().join(format!("pimax-ctrltrack-ext-{}.json", std::process::id()));
        let pose = Pose {
            r: geom::rodrigues([0.01, -0.02, 0.03]),
            t: [0.1, 0.2, -0.3],
        };
        save(&p, "UID", &[(2, pose)], "test").unwrap();
        let (uid, v) = load(&p).unwrap();
        assert_eq!(uid, "UID");
        assert_eq!(v[0].0, 2);
        for i in 0..3 {
            assert!((v[0].1.t[i] - pose.t[i]).abs() < 1e-12);
            for j in 0..3 {
                assert!((v[0].1.r[i][j] - pose.r[i][j]).abs() < 1e-12);
            }
        }
        let _ = std::fs::remove_file(&p);
    }

    fn angle_deg(a: &Pose, b: &Pose) -> f64 {
        let d = geom::mat_mul(&geom::transpose(&a.r), &b.r);
        let tr = d[0][0] + d[1][1] + d[2][2];
        ((tr - 1.0) / 2.0).clamp(-1.0, 1.0).acos().to_degrees()
    }

    #[test]
    fn the_reference_fit_corrects_cams_1_to_3_by_the_documented_angles() {
        let (uid, v) = parse(REFERENCE_JSON).unwrap();
        assert_eq!(uid, "reference", "no serial in the shipped file");
        assert!(!REFERENCE_JSON.contains("/home/"), "no home path in the shipped file");
        let calib = DeviceCalib::parse(REFERENCE_XML).unwrap();
        let want = [(1, 0.46), (2, 1.45), (3, 3.20)];
        assert_eq!(v.iter().map(|x| x.0).collect::<Vec<_>>(), vec![1, 2, 3]);
        for ((id, fitted), (wid, deg)) in v.iter().zip(want) {
            assert_eq!(*id, wid);
            let factory = calib.body_from_cam(*id, RigConvention::CamFromCam0).unwrap().inverse();
            let a = angle_deg(&factory, fitted);
            assert!((a - deg).abs() < 0.01, "cam {id}: {a:.3} deg, want {deg}");
        }
    }

    #[test]
    fn the_reference_fit_applies_only_to_the_reference_headset() {
        let none = |_: &str| None;
        let calib = DeviceCalib::parse(REFERENCE_XML).unwrap();
        assert!(same_rig_as_reference(&calib));
        assert_eq!(reference_for(&calib, &none).map(|v| v.len()), Some(3));
        // the same headset under its own serial (the user's own XML): the rig decides, not the uid
        let own = DeviceCalib::parse(&REFERENCE_XML.replace("deviceUID=\"reference\"", "deviceUID=\"P511X0X000000000001\"")).unwrap();
        assert!(!own.is_reference());
        assert!(reference_for(&own, &none).is_some());
        // another headset (one camera rig differs): its factory extrinsics stay
        let mut other = calib.clone();
        other.cameras[3].rig_r[0][1] += 0.01;
        assert!(reference_for(&other, &none).is_none());
        // PIMAX_NO_REFERENCE=1 turns the reference off
        let off = |k: &str| (k == "PIMAX_NO_REFERENCE").then(|| "1".to_string());
        assert!(reference_for(&calib, &off).is_none());
    }

    #[test]
    fn resolve_prefers_the_users_own_fit_then_the_reference_then_factory() {
        let none = |_: &str| None;
        let calib = DeviceCalib::parse(REFERENCE_XML).unwrap();
        let dir = std::env::temp_dir().join(format!("pimax-ctrltrack-resolve-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let missing = dir.join("missing.json");
        // a clean install: no own file -> the shipped reference fit (the 2026-10-17 regression: factory extrinsics)
        let (v, note) = resolve(&calib, Some(&missing), &none);
        assert_eq!(v.len(), 3);
        assert!(note.contains("shipped Dream Air reference"), "{note}");
        // the user's own fit wins
        let own = dir.join("own.json");
        let pose = Pose { r: geom::rodrigues([0.0, 0.01, 0.0]), t: [0.0; 3] };
        save(&own, "", &[(2, pose)], "test").unwrap();
        let (v, note) = resolve(&calib, Some(&own), &none);
        assert_eq!(v.len(), 1);
        assert!(note.contains("own.json"), "{note}");
        // a fit for another headset is ignored, the reference applies
        save(&own, "P511X0X000000000002", &[(2, pose)], "test").unwrap();
        assert_eq!(resolve(&calib, Some(&own), &none).0.len(), 3);
        // another headset without a fit of its own: factory
        let mut other = calib.clone();
        other.cameras[2].rig_t[0] += 0.01;
        let (v, note) = resolve(&other, Some(&missing), &none);
        assert!(v.is_empty());
        assert_eq!(note, "factory extrinsics");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
