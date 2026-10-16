//! Real frames: the morning2 tiles (lit room, headset still on the desk, 2026-10-14 08:08; the same files as
//! `analysis/overnight/cam/morning2/` and the passthrough fixtures).
//!
//! What this scene supports, and what these tests pin down:
//! * trackingA/B (pair 0-1) has ~75 good matches with a vertical disparity of -0.67 px median: estimating camera
//!   1's rotation from them lowers it on the same scene (re-rectified and re-matched), and lowers the vertical
//!   mismatch between the two cameras in the passthrough eye view (a different method on different pixels).
//! * It is not enough to install a calibration: the matches lie in two clusters (the room at ~2 m, the desk at
//!   ~0.4 m), so a rotation cannot be told from a baseline error and the yaw-like component is poorly determined.
//!   The sanity gate must refuse, and say why.
//! * The controller-camera pairs have too few or inconsistent matches (overexposed frames): they must not be
//!   used, so cameras 2 and 3 are not estimated.
#![allow(clippy::needless_range_loop)]

use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom;
use pimax_selfcal::eyecheck::{self, EyeParams};
use pimax_selfcal::pipeline::{self, EstimateOptions, GateLimits};
use pimax_selfcal::scene;
use std::path::PathBuf;

fn factory() -> DeviceCalib {
    DeviceCalib::parse(include_str!(
        "../../pimax-cam/tests/fixtures/headset-calibration.xml"
    ))
    .unwrap()
}

fn morning2() -> scene::Scene {
    let p = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../omarchyvr-passthrough/tests/fixtures/morning2_tile");
    scene::from_prefix(p.to_str().unwrap()).unwrap()
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn trackinga_b_improves_on_the_morning_scene_but_the_gate_refuses() {
    let f = factory();
    let scenes = vec![morning2()];
    let est = pipeline::estimate(&f, &[[0.0; 3]; 4], &scenes, &EstimateOptions::default())
        .expect("estimate");
    println!("{}", pipeline::report(&f, &est, &scenes));

    // only the tracking pair is usable; the controller cameras are left alone
    assert_eq!(est.used_pairs, vec![(0, 1)]);
    assert_eq!(est.free, vec![1]);
    assert_eq!(est.not_estimated, vec![2, 3]);
    for c in [2, 3] {
        assert_eq!(est.w[c], [0.0; 3]);
    }

    // a small correction, a few tenths of a degree
    let ang = geom::norm(est.w[1]).to_degrees();
    assert!(ang > 0.1 && ang < 1.0, "camera 1 correction {ang:.3} deg");

    // the same scene re-rectified with the correction and matched again: the vertical disparity shrinks
    let (b, a) = (
        pipeline::pooled(&est.before, 0, 1),
        pipeline::pooled(&est.after, 0, 1),
    );
    println!("before {}\nafter  {}", b.line(), a.line());
    assert!(b.n >= 60 && a.n >= 55, "{} / {} matches", b.n, a.n);
    assert!(b.median < -0.5, "before: {}", b.line());
    assert!(a.median.abs() < 0.3, "after: {}", a.line());
    assert!(
        a.median_abs < b.median_abs - 0.1,
        "{} -> {}",
        b.median_abs,
        a.median_abs
    );
    assert!(
        a.within_1 > b.within_1 + 0.1,
        "{} -> {}",
        b.within_1,
        a.within_1
    );

    // the passthrough eye view (camera 0 only vs camera 1 only, left eye, the room at 3 m): the vertical
    // mismatch the eyes would see shrinks too
    let p = EyeParams {
        width: 222,
        height: 240,
        search: 6,
        ..EyeParams::default()
    };
    let corrected = pipeline::corrected(&f, &est.w);
    let eb = eyecheck::shift(&f, &scenes[0], 0, (0, 1), 3.0, &p).unwrap();
    let ea = eyecheck::shift(&corrected, &scenes[0], 0, (0, 1), 3.0, &p).unwrap();
    println!("eye view before {eb:?}\neye view after  {ea:?}");
    assert!(eb.pixels > 3000);
    assert!(eb.dy_deg < -0.3, "before: {eb:?}");
    assert!(ea.dy_deg.abs() < eb.dy_deg.abs() - 0.15, "{eb:?} -> {ea:?}");

    // but it must not be installed: too few matches, poorly determined, rotation and baseline mixed up
    let why = pipeline::gate(&est, &GateLimits::default());
    println!("gate: {why:?}");
    assert!(why.iter().any(|w| w.contains("uncertainty")), "{why:?}");
    assert!(
        why.iter().any(|w| w.contains("inlier observations")),
        "{why:?}"
    );
    assert!(
        why.iter().any(|w| w.contains("cannot be told apart")),
        "{why:?}"
    );
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn the_lens_circle_sits_on_the_principal_point() {
    // trackingA's image circle (all four corners lit in this scene) is centred within a few pixels of the
    // factory principal point: the tiles are cut where the calibration expects them
    let f = factory();
    let s = morning2();
    let k = &f.cameras[0].intr;
    let c = pimax_selfcal::circle::fit(&s.tiles[0], 640, 480, k.cx, k.cy).expect("circle");
    println!("{c:?}");
    assert!(c.corners >= 3 && c.rms < 1.5, "{c:?}");
    assert!(
        (c.cx - k.cx).abs() < 6.0 && (c.cy - k.cy).abs() < 6.0,
        "{c:?} vs ({}, {})",
        k.cx,
        k.cy
    );
    assert!((c.r - 360.0).abs() < 15.0, "{c:?}");
}
