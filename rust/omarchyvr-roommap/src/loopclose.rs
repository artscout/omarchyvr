//! Loop closure inside the mapping session (4-DoF pose graph over the keyframes).
//!
//! Basalt is a VIO: no loop closure, it drifts 1-4 % of the path on our walks (0.3-1.1 m end-start). A map built
//! from its poses is therefore not self-consistent: a place visited twice is stored twice, shifted by the drift
//! between the visits, and a later session relocalises against one copy or the other (or a blend). Here every
//! keyframe is relocalised against the keyframes of *other* times of the same session (more than `min_gap`
//! away); each success is a loop edge. With the VIO's relative motion between consecutive keyframes as odometry
//! edges, a pose graph in yaw + translation per keyframe (roll and pitch are observable from gravity and kept)
//! is solved by Gauss-Newton (robust, outlier loops dropped). The first keyframe stays fixed, so the room frame
//! is the mapping session's world at its start. Keyframes and their points move with their node; the frames
//! between keyframes get the time-interpolated correction (for the multi-view refinement that follows).

use crate::align::{observe_kfs, wrap, ObserveParams};
use crate::refine::MapFrame;
use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use omarchyvr_room::map::{yaw_of_mat, RoomMap};
use omarchyvr_room::pnp::rot_y;
use omarchyvr_room::session::Alignment;
use omarchyvr_room::stereo::Rig;

/// Loop closure parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct LoopParams {
    /// Only keyframes at least this far apart in time can form a loop (ns).
    pub min_gap_ns: i64,
    /// The loop's points come from keyframes within this time of the best matching one (ns).
    pub local_ns: i64,
    /// Inliers needed for a loop edge.
    pub min_inliers: usize,
    /// Inlier fraction needed.
    pub min_frac: f64,
    /// RMS residual allowed (deg).
    pub max_rms_deg: f64,
    /// Largest correction a single loop may ask for (m).
    pub max_jump_m: f64,
    /// ... (deg).
    pub max_jump_deg: f64,
    /// Loop edge 1-sigma (m).
    pub sigma_loop_m: f64,
    /// Loop edge 1-sigma (deg).
    pub sigma_loop_deg: f64,
    /// Odometry 1-sigma: fraction of the distance travelled ...
    pub odo_frac: f64,
    /// ... plus a floor (m).
    pub odo_min_m: f64,
    /// Odometry yaw 1-sigma per metre travelled (deg) ...
    pub odo_deg_per_m: f64,
    /// ... plus a floor (deg).
    pub odo_min_deg: f64,
    /// Loops whose normalised residual exceeds this after the solve are dropped (then solved again).
    pub reject_chi: f64,
}

impl Default for LoopParams {
    fn default() -> Self {
        LoopParams {
            min_gap_ns: 12_000_000_000,
            local_ns: 4_000_000_000,
            min_inliers: 30,
            min_frac: 0.3,
            max_rms_deg: 0.5,
            max_jump_m: 2.0,
            max_jump_deg: 15.0,
            sigma_loop_m: 0.03,
            sigma_loop_deg: 0.3,
            odo_frac: 0.02,
            odo_min_m: 0.003,
            odo_deg_per_m: 0.3,
            odo_min_deg: 0.05,
            reject_chi: 4.0,
        }
    }
}

/// Counters.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LoopStats {
    /// Keyframes tried.
    pub tried: usize,
    /// Loop edges found.
    pub loops: usize,
    /// Loop edges dropped as outliers.
    pub rejected: usize,
    /// Largest keyframe correction (m).
    pub max_corr_m: f64,
    /// Largest keyframe correction (deg).
    pub max_corr_deg: f64,
}

/// Correction of the tracker trajectory at a time: `room_new_from_room_old` (yaw + translation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Correction {
    /// Time (ns).
    pub ts_ns: i64,
    /// The correction.
    pub c: Alignment,
}

/// One pose-graph edge: pose of `b` relative to `a` (yaw difference, translation in `a`'s yaw frame).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Edge {
    /// From node.
    pub a: usize,
    /// To node.
    pub b: usize,
    /// Yaw of b minus yaw of a (rad).
    pub dyaw: f64,
    /// `Ry(yaw_a)^T (t_b - t_a)`.
    pub dt: Vec3,
    /// Weight of the translation residual (1 / sigma^2).
    pub wt: f64,
    /// Weight of the yaw residual.
    pub wy: f64,
    /// Loop edge (robust, can be rejected).
    pub is_loop: bool,
}

