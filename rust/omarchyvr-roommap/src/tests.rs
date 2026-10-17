//! Tests: joint 4-DoF fit (synthetic correspondences with noise and outliers), pose-graph loop closure (injected
//! drift), result/offset files, and end to end on the synthetic room of `omarchyvr-room` seen through the real factory
//! calibration (two sessions with a known transform, plus the loop closure on a drifting mapping session).

use crate::align::{first_fix, joint_fit, observe, wrap, AcceptParams, FrameObs, JointParams, ObserveParams};
use crate::loopclose::{close_loops, correction_at, solve_graph, Correction, Edge, LoopParams};
use crate::pipeline::frame_features;
use crate::refine::MapFrame;
use crate::store::{self, RelocResult};
use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::slam::{mat_of, qmul, qyaw, Quat};
use omarchyvr_room::map::RoomMap;
use omarchyvr_room::orb::{Image, OrbParams};
use omarchyvr_room::pngread::Grey;
use omarchyvr_room::pnp::{rot_y, Corr};
use omarchyvr_room::session::{Alignment, RoomSession, SessionParams};
use omarchyvr_room::stereo::Rig;
use omarchyvr_room::synth::{RayTable, Room};
use std::path::Path;

/// Small deterministic generator.
struct Rng(u64);
impl Rng {
    fn next(&mut self) -> f64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        ((z ^ (z >> 31)) >> 11) as f64 / (1u64 << 53) as f64
    }
    fn sym(&mut self) -> f64 {
        2.0 * self.next() - 1.0
    }
    fn gauss(&mut self) -> f64 {
        let (u, v) = (self.next().max(1e-12), self.next());
        (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * v).cos()
    }
}

fn body_q(yaw: f64, pitch: f64) -> Quat {
    qmul(qyaw(yaw), [(pitch / 2.0).sin(), 0.0, 0.0, (pitch / 2.0).cos()])
}

fn align_err(a: &Alignment, b: &Alignment) -> (f64, f64) {
    (geom::norm(geom::sub(a.t, b.t)), wrap(a.yaw - b.yaw).abs().to_degrees())
}

#[test]
fn joint_fit_recovers_transform_with_noise_and_outliers() {
    let mut rng = Rng(1);
    let truth = Alignment { yaw: 0.7, t: [1.2, -0.05, -2.3] };
    let bfc = vec![
        Pose { r: mat_of(qyaw(0.45)), t: [-0.05, 0.0, 0.0] },
        Pose { r: mat_of(qyaw(-0.45)), t: [0.05, 0.0, 0.0] },
    ];
    let mut obs = Vec::new();
    for f in 0..20 {
        let sb = Pose { r: mat_of(body_q(0.1 * f as f64, -0.2)), t: [0.05 * f as f64, 1.5, 0.02 * f as f64] };
        let room_from_body = truth.pose().compose(&sb);
        let mut corrs = Vec::new();
        for _ in 0..60 {
            let c = (rng.next() * 2.0) as usize;
            let rc = room_from_body.compose(&bfc[c]);
            // a point 1-5 m in front of the camera (OpenCV axes: +z forward)
            let d = 1.0 + 4.0 * rng.next();
            let pc = [rng.sym() * d * 0.6, rng.sym() * d * 0.5, d];
            let p_room = rc.apply(pc);
            let mut b = geom::normalize(pc);
            b = geom::normalize(geom::add(b, [rng.gauss() * 0.003, rng.gauss() * 0.003, 0.0]));
            if rng.next() < 0.2 {
                b = geom::normalize([rng.sym(), rng.sym(), 1.0]); // outlier
            }
            corrs.push(Corr { cam: c, bearing: b, point: p_room });
        }
        let n = corrs.len();
        obs.push(FrameObs {
            ts_ns: f * 100_000_000,
            slam_from_body: sb,
            corr_kf: vec![0; n],
            corrs,
            inliers: (0..n).collect(),
            hyp: Alignment { yaw: truth.yaw + 0.01 * rng.sym(), t: geom::add(truth.t, [0.05 * rng.sym(), 0.0, 0.05 * rng.sym()]) },
            room_from_body,
            rms_deg: 0.0,
        });
    }
    let refs: Vec<&FrameObs> = obs.iter().collect();
    let f = joint_fit(&refs, &bfc, &JointParams::default()).expect("fit");
    let (dm, dd) = align_err(&f.align, &truth);
    assert!(dm < 0.005 && dd < 0.05, "error {dm} m {dd} deg");
    assert!(f.inliers > 800, "inliers {}", f.inliers);
    // the first-fix rule accepts once enough frames agree
    let ap = AcceptParams { min_frames: 8, min_inliers: 200, ..Default::default() };
    let (i, ff) = first_fix(&obs, &bfc, &JointParams::default(), &ap).expect("first fix");
    assert_eq!(i, 7);
    assert!(align_err(&ff.align, &truth).0 < 0.01);
}

