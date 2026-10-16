//! End-to-end tests on the synthetic room with the real factory calibration.

use crate::map::{tilt_of, RelocParams, RoomMap};
use crate::orb::{Image, OrbParams};
use crate::session::{Alignment, Event, RoomSession, SessionParams};
use crate::stereo::{features, triangulate, Rig, StereoParams};
use crate::synth::{RayTable, Room};
use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::slam::{mat_of, qangle_deg, qconj, qmul, quat_of, qyaw, Quat};
use std::path::Path;

fn calib() -> DeviceCalib {
    let p = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../analysis/headset-calibration.xml"
    );
    DeviceCalib::load(Path::new(p)).expect("factory calibration")
}

/// Body (IMU) orientation looking along yaw with a pitch (rad), y up.
fn body_q(yaw: f64, pitch: f64) -> Quat {
    qmul(
        qyaw(yaw),
        [(pitch / 2.0).sin(), 0.0, 0.0, (pitch / 2.0).cos()],
    )
}

fn pose(p: Vec3, q: Quat) -> Pose {
    Pose { r: mat_of(q), t: p }
}

struct Cams {
    rig: Rig,
    rays: Vec<RayTable>,
}

impl Cams {
    fn new() -> Cams {
        let rig = Rig::from_calib(&calib(), &[0, 1]).unwrap();
        let rays = rig.cams.iter().map(|c| RayTable::new(&c.intr)).collect();
        Cams { rig, rays }
    }
    /// Render both cameras for a room_from_body pose.
    fn render(&self, room: &Room, room_from_body: &Pose) -> Vec<Vec<u8>> {
        self.rig
            .cams
            .iter()
            .zip(self.rays.iter())
            .map(|(c, r)| room.render_rays(r, &room_from_body.compose(&c.body_from_cam)))
            .collect()
    }
}

fn img(d: &[u8]) -> Image<'_> {
    Image {
        w: 640,
        h: 480,
        stride: 640,
        data: d,
    }
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn stereo_depth_matches_the_room() {
    let cams = Cams::new();
    let room = Room::default();
    let rb = pose([0.3, 1.6, 0.5], body_q(0.4, -0.25));
    let ims = cams.render(&room, &rb);
    let o = OrbParams::default();
    let fa = features(&img(&ims[0]), &cams.rig.cams[0], &o);
    let fb = features(&img(&ims[1]), &cams.rig.cams[1], &o);
    let pts = triangulate(
        &fa,
        &fb,
        &cams.rig.cams[0],
        &cams.rig.cams[1],
        &StereoParams::default(),
    );
    assert!(
        pts.len() >= 80,
        "{} stereo points from {} / {} features",
        pts.len(),
        fa.kps.len(),
        fb.kps.len()
    );
    let c0 = rb.compose(&cams.rig.cams[0].body_from_cam);
    let mut rel: Vec<f64> = pts
        .iter()
        .filter_map(|s| {
            let truth = room.depth(&c0, fa.bearings[s.ia])?;
            let est = geom::norm(geom::sub(rb.apply(s.p_body), c0.t));
            Some(((est - truth) / truth).abs())
        })
        .collect();
    rel.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let med = rel[rel.len() / 2];
    let p90 = rel[rel.len() * 9 / 10];
    eprintln!(
        "stereo: {} points, depth error median {:.1} %, p90 {:.1} %",
        pts.len(),
        med * 100.0,
        p90 * 100.0
    );
    assert!(med < 0.04 && p90 < 0.15, "median {med}, p90 {p90}");
}

/// A walk through the room: positions on a circle, looking around (yaw turns 360 deg), slight pitch changes.
fn walk(n: usize, r: f64, phase: f64) -> Vec<(Vec3, Quat)> {
    (0..n)
        .map(|i| {
            let a = phase + i as f64 / n as f64 * std::f64::consts::TAU;
            let p = [r * a.sin(), 1.55 + 0.05 * (3.0 * a).sin(), r * a.cos()];
            (
                p,
                body_q(
                    a + std::f64::consts::PI * 0.15,
                    -0.2 + 0.1 * (2.0 * a).sin(),
                ),
            )
        })
        .collect()
}

