//! End to end on synthetic frames: a textured room is rendered through a rig whose cameras are rotated away from
//! the factory calibration by known angles; the pipeline, given only the factory calibration and the tiles, must
//! find those rotations, pass its own sanity gate, and produce an overlay that reproduces the true rig.
//! A second case injects an error the rotation model cannot represent (a displaced camera) and checks that the
//! tool says so instead of writing a wrong calibration.
#![allow(clippy::needless_range_loop)]

use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::geom::{self, Vec3};
use pimax_cam::overlay::{self, CalibOverlay};
use pimax_selfcal::diag::Verdict;
use pimax_selfcal::pipeline::{self, EstimateOptions, GateLimits};
use pimax_selfcal::scene::Scene;
use pimax_selfcal::synth::{self, Room};

fn factory() -> DeviceCalib {
    DeviceCalib::parse(include_str!(
        "../../pimax-cam/tests/fixtures/headset-calibration.xml"
    ))
    .unwrap()
}

fn deg(v: Vec3) -> Vec3 {
    [v[0].to_radians(), v[1].to_radians(), v[2].to_radians()]
}

fn render_all(truth: &DeviceCalib) -> Vec<Scene> {
    let room = Room::new(7);
    synth::default_poses()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            synth::render(
                truth,
                &room,
                p,
                1.5,
                100 + i as u64,
                &format!("synthetic{i}"),
            )
        })
        .collect()
}

#[test]
fn injected_rotations_are_recovered() {
    let f = factory();
    // 0.44, 0.51 and 0.56 deg: the size of the error seen on the real headset and a bit more
    let w_true = vec![
        [0.0; 3],
        deg([0.30, -0.25, 0.20]),
        deg([-0.35, 0.20, 0.30]),
        deg([0.25, 0.30, -0.40]),
    ];
    let truth = pipeline::corrected(&f, &w_true);
    let scenes = render_all(&truth);
    let opt = EstimateOptions::default();
    let est = pipeline::estimate(&f, &[[0.0; 3]; 4], &scenes, &opt).expect("estimate");
    println!("{}", pipeline::report(&f, &est, &scenes));

    // every pair is usable and every camera is estimated
    assert_eq!(est.free, vec![1, 2, 3]);
    assert!(est.not_estimated.is_empty());
    for p in &est.pairs {
        assert!(p.accepted, "pair {}-{}: {}", p.a, p.b, p.why);
        assert!(
            matches!(p.verdict, Verdict::Rotation | Verdict::Fine),
            "pair {}-{}: {:?}",
            p.a,
            p.b,
            p.verdict
        );
    }
    // the rotations, within 0.03 deg and within 4 sigma
    for c in 1..4 {
        let err = geom::norm(geom::sub(est.w[c], w_true[c])).to_degrees();
        let sg = est.sigma_max_deg(c).unwrap();
        assert!(err < 0.03, "camera {c}: error {err:.4} deg (sigma {sg:.4})");
        assert!(sg < 0.02, "camera {c}: sigma {sg:.4} deg");
    }
    // measured on re-rectified, re-matched images: the vertical disparity is gone
    let (before, after) = (
        pipeline::used_stats(&est, &est.before),
        pipeline::used_stats(&est, &est.after),
    );
    assert!(
        before.n > 1500 && after.n >= before.n * 9 / 10,
        "{} -> {}",
        before.n,
        after.n
    );
    assert!(before.median_abs > 0.8, "before: {}", before.line());
    assert!(
        after.median_abs < 0.08 && after.within_1 > 0.97,
        "after: {}",
        after.line()
    );
    assert!(after.median.abs() < 0.03, "after: {}", after.line());
    // each pair on its own too
    for &(a, b) in &opt.pairs {
        let (pb, pa) = (
            pipeline::pooled(&est.before, a, b),
            pipeline::pooled(&est.after, a, b),
        );
        assert!(
            pa.n > 100 && pa.median_abs < 0.1 && pa.median_abs < 0.3 * pb.median_abs,
            "pair {a}-{b}: {} -> {}",
            pb.line(),
            pa.line()
        );
    }
    // held-out image blocks improve as well
    let cv = est.cv.expect("cross-validation");
    assert!(
        cv.folds >= 4 && cv.after.median_abs < 0.25 * cv.before.median_abs,
        "{cv:?}"
    );
    // the sanity gate lets it through
    let why = pipeline::gate(&est, &GateLimits::default());
    assert!(why.is_empty(), "gate: {why:?}");

    // the overlay reproduces the true rig when applied to the factory calibration, also after a JSON round trip
    let ov = pipeline::to_overlay(&f, &est, "test", "synthetic");
    let back = CalibOverlay::parse(&ov.to_json()).unwrap();
    assert_eq!(back.rotations.len(), 3);
    let learned = f
        .with_overlay(&back, overlay::DEFAULT_MAX_ANGLE_DEG)
        .unwrap();
    let conv = RigConvention::CamFromCam0;
    for c in 1..4 {
        let d = geom::mat_mul(
            &learned.cam_from_cam0(c, conv).r,
            &geom::transpose(&truth.cam_from_cam0(c, conv).r),
        );
        let ang = pimax_selfcal::so3::log(&d);
        assert!(geom::norm(ang).to_degrees() < 0.03, "camera {c}");
        // the camera centre did not move
        let (p, q) = (
            learned.cam_from_cam0(c, conv).inverse().t,
            f.cam_from_cam0(c, conv).inverse().t,
        );
        assert!(geom::norm(geom::sub(p, q)) < 1e-9);
    }

    // starting from the learned overlay, a second run finds nothing left to correct
    let again = pipeline::estimate(&f, &est.w, &scenes, &opt).expect("second run");
    for c in 1..4 {
        let step = geom::norm(geom::sub(again.w[c], est.w[c])).to_degrees();
        assert!(
            step < 0.02,
            "camera {c} moved by {step:.4} deg on the second run"
        );
    }
    let b2 = pipeline::used_stats(&again, &again.before);
    assert!(b2.median_abs < 0.08, "already calibrated: {}", b2.line());
}