#[test]
fn pose_graph_removes_injected_drift() {
    // A square loop of 40 nodes; odometry has a yaw drift of 0.3 deg/node and a lateral bias; one loop edge says
    // the last node is back at the start.
    let n = 40;
    let truth: Vec<(f64, Vec3)> = (0..n)
        .map(|i| {
            let a = i as f64 / n as f64 * std::f64::consts::TAU;
            (a, [2.0 * a.sin(), 0.0, 2.0 * (1.0 - a.cos())])
        })
        .collect();
    let mut est = vec![truth[0]];
    let mut edges = Vec::new();
    for i in 1..n {
        let (ya, ta) = truth[i - 1];
        let (yb, tb) = truth[i];
        let dt = geom::mat_vec(&geom::transpose(&rot_y(ya)), geom::sub(tb, ta));
        let dyaw = wrap(yb - ya) + 0.3f64.to_radians();
        let dt = geom::add(dt, [0.004, 0.0, 0.0]);
        edges.push(Edge { a: i - 1, b: i, dyaw, dt, wt: 1.0 / (0.01f64).powi(2), wy: 1.0 / (0.2f64.to_radians()).powi(2), is_loop: false });
        let (yp, tp) = est[i - 1];
        est.push((wrap(yp + dyaw), geom::add(tp, geom::mat_vec(&rot_y(yp), dt))));
    }
    let drift_before = geom::norm(geom::sub(est[n - 1].1, truth[n - 1].1));
    assert!(drift_before > 0.2, "{drift_before}");
    // loop: node n-1 relative to node 0 as measured (truth)
    let (y0, t0) = truth[0];
    let (yl, tl) = truth[n - 1];
    edges.push(Edge {
        a: 0,
        b: n - 1,
        dyaw: wrap(yl - y0),
        dt: geom::mat_vec(&geom::transpose(&rot_y(y0)), geom::sub(tl, t0)),
        wt: 1.0 / (0.01f64).powi(2),
        wy: 1.0 / (0.1f64.to_radians()).powi(2),
        is_loop: true,
    });
    solve_graph(&mut est, &edges, 20, 2.0);
    let after = geom::norm(geom::sub(est[n - 1].1, truth[n - 1].1));
    let mid = geom::norm(geom::sub(est[n / 2].1, truth[n / 2].1));
    assert!(after < 0.02, "end error {after} (before {drift_before})");
    assert!(mid < drift_before / 2.0, "mid error {mid}");
}

#[test]
fn correction_interpolates_and_clamps() {
    let c = vec![
        Correction { ts_ns: 0, c: Alignment { yaw: 0.0, t: [0.0; 3] } },
        Correction { ts_ns: 10, c: Alignment { yaw: 0.2, t: [1.0, 0.0, -1.0] } },
    ];
    let m = correction_at(&c, 5);
    assert!((m.yaw - 0.1).abs() < 1e-12 && (m.t[0] - 0.5).abs() < 1e-12 && (m.t[2] + 0.5).abs() < 1e-12);
    assert_eq!(correction_at(&c, -5), c[0].c);
    assert_eq!(correction_at(&c, 50), c[1].c);
}

