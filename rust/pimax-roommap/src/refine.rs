//! Multi-view refinement of the map points.
//!
//! A keyframe's points come from one stereo pair of the lower cameras: 111 mm baseline, little overlap (the pair
//! diverges by 54 deg). Depth error grows with depth squared: about 0.3 deg of angular noise gives ~18 cm at 2 m.
//! Relocalisation against such points has a translation bias of the same order. The mapping session, though,
//! sees every point from many places along the walk, with baselines of metres. Here every point is searched in the
//! frames of the mapping session within a few seconds of its keyframe (the VIO is locally accurate), by projecting
//! it into each camera and matching its descriptor near the projection, and re-triangulated from all observations
//! (Gauss-Newton on the angular error, outliers gated). Points that cannot be confirmed from enough parallax can
//! be dropped.

use pimax_cam::geom::{self, Pose, Vec3};
use pimax_room::map::RoomMap;
use pimax_room::orb;
use pimax_room::stereo::{Features, Rig};

/// A frame of the mapping session: time, `room_from_body`, features of rig camera 0 and 1 (images dropped).
pub struct MapFrame {
    /// Time (ns).
    pub ts_ns: i64,
    /// Pose in the room frame.
    pub room_from_body: Pose,
    /// Features per camera.
    pub feats: Vec<Features>,
    grids: Vec<Grid>,
}

/// Keypoint buckets of one image.
struct Grid {
    cell: f32,
    cols: usize,
    rows: usize,
    cells: Vec<Vec<u32>>,
}

impl Grid {
    fn new(f: &Features, w: usize, h: usize, cell: f32) -> Grid {
        let cols = (w as f32 / cell).ceil() as usize + 1;
        let rows = (h as f32 / cell).ceil() as usize + 1;
        let mut cells = vec![Vec::new(); cols * rows];
        for (i, k) in f.kps.iter().enumerate() {
            let (cx, cy) = ((k.x / cell) as usize, (k.y / cell) as usize);
            if cx < cols && cy < rows {
                cells[cy * cols + cx].push(i as u32);
            }
        }
        Grid { cell, cols, rows, cells }
    }

    fn near(&self, x: f32, y: f32, r: f32, out: &mut Vec<u32>) {
        out.clear();
        let (x0, x1) = (((x - r) / self.cell).floor().max(0.0) as usize, ((x + r) / self.cell).floor() as usize);
        let (y0, y1) = (((y - r) / self.cell).floor().max(0.0) as usize, ((y + r) / self.cell).floor() as usize);
        for cy in y0..=y1.min(self.rows - 1) {
            for cx in x0..=x1.min(self.cols - 1) {
                out.extend_from_slice(&self.cells[cy * self.cols + cx]);
            }
        }
    }
}

impl MapFrame {
    /// New frame (drops the undistorted images, builds the keypoint grids).
    pub fn new(ts_ns: i64, room_from_body: Pose, mut feats: Vec<Features>, rig: &Rig) -> MapFrame {
        let grids = feats
            .iter()
            .zip(rig.cams.iter())
            .map(|(f, c)| Grid::new(f, c.pinhole.w, c.pinhole.h, 16.0))
            .collect();
        for f in &mut feats {
            f.img = Vec::new();
        }
        MapFrame { ts_ns, room_from_body, feats, grids }
    }
}

/// Refinement parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct RefineParams {
    /// Frames within this time of the keyframe are searched (ns).
    pub window_ns: i64,
    /// Search radius around the projection (pinhole px).
    pub radius_px: f32,
    /// Descriptor distance accepted.
    pub max_dist: u32,
    /// Observations needed (besides the keyframe's stereo pair).
    pub min_obs: usize,
    /// Largest angle between two observing rays must reach this (deg).
    pub min_parallax_deg: f64,
    /// Observation residual gate (deg).
    pub gate_deg: f64,
    /// RMS residual accepted after refinement (deg).
    pub max_rms_deg: f64,
    /// A refined point may move at most this far (m) ...
    pub max_move_m: f64,
    /// ... or this fraction of its stereo depth.
    pub max_move_frac: f64,
    /// Drop points that could not be refined.
    pub drop_unrefined: bool,
}

impl Default for RefineParams {
    fn default() -> Self {
        RefineParams {
            window_ns: 6_000_000_000,
            radius_px: 10.0,
            max_dist: 50,
            min_obs: 4,
            min_parallax_deg: 3.0,
            gate_deg: 0.8,
            max_rms_deg: 0.35,
            max_move_m: 1.5,
            max_move_frac: 0.6,
            drop_unrefined: true,
        }
    }
}

/// Counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RefineStats {
    /// Points before.
    pub points: usize,
    /// Points refined.
    pub refined: usize,
    /// Points dropped.
    pub dropped: usize,
    /// Median move of the refined points (m).
    pub median_move_m: f64,
    /// Median observations of the refined points.
    pub median_obs: f64,
}

struct Obs {
    cam_from_room: Pose,
    centre: Vec3,
    b: Vec3,
}

fn project(rig: &Rig, cam: usize, x_cam: Vec3) -> Option<(f32, f32)> {
    let ph = &rig.cams[cam].pinhole;
    let v = geom::mat_vec(&geom::transpose(&ph.cam_from_virt), x_cam);
    if v[2] <= 1e-6 {
        return None;
    }
    let (x, y) = (ph.f * v[0] / v[2] + ph.cx, ph.f * v[1] / v[2] + ph.cy);
    ph.covered(x as f32, y as f32).then_some((x as f32, y as f32))
}

/// Gauss-Newton on the point (angular residual `b x normalize(x_cam)`).
fn triangulate(obs: &[&Obs], init: Vec3) -> Vec3 {
    let mut p = init;
    for _ in 0..10 {
        let (mut h, mut g) = ([[0.0; 3]; 3], [0.0; 3]);
        for o in obs {
            let r0 = residual(o, p);
            let Some(r0) = r0 else { continue };
            let mut j = [[0.0; 3]; 3];
            for k in 0..3 {
                let mut q = p;
                let s = 1e-6 * (1.0 + geom::norm(sub(p, o.centre)));
                q[k] += s;
                let Some(r1) = residual(o, q) else { continue };
                for i in 0..3 {
                    j[i][k] = (r1[i] - r0[i]) / s;
                }
            }
            for i in 0..3 {
                for a in 0..3 {
                    g[a] += j[i][a] * r0[i];
                    for b in 0..3 {
                        h[a][b] += j[i][a] * j[i][b];
                    }
                }
            }
        }
        let Some(d) = solve3(h, [-g[0], -g[1], -g[2]]) else { break };
        p = geom::add(p, d);
        if geom::norm(d) < 1e-7 {
            break;
        }
    }
    p
}

fn sub(a: Vec3, b: Vec3) -> Vec3 {
    geom::sub(a, b)
}

fn residual(o: &Obs, p: Vec3) -> Option<Vec3> {
    let x = o.cam_from_room.apply(p);
    let n = geom::norm(x);
    if n.is_nan() || n <= 1e-9 || geom::dot(x, o.b) <= 0.0 {
        return None;
    }
    Some(geom::cross(o.b, geom::scale(x, 1.0 / n)))
}

fn res_deg(o: &Obs, p: Vec3) -> f64 {
    residual(o, p).map(|r| geom::norm(r).min(1.0).asin().to_degrees()).unwrap_or(180.0)
}

fn solve3(a: [[f64; 3]; 3], b: [f64; 3]) -> Option<Vec3> {
    let d = geom::det(&a);
    if d.abs() < 1e-18 {
        return None;
    }
    let col = |k: usize| {
        let mut m = a;
        for i in 0..3 {
            m[i][k] = b[i];
        }
        geom::det(&m) / d
    };
    Some([col(0), col(1), col(2)])
}

fn parallax_deg(obs: &[&Obs], p: Vec3) -> f64 {
    let dirs: Vec<Vec3> = obs.iter().map(|o| geom::normalize(sub(p, o.centre))).collect();
    let mut best: f64 = 0.0;
    for i in 0..dirs.len() {
        for j in i + 1..dirs.len() {
            best = best.max(geom::angle_deg(dirs[i], dirs[j]));
        }
    }
    best
}