/// Session 1 builds the map (its own world = room); session 2 runs in a different tracker world (yaw + offset)
/// under dimmer light and must find the room again: alignment error < 3 cm / 1 deg, within a few frames.
#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn second_session_relocalizes_into_the_same_room() {
    let cams = Cams::new();
    let room = Room::default();
    let cal = calib();
    let p = SessionParams {
        kf_min_dist: 0.25,
        kf_min_angle_deg: 20.0,
        ..Default::default()
    };
    // Session 1: tracker world == room frame.
    let mut s1 = RoomSession::new(RoomMap::new(&cal.uid, &[0, 1]), cams.rig.clone(), p.clone());
    let ident = Alignment {
        yaw: 0.0,
        t: [0.0; 3],
    };
    for (k, (pos, q)) in walk(24, 0.9, 0.0).into_iter().enumerate() {
        let rb = pose(pos, q);
        let ims = cams.render(&room, &rb);
        let views = [img(&ims[0]), img(&ims[1])];
        s1.on_frame(k as i64 * 40_000_000, &views, &rb, &ident, true);
    }
    let map = s1.finish();
    eprintln!(
        "session 1: {} keyframes, {} points",
        map.keyframes.len(),
        map.points()
    );
    assert!(
        map.keyframes.len() >= 12,
        "{} keyframes",
        map.keyframes.len()
    );
    // Save and load like a real restart.
    let dir = std::env::temp_dir().join(format!("omarchyvr-room-test-{}", std::process::id()));
    map.save(&dir).unwrap();
    let map = RoomMap::load(&dir).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    assert_eq!(map.sessions, 1);

    // Session 2: new tracker world: slam_from_room = yaw 1.1 rad + offset.
    let room2 = Room {
        gain: 0.7,
        ..Room::default()
    };
    let slam_from_room = Pose {
        r: mat_of(qyaw(1.1)),
        t: [2.0, 0.0, -1.0],
    };
    let true_room_from_slam = slam_from_room.inverse();
    let mut s2 = RoomSession::new(map, cams.rig.clone(), p);
    let mut fixed = None;
    // Start somewhere between the old keyframes (other radius and phase).
    for (k, (pos, q)) in walk(24, 0.75, 0.13).into_iter().enumerate().take(10) {
        let rb = pose(pos, q);
        let sb = slam_from_room.compose(&rb);
        let ims = cams.render(&room2, &rb);
        let views = [img(&ims[0]), img(&ims[1])];
        // The supervisor's own alignment of a new session (unrelated to the room).
        let w = Alignment {
            yaw: 0.0,
            t: [0.0; 3],
        };
        for e in s2.on_frame(k as i64 * 250_000_000, &views, &sb, &w, true) {
            if let Event::Relocalized(a) = e {
                fixed.get_or_insert((k, a));
            }
        }
    }
    let (k, a) = fixed.expect("relocalised");
    let est = a.pose();
    let dt = geom::norm(geom::sub(est.t, true_room_from_slam.t));
    let dr = qangle_deg(qmul(
        qconj(quat_of(&est.r)),
        quat_of(&true_room_from_slam.r),
    ));
    eprintln!(
        "session 2: relocalised at frame {k}: {:.1} mm, {:.2} deg; {:?}",
        dt * 1000.0,
        dr,
        s2.stats
    );
    assert!(k <= 3, "took {k} frames");
    assert!(dt < 0.03 && dr < 1.0, "{dt} m {dr} deg");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn relocalize_single_frame_and_reject_wrong_room() {
    let cams = Cams::new();
    let room = Room::default();
    let cal = calib();
    let mut map = RoomMap::new(&cal.uid, &[0, 1]);
    let o = OrbParams::default();
    for (pos, q) in walk(16, 0.9, 0.0) {
        let rb = pose(pos, q);
        let ims = cams.render(&room, &rb);
        let fa = features(&img(&ims[0]), &cams.rig.cams[0], &o);
        let fb = features(&img(&ims[1]), &cams.rig.cams[1], &o);
        let pts = triangulate(
            &fa,
            &fb,
            &cams.rig.cams[0],
            &cams.rig.cams[1],
            &StereoParams::default(),
        );
        map.add_keyframe(0, rb, &pts);
    }
    // A query between keyframes.
    let (pos, q) = walk(16, 0.8, 0.2)[5];
    let rb = pose(pos, q);
    let ims = cams.render(&room, &rb);
    let qf = vec![
        features(&img(&ims[0]), &cams.rig.cams[0], &o),
        features(&img(&ims[1]), &cams.rig.cams[1], &o),
    ];
    let r = map
        .relocalize(&qf, &cams.rig, &tilt_of(q), None, &RelocParams::default())
        .expect("relocalised");
    let dt = geom::norm(geom::sub(r.room_from_body.t, pos));
    let dr = qangle_deg(qmul(qconj(quat_of(&r.room_from_body.r)), q));
    eprintln!(
        "single frame: {} inliers / {} corrs, {:.1} mm, {:.2} deg",
        r.inliers,
        r.corrs,
        dt * 1000.0,
        dr
    );
    assert!(dt < 0.03 && dr < 1.0);
    // A different room (other texture seed) must not relocalise.
    let other = Room {
        seed: 999,
        ..Room::default()
    };
    let ims = cams.render(&other, &rb);
    let qf = vec![
        features(&img(&ims[0]), &cams.rig.cams[0], &o),
        features(&img(&ims[1]), &cams.rig.cams[1], &o),
    ];
    assert!(map
        .relocalize(&qf, &cams.rig, &tilt_of(q), None, &RelocParams::default())
        .is_none());
}