#[test]
fn result_roundtrip_compose_and_offset_file() {
    let r = RelocResult {
        map: "/x/home".into(),
        source: "rec".into(),
        align: Alignment { yaw: 0.3, t: [0.1, -0.2, 0.3] },
        frames: 12,
        inliers: 345,
        rms_deg: 0.4,
        sigma_deg: 0.02,
        sigma_m: 0.001,
        fix_after_s: 3.5,
        pid: 4242,
    };
    let back = RelocResult::parse(&r.to_toml()).unwrap();
    assert_eq!(back.pid, 4242);
    assert_eq!(back.frames, 12);
    let (dm, dd) = align_err(&back.align, &r.align);
    assert!(dm < 1e-5 && dd < 1e-5);
    // compose / inverse
    let id = store::compose(&r.align, &store::inverse(&r.align));
    assert!(geom::norm(id.t) < 1e-12 && id.yaw.abs() < 1e-12);
    // offset file: the same format Monado (and omarchyvr-anchor) reads; app_from_slam = room_from_slam
    let dir = std::env::temp_dir().join(format!("omarchyvr-roommap-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join("pimax-anchor.offset");
    omarchyvr_anchor::world::take_lease(&p, 4242, omarchyvr_anchor::world::Role::Apply).unwrap();
    omarchyvr_anchor::world::write_offset(&p, 4242, &store::offset_pose(&r.align)).unwrap();
    let (pid, o) = omarchyvr_anchor::world::read_offset(&p).unwrap();
    assert_eq!(pid, 4242);
    let q = r.align.pose();
    assert!(geom::norm(geom::sub(o.t, q.t)) < 1e-5);
    let x = [0.3, 1.1, -0.7];
    assert!(geom::norm(geom::sub(o.apply(x), q.apply(x))) < 1e-5);
    // the offset is a rotation about +Y only
    assert!((o.r[1][1] - 1.0).abs() < 1e-6);
    let _ = std::fs::remove_dir_all(dir);
}

// ---------------------------------------------------------------------------------------------------------------
// End to end on the synthetic room.

fn calib() -> DeviceCalib {
    let p = concat!(env!("CARGO_MANIFEST_DIR"), "/../../analysis/headset-calibration.xml");
    DeviceCalib::load(Path::new(p)).expect("factory calibration")
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
    fn render(&self, room: &Room, room_from_body: &Pose) -> [Grey; 2] {
        let v: Vec<Grey> = self
            .rig
            .cams
            .iter()
            .zip(self.rays.iter())
            .map(|(c, r)| Grey { w: 640, h: 480, data: room.render_rays(r, &room_from_body.compose(&c.body_from_cam)) })
            .collect();
        let mut it = v.into_iter();
        [it.next().unwrap(), it.next().unwrap()]
    }
}

fn image(g: &Grey) -> Image<'_> {
    Image { w: g.w, h: g.h, stride: g.w, data: &g.data }
}

/// A walk around the room centre (room_from_body poses), `n` frames over one circle.
fn walk(n: usize, r: f64, phase: f64) -> Vec<Pose> {
    (0..n)
        .map(|i| {
            let a = phase + i as f64 / n as f64 * std::f64::consts::TAU;
            Pose { r: mat_of(body_q(a + 0.5, -0.1)), t: [r * a.sin(), 1.55, r * a.cos()] }
        })
        .collect()
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn synthetic_room_second_session_relocalises() {
    let cams = Cams::new();
    let room = Room::default();
    // session 1 (room frame = its own world)
    let id = Alignment { yaw: 0.0, t: [0.0; 3] };
    let mut s1 = RoomSession::new(RoomMap::new("test", &[0, 1]), cams.rig.clone(), SessionParams::default());
    for (k, rb) in walk(30, 0.9, 0.0).iter().enumerate() {
        let im = cams.render(&room, rb);
        s1.on_frame(k as i64 * 100_000_000, &[image(&im[0]), image(&im[1])], rb, &id, true);
    }
    let map = s1.finish();
    assert!(map.keyframes.len() >= 5, "{} keyframes", map.keyframes.len());
    // session 2: another world origin, dimmer light, a different walk
    let truth = Alignment { yaw: -0.8, t: [1.0, 0.0, -3.0] };
    let slam_from_room = truth.pose().inverse();
    let dim = Room { gain: 0.6, ..Room::default() };
    let orb = OrbParams::default();
    let mut obs = Vec::new();
    for (k, rb) in walk(30, 0.6, 0.1).iter().enumerate().take(14) {
        let im = cams.render(&dim, rb);
        let feats = frame_features(&cams.rig, [&im[0], &im[1]], &orb);
        let sb = slam_from_room.compose(rb);
        if let Some(o) = observe(&map, &cams.rig, &feats, k as i64 * 250_000_000, &sb, &ObserveParams::default()) {
            obs.push(o);
        }
    }
    assert!(obs.len() >= 8, "only {} frames matched", obs.len());
    let refs: Vec<&FrameObs> = obs.iter().collect();
    let f = joint_fit(&refs, &cams.rig.body_from_cams(), &JointParams::default()).expect("joint fit");
    let (dm, dd) = align_err(&f.align, &truth);
    assert!(dm < 0.01 && dd < 0.2, "alignment error {:.1} mm {:.3} deg", dm * 1e3, dd);
    // a different room is not recognised
    let other = Room { seed: 99, ..Room::default() };
    let mut hits = 0;
    for (k, rb) in walk(30, 0.6, 0.1).iter().enumerate().take(8) {
        let im = cams.render(&other, rb);
        let feats = frame_features(&cams.rig, [&im[0], &im[1]], &orb);
        let sb = slam_from_room.compose(rb);
        if observe(&map, &cams.rig, &feats, k as i64, &sb, &ObserveParams::default()).is_some() {
            hits += 1;
        }
    }
    assert!(hits <= 1, "{hits} frames of another room matched");
}

#[test]
#[cfg_attr(not(lab_data), ignore = "needs the private lab data (real calibration, room recording or eye transcript)")]
fn synthetic_loop_closure_removes_mapping_drift() {
    // Mapping session with a growing yaw/position drift (what Basalt does): the map from the drifting poses is
    // inconsistent where the walk comes back; loop closure must pull the end keyframes back to the truth.
    let cams = Cams::new();
    let room = Room::default();
    let n = 60;
    let truth = walk(n, 0.9, 0.0);
    let drifted: Vec<Pose> = truth
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let f = i as f64 / n as f64;
            let d = Alignment { yaw: (3.0 * f).to_radians(), t: [0.15 * f, 0.0, -0.10 * f] };
            d.pose().compose(p)
        })
        .collect();
    let id = Alignment { yaw: 0.0, t: [0.0; 3] };
    let sp = SessionParams { kf_min_dist: 0.15, kf_min_angle_deg: 12.0, kf_min_points: 40, ..Default::default() };
    let mut s = RoomSession::new(RoomMap::new("test", &[0, 1]), cams.rig.clone(), sp);
    let orb = OrbParams::default();
    let mut frames = Vec::new();
    let dt = 1_000_000_000i64; // 1 s per frame: the walk takes 60 s, so start and end are > 12 s apart
    for (k, (tp, dp)) in truth.iter().zip(drifted.iter()).enumerate() {
        let im = cams.render(&room, tp);
        s.on_frame(k as i64 * dt, &[image(&im[0]), image(&im[1])], dp, &id, true);
        let feats = frame_features(&cams.rig, [&im[0], &im[1]], &orb);
        frames.push(MapFrame::new(k as i64 * dt, *dp, feats, &cams.rig));
    }
    let mut map = s.finish();
    let last = map.keyframes.iter().max_by_key(|k| k.ts_ns).unwrap().clone();
    let k_last = (last.ts_ns / dt) as usize;
    let err_before = geom::norm(geom::sub(last.pose.t, truth[k_last].t));
    let lp = LoopParams { min_gap_ns: 12 * dt, local_ns: 6 * dt, ..Default::default() };
    let (st, corr) = close_loops(&mut map, &cams.rig, &mut frames, &ObserveParams::default(), &lp, 4);
    assert!(st.loops >= 2, "{st:?}");
    assert!(!corr.is_empty());
    let after = map.keyframes.iter().find(|k| k.ts_ns == last.ts_ns).unwrap();
    let err_after = geom::norm(geom::sub(after.pose.t, truth[k_last].t));
    assert!(err_after < 0.5 * err_before && err_after < 0.04, "end keyframe error {err_before:.3} -> {err_after:.3} m ({st:?})");
}