#[test]
fn a_displaced_camera_is_reported_not_absorbed() {
    let f = factory();
    // camera 1 sits 6 mm from where the factory calibration says (and nothing is rotated): a rotation cannot
    // describe that, the vertical disparity depends on depth
    let mut truth = f.clone();
    let conv = RigConvention::CamFromCam0;
    let c1 = f.cam_from_cam0(1, conv).inverse().t;
    let moved = geom::add(c1, [0.0, 0.006, 0.0]);
    truth.cameras[1].rig_t = geom::scale(geom::mat_vec(&f.cameras[1].rig_r, moved), -1.0);
    let scenes = render_all(&truth);
    let opt = EstimateOptions {
        pairs: vec![(0, 1)],
        ..EstimateOptions::default()
    };
    let est = pipeline::estimate(&f, &[[0.0; 3]; 4], &scenes, &opt).expect("estimate");
    println!("{}", pipeline::report(&f, &est, &scenes));
    let d = &est.pairs[0];
    assert!(
        matches!(
            d.verdict,
            Verdict::RotationAndTranslation | Verdict::Ambiguous
        ),
        "{:?}",
        d.verdict
    );
    // the baseline-direction terms of the combined model see the displacement: 6 mm on 111.6 mm
    let both = d
        .fits
        .iter()
        .find(|x| x.model == pimax_selfcal::diag::Model::Both)
        .unwrap();
    let size = (both.params[3].powi(2) + both.params[4].powi(2)).sqrt();
    assert!(
        (size - 0.006 / 0.1116).abs() < 0.012,
        "baseline direction error {size:.4}"
    );
    // and the gate refuses to install a rotation for it
    let why = pipeline::gate(&est, &GateLimits::default());
    assert!(
        why.iter().any(|w| w.contains("baseline")),
        "gate must name the baseline problem: {why:?}"
    );
}