#[test]
fn map_file_roundtrip() {
    let mut m = RoomMap::new("P511X0X000000000002", &[0, 1]);
    let pts = vec![crate::stereo::StereoPoint {
        p_body: [0.1, 0.2, -1.5],
        desc: [7u8; 32],
        depth: 1.5,
        ia: 0,
    }];
    m.add_keyframe(123, pose([1.0, 1.6, 2.0], body_q(0.3, -0.1)), &pts);
    m.set_anchor("desk", [0.5, 0.75, -0.4], body_q(1.0, 0.0));
    let back = RoomMap::decode(&m.encode()).unwrap();
    assert_eq!(back.keyframes.len(), 1);
    assert_eq!(back.anchors, m.anchors);
    assert!(
        geom::norm(geom::sub(
            back.keyframes[0].points[0].p,
            m.keyframes[0].points[0].p
        )) < 1e-6
    );
    let mut b2 = back.clone();
    assert!(b2.remove_anchor("desk") && b2.anchors.is_empty());
    assert!(RoomMap::decode(b"XXXX").is_err());
}

#[test]
#[ignore]
fn debug_stereo_potential() {
    use crate::orb;
    let cams = Cams::new();
    let room = Room::default();
    let rb = pose([0.3, 1.6, 0.5], body_q(0.4, -0.25));
    let ims = cams.render(&room, &rb);
    let o = OrbParams::default();
    let (ca, cb) = (&cams.rig.cams[0], &cams.rig.cams[1]);
    let fa = features(&img(&ims[0]), ca, &o);
    let fb = features(&img(&ims[1]), cb, &o);
    let room_from_a = rb.compose(&ca.body_from_cam);
    let room_from_b = rb.compose(&cb.body_from_cam);
    let b_from_room = room_from_b.inverse();
    let (mut has_true, mut true_pairs) = (0, Vec::new());
    for i in 0..fa.kps.len() {
        let Some(d) = room.depth(&room_from_a, fa.bearings[i]) else {
            continue;
        };
        let x = room_from_a.apply(geom::scale(fa.bearings[i], d));
        let xb = b_from_room.apply(x);
        if xb[2] <= 0.0 {
            continue;
        }
        let xv = geom::mat_vec(&geom::transpose(&cb.pinhole.cam_from_virt), xb);
        if xv[2] <= 0.0 {
            continue;
        }
        let (u, v) = (
            cb.pinhole.f * xv[0] / xv[2] + cb.pinhole.cx,
            cb.pinhole.f * xv[1] / xv[2] + cb.pinhole.cy,
        );
        if let Some((j, dist)) = fb
            .kps
            .iter()
            .enumerate()
            .map(|(j, k)| {
                (
                    j,
                    ((k.x as f64 - u).powi(2) + (k.y as f64 - v).powi(2)).sqrt(),
                )
            })
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        {
            if dist < 2.0 {
                has_true += 1;
                true_pairs.push((i, j));
            }
        }
    }
    let ham: Vec<u32> = true_pairs
        .iter()
        .map(|&(i, j)| orb::hamming(&fa.desc[i], &fb.desc[j]))
        .collect();
    let mut hs = ham.clone();
    hs.sort();
    let (oa, ob) = (ca.body_from_cam.t, cb.body_from_cam.t);
    let base = geom::sub(ob, oa);
    let epi: Vec<f64> = true_pairs
        .iter()
        .map(|&(i, j)| {
            let da = geom::mat_vec(&ca.body_from_cam.r, fa.bearings[i]);
            let db = geom::mat_vec(&cb.body_from_cam.r, fb.bearings[j]);
            geom::dot(geom::normalize(geom::cross(base, da)), db)
                .abs()
                .asin()
                .to_degrees()
        })
        .collect();
    let mut es = epi.clone();
    es.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let bf = orb::match_bf(&fa.desc, &fb.desc, 64, 0.8);
    eprintln!(
        "features {} / {}; true stereo pairs {}; their hamming p10/p50/p90 {:?}; epipolar angle p50/p90/max {:.3}/{:.3}/{:.3} deg; ungated bf matches {}",
        fa.kps.len(), fb.kps.len(), has_true,
        [hs.get(hs.len() / 10), hs.get(hs.len() / 2), hs.get(hs.len() * 9 / 10)],
        es.get(es.len() / 2).unwrap_or(&f64::NAN), es.get(es.len() * 9 / 10).unwrap_or(&f64::NAN), es.last().unwrap_or(&f64::NAN),
        bf.len()
    );
    let correct_bf = bf
        .iter()
        .filter(|(i, j, _)| true_pairs.contains(&(*i, *j)))
        .count();
    eprintln!("bf correct {correct_bf}");
}