/// Refine the points of every keyframe from the frames of the mapping session (`threads` workers).
pub fn refine_map(map: &mut RoomMap, rig: &Rig, frames: &[MapFrame], p: &RefineParams, threads: usize) -> RefineStats {
    let bfc = rig.body_from_cams();
    let mut st = RefineStats::default();
    let n_kf = map.keyframes.len();
    let chunk = n_kf.div_ceil(threads.clamp(1, 64)).max(1);
    let mut moves: Vec<f64> = Vec::new();
    let mut nobs: Vec<f64> = Vec::new();
    std::thread::scope(|sc| {
        let hs: Vec<_> = map
            .keyframes
            .chunks_mut(chunk)
            .map(|kfs| {
                let bfc = &bfc;
                sc.spawn(move || {
                    let mut st = RefineStats::default();
                    let mut moves = Vec::new();
                    let mut nobs = Vec::new();
                    let mut cand = Vec::new();
                    for kf in kfs.iter_mut() {
                        let near: Vec<&MapFrame> =
                            frames.iter().filter(|f| (f.ts_ns - kf.ts_ns).abs() <= p.window_ns).collect();
                        // camera poses of the candidate frames
                        let cams: Vec<Vec<(Pose, Vec3)>> = near
                            .iter()
                            .map(|f| {
                                bfc.iter()
                                    .map(|b| {
                                        let rc = f.room_from_body.compose(b);
                                        (rc.inverse(), rc.t)
                                    })
                                    .collect()
                            })
                            .collect();
                        let kf_body = kf.pose;
                        let mut keep = Vec::with_capacity(kf.points.len());
                        for mp in kf.points.iter() {
                            st.points += 1;
                            let mut obs: Vec<Obs> = Vec::new();
                            for (fi, f) in near.iter().enumerate() {
                                for c in 0..f.feats.len().min(rig.cams.len()) {
                                    let (cfr, centre) = cams[fi][c];
                                    let xc = cfr.apply(mp.p);
                                    let Some((u, v)) = project(rig, c, xc) else { continue };
                                    f.grids[c].near(u, v, p.radius_px, &mut cand);
                                    let (mut best, mut second, mut bi) = (u32::MAX, u32::MAX, usize::MAX);
                                    for &i in cand.iter() {
                                        let k = &f.feats[c].kps[i as usize];
                                        if (k.x - u).powi(2) + (k.y - v).powi(2) > p.radius_px * p.radius_px {
                                            continue;
                                        }
                                        let d = orb::hamming(&mp.desc, &f.feats[c].desc[i as usize]);
                                        if d < best {
                                            second = best;
                                            best = d;
                                            bi = i as usize;
                                        } else if d < second {
                                            second = d;
                                        }
                                    }
                                    if best <= p.max_dist && (second == u32::MAX || (best as f32) < 0.9 * second as f32) {
                                        obs.push(Obs { cam_from_room: cfr, centre, b: f.feats[c].bearings[bi] });
                                    }
                                }
                            }
                            let depth = geom::norm(sub(mp.p, kf_body.t));
                            let mut refined = None;
                            if obs.len() >= p.min_obs {
                                let all: Vec<&Obs> = obs.iter().collect();
                                let mut x = triangulate(&all, mp.p);
                                let mut inl: Vec<&Obs> = all.iter().copied().filter(|o| res_deg(o, x) <= p.gate_deg * 2.0).collect();
                                if inl.len() >= p.min_obs {
                                    x = triangulate(&inl, x);
                                    inl = all.iter().copied().filter(|o| res_deg(o, x) <= p.gate_deg).collect();
                                    if inl.len() >= p.min_obs {
                                        x = triangulate(&inl, x);
                                        let rms = (inl.iter().map(|o| res_deg(o, x).powi(2)).sum::<f64>() / inl.len() as f64).sqrt();
                                        let mv = geom::norm(sub(x, mp.p));
                                        // in front of every camera that saw it, enough parallax, consistent
                                        let front = inl.iter().all(|o| geom::dot(o.cam_from_room.apply(x), o.b) > 0.0);
                                        if front
                                            && rms <= p.max_rms_deg
                                            && parallax_deg(&inl, x) >= p.min_parallax_deg
                                            && mv <= p.max_move_m.min(p.max_move_frac * depth.max(0.3))
                                        {
                                            refined = Some((x, mv, inl.len()));
                                        }
                                    }
                                }
                            }
                            match refined {
                                Some((x, mv, n)) => {
                                    st.refined += 1;
                                    moves.push(mv);
                                    nobs.push(n as f64);
                                    let mut q = *mp;
                                    q.p = x;
                                    keep.push(q);
                                }
                                None if p.drop_unrefined => st.dropped += 1,
                                None => keep.push(*mp),
                            }
                        }
                        kf.points = keep;
                    }
                    (st, moves, nobs)
                })
            })
            .collect();
        for h in hs {
            if let Ok((s, m, o)) = h.join() {
                st.points += s.points;
                st.refined += s.refined;
                st.dropped += s.dropped;
                moves.extend(m);
                nobs.extend(o);
            }
        }
    });
    let med = |mut v: Vec<f64>| {
        v.sort_by(|a, b| a.total_cmp(b));
        v.get(v.len() / 2).copied().unwrap_or(f64::NAN)
    };
    st.median_move_m = med(moves);
    st.median_obs = med(nobs);
    st
}
