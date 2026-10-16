//! Relocalisation against a saved room map, pooled over many frames.
//!
//! `omarchyvr-room` relocalises one frame at a time (gravity-aware 4-DoF PnP on ~100 stereo points of the lower
//! camera pair). On real recordings a single frame is noisy (cm to dm, about a degree). Here every frame that
//! matches the map contributes its 2D-3D correspondences, and **one** transform `room_from_slam` (yaw about up +
//! translation; both worlds are gravity aligned) is fitted jointly over all frames of a window, using the tracker's
//! pose of each frame. The VIO is accurate over a few seconds, so the frames act as one wide multi-camera rig:
//! many more points, many viewpoints, and the noise of single fits averages out.
//!
//! Steps per frame ([`observe`]): ORB on both cameras, rank keyframes by quick descriptor hits, brute-force match
//! against the points of the best keyframes, RANSAC 4-DoF PnP (`omarchyvr_room::pnp`) for a per-frame hypothesis.
//! Per window ([`joint_fit`]): robust start (median of the per-frame hypotheses), then Huber Levenberg-Marquardt
//! over all correspondences of all frames, re-gated against the joint model.

use pimax_cam::geom::{self, Mat3, Pose, Vec3};
use pimax_cam::slam::quat_of;
use omarchyvr_room::map::{room_from_slam, tilt_of, Reloc, RoomMap};
use omarchyvr_room::orb::{self, Descriptor};
use omarchyvr_room::pnp::{self, Corr, PnpParams};
use omarchyvr_room::session::Alignment;
use omarchyvr_room::stereo::{Features, Rig};

/// Per-frame matching parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct ObserveParams {
    /// Keyframes matched in full after ranking.
    pub top_k: usize,
    /// Query descriptors used for ranking.
    pub rank_sample: usize,
    /// Descriptor distance for a ranking hit.
    pub rank_dist: u32,
    /// Matching distance.
    pub max_dist: u32,
    /// Lowe ratio.
    pub ratio: f32,
    /// Per-frame RANSAC.
    pub pnp: PnpParams,
    /// Per-frame inliers needed for a hypothesis.
    pub min_inliers: usize,
    /// Inlier fraction needed.
    pub min_inlier_frac: f64,
}

impl Default for ObserveParams {
    fn default() -> Self {
        ObserveParams {
            top_k: 4,
            rank_sample: 250,
            rank_dist: 45,
            max_dist: 64,
            ratio: 0.85,
            pnp: PnpParams {
                thresh_deg: 0.8,
                max_iters: 500,
                confidence: 0.999,
                min_inliers: 12,
                seed: 7,
            },
            min_inliers: 15,
            min_inlier_frac: 0.15,
        }
    }
}

/// One frame matched against the map.
#[derive(Clone, Debug, PartialEq)]
pub struct FrameObs {
    /// Frame time (ns).
    pub ts_ns: i64,
    /// Tracker pose of the frame (y-up tracker world, IMU body).
    pub slam_from_body: Pose,
    /// All correspondences found for the frame (bearing in the camera frame, point in room coordinates).
    pub corrs: Vec<Corr>,
    /// Keyframe index (into `map.keyframes`) of each correspondence's map point.
    pub corr_kf: Vec<u32>,
    /// Per-frame RANSAC inliers (indices into `corrs`).
    pub inliers: Vec<usize>,
    /// Per-frame PnP result: the body in the room frame.
    pub room_from_body: Pose,
    /// Per-frame alignment hypothesis `room_from_slam`.
    pub hyp: Alignment,
    /// Per-frame RMS residual (deg).
    pub rms_deg: f64,
}

/// Match one frame (`feats[i]` = features of rig camera i) against the map; `None` when the room is not seen.
pub fn observe(
    map: &RoomMap,
    rig: &Rig,
    feats: &[Features],
    ts_ns: i64,
    slam_from_body: &Pose,
    p: &ObserveParams,
) -> Option<FrameObs> {
    let all: Vec<usize> = (0..map.keyframes.len()).collect();
    observe_kfs(map, &all, rig, feats, ts_ns, slam_from_body, p)
}

