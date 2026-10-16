//! Map building and frame observation over a recording.

use crate::align::{observe, FrameObs, ObserveParams};
use crate::rec::{pose_at, Recording};
use pimax_cam::geom::Pose;
use pimax_room::map::RoomMap;
use pimax_room::orb::{Image, OrbParams};
use pimax_room::pngread;
use pimax_room::session::{Alignment, RoomSession, SessionParams};
use pimax_room::stereo::{features, Features, Rig};

/// Mapping parameters (keyframe density).
pub fn build_params(kf_dist: f64, kf_angle_deg: f64) -> SessionParams {
    SessionParams { kf_min_dist: kf_dist, kf_min_angle_deg: kf_angle_deg, kf_min_points: 40, ..Default::default() }
}

/// ORB features of both cameras of a frame set.
pub fn frame_features(rig: &Rig, imgs: [&pngread::Grey; 2], orb: &OrbParams) -> Vec<Features> {
    imgs.iter()
        .zip(rig.cams.iter())
        .map(|(g, c)| features(&Image { w: g.w, h: g.h, stride: g.w, data: &g.data }, c, orb))
        .collect()
}

/// Build counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BuildStats {
    /// Frames fed.
    pub frames: usize,
    /// Frames skipped (no tracker pose).
    pub no_pose: usize,
}

/// Session 1: keyframes of a recording in the room frame `room_from_slam` (identity = the recording's own world).
pub fn build_from_recording(
    rec: &Recording,
    rig: &Rig,
    map: RoomMap,
    sp: SessionParams,
    every: usize,
    room_from_slam: Alignment,
) -> Result<(RoomMap, BuildStats), String> {
    let mut s = RoomSession::new(map, rig.clone(), sp);
    let mut st = BuildStats::default();
    for (k, f) in rec.frames.iter().enumerate() {
        if k % every.max(1) != 0 {
            continue;
        }
        let Some(sb) = pose_at(&rec.traj, f.ts_ns) else {
            st.no_pose += 1;
            continue;
        };
        let (a, b) = (pngread::read(&f.cam0)?, pngread::read(&f.cam1)?);
        let v = [
            Image { w: a.w, h: a.h, stride: a.w, data: &a.data },
            Image { w: b.w, h: b.h, stride: b.w, data: &b.data },
        ];
        s.on_frame(f.ts_ns, &v, &sb, &room_from_slam, true);
        st.frames += 1;
    }
    Ok((s.finish(), st))
}