#[test]
fn export_json_and_viewer_page() {
    use omarchyvr_room::map::{Anchor, Keyframe, MapPoint};
    let mut m = RoomMap::new("UID", &[0, 1]);
    for i in 0..3u32 {
        let pts = (0..4)
            .map(|j| MapPoint { p: [i as f64 * 0.001 + j as f64, 0.5 + j as f64 * 0.5, -1.0], desc: [0; 32] })
            .collect();
        m.keyframes.push(Keyframe { id: i, ts_ns: i as i64, session: 0, pose: Pose { r: mat_of(qyaw(0.3)), t: [i as f64, 1.2, 0.0] }, points: pts });
    }
    m.anchors.push(Anchor { name: "desk\"</x>".into(), p: [1.0, 0.7, 2.0], q: [0.0, 0.0, 0.0, 1.0] });
    let floor = crate::floor::StoredFloor { y: 0.05, source: crate::floor::FloorSource::Manual, confidence: 0.9, sigma_m: 0.01, inliers: 0, head_above_m: 1.2, when: 1 };
    let j = crate::export::map_json(&m, "t", Some(&floor), Some("source = \"x\"\ncreated = \"unix 5\"\n"));
    // 12 observations merge to 4 points seen 3 times each
    assert!(j.contains("\"raw_observations\":12,\"points\":4"), "{j}");
    assert!(j.contains("\"floor\":{\"y\":0.05,\"source\":\"manual\""));
    assert!(j.contains("\"columns\":[\"x\",\"y\",\"z\",\"n\"],\"data\":[0.001,0.5,-1,3,"), "{j}");
    assert_eq!(j.matches("\"id\":").count(), 3);
    // quaternion of the 0.3 rad yaw: y = sin(0.15)
    assert!(j.contains("\"q\":[0,0.149,0,0.989]"), "{j}");
    assert!(j.contains("source") && j.contains("unix 5"));
    assert!(!j.contains("</"), "script-safe");
    let page = crate::export::html_page(&j, "t");
    assert!(page.contains("const DATA = {") && !page.contains("/*__DATA__*/") && !page.contains("__TITLE__"));
    assert!(!page.contains("https://") && !page.contains("http://"), "no network");
}

