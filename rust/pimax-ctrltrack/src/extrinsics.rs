//! Camera extrinsic corrections fitted on controller poses (`track --fit-extrinsics --save-extrinsics P`).
//! Stores the full corrected `cam_from_body` of each refitted camera; `live`/`track` load them with `--extrinsics`
//! (default `~/.config/pimax/ctrl-cam-extrinsics.json` when it exists).

use pimax_cam::geom::Pose;
use pimax_cam::json;
use std::path::{Path, PathBuf};

pub fn default_path() -> PathBuf {
    let h = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_default();
    h.join(".config/pimax/ctrl-cam-extrinsics.json")
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
    let v = json::parse(&t).map_err(|e| format!("{}: {e:?}", path.display()))?;
    if v.get("format").and_then(|f| f.as_str()) != Some("pimax-ctrltrack-extrinsics") {
        return Err(format!(
            "{}: not a pimax-ctrltrack-extrinsics file",
            path.display()
        ));
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
}