/// [`observe`] restricted to the keyframes `kfs` (indices into `map.keyframes`).
pub fn observe_kfs(
    map: &RoomMap,
    kfs: &[usize],
    rig: &Rig,
    feats: &[Features],
    ts_ns: i64,
    slam_from_body: &Pose,
    p: &ObserveParams,
) -> Option<FrameObs> {
    if kfs.is_empty() {
        return None;
    }
    let all_q: Vec<&Descriptor> = feats.iter().flat_map(|f| f.desc.iter()).collect();
    if all_q.is_empty() {
        return None;
    }
    let stride = (all_q.len() / p.rank_sample.max(1)).max(1);
    let sample: Vec<&Descriptor> = all_q.iter().step_by(stride).copied().collect();
    let mut scored: Vec<(usize, usize)> = kfs
        .iter()
        .map(|&i| {
            let k = &map.keyframes[i];
            let hits = sample
                .iter()
                .filter(|q| k.points.iter().any(|m| orb::hamming(q, &m.desc) <= p.rank_dist))
                .count();
            (hits, i)
        })
        .collect();
    scored.sort_by_key(|s| std::cmp::Reverse(s.0));
    let top: Vec<usize> = scored.iter().take(p.top_k.max(1)).filter(|s| s.0 > 0).map(|s| s.1).collect();
    if top.is_empty() {
        return None;
    }
    let mut pts: Vec<Vec3> = Vec::new();
    let mut pdesc: Vec<Descriptor> = Vec::new();
    let mut pkf: Vec<u32> = Vec::new();
    for &k in &top {
        for m in &map.keyframes[k].points {
            pts.push(m.p);
            pdesc.push(m.desc);
            pkf.push(k as u32);
        }
    }
    let mut corrs = Vec::new();
    let mut corr_kf = Vec::new();
    for (c, f) in feats.iter().enumerate() {
        for (i, j, _) in orb::match_bf(&f.desc, &pdesc, p.max_dist, p.ratio) {
            corrs.push(Corr { cam: c, bearing: f.bearings[i], point: pts[j] });
            corr_kf.push(pkf[j]);
        }
    }
    if corrs.len() < p.min_inliers {
        return None;
    }
    let bfc = rig.body_from_cams();
    let tilt = tilt_of(quat_of(&slam_from_body.r));
    let res = pnp::ransac(&corrs, &bfc, &tilt, &p.pnp)?;
    if res.inliers.len() < p.min_inliers || (res.inliers.len() as f64) < p.min_inlier_frac * corrs.len() as f64 {
        return None;
    }
    let room_from_body = res.pose.to_pose(&tilt);
    let reloc = Reloc {
        room_from_body,
        inliers: res.inliers.len(),
        corrs: corrs.len(),
        rms_deg: res.rms_deg,
        keyframes: Vec::new(),
    };
    let (yaw, t) = room_from_slam(&reloc, slam_from_body);
    Some(FrameObs {
        ts_ns,
        slam_from_body: *slam_from_body,
        corrs,
        corr_kf,
        inliers: res.inliers,
        hyp: Alignment { yaw, t },
        room_from_body,
        rms_deg: res.rms_deg,
    })
}

/// Joint fit parameters.
#[derive(Clone, Debug, PartialEq)]
pub struct JointParams {
    /// A correspondence counts when its residual under the joint model is below this (deg).
    pub gate_deg: f64,
    /// Huber scale (deg).
    pub huber_deg: f64,
    /// Gate / refit rounds.
    pub rounds: usize,
    /// Frames whose hypothesis is further than this from the robust start are dropped (m) ...
    pub hyp_m: f64,
    /// ... or (deg).
    pub hyp_deg: f64,
}

impl Default for JointParams {
    fn default() -> Self {
        JointParams { gate_deg: 1.0, huber_deg: 0.4, rounds: 3, hyp_m: 0.25, hyp_deg: 4.0 }
    }
}

/// Result of a joint fit.
#[derive(Clone, Debug, PartialEq)]
pub struct JointFit {
    /// `room_from_slam`.
    pub align: Alignment,
    /// Frames that contributed.
    pub frames: usize,
    /// Correspondences used (inliers of the joint model).
    pub inliers: usize,
    /// RMS residual of the inliers (deg).
    pub rms_deg: f64,
    /// Formal 1-sigma of yaw (deg) and translation (m, norm). Optimistic: frames are not independent.
    pub sigma_deg: f64,
    /// Formal 1-sigma of the translation (m).
    pub sigma_m: f64,
    /// Spread of the per-frame hypotheses around the joint result: RMS translation (m) and yaw (deg).
    pub hyp_rms_m: f64,
    /// Per-frame yaw spread (deg).
    pub hyp_rms_deg: f64,
    /// Time span of the frames (first, last) in ns.
    pub span_ns: (i64, i64),
}