/// Night: the same room at 1/8 of the light with sensor noise (the cameras at their auto-exposure ceiling). The
/// low-light stretch keeps enough stereo points for keyframes, and a map made in daylight is still recognised.
#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn night_frames_keep_stereo_points_and_relocalise_on_a_daylight_map() {
    use crate::stereo::{features_with, LOW_LIGHT};
    let cams = Cams::new();
    let cal = calib();
    let day = Room::default();
    let night = Room { gain: 0.12, ..Room::default() };
    let noisy = |mut v: Vec<u8>, seed: u64| {
        let mut x = seed | 1;
        for p in v.iter_mut() {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let n = (x >> 61) as i32 - 3; // -3..=4
            *p = (*p as i32 + n).clamp(0, 255) as u8;
        }
        v
    };
    let o = OrbParams::default();
    let sp = StereoParams::default();
    let (mut with, mut without) = (0usize, 0usize);
    for (k, (pos, q)) in walk(6, 0.9, 0.3).into_iter().enumerate() {
        let rb = pose(pos, q);
        let ims: Vec<Vec<u8>> = cams.render(&night, &rb).into_iter().enumerate().map(|(i, v)| noisy(v, (k * 2 + i) as u64 + 7)).collect();
        let n = |low| {
            let fa = features_with(&img(&ims[0]), &cams.rig.cams[0], &o, low);
            let fb = features_with(&img(&ims[1]), &cams.rig.cams[1], &o, low);
            triangulate(&fa, &fb, &cams.rig.cams[0], &cams.rig.cams[1], &sp).len()
        };
        with += n(Some(LOW_LIGHT));
        without += n(None);
    }
    eprintln!("night stereo points over 6 views: {with} with the stretch, {without} without");
    assert!(with > without + without / 4, "stretch {with} vs none {without}");
    assert!(with / 6 >= 40, "{} stereo points per view", with / 6);

    // daylight map, night session in another tracker world
    let p = SessionParams { kf_min_dist: 0.25, kf_min_angle_deg: 20.0, ..Default::default() };
    let mut s1 = RoomSession::new(RoomMap::new(&cal.uid, &[0, 1]), cams.rig.clone(), p.clone());
    let ident = Alignment { yaw: 0.0, t: [0.0; 3] };
    for (k, (pos, q)) in walk(24, 0.9, 0.0).into_iter().enumerate() {
        let rb = pose(pos, q);
        let ims = cams.render(&day, &rb);
        s1.on_frame(k as i64 * 40_000_000, &[img(&ims[0]), img(&ims[1])], &rb, &ident, true);
    }
    let map = s1.finish();
    let slam_from_room = Pose { r: mat_of(qyaw(-0.7)), t: [0.5, 0.0, 1.0] };
    let mut s2 = RoomSession::new(map, cams.rig.clone(), p);
    let mut fixed = None;
    for (k, (pos, q)) in walk(24, 0.8, 0.1).into_iter().enumerate().take(12) {
        let rb = pose(pos, q);
        let ims: Vec<Vec<u8>> = cams.render(&night, &rb).into_iter().enumerate().map(|(i, v)| noisy(v, (k * 2 + i) as u64 + 99)).collect();
        for e in s2.on_frame(k as i64 * 250_000_000, &[img(&ims[0]), img(&ims[1])], &slam_from_room.compose(&rb), &ident, true) {
            if let Event::Relocalized(a) = e {
                fixed.get_or_insert((k, a));
            }
        }
    }
    let (k, a) = fixed.expect("night session relocalised on the daylight map");
    let truth = slam_from_room.inverse();
    let dt = geom::norm(geom::sub(a.pose().t, truth.t));
    eprintln!("night relocalised at frame {k}: {:.1} mm", dt * 1000.0);
    assert!(dt < 0.05, "{dt} m");
}

