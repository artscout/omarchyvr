//! Synthetic tag images (known pose) through the KB4 fisheye model -> detection -> pose, in `cargo test`.
use pimax_anchor::detect::{self, CamRect, Detector};
use pimax_anchor::tag::{self, Family};
use pimax_cam::geom::{self, Pose};
use pimax_cam::kb4::Kb4;
use pimax_ctrltrack::solve::Cam;

fn kb() -> Kb4 {
    // close to the real OV7251 fisheye: ~230 px focal length, 640x480
    Kb4::new(640, 480, 233.0, 233.0, 320.0, 240.0, [0.02, -0.01, 0.003, -0.0005], vec![])
}

/// Two cameras 13 cm apart looking forward and 29 deg down (body = x right, y up, z back; cameras OpenCV axes).
fn rig() -> Vec<Cam> {
    let mut v = Vec::new();
    for sx in [-0.065, 0.065] {
        // camera axes in body: x right, y down = -body y, z forward = -body z; then pitched down by 29 deg
        let pitch = geom::rodrigues([-(29f64.to_radians()), 0.0, 0.0]);
        let flip = [[1.0, 0.0, 0.0], [0.0, -1.0, 0.0], [0.0, 0.0, -1.0]];
        let body_from_cam = Pose {
            r: geom::mat_mul(&flip, &pitch),
            t: [sx, 0.0, 0.0],
        };
        v.push(Cam {
            intr: kb(),
            cam_from_body: body_from_cam.inverse(),
        });
    }
    v
}

fn lookat_tag(body_pos: [f64; 3], yaw_deg: f64, tilt_deg: f64) -> Pose {
    // tag lies on a desk (normal = body +y) in front of the head, rotated by yaw about the vertical and tilted
    let up_to_y = geom::rodrigues([-std::f64::consts::FRAC_PI_2, 0.0, 0.0]); // tag z -> body y
    let r = geom::mat_mul(
        &geom::rodrigues([0.0, yaw_deg.to_radians(), 0.0]),
        &geom::mat_mul(&geom::rodrigues([tilt_deg.to_radians(), 0.0, 0.0]), &up_to_y),
    );
    Pose { r, t: body_pos }
}

struct Stats {
    pos_mm: f64,
    rot_deg: f64,
}

fn run(body_from_tag: &Pose, size: f64, cams_used: &[usize], seed: u64) -> Option<Stats> {
    let fam = Family::tag36h11();
    let (w, raster) = fam.raster(0);
    let cams = rig();
    let rects: Vec<CamRect> = cams.iter().map(|c| CamRect::new(&c.intr)).collect();
    let det = Detector::new(2);
    let mut dets = Vec::new();
    for &ci in cams_used {
        let cft = cams[ci].cam_from_body.compose(body_from_tag);
        let img = tag::render(&cams[ci].intr, &cft, size, (w, &raster), seed + ci as u64);
        for d in det.detect(&rects[ci], &img) {
            if d.id == 0 {
                dets.push((ci, d));
            }
        }
    }
    if dets.is_empty() {
        return None;
    }
    let rr: Vec<&CamRect> = rects.iter().collect();
    let fix = detect::solve(&cams, &rr, &dets, size)?;
    let (dt, dr) = detect::pose_error(&fix.body_from_tag, body_from_tag);
    println!(
        "  cams {:?} -> {} dets, rms {:.2} px, err {:.2} mm {:.3} deg{}",
        cams_used,
        dets.len(),
        fix.rms_px,
        dt * 1e3,
        dr,
        if fix.ambiguous { " (ambiguous)" } else { "" }
    );
    Some(Stats {
        pos_mm: dt * 1e3,
        rot_deg: dr,
    })
}

#[test]
fn tag_pose_two_cameras() {
    let mut worst_p: f64 = 0.0;
    let mut worst_r: f64 = 0.0;
    let mut n = 0;
    for (i, (pos, yaw, tilt)) in [
        ([0.0, -0.35, -0.55], 0.0, 0.0),
        ([0.05, -0.40, -0.60], 25.0, 0.0),
        ([-0.10, -0.40, -0.50], -40.0, 5.0),
        ([0.0, -0.30, -0.70], 70.0, -8.0),
        ([0.12, -0.45, -0.45], 150.0, 3.0),
    ]
    .iter()
    .enumerate()
    {
        let tag = lookat_tag(*pos, *yaw, *tilt);
        println!("pose {i}: {pos:?} yaw {yaw} tilt {tilt}");
        let s = run(&tag, 0.15, &[0, 1], 7 + i as u64).expect("tag not detected");
        worst_p = worst_p.max(s.pos_mm);
        worst_r = worst_r.max(s.rot_deg);
        n += 1;
    }
    println!("TEST NUMBERS two cameras: {n} poses, worst position error {worst_p:.2} mm, worst rotation error {worst_r:.3} deg");
    assert!(worst_p < 5.0, "position error {worst_p} mm");
    assert!(worst_r < 1.5, "rotation error {worst_r} deg");
}