// ---------------------------------------------------------------------------------------------------------------
// Replay of the 2026-10-17 16:16-16:17 floor jump (Monado pid 1092498): Fast room setup mapped the room anew and
// placed it; the detached `auto` (started at the session start) recognised the new map 73 s later and wrote a second
// offset 32 cm away. Numbers from that session's map directory and room-auto.log.

const JUMP_SETUP_TOML: &str = "label = \"home\"\nmode = \"fast\"\ncentre = [0.8017, -0.5628]\nyaw_deg = 19.1557\nplay_area = [0.800, 0.800]\nhead_m = 1.7573\nwhen = 1792225029\nboundary = [[0.2926, -0.8094], [1.0483, -1.0720], [1.3272, -0.2690], [0.5715, -0.0065]]\n";
const JUMP_FLOOR_Y: f64 = -0.63519;
/// The last correction.csv row of the new map: room_new_from_room_old at the end of the 30 s mapping walk.
const JUMP_CORRECTION: Alignment = Alignment { yaw: -0.003655764, t: [-0.192141, -0.067361, 0.043380] };
/// auto's relocalisation 73 s later (last-reloc.toml).
const JUMP_RELOC: Alignment = Alignment { yaw: -0.971515 * std::f64::consts::PI / 180.0, t: [-0.284277, -0.069415, 0.139813] };

/// What `write_offset` writes for a room fix: the floor onto Monado's STAGE floor (origin height 1.6 m), the play
/// area's centre at the origin.
fn jump_offset(fix: &Alignment) -> Alignment {
    let st = crate::setup::RoomSetup::parse(JUMP_SETUP_TOML).unwrap();
    let d = crate::floor::floor_shift(JUMP_FLOOR_Y, crate::floor::MONADO_ORIGIN_HEIGHT);
    store::compose(&st.setup_from_room(), &Alignment { yaw: fix.yaw, t: [fix.t[0], fix.t[1] + d, fix.t[2]] })
}

