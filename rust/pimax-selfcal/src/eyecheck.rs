//! Eye-view check: what the calibration error looks like in the passthrough, and whether a correction removes it.
//!
//! The passthrough (`pimax-passthrough`) renders an eye's view from the camera on that eye's side. Rendering the
//! same eye view once only from camera A and once only from camera B must give the same picture where both see
//! the scene (up to horizontal parallax for things not at the reprojection depth). A relative rotation between
//! the cameras shows as a shift between the two pictures; the vertical part of it is what the two eyes of the
//! user see as vertical disparity, since each eye is fed by its own camera. This is measured by image correlation
//! in eye pixels, a method that shares nothing with the corner matcher and the bundle adjustment.

use crate::scene::Scene;
use crate::viz::Rgb;
use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom;
use pimax_passthrough::check;
use pimax_passthrough::lut::{Lut, LutParams};
use pimax_passthrough::remap;
use pimax_passthrough::rig::{Rig, DEFAULT_HEAD_OFFSET_MM};
use pimax_passthrough::view::{EyeView, Fov};

/// Geometry of the check.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeParams {
    /// Eye image width (pixels).
    pub width: usize,
    /// Eye image height.
    pub height: usize,
    /// Search range of the shift (eye pixels).
    pub search: i32,
    /// Interpupillary distance (metres).
    pub ipd_m: f64,
}

impl Default for EyeParams {
    fn default() -> Self {
        // the passthrough's offline resolution halved: 0.31 deg per pixel, shifts refined to ~0.1 px
        EyeParams {
            width: 444,
            height: 480,
            search: 10,
            ipd_m: 0.063,
        }
    }
}

impl EyeParams {
    /// Degrees per eye pixel at the centre of the view.
    pub fn deg_per_px(&self) -> f64 {
        let f = Fov::dream_air_default();
        ((f.right.tan() - f.left.tan()) / self.width as f64).to_degrees()
    }
}

/// Shift between the two single-camera renders of one eye view.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EyeShift {
    /// Eye (0 = left, 1 = right).
    pub eye: usize,
    /// Reprojection depth (metres).
    pub depth_m: f64,
    /// Horizontal shift of camera b's picture relative to camera a's (degrees; parallax plus calibration).
    pub dx_deg: f64,
    /// Vertical shift (degrees): the calibration error the eyes see.
    pub dy_deg: f64,
    /// Correlation at the best shift.
    pub ncc: f64,
    /// Correlation without any shift.
    pub ncc0: f64,
    /// Pixels compared.
    pub pixels: usize,
}

fn rig_of(cal: &DeviceCalib) -> Result<Rig, String> {
    Rig::from_calib(cal, geom::scale(DEFAULT_HEAD_OFFSET_MM, 1e-3))
}

fn tiles_of(scene: &Scene) -> Result<remap::Tiles<'_>, String> {
    if scene.tiles.len() < 4 {
        return Err(format!(
            "scene {} has {} tiles",
            scene.name,
            scene.tiles.len()
        ));
    }
    Ok([
        &scene.tiles[0],
        &scene.tiles[1],
        &scene.tiles[2],
        &scene.tiles[3],
    ])
}

/// Measure the shift between cameras `cams.0` and `cams.1` in `eye`'s view at `depth_m`.
pub fn shift(
    cal: &DeviceCalib,
    scene: &Scene,
    eye: usize,
    cams: (usize, usize),
    depth_m: f64,
    p: &EyeParams,
) -> Result<EyeShift, String> {
    let rig = rig_of(cal)?;
    let view =
        EyeView::default_pair(p.width, p.height, p.ipd_m, Fov::dream_air_default())[eye.min(1)];
    let o = check::overlap(&rig, &view, &tiles_of(scene)?, cams, depth_m, p.search);
    let k = p.deg_per_px();
    Ok(EyeShift {
        eye,
        depth_m,
        dx_deg: o.sub_dx * k,
        dy_deg: o.sub_dy * k,
        ncc: o.best_ncc,
        ncc0: o.ncc0,
        pixels: o.pixels,
    })
}

/// The depth (of [`check::SWEEP_DEPTHS`]) at which the two renders agree best, i.e. the distance of the bulk of
/// the scene. `None` when the two cameras share too little of the view.
pub fn best_depth(
    cal: &DeviceCalib,
    scene: &Scene,
    eye: usize,
    cams: (usize, usize),
    p: &EyeParams,
) -> Result<Option<f64>, String> {
    let mut best: Option<(f64, f64)> = None;
    for &d in &check::SWEEP_DEPTHS {
        let s = shift(cal, scene, eye, cams, d, p)?;
        if s.pixels >= 2000 && best.is_none_or(|b| s.ncc > b.1) {
            best = Some((d, s.ncc));
        }
    }
    Ok(best.map(|b| b.0))
}

