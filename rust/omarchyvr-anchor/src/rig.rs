//! The camera rig: KB4 intrinsics and `cam_from_body` (factory, with the controller-fit corrections when present).

use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::frame::TILE_COUNT;
use pimax_ctrltrack::{extrinsics, solve::Cam};
use std::path::Path;

pub fn load(calib: Option<&Path>, extrinsics_path: Option<&Path>, factory: bool) -> Result<Vec<Cam>, String> {
    let (c, _) = DeviceCalib::load_default(calib).map_err(|e| format!("calibration: {e:?}"))?;
    let mut cams: Vec<Cam> = (0..TILE_COUNT)
        .map(|i| {
            Ok(Cam {
                intr: c.cameras[i].intr.clone(),
                cam_from_body: c
                    .body_from_cam(i, RigConvention::CamFromCam0)
                    .ok_or("calibration has no IMU extrinsic")?
                    .inverse(),
            })
        })
        .collect::<Result<_, String>>()?;
    if !factory {
        let p = extrinsics_path
            .map(|p| p.to_path_buf())
            .unwrap_or_else(extrinsics::default_path);
        if p.exists() {
            if let Ok((uid, v)) = extrinsics::load(&p) {
                if uid.is_empty() || uid == c.uid {
                    for (id, pose) in v {
                        if id < cams.len() {
                            cams[id].cam_from_body = pose;
                        }
                    }
                }
            }
        }
    }
    Ok(cams)
}