/// Wrap an angle to (-pi, pi].
pub fn wrap(mut a: f64) -> f64 {
    use std::f64::consts::PI;
    while a > PI {
        a -= 2.0 * PI;
    }
    while a <= -PI {
        a += 2.0 * PI;
    }
    a
}

fn median(mut v: Vec<f64>) -> f64 {
    v.sort_by(|a, b| a.total_cmp(b));
    if v.is_empty() {
        return f64::NAN;
    }
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// Component-wise median of alignments (yaw unwrapped around the first).
pub fn median_alignment(v: &[Alignment]) -> Option<Alignment> {
    let a0 = v.first()?;
    let yaw = a0.yaw + median(v.iter().map(|a| wrap(a.yaw - a0.yaw)).collect());
    let t = [0, 1, 2].map(|i| median(v.iter().map(|a| a.t[i]).collect()));
    Some(Alignment { yaw: wrap(yaw), t })
}

/// Distance of two alignments: (translation m, yaw deg).
pub fn diff(a: &Alignment, b: &Alignment) -> (f64, f64) {
    (geom::norm(geom::sub(a.t, b.t)), wrap(a.yaw - b.yaw).abs().to_degrees())
}

/// Precomputed per correspondence: body-frame bearing origin/direction and the tracker pose, so the residual only
/// needs the 4 alignment parameters.
struct Ray {
    /// `cam_from_slam` of the observation.
    cam_from_slam: Pose,
    /// Unit bearing in the camera frame.
    b: Vec3,
    /// Map point (room).
    p: Vec3,
}

fn ry(yaw: f64) -> Mat3 {
    pnp::rot_y(yaw)
}

/// Residual `b x normalize(x_cam)` (norm = sine of the angle); `None` when the point is behind the camera.
fn residual(r: &Ray, x: &[f64; 4]) -> Option<Vec3> {
    // p_slam = Ry(yaw)^T (p_room - t)
    let p_slam = geom::mat_vec(&geom::transpose(&ry(x[0])), geom::sub(r.p, [x[1], x[2], x[3]]));
    let xc = r.cam_from_slam.apply(p_slam);
    let n = geom::norm(xc);
    if n.is_nan() || n <= 1e-9 || geom::dot(xc, r.b) <= 0.0 {
        return None;
    }
    Some(geom::cross(r.b, geom::scale(xc, 1.0 / n)))
}

fn angle_deg(r: &Vec3) -> f64 {
    geom::norm(*r).clamp(0.0, 1.0).asin().to_degrees()
}

fn solve4(a: [[f64; 4]; 4], b: [f64; 4]) -> Option<[f64; 4]> {
    let mut m = [[0.0; 5]; 4];
    for i in 0..4 {
        m[i][..4].copy_from_slice(&a[i]);
        m[i][4] = b[i];
    }
    for c in 0..4 {
        let p = (c..4).max_by(|&i, &j| m[i][c].abs().total_cmp(&m[j][c].abs()))?;
        if m[p][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, p);
        for r in 0..4 {
            if r != c {
                let f = m[r][c] / m[c][c];
                for k in c..5 {
                    m[r][k] -= f * m[c][k];
                }
            }
        }
    }
    Some([0, 1, 2, 3].map(|i| m[i][4] / m[i][i]))
}

fn inv4(a: [[f64; 4]; 4]) -> Option<[[f64; 4]; 4]> {
    let mut out = [[0.0; 4]; 4];
    for j in 0..4 {
        let mut e = [0.0; 4];
        e[j] = 1.0;
        let c = solve4(a, e)?;
        for i in 0..4 {
            out[i][j] = c[i];
        }
    }
    Some(out)
}

/// Huber cost, normal equations (H, g) at x over `rays`.
fn linearize(rays: &[Ray], x: &[f64; 4], k: f64) -> (f64, [[f64; 4]; 4], [f64; 4], usize) {
    let (mut cost, mut h, mut g, mut n) = (0.0, [[0.0; 4]; 4], [0.0; 4], 0);
    let steps = [1e-6, 1e-6, 1e-6, 1e-6];
    for r in rays {
        let Some(r0) = residual(r, x) else { continue };
        let mut jac = [[0.0; 4]; 3];
        let mut ok = true;
        for p in 0..4 {
            let (mut xp, mut xm) = (*x, *x);
            xp[p] += steps[p];
            xm[p] -= steps[p];
            match (residual(r, &xp), residual(r, &xm)) {
                (Some(a), Some(b)) => {
                    for i in 0..3 {
                        jac[i][p] = (a[i] - b[i]) / (2.0 * steps[p]);
                    }
                }
                _ => ok = false,
            }
        }
        if !ok {
            continue;
        }
        let e = geom::norm(r0);
        let w = if e <= k { 1.0 } else { k / e };
        cost += if e <= k { e * e } else { 2.0 * k * e - k * k };
        for i in 0..3 {
            for a in 0..4 {
                g[a] += w * jac[i][a] * r0[i];
                for b in 0..4 {
                    h[a][b] += w * jac[i][a] * jac[i][b];
                }
            }
        }
        n += 1;
    }
    (cost, h, g, n)
}

fn huber_cost(rays: &[Ray], x: &[f64; 4], k: f64) -> f64 {
    rays.iter()
        .filter_map(|r| residual(r, x))
        .map(|r| {
            let e = geom::norm(r);
            if e <= k {
                e * e
            } else {
                2.0 * k * e - k * k
            }
        })
        .sum()
}

/// Levenberg-Marquardt on (yaw, tx, ty, tz) with a Huber cost.
fn lm(rays: &[Ray], x0: [f64; 4], k: f64) -> ([f64; 4], [[f64; 4]; 4]) {
    let mut x = x0;
    let mut lambda = 1e-3;
    let (mut cost, mut h, mut g, _) = linearize(rays, &x, k);
    for _ in 0..40 {
        let mut improved = false;
        for _ in 0..12 {
            let mut a = h;
            for i in 0..4 {
                a[i][i] += lambda * h[i][i].max(1e-12);
            }
            let Some(d) = solve4(a, g.map(|v| -v)) else { break };
            let xn = [x[0] + d[0], x[1] + d[1], x[2] + d[2], x[3] + d[3]];
            let cn = huber_cost(rays, &xn, k);
            if cn < cost {
                x = xn;
                lambda = (lambda * 0.3).max(1e-9);
                improved = true;
                let step = d.iter().map(|v| v * v).sum::<f64>().sqrt();
                let lin = linearize(rays, &x, k);
                cost = lin.0;
                h = lin.1;
                g = lin.2;
                if step < 1e-9 {
                    return (x, h);
                }
                break;
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    (x, h)
}

fn build_rays(obs: &[&FrameObs], bfc: &[Pose]) -> Vec<Ray> {
    let mut v = Vec::new();
    for o in obs {
        for c in &o.corrs {
            let Some(b) = bfc.get(c.cam) else { continue };
            let n = geom::norm(c.bearing);
            if n.is_nan() || n <= 0.0 {
                continue;
            }
            v.push(Ray {
                cam_from_slam: o.slam_from_body.compose(b).inverse(),
                b: geom::scale(c.bearing, 1.0 / n),
                p: c.point,
            });
        }
    }
    v
}

/// Fit one `room_from_slam` over all correspondences of `obs` (needs `body_from_cam` of the rig).
pub fn joint_fit(obs: &[&FrameObs], bfc: &[Pose], p: &JointParams) -> Option<JointFit> {
    let hyps: Vec<Alignment> = obs.iter().map(|o| o.hyp).collect();
    let start = median_alignment(&hyps)?;
    let keep: Vec<&FrameObs> = obs
        .iter()
        .copied()
        .filter(|o| {
            let (dm, dd) = diff(&o.hyp, &start);
            dm <= p.hyp_m && dd <= p.hyp_deg
        })
        .collect();
    if keep.is_empty() {
        return None;
    }
    let all = build_rays(&keep, bfc);
    let mut x = [start.yaw, start.t[0], start.t[1], start.t[2]];
    let gate = p.gate_deg.to_radians().sin();
    let k = p.huber_deg.to_radians().sin();
    let mut used: Vec<&Ray> = Vec::new();
    let mut h = [[0.0; 4]; 4];
    for _ in 0..p.rounds.max(1) {
        used = all.iter().filter(|r| residual(r, &x).is_some_and(|e| geom::norm(e) <= gate)).collect();
        if used.len() < 8 {
            return None;
        }
        let sub: Vec<Ray> = used
            .iter()
            .map(|r| Ray { cam_from_slam: r.cam_from_slam, b: r.b, p: r.p })
            .collect();
        let (xn, hn) = lm(&sub, x, k);
        x = xn;
        h = hn;
    }
    let res: Vec<f64> = used.iter().filter_map(|r| residual(r, &x)).map(|e| angle_deg(&e)).collect();
    let n = res.len();
    let rms = (res.iter().map(|a| a * a).sum::<f64>() / n.max(1) as f64).sqrt();
    // Formal covariance: sigma^2 (per residual component) * H^-1.
    let s2 = (rms.to_radians()).powi(2) / 2.0;
    let (sigma_deg, sigma_m) = match inv4(h) {
        Some(c) => ((c[0][0] * s2).sqrt().to_degrees(), ((c[1][1] + c[2][2] + c[3][3]) * s2).sqrt()),
        None => (f64::NAN, f64::NAN),
    };
    let align = Alignment { yaw: wrap(x[0]), t: [x[1], x[2], x[3]] };
    let nk = keep.len() as f64;
    let hyp_rms_m = (keep.iter().map(|o| diff(&o.hyp, &align).0.powi(2)).sum::<f64>() / nk).sqrt();
    let hyp_rms_deg = (keep.iter().map(|o| wrap(o.hyp.yaw - align.yaw).to_degrees().powi(2)).sum::<f64>() / nk).sqrt();
    let span_ns = (
        keep.iter().map(|o| o.ts_ns).min().unwrap_or(0),
        keep.iter().map(|o| o.ts_ns).max().unwrap_or(0),
    );
    Some(JointFit {
        align,
        frames: keep.len(),
        inliers: n,
        rms_deg: rms,
        sigma_deg,
        sigma_m,
        hyp_rms_m,
        hyp_rms_deg,
        span_ns,
    })
}

/// Acceptance rule for a live / first-fix decision.
#[derive(Clone, Debug, PartialEq)]
pub struct AcceptParams {
    /// Window length (ns): only frames this recent are pooled.
    pub window_ns: i64,
    /// Frames with a hypothesis needed in the window.
    pub min_frames: usize,
    /// Joint inliers needed.
    pub min_inliers: usize,
    /// Fraction of the window's frames that must agree with the joint result (within `JointParams::hyp_*`).
    pub min_agree: f64,
}

impl Default for AcceptParams {
    fn default() -> Self {
        AcceptParams { window_ns: 4_000_000_000, min_frames: 12, min_inliers: 300, min_agree: 0.6 }
    }
}

/// First accepted fit when the frames arrive in time order (what a live `relocalize` does): returns the index of the
/// frame at which it was accepted and the fit.
pub fn first_fix(
    obs: &[FrameObs],
    bfc: &[Pose],
    jp: &JointParams,
    ap: &AcceptParams,
) -> Option<(usize, JointFit)> {
    for i in 0..obs.len() {
        let t1 = obs[i].ts_ns;
        let win: Vec<&FrameObs> = obs[..=i].iter().filter(|o| t1 - o.ts_ns <= ap.window_ns).collect();
        if win.len() < ap.min_frames {
            continue;
        }
        if let Some(f) = joint_fit(&win, bfc, jp) {
            if f.inliers >= ap.min_inliers && f.frames as f64 >= ap.min_agree * win.len() as f64 {
                return Some((i, f));
            }
        }
    }
    None
}

/// Joint fits over consecutive windows (consistency over a session = tracker drift + relocalisation noise).
pub fn window_fits(obs: &[FrameObs], bfc: &[Pose], jp: &JointParams, window_ns: i64, min_frames: usize) -> Vec<JointFit> {
    let mut out = Vec::new();
    let Some(t0) = obs.first().map(|o| o.ts_ns) else { return out };
    let t_end = obs.last().map(|o| o.ts_ns).unwrap_or(t0);
    let mut a = t0;
    while a <= t_end {
        let win: Vec<&FrameObs> = obs.iter().filter(|o| o.ts_ns >= a && o.ts_ns < a + window_ns).collect();
        if win.len() >= min_frames {
            if let Some(f) = joint_fit(&win, bfc, jp) {
                out.push(f);
            }
        }
        a += window_ns;
    }
    out
}