/// Both eyes of one scene with two calibrations (`before`, `after`), cameras 0 and 1, at the depth where the
/// renders of the `before` calibration agree best (left eye). Empty when the cameras share too little of the view.
pub fn before_after(
    before: &DeviceCalib,
    after: &DeviceCalib,
    scene: &Scene,
    p: &EyeParams,
) -> Result<Vec<(EyeShift, EyeShift)>, String> {
    if before.cameras.len() < 2 {
        return Ok(Vec::new());
    }
    let Some(depth) = best_depth(before, scene, 0, (0, 1), p)? else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for eye in 0..2 {
        out.push((
            shift(before, scene, eye, (0, 1), depth, p)?,
            shift(after, scene, eye, (0, 1), depth, p)?,
        ));
    }
    Ok(out)
}

/// The two single-camera renders of `eye`'s view (camera `cams.0` only, camera `cams.1` only), grey,
/// `p.width` x `p.height`.
pub fn renders(
    cal: &DeviceCalib,
    scene: &Scene,
    eye: usize,
    cams: (usize, usize),
    depth_m: f64,
    p: &EyeParams,
) -> Result<(Vec<u8>, Vec<u8>), String> {
    let rig = rig_of(cal)?;
    let view =
        EyeView::default_pair(p.width, p.height, p.ipd_m, Fov::dream_air_default())[eye.min(1)];
    let tiles = tiles_of(scene)?;
    let single = |cam: usize| {
        let l = Lut::build(
            &rig,
            &view,
            eye.min(1),
            &LutParams {
                depth_m,
                order: Some(vec![cam]),
                ..LutParams::default()
            },
        );
        let mut g = vec![0u8; p.width * p.height];
        remap::remap_grey(&l, &tiles, &mut g, 0);
        g
    };
    Ok((single(cams.0), single(cams.1)))
}

/// Red/cyan picture of `eye`'s view: camera `cams.0` only in red, camera `cams.1` only in cyan. Grey where they
/// agree; coloured fringes where one is displaced.
pub fn anaglyph(
    cal: &DeviceCalib,
    scene: &Scene,
    eye: usize,
    cams: (usize, usize),
    depth_m: f64,
    p: &EyeParams,
) -> Result<Rgb, String> {
    let (a, b) = renders(cal, scene, eye, cams, depth_m, p)?;
    Ok(crate::viz::anaglyph(&a, &b, p.width, p.height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pipeline;
    use crate::synth::{self, Room};

    #[test]
    fn a_camera_rotation_shows_as_a_shift_and_the_correction_removes_it() {
        let f = DeviceCalib::parse(include_str!(
            "../../pimax-cam/tests/fixtures/headset-calibration.xml"
        ))
        .unwrap();
        // camera 1 pitched by 0.9 deg about its x axis (image rows move by ~3.7 px in the tile)
        let w = vec![
            [0.0; 3],
            [0.9f64.to_radians(), 0.0, 0.0],
            [0.0; 3],
            [0.0; 3],
        ];
        let truth = pipeline::corrected(&f, &w);
        let scene = synth::render(
            &truth,
            &Room::new(3),
            &synth::default_poses()[0],
            1.0,
            1,
            "s",
        );
        let p = EyeParams {
            width: 222,
            height: 240,
            search: 6,
            ..EyeParams::default()
        };
        assert!((p.deg_per_px() - 0.615).abs() < 0.01, "{}", p.deg_per_px());
        // the walls in view are 2.5-3 m away
        let wrong = shift(&f, &scene, 0, (0, 1), 3.0, &p).unwrap();
        let right = shift(&truth, &scene, 0, (0, 1), 3.0, &p).unwrap();
        assert!(wrong.pixels > 2000 && right.pixels > 2000);
        assert!(right.dy_deg.abs() < 0.12, "true calibration: {right:?}");
        assert!(
            wrong.dy_deg.abs() > 0.5 && wrong.dy_deg.abs() < 1.3,
            "factory calibration on a rotated camera: {wrong:?}"
        );
        assert!(right.ncc0 > wrong.ncc0, "{} vs {}", right.ncc0, wrong.ncc0);
        let a = anaglyph(&f, &scene, 0, (0, 1), 3.0, &p).unwrap();
        assert_eq!((a.w, a.h), (222, 240));
        assert!(best_depth(&truth, &scene, 0, (0, 1), &p).unwrap().is_some());
        let bad = Scene {
            name: "x".into(),
            tiles: vec![],
        };
        assert!(shift(&f, &bad, 0, (0, 1), 3.0, &p).is_err());
    }
}