fn ryt(y: f64) -> Mat3 {
    geom::transpose(&rot_y(y))
}

fn dryt(y: f64) -> Mat3 {
    let (s, c) = y.sin_cos();
    [[-s, 0.0, -c], [0.0, 0.0, 0.0], [c, 0.0, -s]]
}

/// Residual of an edge: (yaw, translation).
pub fn edge_residual(e: &Edge, x: &[(f64, Vec3)]) -> (f64, Vec3) {
    let (ya, ta) = x[e.a];
    let (yb, tb) = x[e.b];
    let ry = wrap(yb - ya - e.dyaw);
    let rt = geom::sub(geom::mat_vec(&ryt(ya), geom::sub(tb, ta)), e.dt);
    (ry, rt)
}

fn chi(e: &Edge, x: &[(f64, Vec3)]) -> f64 {
    let (ry, rt) = edge_residual(e, x);
    (ry * ry * e.wy + geom::dot(rt, rt) * e.wt).sqrt()
}

fn cholesky_solve(mut a: Vec<f64>, n: usize, mut b: Vec<f64>) -> Option<Vec<f64>> {
    for j in 0..n {
        let mut d = a[j * n + j];
        for k in 0..j {
            d -= a[j * n + k] * a[j * n + k];
        }
        if d <= 0.0 {
            return None;
        }
        let d = d.sqrt();
        a[j * n + j] = d;
        for i in j + 1..n {
            let mut s = a[i * n + j];
            for k in 0..j {
                s -= a[i * n + k] * a[j * n + k];
            }
            a[i * n + j] = s / d;
        }
    }
    for i in 0..n {
        let mut s = b[i];
        for k in 0..i {
            s -= a[i * n + k] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    for i in (0..n).rev() {
        let mut s = b[i];
        for k in i + 1..n {
            s -= a[k * n + i] * b[k];
        }
        b[i] = s / a[i * n + i];
    }
    Some(b)
}

/// Gauss-Newton on the node poses (node 0 fixed); loop edges get a Huber weight at `huber` sigmas.
pub fn solve_graph(x: &mut [(f64, Vec3)], edges: &[Edge], iters: usize, huber: f64) {
    let n = x.len();
    if n < 2 {
        return;
    }
    let dim = 4 * (n - 1);
    for _ in 0..iters {
        let mut h = vec![0.0; dim * dim];
        let mut g = vec![0.0; dim];
        for e in edges {
            let (ry, rt) = edge_residual(e, x);
            let w = if e.is_loop {
                let c = chi(e, x);
                if c > huber {
                    huber / c
                } else {
                    1.0
                }
            } else {
                1.0
            };
            let (ya, ta) = x[e.a];
            let tb = x[e.b].1;
            let rta = ryt(ya);
            let d_rt_dya = geom::mat_vec(&dryt(ya), geom::sub(tb, ta));
            // rows: 0 = yaw residual, 1..3 = translation residual; columns: (node, param)
            let mut rows: Vec<(f64, f64, Vec<(usize, f64)>)> = Vec::with_capacity(4);
            rows.push((ry, e.wy * w, vec![(e.b * 4, 1.0), (e.a * 4, -1.0)]));
            for i in 0..3 {
                let mut cols = vec![(e.a * 4, d_rt_dya[i])];
                for k in 0..3 {
                    cols.push((e.b * 4 + 1 + k, rta[i][k]));
                    cols.push((e.a * 4 + 1 + k, -rta[i][k]));
                }
                rows.push((rt[i], e.wt * w, cols));
            }
            for (r, wr, cols) in &rows {
                for &(ci, vi) in cols {
                    if ci < 4 {
                        continue;
                    }
                    let i = ci - 4;
                    g[i] += wr * vi * r;
                    for &(cj, vj) in cols {
                        if cj < 4 {
                            continue;
                        }
                        h[i * dim + cj - 4] += wr * vi * vj;
                    }
                }
            }
        }
        for i in 0..dim {
            h[i * dim + i] += 1e-9;
        }
        let Some(d) = cholesky_solve(h, dim, g.iter().map(|v| -v).collect()) else { return };
        let mut step: f64 = 0.0;
        for k in 1..n {
            let o = 4 * (k - 1);
            x[k].0 = wrap(x[k].0 + d[o]);
            for i in 0..3 {
                x[k].1[i] += d[o + 1 + i];
            }
            step = step.max(d[o..o + 4].iter().map(|v| v.abs()).fold(0.0, f64::max));
        }
        if step < 1e-8 {
            break;
        }
    }
}

fn odo_edge(a: usize, b: usize, pa: &Pose, pb: &Pose, p: &LoopParams) -> Edge {
    let (ya, yb) = (yaw_of_mat(&pa.r), yaw_of_mat(&pb.r));
    let dt = geom::mat_vec(&ryt(ya), geom::sub(pb.t, pa.t));
    let dist = geom::norm(dt);
    let st = p.odo_min_m + p.odo_frac * dist;
    let sy = (p.odo_min_deg + p.odo_deg_per_m * dist).to_radians();
    Edge { a, b, dyaw: wrap(yb - ya), dt, wt: 1.0 / (st * st), wy: 1.0 / (sy * sy), is_loop: false }
}

/// Interpolated correction at `ts` (clamped at the ends).
pub fn correction_at(c: &[Correction], ts: i64) -> Alignment {
    if c.is_empty() {
        return Alignment { yaw: 0.0, t: [0.0; 3] };
    }
    let i = c.partition_point(|x| x.ts_ns < ts);
    if i == 0 {
        return c[0].c;
    }
    if i >= c.len() {
        return c[c.len() - 1].c;
    }
    let (a, b) = (&c[i - 1], &c[i]);
    let f = (ts - a.ts_ns) as f64 / (b.ts_ns - a.ts_ns).max(1) as f64;
    Alignment {
        yaw: wrap(a.c.yaw + f * wrap(b.c.yaw - a.c.yaw)),
        t: [0, 1, 2].map(|k| a.c.t[k] * (1.0 - f) + b.c.t[k] * f),
    }
}

/// Detect loops, solve the pose graph, move keyframes, points and frames. Returns the counters and the
/// per-keyframe corrections (sorted by time).
pub fn close_loops(
    map: &mut RoomMap,
    rig: &Rig,
    frames: &mut [MapFrame],
    op: &ObserveParams,
    p: &LoopParams,
    threads: usize,
) -> (LoopStats, Vec<Correction>) {
    let mut st = LoopStats::default();
    let mut order: Vec<usize> = (0..map.keyframes.len()).collect();
    order.sort_by_key(|&i| map.keyframes[i].ts_ns);
    let n = order.len();
    if n < 3 {
        return (st, Vec::new());
    }
    let node_of: std::collections::HashMap<usize, usize> = order.iter().enumerate().map(|(nd, &k)| (k, nd)).collect();
    let old: Vec<Pose> = order.iter().map(|&k| map.keyframes[k].pose).collect();
    let mut edges: Vec<Edge> = (1..n).map(|i| odo_edge(i - 1, i, &old[i - 1], &old[i], p)).collect();
    // Loop detection (parallel over keyframes).
    let found: Vec<Edge> = {
        let chunk = n.div_ceil(threads.clamp(1, 64)).max(1);
        let nodes: Vec<usize> = (0..n).collect();
        let map_ref: &RoomMap = map;
        let frames_ref: &[MapFrame] = frames;
        let order_ref = &order;
        let old_ref = &old;
        let node_of_ref = &node_of;
        std::thread::scope(|sc| {
            let hs: Vec<_> = nodes
                .chunks(chunk)
                .map(|part| {
                    sc.spawn(move || {
                        let mut out = Vec::new();
                        for &ni in part {
                            let ki = order_ref[ni];
                            let ti = map_ref.keyframes[ki].ts_ns;
                            let fi = frames_ref.partition_point(|f| f.ts_ns < ti);
                            let Some(f) = [fi.saturating_sub(1), fi]
                                .iter()
                                .filter_map(|&j| frames_ref.get(j))
                                .filter(|f| (f.ts_ns - ti).abs() <= 60_000_000)
                                .min_by_key(|f| (f.ts_ns - ti).abs())
                            else {
                                continue;
                            };
                            let far: Vec<usize> = (0..map_ref.keyframes.len())
                                .filter(|&j| (map_ref.keyframes[j].ts_ns - ti).abs() > p.min_gap_ns)
                                .collect();
                            let Some(o1) = observe_kfs(map_ref, &far, rig, &f.feats, f.ts_ns, &f.room_from_body, op) else {
                                continue;
                            };
                            // dominant keyframe among the inliers
                            let mut cnt: std::collections::HashMap<u32, usize> = std::collections::HashMap::new();
                            for &i in &o1.inliers {
                                *cnt.entry(o1.corr_kf[i]).or_default() += 1;
                            }
                            let Some((&kk, _)) = cnt.iter().max_by_key(|(k, c)| (**c, std::cmp::Reverse(**k))) else { continue };
                            let tk = map_ref.keyframes[kk as usize].ts_ns;
                            let local: Vec<usize> = far
                                .iter()
                                .copied()
                                .filter(|&j| (map_ref.keyframes[j].ts_ns - tk).abs() <= p.local_ns)
                                .collect();
                            let Some(o2) = observe_kfs(map_ref, &local, rig, &f.feats, f.ts_ns, &f.room_from_body, op) else {
                                continue;
                            };
                            if o2.inliers.len() < p.min_inliers
                                || (o2.inliers.len() as f64) < p.min_frac * o2.corrs.len() as f64
                                || o2.rms_deg > p.max_rms_deg
                            {
                                continue;
                            }
                            // node i measured: PnP pose of the frame, carried to the keyframe by the VIO
                            let ti_old = old_ref[ni];
                            let meas = o2.room_from_body.compose(&f.room_from_body.inverse().compose(&ti_old));
                            let jump_m = geom::norm(geom::sub(meas.t, ti_old.t));
                            let jump_d = wrap(yaw_of_mat(&meas.r) - yaw_of_mat(&ti_old.r)).abs().to_degrees();
                            if jump_m > p.max_jump_m || jump_d > p.max_jump_deg {
                                continue;
                            }
                            let nk = node_of_ref[&(kk as usize)];
                            let mut e = odo_edge(nk, ni, &old_ref[nk], &meas, p);
                            let sy = p.sigma_loop_deg.to_radians();
                            e.wt = 1.0 / (p.sigma_loop_m * p.sigma_loop_m);
                            e.wy = 1.0 / (sy * sy);
                            e.is_loop = true;
                            out.push(e);
                        }
                        out
                    })
                })
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap_or_default()).collect()
        })
    };
    st.tried = n;
    st.loops = found.len();
    edges.extend(found);
    let x0: Vec<(f64, Vec3)> = old.iter().map(|q| (yaw_of_mat(&q.r), q.t)).collect();
    let mut x = x0.clone();
    solve_graph(&mut x, &edges, 20, 2.0);
    for _ in 0..3 {
        let before = edges.len();
        edges.retain(|e| !e.is_loop || chi(e, &x) <= p.reject_chi);
        st.rejected += before - edges.len();
        if edges.len() == before {
            break;
        }
        x = x0.clone();
        solve_graph(&mut x, &edges, 20, 2.0);
    }
    // Apply: corrections per node.
    let mut corr: Vec<Correction> = Vec::with_capacity(n);
    for (nd, &k) in order.iter().enumerate() {
        let (yo, to) = x0[nd];
        let (yn, tn) = x[nd];
        let yc = wrap(yn - yo);
        let tc = geom::sub(tn, geom::mat_vec(&rot_y(yc), to));
        let c = Alignment { yaw: yc, t: tc };
        st.max_corr_m = st.max_corr_m.max(geom::norm(geom::sub(tn, to)));
        st.max_corr_deg = st.max_corr_deg.max(yc.abs().to_degrees());
        let cp = c.pose();
        let kf = &mut map.keyframes[k];
        kf.pose = cp.compose(&kf.pose);
        for m in &mut kf.points {
            m.p = cp.apply(m.p);
        }
        corr.push(Correction { ts_ns: kf.ts_ns, c });
    }
    for f in frames.iter_mut() {
        let c = correction_at(&corr, f.ts_ns).pose();
        f.room_from_body = c.compose(&f.room_from_body);
    }
    (st, corr)
}