/// Match every `every`-th frame set of a recording against the map (parallel, `threads` workers). Frames without a
/// tracker pose or without a match are left out. Sorted by time.
pub fn observe_recording(
    rec: &Recording,
    map: &RoomMap,
    rig: &Rig,
    every: usize,
    orb: &OrbParams,
    op: &ObserveParams,
    threads: usize,
) -> Result<(Vec<FrameObs>, usize), String> {
    let jobs: Vec<(i64, Pose, &crate::rec::FrameRef)> = rec
        .frames
        .iter()
        .enumerate()
        .filter(|(k, _)| k % every.max(1) == 0)
        .filter_map(|(_, f)| pose_at(&rec.traj, f.ts_ns).map(|p| (f.ts_ns, p, f)))
        .collect();
    let n = jobs.len();
    let threads = threads.clamp(1, 64);
    let chunk = n.div_ceil(threads).max(1);
    let mut out: Vec<FrameObs> = Vec::new();
    let mut err: Option<String> = None;
    std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                sc.spawn(move || -> Result<Vec<FrameObs>, String> {
                    let mut v = Vec::new();
                    for (ts, sb, f) in part {
                        let (a, b) = (pngread::read(&f.cam0)?, pngread::read(&f.cam1)?);
                        let feats = frame_features(rig, [&a, &b], orb);
                        if let Some(o) = observe(map, rig, &feats, *ts, sb, op) {
                            v.push(o);
                        }
                    }
                    Ok(v)
                })
            })
            .collect();
        for h in hs {
            match h.join() {
                Ok(Ok(v)) => out.extend(v),
                Ok(Err(e)) => err = Some(e),
                Err(_) => err = Some("worker panicked".into()),
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    out.sort_by_key(|o| o.ts_ns);
    Ok((out, n))
}

/// Features of every `every`-th frame set of a recording as mapping frames (`room_from_slam` puts them in the room
/// frame; identity for the mapping session itself). Parallel.
pub fn map_frames(
    rec: &Recording,
    rig: &Rig,
    every: usize,
    orb: &OrbParams,
    room_from_slam: &Alignment,
    threads: usize,
) -> Result<Vec<crate::refine::MapFrame>, String> {
    let a = room_from_slam.pose();
    let jobs: Vec<(i64, Pose, &crate::rec::FrameRef)> = rec
        .frames
        .iter()
        .enumerate()
        .filter(|(k, _)| k % every.max(1) == 0)
        .filter_map(|(_, f)| pose_at(&rec.traj, f.ts_ns).map(|p| (f.ts_ns, a.compose(&p), f)))
        .collect();
    let chunk = jobs.len().div_ceil(threads.clamp(1, 64)).max(1);
    let mut out = Vec::new();
    let mut err = None;
    std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                sc.spawn(move || -> Result<Vec<crate::refine::MapFrame>, String> {
                    let mut v = Vec::new();
                    for (ts, rb, f) in part {
                        let (x, y) = (pngread::read(&f.cam0)?, pngread::read(&f.cam1)?);
                        let feats = frame_features(rig, [&x, &y], orb);
                        v.push(crate::refine::MapFrame::new(*ts, *rb, feats, rig));
                    }
                    Ok(v)
                })
            })
            .collect();
        for h in hs {
            match h.join() {
                Ok(Ok(v)) => out.extend(v),
                Ok(Err(e)) => err = Some(e),
                Err(_) => err = Some("worker panicked".into()),
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    out.sort_by_key(|f: &crate::refine::MapFrame| f.ts_ns);
    Ok(out)
}

/// Stereo points of one frame set in the tracker world (`slam_from_body` applied), points closer than
/// `max_range` to the head only.
pub fn frame_points(rig: &Rig, feats: &[Features], slam_from_body: &Pose, max_range: f64) -> Vec<pimax_cam::geom::Vec3> {
    let sp = pimax_room::stereo::StereoParams::default();
    pimax_room::stereo::triangulate(&feats[0], &feats[1], &rig.cams[0], &rig.cams[1], &sp)
        .into_iter()
        .filter(|p| pimax_cam::geom::norm(p.p_body) <= max_range)
        .map(|p| slam_from_body.apply(p.p_body))
        .collect()
}

/// A frame set's head pose and stereo points in the tracker world.
#[derive(Clone, Debug)]
pub struct PointFrame {
    /// Frame time (ns).
    pub ts_ns: i64,
    /// `slam_from_body`.
    pub pose: Pose,
    /// Points (tracker world).
    pub pts: Vec<pimax_cam::geom::Vec3>,
}

/// Stereo points of every `every`-th frame set of a recording (parallel). Sorted by time.
pub fn recording_points(rec: &Recording, rig: &Rig, every: usize, max_range: f64, threads: usize) -> Result<Vec<PointFrame>, String> {
    let orb = OrbParams::default();
    let jobs: Vec<(i64, Pose, &crate::rec::FrameRef)> = rec
        .frames
        .iter()
        .enumerate()
        .filter(|(k, _)| k % every.max(1) == 0)
        .filter_map(|(_, f)| pose_at(&rec.traj, f.ts_ns).map(|p| (f.ts_ns, p, f)))
        .collect();
    let chunk = jobs.len().div_ceil(threads.clamp(1, 64)).max(1);
    let mut out = Vec::new();
    let mut err = None;
    std::thread::scope(|sc| {
        let hs: Vec<_> = jobs
            .chunks(chunk)
            .map(|part| {
                let orb = &orb;
                sc.spawn(move || -> Result<Vec<PointFrame>, String> {
                    let mut v = Vec::new();
                    for (ts, sb, f) in part {
                        let (x, y) = (pngread::read(&f.cam0)?, pngread::read(&f.cam1)?);
                        let feats = frame_features(rig, [&x, &y], orb);
                        v.push(PointFrame { ts_ns: *ts, pose: *sb, pts: frame_points(rig, &feats, sb, max_range) });
                    }
                    Ok(v)
                })
            })
            .collect();
        for h in hs {
            match h.join() {
                Ok(Ok(v)) => out.extend(v),
                Ok(Err(e)) => err = Some(e),
                Err(_) => err = Some("worker panicked".into()),
            }
        }
    });
    if let Some(e) = err {
        return Err(e);
    }
    out.sort_by_key(|f: &PointFrame| f.ts_ns);
    Ok(out)
}