#[test]
fn tag_pose_single_camera() {
    let tag = lookat_tag([0.03, -0.38, -0.60], 30.0, 0.0);
    let s = run(&tag, 0.15, &[0], 3).expect("tag not detected");
    println!("TEST NUMBERS one camera: {:.2} mm, {:.3} deg", s.pos_mm, s.rot_deg);
    assert!(s.pos_mm < 10.0 && s.rot_deg < 4.0);
}

/// Two sessions with different SLAM origins and the same real tag: calibrate in A, apply in B. The head, held at the
/// same real place, must end up at the same app-world pose in both. Goes through the ring file and the offset file.
#[test]
fn anchor_round_trip_between_sessions() {
    use pimax_anchor::world::{self, HeadRing};
    let size = 0.15;
    let cams = rig();
    let fam = Family::tag36h11();
    let (w, raster) = fam.raster(0);
    let rects: Vec<CamRect> = cams.iter().map(|c| CamRect::new(&c.intr)).collect();
    let det = Detector::new(2);
    let rr: Vec<&CamRect> = rects.iter().collect();

    // real life: head at H_real (room frame), tag at T_real (room frame)
    let head_real = Pose {
        r: geom::rodrigues([0.0, 0.3, 0.0]),
        t: [1.0, 1.6, 0.5],
    };
    let tag_head = lookat_tag([0.02, -0.40, -0.58], 20.0, 0.0); // body_from_tag as the cameras see it
    let tag_real = head_real.compose(&tag_head);
    // session A / B: SLAM world = room shifted and yawed by different amounts (both gravity aligned)
    let slam_from_room = |yaw: f64, t: [f64; 3]| Pose {
        r: geom::rodrigues([0.0, yaw.to_radians(), 0.0]),
        t,
    };
    let sa = slam_from_room(0.0, [0.0, 0.0, 0.0]);
    let sb = slam_from_room(37.0, [0.8, -0.1, -1.4]);
    let measure = |s: &Pose, seed: u64, hr: &Pose| -> Pose {
        // the headset moves a little between calibrate and apply: cameras see the tag from hr
        let body_from_tag = hr.inverse().compose(&tag_real);
        let mut dets = Vec::new();
        for ci in 0..2 {
            let img = tag::render(&cams[ci].intr, &cams[ci].cam_from_body.compose(&body_from_tag), size, (w, &raster), seed + ci as u64);
            for d in det.detect(&rects[ci], &img) {
                dets.push((ci, d));
            }
        }
        let fix = detect::solve(&cams, &rr, &dets, size).unwrap();
        // head ring as Monado would write it: raw slam head pose at the frame time (one sample before, one after)
        let w_from_head = s.compose(hr);
        let ring = world::ring_bytes(42, &[(1_000_000_000, w_from_head, true), (1_040_000_000, w_from_head, true)]);
        let path = std::env::temp_dir().join(format!("pimax-anchor-test-{}-{seed}.head", std::process::id()));
        std::fs::write(&path, ring).unwrap();
        let r = HeadRing::read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        r.at(1_020_000_000, 100_000_000).unwrap().compose(&fix.body_from_tag)
    };
    let hr_a = head_real;
    let hr_b = Pose {
        r: geom::mat_mul(&geom::rodrigues([0.0, 0.05, 0.0]), &head_real.r),
        t: [1.03, 1.58, 0.46],
    };
    let w_from_tag_a = measure(&sa, 11, &hr_a);
    let reference = w_from_tag_a; // calibrate (no offset active)
    let w_from_tag_b = measure(&sb, 21, &hr_b);
    let off = world::compute_offset(&reference, &w_from_tag_b, true);
    // offset file round trip
    let f = std::env::temp_dir().join(format!("pimax-anchor-test-{}.offset", std::process::id()));
    world::write_offset(&f, 42, &off).unwrap();
    let (pid, off2) = world::read_offset(&f).unwrap();
    let _ = std::fs::remove_file(&f);
    assert_eq!(pid, 42);
    // where does the app see a head held at head_real in session B, vs. in session A?
    let app_a = sa.compose(&head_real);
    let app_b = off2.compose(&sb.compose(&head_real));
    let (dt, dr) = detect::pose_error(&app_a, &app_b);
    println!("TEST NUMBERS round trip (different SLAM origin: 37 deg yaw, 1.6 m shift): head lands {:.2} mm, {:.3} deg from the calibrated place", dt * 1e3, dr);
    assert!(dt < 0.006 && dr < 1.0);
}

