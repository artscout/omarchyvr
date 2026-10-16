//! Offline tests on recorded frames: golden eye images and the cam0/cam1 overlap check.
//!
//! Golden images: `tests/fixtures/golden_{left,right}.png`, rendered from the morning2 tiles (lit room, headset
//! still, 2026-10-14 08:08) at 296x320 per eye, depth 2 m, default FOV/IPD/eye centre. Regenerate after an
//! intended change with `PIMAX_PT_BLESS=1 cargo test -p omarchyvr-passthrough --test offline` and look at them.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom;
use omarchyvr_passthrough::check;
use omarchyvr_passthrough::frames::TileSet;
use omarchyvr_passthrough::lut::{Lut, LutParams};
use omarchyvr_passthrough::pngdec;
use omarchyvr_passthrough::remap;
use omarchyvr_passthrough::rig::{Rig, DEFAULT_HEAD_OFFSET_MM};
use omarchyvr_passthrough::view::{EyeView, Fov};
use std::path::PathBuf;

fn dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn rig() -> Rig {
    let c = DeviceCalib::load(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../pimax-cam/tests/fixtures/headset-calibration.xml"),
    )
    .unwrap();
    Rig::from_calib(&c, geom::scale(DEFAULT_HEAD_OFFSET_MM, 1e-3)).unwrap()
}

fn tiles() -> TileSet {
    TileSet::from_pngs(dir().join("morning2_tile").to_str().unwrap()).unwrap()
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn golden_eye_images() {
    let (w, h) = (296, 320);
    let r = rig();
    let t = tiles();
    let views = EyeView::default_pair(w, h, 0.063, Fov::dream_air_default());
    let bless = std::env::var_os("PIMAX_PT_BLESS").is_some();
    for (e, name) in ["left", "right"].iter().enumerate() {
        let lut = Lut::build(&r, &views[e], e, &LutParams::default());
        let mut img = vec![0u8; w * h];
        remap::remap_grey(&lut, &t.view(), &mut img, 0);
        let path = dir().join(format!("golden_{name}.png"));
        if bless {
            std::fs::write(&path, pimax_cam::png::encode(w, h, 1, &img)).unwrap();
            continue;
        }
        let g = pngdec::read(&path).expect("golden image missing: run with PIMAX_PT_BLESS=1");
        assert_eq!((g.width, g.height), (w, h));
        let diffs: Vec<u32> = img
            .iter()
            .zip(&g.pixels)
            .map(|(&a, &b)| (a as i32 - b as i32).unsigned_abs())
            .collect();
        let mean = diffs.iter().map(|&d| d as f64).sum::<f64>() / diffs.len() as f64;
        let big = diffs.iter().filter(|&&d| d > 2).count();
        // bit-exact today; the slack only absorbs floating-point differences between platforms
        assert!(
            mean < 0.05 && big < diffs.len() / 1000,
            "{name}: mean diff {mean}, {big} pixels > 2"
        );
    }
}

/// Real-image geometry check: rendering the left eye only from trackingA and only from trackingB must agree
/// (vertical offset ~0) and the horizontal offset must change with the reprojection depth like parallax does:
/// positive for too-near depths, about 0 near the room's depth (~3 m, the window wall), negative for infinity.
#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn trackinga_and_trackingb_agree_on_real_images() {
    let r = rig();
    let t = tiles();
    let eye = EyeView::default_pair(222, 240, 0.063, Fov::dream_air_default())[0];
    let near = check::overlap(&r, &eye, &t.view(), (0, 1), 1.0, 8);
    let room = check::overlap(&r, &eye, &t.view(), (0, 1), 3.0, 8);
    let far = check::overlap(&r, &eye, &t.view(), (0, 1), f64::INFINITY, 8);
    println!("near {near:?}\nroom {room:?}\nfar {far:?}");
    assert!(room.pixels > 3000);
    assert!(room.best_ncc > 0.4, "{room:?}");
    assert!(
        room.best_dx.abs() <= 1 && room.best_dy.abs() <= 2,
        "{room:?}"
    );
    assert!(room.ncc0 > far.ncc0 && room.ncc0 > near.ncc0);
    assert!(
        near.best_dx > room.best_dx && far.best_dx < room.best_dx,
        "parallax order: {near:?} {far:?}"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn replayed_recording_renders() {
    // the 2-frame evening fixture: raw .pxcam through the capture parse path, then both eyes
    let src = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../pimax-cam/tests/fixtures/evening2.pxcam");
    let mut cam =
        omarchyvr_passthrough::frames::open_source(src.to_str().unwrap(), false, false).unwrap();
    let mut t = TileSet::black();
    assert!(omarchyvr_passthrough::frames::next_tracking(&mut cam, 100, &mut t).unwrap());
    assert!(t.mono_ns > 0);
    let r = rig();
    let views = EyeView::default_pair(100, 108, 0.063, Fov::dream_air_default());
    for (e, v) in views.iter().enumerate() {
        let lut = Lut::build(&r, v, e, &LutParams::default());
        let mut img = vec![0u8; 100 * 108];
        remap::remap_grey(&lut, &t.view(), &mut img, 2);
        let mean = img.iter().map(|&x| x as f64).sum::<f64>() / img.len() as f64;
        assert!(mean > 5.0, "eye {e} mean {mean}");
    }
}