#[test]
fn low_light_stretch_leaves_lit_frames_alone() {
    use crate::stereo::{low_light_stretch, LowLight, LOW_LIGHT};
    let mut lit: Vec<u8> = (0..1000).map(|i| (i % 250) as u8).collect();
    let before = lit.clone();
    let plain = LowLight { denoise: false, ..LOW_LIGHT };
    assert_eq!(low_light_stretch(&mut lit, 100, plain), 1.0);
    assert_eq!(lit, before);
    // dark: p99 ~ 20 -> 200 would be 10x, capped at 6x; black (outside the fisheye) stays black
    let mut dark: Vec<u8> = (0..1000).map(|i| if i < 100 { 0 } else { (i % 21) as u8 }).collect();
    let g = low_light_stretch(&mut dark, 100, plain);
    assert!((g - 6.0).abs() < 1e-6, "{g}");
    assert_eq!(dark[0], 0);
    assert_eq!(dark[110], 30);
    assert!(dark.iter().all(|&v| v <= 120));
    let mut black = vec![0u8; 100];
    assert_eq!(low_light_stretch(&mut black, 10, LOW_LIGHT), 1.0);
    // denoised: a flat dark patch stays flat, scaled
    let mut flat = vec![10u8; 100];
    assert!((low_light_stretch(&mut flat, 10, LOW_LIGHT) - 6.0).abs() < 1e-6);
    assert!(flat.iter().all(|&v| v == 60));
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn per_session_keyframe_spacing() {
    let cal = calib();
    let mut map = RoomMap::new(&cal.uid, &[0, 1]);
    let at = pose([0.0, 1.5, 0.0], body_q(0.0, 0.0));
    map.add_keyframe(0, at, &[]);
    assert!(!map.wants_keyframe(&at, 0.2, 15.0));
    assert!(!map.wants_keyframe_in(&at, 0.2, 15.0, Some(0)));
    map.sessions = 1;
    assert!(map.wants_keyframe_in(&at, 0.2, 15.0, Some(1)), "a later session may add its own view of the same place");
}