#[test]
fn offset_math_exact() {
    use pimax_anchor::world;
    let reference = Pose { r: geom::rodrigues([0.1, 0.4, -0.05]), t: [0.3, -0.2, 0.9] };
    let now = Pose { r: geom::rodrigues([0.1, -0.7, -0.05]), t: [-1.0, 0.1, 0.2] };
    let full = world::compute_offset(&reference, &now, false);
    let (dt, dr) = detect::pose_error(&reference, &full.compose(&now));
    assert!(dt < 1e-12 && dr < 1e-6);
    // gravity-aligned worlds differ by yaw + shift only: yaw-only recovers exactly
    let yawed = Pose { r: geom::mat_mul(&geom::rodrigues([0.0, 0.6, 0.0]), &now.r), t: [0.5, 0.4, -0.3] };
    let off = world::compute_offset(&yawed, &now, true);
    let (dt, dr) = detect::pose_error(&yawed, &off.compose(&now));
    assert!(dt < 1e-9 && dr < 1e-6, "{dt} {dr}");
}

#[test]
fn config_and_printables() {
    use pimax_anchor::config::{Anchor, WindowRel};
    let a = Anchor {
        id: 0,
        size_m: 0.15,
        reference: Pose { r: geom::rodrigues([0.0, 0.5, 0.0]), t: [0.1, 0.7, -0.4] },
        saved: "unix 1".into(),
        windows: vec![WindowRel { name: "term".into(), tag_from_window: Pose { r: geom::rodrigues([0.1, 0.0, 0.0]), t: [0.0, 0.3, 0.2] } }],
    };
    let b = Anchor::parse(&a.to_toml()).unwrap();
    assert_eq!(b.windows.len(), 1);
    assert!(detect::pose_error(&a.reference, &b.reference).0 < 1e-5);
    assert!(detect::pose_error(&a.reference, &b.reference).1 < 1e-3);
    let fam = Family::tag36h11();
    assert_eq!(fam.ncodes(), 587);
    let svg = tag::svg(&fam, 0, 150.0);
    assert!(svg.contains("150 mm") && svg.contains("</svg>"));
    let pdf = tag::pdf(&fam, 0, 150.0);
    assert!(pdf.starts_with(b"%PDF-1.4") && pdf.ends_with(b"%%EOF\n"));
}

/// Same with the factory calibration of the real headset (skipped when the XML is not around).
#[test]
fn tag_pose_real_calibration() {
    let Ok(cams) = pimax_anchor::rig::load(None, None, true) else {
        eprintln!("no calibration XML: skipped");
        return;
    };
    let fam = Family::tag36h11();
    let (w, raster) = fam.raster(0);
    let rects: Vec<CamRect> = cams.iter().map(|c| CamRect::new(&c.intr)).collect();
    let rr: Vec<&CamRect> = rects.iter().collect();
    let det = Detector::new(2);
    let (mut worst_p, mut worst_r, mut found) = (0.0f64, 0.0f64, 0);
    for (i, (pos, yaw)) in [([0.0, -0.35, -0.55], 10.0), ([0.1, -0.4, -0.6], -30.0), ([-0.08, -0.3, -0.5], 80.0)].iter().enumerate() {
        let tag = lookat_tag(*pos, *yaw, 0.0);
        let mut dets = Vec::new();
        for ci in 0..4 {
            let img = tag::render(&cams[ci].intr, &cams[ci].cam_from_body.compose(&tag), 0.15, (w, &raster), 5 + ci as u64 + i as u64);
            for d in det.detect(&rects[ci], &img) {
                dets.push((ci, d));
            }
        }
        if dets.is_empty() {
            continue; // not in view of any camera at this spot
        }
        let fix = detect::solve(&cams, &rr, &dets, 0.15).unwrap();
        let (dt, dr) = detect::pose_error(&fix.body_from_tag, &tag);
        println!("  real calib pose {i}: cams {:?}, rms {:.2} px, err {:.2} mm {:.3} deg", fix.cams, fix.rms_px, dt * 1e3, dr);
        worst_p = worst_p.max(dt * 1e3);
        worst_r = worst_r.max(dr);
        found += 1;
    }
    println!("TEST NUMBERS real calibration: {found} of 3 poses visible, worst {worst_p:.2} mm, {worst_r:.3} deg");
    assert!(worst_p < 6.0 && worst_r < 1.5);
}