#[test]
fn floor_jump_replay_setup_includes_the_loop_closure() {
    let id = Alignment { yaw: 0.0, t: [0.0; 3] };
    // the chain reproduces both logged offsets
    let raw = jump_offset(&id);
    assert!((raw.yaw.to_degrees() + 19.156).abs() < 0.01 && geom::norm(geom::sub(raw.t, [-0.9420, -0.9648, 0.2686])) < 0.001, "{raw:?}");
    let auto = jump_offset(&JUMP_RELOC);
    assert!((auto.yaw.to_degrees() + 20.127).abs() < 0.01 && geom::norm(geom::sub(auto.t, [-1.2564, -1.0342, 0.3074])) < 0.001, "{auto:?}");
    // the setup now includes the map's loop closure (the offset at mapping was none: Monado deleted it at start)
    let fixed = jump_offset(&store::fix_after_mapping(&id, Some(&JUMP_CORRECTION)));
    let (d_raw, d_fixed) = (geom::norm(geom::sub(raw.t, auto.t)), geom::norm(geom::sub(fixed.t, auto.t)));
    let (v_raw, v_fixed) = ((raw.t[1] - auto.t[1]).abs(), (fixed.t[1] - auto.t[1]).abs());
    println!(
        "TEST NUMBERS floor jump: setup vs relocalisation 73 s later: raw {:.1} cm ({:.1} cm vertical), with the loop closure {:.1} cm ({:.1} cm vertical; the rest is 73 s more of the walk's drift)",
        d_raw * 100.0,
        v_raw * 100.0,
        d_fixed * 100.0,
        v_fixed * 100.0
    );
    assert!(v_fixed < 0.01, "the floor height agrees within 1 cm: {v_fixed}");
    assert!(v_raw > 0.05 && d_fixed < d_raw / 2.0);
    // the stale-file guard: a correction.csv older than map.bin is not used
    let dir = std::env::temp_dir().join(format!("omarchyvr-jump-corr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    store::save_corrections(&dir, &[Correction { ts_ns: 1, c: JUMP_CORRECTION }]).unwrap();
    std::fs::write(dir.join("map.bin"), b"x").unwrap();
    let c = store::last_correction(&dir).unwrap();
    assert!((c.t[1] - JUMP_CORRECTION.t[1]).abs() < 1e-6);
    let old = std::fs::File::options().write(true).open(dir.join("correction.csv")).unwrap();
    old.set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(600)).unwrap();
    assert!(store::last_correction(&dir).is_none());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn floor_jump_replay_one_offset_only() {
    use omarchyvr_anchor::world::{self, Role};
    let dir = std::env::temp_dir().join(format!("omarchyvr-jump-lease-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let off = dir.join("pimax-anchor.offset");
    let (monado, auto, setup) = (1092498u32, 1092600u32, 1093007u32);
    // the session start: auto takes the lease and searches (the old map does not match)
    world::take_lease_as(&off, monado, auto, Role::Auto, |_| false, |_| Ok(())).unwrap();
    // Fast setup starts: it takes the lease and ends auto (here the end "fails": auto keeps running)
    let ended = std::cell::Cell::new(0);
    world::take_lease_as(&off, monado, setup, Role::Setup, |p| p == auto, |p| {
        ended.set(p);
        Ok(())
    })
    .unwrap();
    assert_eq!(ended.get(), auto);
    let id = Alignment { yaw: 0.0, t: [0.0; 3] };
    let placed = jump_offset(&store::fix_after_mapping(&id, Some(&JUMP_CORRECTION)));
    let mut writes = 0;
    if world::write_offset_as(&off, monado, setup, &store::offset_pose(&placed)).is_ok() {
        writes += 1;
    }
    let setup_text = std::fs::read_to_string(&off).unwrap();
    // 73 s later the still running auto recognised the new map: its write is refused, and so is a new lease (the
    // setup has ended by now)
    let second = jump_offset(&JUMP_RELOC);
    let e = world::write_offset_as(&off, monado, auto, &store::offset_pose(&second)).unwrap_err();
    assert!(e.to_string().contains("room setup"), "{e}");
    if world::write_offset_as(&off, monado, auto, &store::offset_pose(&second)).is_ok() {
        writes += 1;
    }
    let e = world::take_lease_as(&off, monado, auto, Role::Auto, |p| p == auto, |_| Ok(())).unwrap_err();
    assert!(e.contains("already placed"), "{e}");
    // a new auto of the same session, too
    assert!(world::take_lease_as(&off, monado, 1099999, Role::Auto, |_| false, |_| Ok(())).is_err());
    assert_eq!(writes, 1, "exactly one offset in the session");
    assert_eq!(std::fs::read_to_string(&off).unwrap(), setup_text);
    let _ = std::fs::remove_dir_all(dir);
}
