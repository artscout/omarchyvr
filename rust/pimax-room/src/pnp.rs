//! Gravity-aware absolute pose of a multi-camera fisheye rig against a 3D map (4 DoF: yaw + translation).
//!
//! The headset's VIO supplies roll and pitch (the gravity direction), so relocalising against a saved room map only
//! has to find the heading and the position: four unknowns instead of six. Every 2D-3D correspondence constrains two of
//! them, so two correspondences are a minimal sample ([`solve_p2`]); [`ransac`] samples pairs, scores the hypotheses by
//! angular reprojection error over all cameras and polishes the winner with a robust Levenberg-Marquardt fit
//! ([`refine`]).
//!
//! # Conventions
//!
//! * `body_from_cam[c]` is the [`Pose`] `body_from_cam_c` (`p_body = R p_cam + t`); body is the IMU frame = OpenXR head
//!   axes (x right, y up, z back), camera axes are OpenCV (x right, y down, z forward). Index `c` is the camera index
//!   stored in [`Corr::cam`], so one slice describes the whole rig (e.g. `pimax_cam::slam::t_imu_cam` per camera).
//! * The map frame is gravity aligned with +Y up. The unknown is `map_from_body = (R, t)` with
//!   `R = Ry(yaw) * R_tilt`: `Ry` is the rotation about +Y (same as `pimax_cam::slam::qyaw`) and `R_tilt`
//!   (roll and pitch) is known. `p_map = R p_body + t`. `R_tilt` does not have to be free of yaw: passing the
//!   full gravity-aligned VIO orientation `R_vio` makes `yaw` the heading offset between the VIO world and the map,
//!   and `Pose4::to_pose` then gives the body pose in the map.
//! * A correspondence is a camera index, a bearing (the unit ray of the observed pixel in that camera, e.g.
//!   `Kb4::unproject`) and the 3D map point. The residual is the angle between the bearing and the direction in which
//!   the camera sees the map point under a pose: `X_c = body_from_cam_c^-1 (R^T (X - t))`.
//!
//! # The minimal solver
//!
//! Substitute `u = Ry^T t`. Then `Ry^T (X - t) = Ry^T X - u` is linear in `[cos yaw, sin yaw, u, 1]`, hence so is the
//! camera point `X_c`, and the two rows of `[f]x` per correspondence give four linear equations in the five unknowns
//! `[cos, sin, u]`: a one-dimensional affine family. Written in the *level frame* `l = Ry^T (map - t) = R_tilt * body`
//! (body centred, yaw free) a correspondence reads `Ry^T X - u = p + lambda d`: `p = R_tilt t_bc` is the camera centre,
//! `d = R_tilt R_bc f` the unit ray and `lambda > 0` the depth along it. Both rays are perpendicular to
//! `n = d_a x d_b`, so projecting the two equations on `n` and subtracting them removes `u` and leaves one equation
//! `cos * alpha + sin * beta = gamma` in the yaw alone (the affine family, `alpha`, `beta` and `gamma` follow from the
//! point difference and the camera offset). Intersecting that line with `cos^2 + sin^2 = 1` is a quadratic with zero,
//! one or two roots; each root gives the two depths by a 2x2 least squares problem and `t` from the points, and roots
//! with a non-positive depth (the point behind the camera) are dropped.
//!
//! # Typical use
//!
//! ```text
//! let body_from_cam: Vec<Pose> = (0..4).map(|i| t_imu_cam(&cal, i).unwrap()).collect();
//! let corrs: Vec<Corr> = ...;   // bearing = Kb4::unproject(u, v) of a feature, point = its landmark in the map
//! let r_tilt = ...;             // roll/pitch (or the whole gravity-aligned VIO orientation) of this frame
//! if let Some(res) = ransac(&corrs, &body_from_cam, &r_tilt, &PnpParams::default()) {
//!     let map_from_body = res.pose.to_pose(&r_tilt);   // res.inliers: indices into corrs, res.rms_deg: fit quality
//! }
//! ```
//!
//! # Degenerate input
//!
//! Nothing here panics on bad data: invalid correspondences (camera index outside the rig, zero or non-finite bearing
//! or point) are ignored, and the minimal solver returns no solution for identical points, parallel rays, two points
//! on a vertical line, rays whose plane is horizontal (yaw is unobservable) and configurations with no real root.

#![warn(missing_docs)]

use pimax_cam::geom::{add, cross, dot, mat_mul, mat_vec, norm, scale, sub, Mat3, Pose, Vec3};
use std::f64::consts::{FRAC_PI_2, PI};

/// Smallest `|d_a x d_b|` (sine of the angle between the two level-frame rays) the minimal solver accepts.
const MIN_RAY_SIN: f64 = 1e-6;
/// Smallest `|alpha, beta|` (metres: the horizontal extent of the point pair seen through the ray plane, i.e. the
/// lever arm the heading acts on) the solver accepts.
const MIN_YAW_LEVER: f64 = 1e-6;
/// Smallest distance (squared, m^2) between the two map points of a minimal sample.
const MIN_POINT_SEP2: f64 = 1e-12;
/// Smallest accepted depth (m) along a ray for a minimal-solver solution.
const MIN_DEPTH: f64 = 1e-6;
/// How far above 1 `gamma^2 / (alpha^2 + beta^2)` may be and still count as a double root (round-off).
const TANGENT_TOL: f64 = 1e-9;
/// Levenberg-Marquardt: linearisations per `refine`.
const LM_MAX_ITERS: usize = 60;
/// Levenberg-Marquardt: damping retries per linearisation.
const LM_MAX_RETRIES: usize = 14;

/// One 2D-3D correspondence of the rig.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Corr {
    /// Index of the camera in the `body_from_cam` slice given to the solvers.
    pub cam: usize,
    /// Observed ray in that camera's frame (OpenCV axes), normally a unit vector from `Kb4::unproject`; it is
    /// normalised internally, zero or non-finite bearings make the correspondence invalid.
    pub bearing: Vec3,
    /// The matching 3D point in the map frame (metres).
    pub point: Vec3,
}

/// The four unknowns: heading and position of the body in the map. With a known `R_tilt` the body pose is
/// `map_from_body = (Ry(yaw) * R_tilt, t)`.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Pose4 {
    /// Rotation about +Y (up) in radians, in `(-pi, pi]`.
    pub yaw: f64,
    /// Body origin in the map frame (metres).
    pub t: Vec3,
}

impl Pose4 {
    /// `map_from_body` for a given roll/pitch rotation: `R = Ry(yaw) * r_tilt`, translation `t`.
    pub fn to_pose(&self, r_tilt: &Mat3) -> Pose {
        Pose {
            r: mat_mul(&rot_y(self.yaw), r_tilt),
            t: self.t,
        }
    }
}

/// Rotation about +Y by `yaw` radians (`Ry`; the same convention as `pimax_cam::slam::qyaw`).
pub fn rot_y(yaw: f64) -> Mat3 {
    let (s, c) = yaw.sin_cos();
    rot_y_cs(c, s)
}

/// `Ry` from the cosine and sine of the angle.
fn rot_y_cs(c: f64, s: f64) -> Mat3 {
    [[c, 0.0, s], [0.0, 1.0, 0.0], [-s, 0.0, c]]
}

/// RANSAC settings; [`PnpParams::default`] gives the values in the field docs.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PnpParams {
    /// Inlier threshold: angle between the observed and the predicted ray, degrees (default 1.0; must be positive,
    /// values above 89 act as 89). Also the Huber scale of the refinement.
    pub thresh_deg: f64,
    /// Upper bound on the number of minimal samples drawn (default 300).
    pub max_iters: usize,
    /// Probability of having drawn at least one all-inlier sample; sets the adaptive iteration count from the best
    /// inlier ratio so far (default 0.999; values outside `(0, 1)` act as the default).
    pub confidence: f64,
    /// A pose is only returned with at least this many inliers (default 15; values below 2 act as 2).
    pub min_inliers: usize,
    /// Seed of the sampler; equal seeds and inputs give identical results (default a fixed constant).
    pub seed: u64,
}

impl Default for PnpParams {
    fn default() -> PnpParams {
        PnpParams {
            thresh_deg: 1.0,
            max_iters: 300,
            confidence: 0.999,
            min_inliers: 15,
            seed: 0x9E37_79B9_7F4A_7C15,
        }
    }
}

/// Outcome of [`ransac`].
#[derive(Clone, Debug, PartialEq)]
pub struct PnpResult {
    /// Refined body pose.
    pub pose: Pose4,
    /// Indices into the input `corrs` that agree with `pose` within the threshold, ascending.
    pub inliers: Vec<usize>,
    /// RMS angular residual of the inliers under `pose`, degrees.
    pub rms_deg: f64,
    /// Number of minimal samples drawn (including samples that gave no hypothesis).
    pub iterations: usize,
}

/// Small deterministic generator (splitmix64); the sampler must not depend on external crates.
struct SplitMix64(u64);

impl SplitMix64 {
    fn new(seed: u64) -> SplitMix64 {
        SplitMix64(seed)
    }

    fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Uniform integer in `0..n` (multiply-shift, bias below 2^-40 for any realistic `n`).
    fn below(&mut self, n: usize) -> usize {
        ((u128::from(self.next_u64()) * n as u128) >> 64) as usize
    }

    /// Uniform float in `[0, 1)`.
    #[cfg(test)]
    fn unit(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / 9_007_199_254_740_992.0)
    }
}

/// Angle wrapped to `(-pi, pi]`.
fn wrap_pi(a: f64) -> f64 {
    let mut x = a % (2.0 * PI);
    if x > PI {
        x -= 2.0 * PI;
    } else if x <= -PI {
        x += 2.0 * PI;
    }
    x
}

fn finite3(v: Vec3) -> bool {
    v.iter().all(|x| x.is_finite())
}

/// A correspondence in the level frame `l = Ry^T (map - t)` (see the module docs): everything that does not depend
/// on the unknowns is rotated by `R_tilt` once.
#[derive(Clone, Copy, Debug)]
struct Ray {
    /// Camera centre relative to the body origin, `R_tilt * t_body_cam`.
    p: Vec3,
    /// Unit viewing direction, `R_tilt * R_body_cam * bearing`.
    d: Vec3,
    /// Map point.
    x: Vec3,
}

impl Ray {
    /// `None` if the camera index is outside the rig or the bearing / point are unusable.
    fn new(c: &Corr, body_from_cam: &[Pose], r_tilt: &Mat3) -> Option<Ray> {
        let b = body_from_cam.get(c.cam)?;
        if !finite3(c.bearing) || !finite3(c.point) {
            return None;
        }
        let d = mat_vec(r_tilt, mat_vec(&b.r, c.bearing));
        let n = norm(d);
        let p = mat_vec(r_tilt, b.t);
        if !n.is_finite() || n < 1e-12 || !finite3(p) {
            return None;
        }
        Some(Ray {
            p,
            d: scale(d, 1.0 / n),
            x: c.point,
        })
    }

    /// Vector from the camera centre to the map point in the level frame, for `c = cos yaw`, `s = sin yaw`.
    fn offset(&self, c: f64, s: f64, t: Vec3) -> Vec3 {
        let y = sub(self.x, t);
        [
            c * y[0] - s * y[2] - self.p[0],
            y[1] - self.p[1],
            s * y[0] + c * y[2] - self.p[2],
        ]
    }

    /// Cosine of the angle between the observed and the predicted ray; `None` if the point sits on the camera.
    fn cos_angle(&self, c: f64, s: f64, t: Vec3) -> Option<f64> {
        let v = self.offset(c, s, t);
        let n = norm(v);
        (n > 1e-12).then(|| dot(self.d, v) / n)
    }

    /// Angle between the observed and the predicted ray in radians (`inf` if undefined).
    fn angle(&self, c: f64, s: f64, t: Vec3) -> f64 {
        let v = self.offset(c, s, t);
        let n = norm(v);
        if n.is_nan() || n <= 1e-12 {
            return f64::INFINITY;
        }
        norm(cross(self.d, v)).atan2(dot(self.d, v))
    }
}

/// Valid correspondences as rays, with their index in `corrs`.
fn build_rays(
    corrs: &[Corr],
    idx: impl Iterator<Item = usize>,
    body_from_cam: &[Pose],
    r_tilt: &Mat3,
) -> (Vec<Ray>, Vec<usize>) {
    let mut rays = Vec::new();
    let mut ids = Vec::new();
    for i in idx {
        if let Some(r) = corrs
            .get(i)
            .and_then(|c| Ray::new(c, body_from_cam, r_tilt))
        {
            rays.push(r);
            ids.push(i);
        }
    }
    (rays, ids)
}

// ------------------------------------------------------------------------------------------------------------------
// Minimal solver
// ------------------------------------------------------------------------------------------------------------------

/// Minimal solver: two correspondences give up to two poses.
///
/// With `u = Ry^T t` the camera point `X_c` is linear in `[cos yaw, sin yaw, u, 1]`; two rows of `[f]x` per
/// correspondence give four equations in the five unknowns `[cos, sin, u]`, i.e. a one-dimensional affine family. The
/// part of it that is free of `u` is `cos * alpha + sin * beta = gamma`; intersecting that line with
/// `cos^2 + sin^2 = 1` (a quadratic) gives the yaw candidates, and the depths and `t` follow in closed form (module
/// docs). Candidates that put a sampled point behind its camera (non-positive depth along the bearing, which for
/// bearings in the front half space is `X_c.z <= 0`) are rejected.
///
/// Returns an empty vector for degenerate samples (identical points, parallel rays, a vertical point pair, a
/// horizontal ray plane, an invalid correspondence or no real solution); never panics. `Corr::cam` indexes
/// `body_from_cam`; `r_tilt` is the known roll/pitch rotation (`R = Ry(yaw) * r_tilt`).
pub fn solve_p2(c: &[Corr; 2], body_from_cam: &[Pose], r_tilt: &Mat3) -> Vec<Pose4> {
    match (
        Ray::new(&c[0], body_from_cam, r_tilt),
        Ray::new(&c[1], body_from_cam, r_tilt),
    ) {
        (Some(a), Some(b)) => {
            let (sols, n) = solve_rays(&a, &b);
            sols[..n].to_vec()
        }
        _ => Vec::new(),
    }
}

/// [`solve_p2`] on prepared rays: up to two solutions and their count.
fn solve_rays(a: &Ray, b: &Ray) -> ([Pose4; 2], usize) {
    let none = ([Pose4::default(); 2], 0);
    let n = cross(a.d, b.d);
    let nn2 = dot(n, n);
    if nn2.is_nan() || nn2 <= MIN_RAY_SIN * MIN_RAY_SIN {
        return none;
    }
    let dx = sub(a.x, b.x);
    if dot(dx, dx).is_nan() || dot(dx, dx) <= MIN_POINT_SEP2 {
        return none;
    }
    let dp = sub(a.p, b.p);
    // Unit normal of the ray plane; n . (Ry^T dx - dp) = 0 is the u-free equation  cos*alpha + sin*beta = gamma.
    let m = scale(n, 1.0 / nn2.sqrt());
    let alpha = m[0] * dx[0] + m[2] * dx[2];
    let beta = m[2] * dx[0] - m[0] * dx[2];
    let gamma = dot(m, dp) - m[1] * dx[1];
    let r2 = alpha * alpha + beta * beta;
    if r2.is_nan() || r2 <= MIN_YAW_LEVER * MIN_YAW_LEVER || gamma.is_nan() {
        return none;
    }
    let ratio2 = gamma * gamma / r2;
    if ratio2 > 1.0 + TANGENT_TOL {
        return none;
    }
    // Line-circle intersection: foot point (c0, s0) of the line, half chord h along the direction (lx, ly).
    let h = (1.0 - ratio2).max(0.0).sqrt();
    let r = r2.sqrt();
    let (c0, s0) = (gamma * alpha / r2, gamma * beta / r2);
    let (lx, ly) = (-beta / r, alpha / r);
    let mut out = [Pose4::default(); 2];
    let mut count = 0;
    let signs: &[f64] = if h < 1e-12 { &[0.0] } else { &[1.0, -1.0] };
    for &sg in signs {
        let (cy, sy) = (c0 + sg * h * lx, s0 + sg * h * ly);
        let nrm = cy.hypot(sy);
        if let Some(pose) = complete(a, b, nn2, cy / nrm, sy / nrm) {
            out[count] = pose;
            count += 1;
        }
    }
    (out, count)
}

/// Depths and translation for one yaw root (`c = cos`, `s = sin`); `None` if a depth is not positive.
fn complete(a: &Ray, b: &Ray, nn2: f64, c: f64, s: f64) -> Option<Pose4> {
    let dx = sub(a.x, b.x);
    let dp = sub(a.p, b.p);
    // Ry^T dx - dp = la * d_a - lb * d_b: least squares for the depths la, lb (nn2 = 1 - (d_a . d_b)^2).
    let w = [
        c * dx[0] - s * dx[2] - dp[0],
        dx[1] - dp[1],
        s * dx[0] + c * dx[2] - dp[2],
    ];
    let k = dot(a.d, b.d);
    let (wa, wb) = (dot(a.d, w), dot(b.d, w));
    let la = (wa - k * wb) / nn2;
    let lb = (k * wa - wb) / nn2;
    if la.is_nan() || lb.is_nan() || la <= MIN_DEPTH || lb <= MIN_DEPTH {
        return None;
    }
    // Ry^T (X - t) = p + lambda d  =>  t = X - Ry (p + lambda d); the two points agree up to round-off.
    let ry = rot_y_cs(c, s);
    let ta = sub(a.x, mat_vec(&ry, add(a.p, scale(a.d, la))));
    let tb = sub(b.x, mat_vec(&ry, add(b.p, scale(b.d, lb))));
    let t = scale(add(ta, tb), 0.5);
    finite3(t).then_some(Pose4 { yaw: s.atan2(c), t })
}

// ------------------------------------------------------------------------------------------------------------------
// Scoring and RANSAC
// ------------------------------------------------------------------------------------------------------------------

/// Hypothesis quality: truncated cost (MSAC, `1 - cos(angle)` capped at the threshold) and inlier count.
#[derive(Clone, Copy, Debug)]
struct Score {
    count: usize,
    cost: f64,
}

impl Score {
    fn better_than(&self, other: &Score) -> bool {
        self.cost < other.cost || (self.cost == other.cost && self.count > other.count)
    }
}

fn score(rays: &[Ray], pose: &Pose4, cos_thr: f64) -> Score {
    let (s, c) = pose.yaw.sin_cos();
    let cap = 1.0 - cos_thr;
    let mut sc = Score {
        count: 0,
        cost: 0.0,
    };
    for r in rays {
        let e = r.cos_angle(c, s, pose.t).map_or(2.0, |ca| 1.0 - ca);
        if e <= cap {
            sc.count += 1;
        }
        sc.cost += e.min(cap);
    }
    sc
}

/// Indices (into `rays`) of the rays within the threshold of the pose.
fn inlier_ids(rays: &[Ray], pose: &Pose4, cos_thr: f64) -> Vec<usize> {
    let (s, c) = pose.yaw.sin_cos();
    rays.iter()
        .enumerate()
        .filter(|(_, r)| r.cos_angle(c, s, pose.t).is_some_and(|ca| ca >= cos_thr))
        .map(|(i, _)| i)
        .collect()
}

/// Local optimisation of a promising hypothesis: refine on its inliers, keep the result if the cost improves.
fn local_opt(rays: &[Ray], start: &Pose4, cos_thr: f64, huber_rad: f64) -> (Pose4, Score) {
    let mut pose = *start;
    let mut best = score(rays, &pose, cos_thr);
    for _ in 0..2 {
        let sel: Vec<Ray> = inlier_ids(rays, &pose, cos_thr)
            .into_iter()
            .map(|i| rays[i])
            .collect();
        if sel.len() < 3 {
            break;
        }
        let cand = refine_rays(&sel, &pose, huber_rad);
        let sc = score(rays, &cand, cos_thr);
        if !sc.better_than(&best) {
            break;
        }
        pose = cand;
        best = sc;
    }
    (pose, best)
}

/// Samples needed so that an all-inlier pair is drawn with probability `confidence`, for inlier ratio `w`.
fn iterations_needed(w: f64, confidence: f64) -> usize {
    let conf = if confidence > 0.0 && confidence < 1.0 {
        confidence
    } else {
        0.999
    };
    let w2 = (w * w).clamp(0.0, 1.0);
    if w2 >= 1.0 - 1e-12 {
        return 1;
    }
    if w2 <= 1e-12 {
        return usize::MAX;
    }
    let k = ((1.0 - conf).ln() / (-w2).ln_1p()).ceil();
    if k.is_finite() && k >= 1.0 {
        k as usize
    } else {
        usize::MAX
    }
}

/// RANSAC with [`solve_p2`] as the hypothesis generator.
///
/// Pairs of correspondences are drawn at random (all cameras together; invalid correspondences are skipped), each
/// hypothesis is scored by the truncated angular error over all correspondences, and promising ones are polished on
/// their inliers. The loop stops after `p.max_iters` samples or earlier once the iteration count implied by the best
/// inlier ratio and `p.confidence` is reached (`log(1 - confidence) / log(1 - ratio^2)`). The winner is then refined on
/// its inliers ([`refine`], Huber scale `p.thresh_deg`), the inliers are collected again, refined once more, and
/// collected a last time for the report.
///
/// Returns `None` if there are fewer than `p.min_inliers` valid correspondences, if no hypothesis is found or the
/// final pose has fewer than `p.min_inliers` inliers, or if `p.thresh_deg` is not positive.
pub fn ransac(
    corrs: &[Corr],
    body_from_cam: &[Pose],
    r_tilt: &Mat3,
    p: &PnpParams,
) -> Option<PnpResult> {
    let min_inliers = p.min_inliers.max(2);
    if !(p.thresh_deg.is_finite() && p.thresh_deg > 0.0) || p.max_iters == 0 {
        return None;
    }
    let (rays, ids) = build_rays(corrs, 0..corrs.len(), body_from_cam, r_tilt);
    let m = rays.len();
    if m < min_inliers {
        return None;
    }
    let thresh_rad = p.thresh_deg.min(89.0).to_radians();
    let cos_thr = thresh_rad.cos();

    let mut rng = SplitMix64::new(p.seed);
    // `best` is the best polished hypothesis; local optimisation is triggered by records of the raw score so that a
    // polished wrong model can never keep a better raw hypothesis from being polished in turn.
    let mut best: Option<(Pose4, Score)> = None;
    let mut best_raw: Option<Score> = None;
    let mut needed = p.max_iters;
    let mut iterations = 0;
    while iterations < needed.min(p.max_iters) {
        iterations += 1;
        let i = rng.below(m);
        let mut j = rng.below(m - 1);
        if j >= i {
            j += 1;
        }
        let (sols, ns) = solve_rays(&rays[i], &rays[j]);
        for sol in &sols[..ns] {
            let raw = score(&rays, sol, cos_thr);
            if best_raw.is_some_and(|b| !raw.better_than(&b)) {
                continue;
            }
            best_raw = Some(raw);
            let (pose, polished) = local_opt(&rays, sol, cos_thr, thresh_rad);
            if best.is_some_and(|(_, b)| !polished.better_than(&b)) {
                continue;
            }
            needed = iterations_needed(polished.count as f64 / m as f64, p.confidence);
            best = Some((pose, polished));
        }
    }
    let (mut pose, _) = best?;

    // Refine on the inliers, re-collect, refine again; accept a step only if the truncated cost does not rise.
    let mut cost = score(&rays, &pose, cos_thr).cost;
    for _ in 0..2 {
        let sel: Vec<Ray> = inlier_ids(&rays, &pose, cos_thr)
            .into_iter()
            .map(|i| rays[i])
            .collect();
        if sel.len() < 2 {
            break;
        }
        let cand = refine_rays(&sel, &pose, thresh_rad);
        let c2 = score(&rays, &cand, cos_thr).cost;
        if c2 <= cost {
            pose = cand;
            cost = c2;
        }
    }
    let inl = inlier_ids(&rays, &pose, cos_thr);
    if inl.len() < min_inliers {
        return None;
    }
    let (s, c) = pose.yaw.sin_cos();
    let sum2: f64 = inl
        .iter()
        .map(|&i| rays[i].angle(c, s, pose.t).powi(2))
        .sum();
    Some(PnpResult {
        pose,
        rms_deg: (sum2 / inl.len() as f64).sqrt().to_degrees(),
        inliers: inl.into_iter().map(|i| ids[i]).collect(),
        iterations,
    })
}

// ------------------------------------------------------------------------------------------------------------------
// Refinement
// ------------------------------------------------------------------------------------------------------------------

/// Angular residual (degrees) of one correspondence for a pose: the angle between the bearing and the direction in
/// which the camera sees the map point. `inf` for invalid correspondences (camera index outside the rig, bad
/// numbers) and for a point at the camera centre.
pub fn residual_deg(c: &Corr, body_from_cam: &[Pose], r_tilt: &Mat3, pose: &Pose4) -> f64 {
    match Ray::new(c, body_from_cam, r_tilt) {
        Some(r) => {
            let (s, cy) = pose.yaw.sin_cos();
            r.angle(cy, s, pose.t).to_degrees()
        }
        None => f64::INFINITY,
    }
}

/// Gauss-Newton / Levenberg-Marquardt on `(yaw, tx, ty, tz)` over the correspondences `idx`.
///
/// Minimises the Huber cost of the angular residuals (quadratic below `huber_deg`, linear above; a non-positive or
/// non-finite `huber_deg` means plain least squares) with Marquardt-scaled damping and step rejection, so the cost
/// never increases. The residual vector is `f x normalize(X_c)` (norm = sine of the angle, two independent
/// components, analytic Jacobian); points behind the camera carry no gradient. Entries of `idx` that are out of range
/// and invalid correspondences are ignored.
///
/// Returns the refined pose (yaw wrapped to `(-pi, pi]`) and the RMS of the plain angular residuals in degrees over
/// the valid entries (`NaN` if there are none, in which case the pose is `init`).
pub fn refine(
    corrs: &[Corr],
    idx: &[usize],
    body_from_cam: &[Pose],
    r_tilt: &Mat3,
    init: &Pose4,
    huber_deg: f64,
) -> (Pose4, f64) {
    let (rays, _) = build_rays(corrs, idx.iter().copied(), body_from_cam, r_tilt);
    let huber_rad = if huber_deg.is_finite() && huber_deg > 0.0 {
        huber_deg.to_radians()
    } else {
        0.0
    };
    let pose = refine_rays(&rays, init, huber_rad);
    let (s, c) = pose.yaw.sin_cos();
    let (sum2, n) = rays
        .iter()
        .map(|r| r.angle(c, s, pose.t))
        .filter(|a| a.is_finite())
        .fold((0.0, 0usize), |(s2, n), a| (s2 + a * a, n + 1));
    let rms = if n == 0 {
        f64::NAN
    } else {
        (sum2 / n as f64).sqrt().to_degrees()
    };
    (pose, rms)
}

/// Residual `r = d x normalize(v)` (norm = sine of the angle) and its Jacobian with respect to
/// `(yaw, tx, ty, tz)`.
struct Lin {
    r: Vec3,
    j: [[f64; 4]; 3],
    cos: f64,
}

fn linearize(ray: &Ray, c: f64, s: f64, t: Vec3) -> Option<Lin> {
    let y = sub(ray.x, t);
    let v = ray.offset(c, s, t);
    let nv = norm(v);
    if nv.is_nan() || nv <= 1e-9 {
        return None;
    }
    let vh = scale(v, 1.0 / nv);
    // dv/dyaw, dv/dt (= minus the columns of Ry^T)
    let dv = [
        [-s * y[0] - c * y[2], 0.0, c * y[0] - s * y[2]],
        [-c, 0.0, -s],
        [0.0, -1.0, 0.0],
        [s, 0.0, -c],
    ];
    let mut j = [[0.0; 4]; 3];
    for (k, dvk) in dv.iter().enumerate() {
        // d(vh) = (I - vh vh^T) dv / |v|
        let dvh = scale(sub(*dvk, scale(vh, dot(vh, *dvk))), 1.0 / nv);
        let col = cross(ray.d, dvh);
        for (row, jr) in j.iter_mut().enumerate() {
            jr[k] = col[row];
        }
    }
    Some(Lin {
        r: cross(ray.d, vh),
        j,
        cos: dot(ray.d, vh),
    })
}

fn huber(rn: f64, delta: f64) -> f64 {
    if rn <= delta {
        0.5 * rn * rn
    } else {
        delta * (rn - 0.5 * delta)
    }
}

/// Robust cost of a pose; points behind the camera (or on it) count with the saturated residual 1.
fn robust_cost(rays: &[Ray], pose: &Pose4, delta: f64) -> f64 {
    let (s, c) = pose.yaw.sin_cos();
    rays.iter()
        .map(|ray| match linearize_cost(ray, c, s, pose.t) {
            Some(rn) => huber(rn, delta),
            None => huber(1.0, delta),
        })
        .sum()
}

/// Sine of the angle for points in front of the camera, `None` otherwise.
fn linearize_cost(ray: &Ray, c: f64, s: f64, t: Vec3) -> Option<f64> {
    let v = ray.offset(c, s, t);
    let nv = norm(v);
    if nv.is_nan() || nv <= 1e-9 || dot(ray.d, v) <= 0.0 {
        return None;
    }
    Some((norm(cross(ray.d, v)) / nv).min(1.0))
}

fn normal_equations(rays: &[Ray], pose: &Pose4, delta: f64) -> ([[f64; 4]; 4], [f64; 4]) {
    let (s, c) = pose.yaw.sin_cos();
    let mut h = [[0.0; 4]; 4];
    let mut g = [0.0; 4];
    for ray in rays {
        let Some(l) = linearize(ray, c, s, pose.t) else {
            continue;
        };
        if l.cos <= 0.0 {
            continue;
        }
        let rn = norm(l.r);
        let w = if rn <= delta { 1.0 } else { delta / rn };
        for (jr, &rr) in l.j.iter().zip(l.r.iter()) {
            for (a, &ja) in jr.iter().enumerate() {
                g[a] += w * ja * rr;
                for (b, &jb) in jr.iter().enumerate() {
                    h[a][b] += w * ja * jb;
                }
            }
        }
    }
    (h, g)
}

/// Solve a 4x4 system (partial pivoting); `None` if singular or the result is not finite.
fn solve4(mut a: [[f64; 4]; 4], mut b: [f64; 4]) -> Option<[f64; 4]> {
    for col in 0..4 {
        let mut piv = col;
        for row in col + 1..4 {
            if a[row][col].abs() > a[piv][col].abs() {
                piv = row;
            }
        }
        let p = a[piv][col];
        if p.is_nan() || p.abs() <= 1e-300 {
            return None;
        }
        a.swap(col, piv);
        b.swap(col, piv);
        for row in col + 1..4 {
            let f = a[row][col] / a[col][col];
            let pivot_row = a[col];
            for (x, pv) in a[row].iter_mut().zip(pivot_row.iter()).skip(col) {
                *x -= f * pv;
            }
            b[row] -= f * b[col];
        }
    }
    let mut x = [0.0; 4];
    for row in (0..4).rev() {
        let tail: f64 = a[row][row + 1..]
            .iter()
            .zip(x[row + 1..].iter())
            .map(|(av, xv)| av * xv)
            .sum();
        x[row] = (b[row] - tail) / a[row][row];
    }
    x.iter().all(|v| v.is_finite()).then_some(x)
}

/// Levenberg-Marquardt on prepared rays (`huber_rad <= 0`: plain least squares).
fn refine_rays(rays: &[Ray], init: &Pose4, huber_rad: f64) -> Pose4 {
    let mut pose = *init;
    if rays.is_empty() || !pose.yaw.is_finite() || !finite3(pose.t) {
        return pose;
    }
    // The residual magnitude is the sine of the angle, so the Huber scale is the sine of the threshold.
    let delta = if huber_rad > 0.0 {
        huber_rad.min(FRAC_PI_2).sin()
    } else {
        f64::INFINITY
    };
    let mut cost = robust_cost(rays, &pose, delta);
    let mut mu = 1e-3;
    for _ in 0..LM_MAX_ITERS {
        let (h, g) = normal_equations(rays, &pose, delta);
        let mut accepted = None;
        for _ in 0..LM_MAX_RETRIES {
            let mut a = h;
            for (i, row) in a.iter_mut().enumerate() {
                row[i] += mu * h[i][i].max(1e-9);
            }
            let Some(step) = solve4(a, [-g[0], -g[1], -g[2], -g[3]]) else {
                mu *= 10.0;
                continue;
            };
            let cand = Pose4 {
                yaw: pose.yaw + step[0],
                t: [
                    pose.t[0] + step[1],
                    pose.t[1] + step[2],
                    pose.t[2] + step[3],
                ],
            };
            let c2 = robust_cost(rays, &cand, delta);
            if c2 <= cost {
                accepted = Some((cand, c2, step));
                mu = (mu * 0.3).max(1e-12);
                break;
            }
            mu *= 4.0;
        }
        let Some((cand, c2, step)) = accepted else {
            break;
        };
        let decrease = cost - c2;
        pose = cand;
        cost = c2;
        let step_size = step[0].abs().max(norm([step[1], step[2], step[3]]));
        if step_size < 1e-11 || decrease <= 1e-15 * cost {
            break;
        }
    }
    pose.yaw = wrap_pi(pose.yaw);
    pose
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_cam::calib::DeviceCalib;
    use pimax_cam::geom::{normalize, orthonormality_error, rodrigues, transpose};
    use pimax_cam::slam::{mat_of, qmul, quat_of, qyaw, t_imu_cam};
    use std::path::Path;

    /// Room box (x, y, z extent in metres): x in [-3, 3], y in [0, 3], z in [-3, 3].
    const ROOM: [f64; 3] = [6.0, 3.0, 6.0];
    /// Pixel noise of the synthetic observations.
    const NOISE_PX: f64 = 0.5;

    /// The factory calibration: intrinsics from `cal`, `body_from_cam` for all four cameras.
    struct Rig {
        cal: DeviceCalib,
        body_from_cam: Vec<Pose>,
    }

    /// Nearest rotation by Gram-Schmidt on the columns.
    fn orthonormalize(m: &Mat3) -> Mat3 {
        let col = |j: usize| [m[0][j], m[1][j], m[2][j]];
        let c0 = normalize(col(0));
        let c1 = normalize(sub(col(1), scale(c0, dot(c0, col(1)))));
        let c2 = cross(c0, c1);
        [
            [c0[0], c1[0], c2[0]],
            [c0[1], c1[1], c2[1]],
            [c0[2], c1[2], c2[2]],
        ]
    }

    fn rig() -> Rig {
        let cands = [
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../analysis/headset-calibration.xml"
            ),
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../pimax-cam/tests/fixtures/headset-calibration.xml"
            ),
        ];
        let cal = cands
            .iter()
            .find(|p| Path::new(p).is_file())
            .map(|p| DeviceCalib::load(Path::new(p)).expect("factory calibration parses"))
            .or_else(|| DeviceCalib::load_default(None).ok().map(|(c, _)| c))
            .expect("headset-calibration.xml not found");
        assert_eq!(cal.cameras.len(), 4);
        // The XML rotations are orthonormal to ~1e-10 only; the synthetic data must be exactly consistent with the
        // model, which treats them as rotations (the tests invert them by transposition).
        let body_from_cam = (0..4)
            .map(|i| t_imu_cam(&cal, i).expect("camera extrinsic"))
            .map(|p| Pose {
                r: orthonormalize(&p.r),
                t: p.t,
            })
            .collect();
        Rig { cal, body_from_cam }
    }

    /// Ground truth: the 4 DoF pose and the known roll/pitch rotation.
    #[derive(Clone, Copy)]
    struct Truth {
        pose: Pose4,
        r_tilt: Mat3,
    }

    impl Truth {
        fn map_from_body(&self) -> Pose {
            self.pose.to_pose(&self.r_tilt)
        }
    }

    fn uni(rng: &mut SplitMix64, lo: f64, hi: f64) -> f64 {
        lo + (hi - lo) * rng.unit()
    }

    fn gauss(rng: &mut SplitMix64) -> f64 {
        let u1 = 1.0 - rng.unit();
        let u2 = rng.unit();
        (-2.0 * u1.ln()).sqrt() * (2.0 * PI * u2).cos()
    }

    /// Random heading, tilt up to 30 degrees about a random horizontal axis, position inside the room.
    fn random_truth(rng: &mut SplitMix64) -> Truth {
        let yaw = uni(rng, -PI, PI);
        let tilt = uni(rng, 0.0, 30f64.to_radians());
        let az = uni(rng, 0.0, 2.0 * PI);
        let r_tilt = rodrigues(scale([az.cos(), 0.0, az.sin()], tilt));
        let t = [uni(rng, -2.5, 2.5), uni(rng, 1.0, 2.0), uni(rng, -2.5, 2.5)];
        Truth {
            pose: Pose4 { yaw, t },
            r_tilt,
        }
    }

    fn room_point(rng: &mut SplitMix64) -> Vec3 {
        [
            uni(rng, -ROOM[0] / 2.0, ROOM[0] / 2.0),
            uni(rng, 0.0, ROOM[1]),
            uni(rng, -ROOM[2] / 2.0, ROOM[2] / 2.0),
        ]
    }

    /// Camera-frame coordinates of a map point under the truth.
    fn camera_point(rig: &Rig, truth: &Truth, cam: usize, x: Vec3) -> Vec3 {
        rig.body_from_cam[cam]
            .inverse()
            .apply(truth.map_from_body().inverse().apply(x))
    }

    /// Exact correspondence of a map point for a camera, without any visibility requirement.
    fn exact(rig: &Rig, truth: &Truth, cam: usize, x: Vec3) -> Corr {
        Corr {
            cam,
            bearing: normalize(camera_point(rig, truth, cam, x)),
            point: x,
        }
    }

    /// Observation of a map point by a camera as the real pipeline would see it: projected with the KB4 model, pixel
    /// noise added, unprojected. `None` unless the point is 0.3-6 m in front and lands inside the 640x480 image.
    fn observe(
        rng: &mut SplitMix64,
        rig: &Rig,
        truth: &Truth,
        cam: usize,
        x: Vec3,
        noise_px: f64,
    ) -> Option<Corr> {
        let xc = camera_point(rig, truth, cam, x);
        if !(0.3..=6.0).contains(&xc[2]) {
            return None;
        }
        let k = &rig.cal.cameras[cam].intr;
        let (u, v) = k.project(xc)?;
        if u < 0.0 || v < 0.0 || u >= f64::from(k.width) || v >= f64::from(k.height) {
            return None;
        }
        let bearing = k.unproject(u + noise_px * gauss(rng), v + noise_px * gauss(rng))?;
        // The noise-free bearing must be the exact direction of the point (guards the generator itself).
        if noise_px == 0.0 && norm(cross(bearing, normalize(xc))) > 1e-10 {
            return None;
        }
        Some(Corr {
            cam,
            bearing,
            point: x,
        })
    }

    /// A random visible correspondence from a random camera of `cams`.
    fn visible(
        rng: &mut SplitMix64,
        rig: &Rig,
        truth: &Truth,
        cams: &[usize],
        noise_px: f64,
    ) -> Corr {
        for _ in 0..1_000_000 {
            let cam = cams[rng.below(cams.len())];
            let x = room_point(rng);
            if let Some(c) = observe(rng, rig, truth, cam, x, noise_px) {
                return c;
            }
        }
        panic!("no visible point found");
    }

    /// Random map point with a random (plausible) bearing; redrawn if it happens to agree with the truth.
    fn outlier(rng: &mut SplitMix64, rig: &Rig, truth: &Truth, cams: &[usize]) -> Corr {
        loop {
            let cam = cams[rng.below(cams.len())];
            let k = &rig.cal.cameras[cam].intr;
            let (u, v) = (
                uni(rng, 0.0, f64::from(k.width)),
                uni(rng, 0.0, f64::from(k.height)),
            );
            let Some(bearing) = k.unproject(u, v) else {
                continue;
            };
            let c = Corr {
                cam,
                bearing,
                point: room_point(rng),
            };
            if residual_deg(&c, &rig.body_from_cam, &truth.r_tilt, &truth.pose) > 3.0 {
                return c;
            }
        }
    }

    /// A correspondence more than ~90.3 degrees off the optical axis (`z < 0`): only the image corners get there.
    fn peripheral(rng: &mut SplitMix64, rig: &Rig, truth: &Truth) -> Corr {
        for _ in 0..1_000_000 {
            let cam = rng.below(4);
            let k = &rig.cal.cameras[cam].intr;
            let (u, v) = (
                uni(rng, 0.0, f64::from(k.width)),
                uni(rng, 0.0, f64::from(k.height)),
            );
            let Some(f) = k.unproject(u, v) else {
                continue;
            };
            if f[2] > -0.005 {
                continue;
            }
            let xc = scale(f, uni(rng, 0.5, 4.0));
            let x = truth
                .map_from_body()
                .apply(rig.body_from_cam[cam].apply(xc));
            return Corr {
                cam,
                bearing: f,
                point: x,
            };
        }
        panic!("no peripheral pixel found");
    }

    fn yaw_err_deg(a: f64, b: f64) -> f64 {
        wrap_pi(a - b).abs().to_degrees()
    }

    fn pos_err(a: Vec3, b: Vec3) -> f64 {
        norm(sub(a, b))
    }

    fn is_truth(s: &Pose4, truth: &Truth, yaw_tol_deg: f64, t_tol: f64) -> bool {
        yaw_err_deg(s.yaw, truth.pose.yaw) < yaw_tol_deg && pos_err(s.t, truth.pose.t) < t_tol
    }

    fn with_cam(c: Corr, cam: usize) -> Corr {
        Corr { cam, ..c }
    }

    fn with_bearing(c: Corr, bearing: Vec3) -> Corr {
        Corr { bearing, ..c }
    }

    fn with_point(c: Corr, point: Vec3) -> Corr {
        Corr { point, ..c }
    }

    /// Default parameters with one modification.
    fn params(f: impl FnOnce(&mut PnpParams)) -> PnpParams {
        let mut p = PnpParams::default();
        f(&mut p);
        p
    }

    fn quantile(v: &[f64], q: f64) -> f64 {
        let mut s = v.to_vec();
        s.sort_by(|a, b| a.total_cmp(b));
        s[((s.len() - 1) as f64 * q).round() as usize]
    }

    fn summary(v: &[f64]) -> String {
        format!(
            "median {:.3e}  p95 {:.3e}  max {:.3e}",
            quantile(v, 0.5),
            quantile(v, 0.95),
            quantile(v, 1.0)
        )
    }

    /// Both bearings are reproduced and both points are in front of their cameras.
    fn assert_valid_solution(rig: &Rig, r_tilt: &Mat3, s: &Pose4, pair: [&Corr; 2], what: &str) {
        let pose = s.to_pose(r_tilt);
        for c in pair {
            let r = residual_deg(c, &rig.body_from_cam, r_tilt, s);
            assert!(r < 1e-6, "{what}: residual {r} deg");
            let xc = rig.body_from_cam[c.cam]
                .inverse()
                .apply(pose.inverse().apply(c.point));
            assert!(dot(xc, c.bearing) > 0.0, "{what}: point behind its camera");
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // basics
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn splitmix_reference_values() {
        // First outputs of splitmix64 seeded with 0 (reference implementation).
        let mut r = SplitMix64::new(0);
        assert_eq!(r.next_u64(), 0xE220_A839_7B1D_CDAF);
        assert_eq!(r.next_u64(), 0x6E78_9E6A_A1B9_65F4);
        let mut r = SplitMix64::new(7);
        for n in [1usize, 2, 3, 250] {
            for _ in 0..1000 {
                assert!(r.below(n) < n);
            }
        }
        assert!((0.0..1.0).contains(&r.unit()));
    }

    #[test]
    fn default_params_are_the_documented_ones() {
        let p = PnpParams::default();
        assert_eq!(
            (p.thresh_deg, p.max_iters, p.confidence, p.min_inliers),
            (1.0, 300, 0.999, 15)
        );
    }

    #[test]
    fn to_pose_matches_quaternion_helpers() {
        let mut rng = SplitMix64::new(11);
        for _ in 0..20 {
            let truth = random_truth(&mut rng);
            let p = truth.map_from_body();
            let m = mat_of(qmul(qyaw(truth.pose.yaw), quat_of(&truth.r_tilt)));
            for (pr, mr) in p.r.iter().zip(&m) {
                for (a, b) in pr.iter().zip(mr) {
                    assert!((a - b).abs() < 1e-12);
                }
            }
            assert!(orthonormality_error(&p.r) < 1e-12);
            assert_eq!(p.t, truth.pose.t);
            // the body's up axis stays within the 30 degrees of tilt
            assert!(mat_vec(&p.r, [0.0, 1.0, 0.0])[1] >= 30f64.to_radians().cos() - 1e-9);
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // minimal solver
    // ---------------------------------------------------------------------------------------------------------

    /// Geometry of a sampled pair, independent of the solver, used to skip hopeless draws in the exactness test:
    /// rays within 0.6 degrees, points closer than 5 cm, a nearly horizontal ray plane or nearly vertical point pair
    /// (no yaw leverage), and the tangent case where the two yaw roots merge.
    fn pair_is_degenerate(rig: &Rig, truth: &Truth, a: &Corr, b: &Corr) -> bool {
        let lvl = |c: &Corr| {
            let r = Ray::new(c, &rig.body_from_cam, &truth.r_tilt).unwrap();
            (r.d, r.p)
        };
        let ((da, pa), (db, pb)) = (lvl(a), lvl(b));
        let n = cross(da, db);
        let dx = sub(a.point, b.point);
        let m = normalize(n);
        let horiz = (m[0] * m[0] + m[2] * m[2]).sqrt() * (dx[0] * dx[0] + dx[2] * dx[2]).sqrt();
        let gamma = dot(m, sub(pa, pb)) - m[1] * dx[1];
        let ratio = if horiz > 0.0 {
            (gamma / horiz).abs()
        } else {
            f64::INFINITY
        };
        norm(n) < 1e-2 || norm(dx) < 5e-2 || horiz < 2e-2 || ratio > 1.0 - 1e-5
    }

    fn check_p2_exact(cams: &[usize], seed: u64, trials: usize, label: &str) {
        let rig = rig();
        let mut rng = SplitMix64::new(seed);
        let (mut yaw_errs, mut t_errs) = (Vec::new(), Vec::new());
        let (mut skipped, mut two_sols, mut mixed) = (0, 0, 0);
        while yaw_errs.len() < trials {
            let truth = random_truth(&mut rng);
            let a = visible(&mut rng, &rig, &truth, cams, 0.0);
            let b = visible(&mut rng, &rig, &truth, cams, 0.0);
            if pair_is_degenerate(&rig, &truth, &a, &b) {
                skipped += 1;
                continue;
            }
            mixed += usize::from(a.cam != b.cam);
            let sols = solve_p2(&[a, b], &rig.body_from_cam, &truth.r_tilt);
            two_sols += usize::from(sols.len() == 2);
            assert!(
                !sols.is_empty() && sols.len() <= 2,
                "{label}: {} solutions",
                sols.len()
            );
            for s in &sols {
                assert_valid_solution(&rig, &truth.r_tilt, s, [&a, &b], label);
            }
            let (ye, te) = sols
                .iter()
                .map(|s| {
                    (
                        yaw_err_deg(s.yaw, truth.pose.yaw).to_radians(),
                        pos_err(s.t, truth.pose.t),
                    )
                })
                .min_by(|x, y| (x.0 + x.1).total_cmp(&(y.0 + y.1)))
                .unwrap();
            assert!(
                ye < 1e-6 && te < 1e-6,
                "{label}: no solution matches the truth (yaw err {ye:.2e} rad, t err {te:.2e} m)"
            );
            yaw_errs.push(ye);
            t_errs.push(te);
        }
        println!(
            "solve_p2 {label}: {trials} trials ({skipped} degenerate draws skipped, {mixed} cross-camera pairs, {two_sols} with two solutions)\n  yaw error [rad] {}\n  t error [m]     {}",
            summary(&yaw_errs),
            summary(&t_errs)
        );
    }

    #[test]
    fn p2_noise_free_recovers_the_pose_stereo_pair() {
        check_p2_exact(&[0, 1], 1, 200, "cams 0,1");
    }

    #[test]
    fn p2_noise_free_recovers_the_pose_four_cameras() {
        check_p2_exact(&[0, 1, 2, 3], 2, 200, "cams 0..3");
    }

    #[test]
    fn p2_same_camera_pairs_work() {
        let rig = rig();
        let mut rng = SplitMix64::new(3);
        let mut done = 0;
        while done < 100 {
            let truth = random_truth(&mut rng);
            let cam = rng.below(4);
            let a = visible(&mut rng, &rig, &truth, &[cam], 0.0);
            let b = visible(&mut rng, &rig, &truth, &[cam], 0.0);
            if pair_is_degenerate(&rig, &truth, &a, &b) {
                continue;
            }
            let sols = solve_p2(&[a, b], &rig.body_from_cam, &truth.r_tilt);
            assert!(sols.iter().any(|s| is_truth(s, &truth, 1e-4, 1e-6)));
            done += 1;
        }
    }

    #[test]
    fn p2_accepts_rays_beyond_90_degrees() {
        // The cheirality test is the depth along the bearing, so fisheye rays with z < 0 (image corners, up to
        // ~93 degrees off the axis) are not mistaken for points behind the camera.
        let rig = rig();
        let mut rng = SplitMix64::new(9);
        let mut done = 0;
        while done < 60 {
            let truth = random_truth(&mut rng);
            let a = peripheral(&mut rng, &rig, &truth);
            let b = visible(&mut rng, &rig, &truth, &[0, 1, 2, 3], 0.0);
            if pair_is_degenerate(&rig, &truth, &a, &b) {
                continue;
            }
            assert!(a.bearing[2] < 0.0);
            let sols = solve_p2(&[a, b], &rig.body_from_cam, &truth.r_tilt);
            assert!(
                sols.iter().any(|s| is_truth(s, &truth, 1e-4, 1e-6)),
                "peripheral ray lost"
            );
            done += 1;
        }
    }

    #[test]
    fn p2_degenerate_inputs_give_no_solution_and_do_not_panic() {
        let rig = rig();
        let mut rng = SplitMix64::new(5);
        let truth = random_truth(&mut rng);
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        let a = visible(&mut rng, &rig, &truth, &[0, 1], 0.0);
        let b = visible(&mut rng, &rig, &truth, &[0, 1], 0.0);
        assert!(!solve_p2(&[a, b], bc, rt).is_empty(), "reference pair");
        // identical correspondences, identical points seen by the same camera and by the other one
        assert!(solve_p2(&[a, a], bc, rt).is_empty());
        assert!(solve_p2(&[a, exact(&rig, &truth, a.cam, a.point)], bc, rt).is_empty());
        assert!(solve_p2(&[a, exact(&rig, &truth, 1 - a.cam, a.point)], bc, rt).is_empty());
        // parallel rays: same camera and bearing, different map points
        let par = with_point(a, add(a.point, [0.3, 0.2, -0.4]));
        assert!(solve_p2(&[a, par], bc, rt).is_empty());
        // two points on a vertical line: the heading cannot be observed
        let low = exact(&rig, &truth, 0, a.point);
        let high = exact(&rig, &truth, 0, add(a.point, [0.0, 0.8, 0.0]));
        assert!(solve_p2(&[low, high], bc, rt).is_empty());
        // two level rays (horizontal ray plane): the heading cannot be observed either
        let level = |az: f64, x: Vec3| {
            let down_the_ray = mat_vec(&transpose(rt), [az.cos(), 0.0, az.sin()]);
            Corr {
                cam: 0,
                bearing: mat_vec(&transpose(&bc[0].r), down_the_ray),
                point: x,
            }
        };
        assert!(solve_p2(&[level(0.3, a.point), level(1.1, b.point)], bc, rt).is_empty());
        // unusable correspondences (in either slot) and rigs
        let unusable = [
            with_cam(a, 9),
            with_bearing(a, [0.0; 3]),
            with_bearing(a, [f64::NAN, 0.0, 1.0]),
            with_point(a, [f64::INFINITY, 0.0, 0.0]),
        ];
        for bad in unusable {
            assert!(solve_p2(&[bad, b], bc, rt).is_empty());
            assert!(solve_p2(&[b, bad], bc, rt).is_empty());
        }
        assert!(solve_p2(&[a, b], &[], rt).is_empty());
        // a non-normalised bearing gives the same answer as the unit one
        let scaled = with_bearing(a, scale(a.bearing, 3.7));
        let (s1, s2) = (solve_p2(&[a, b], bc, rt), solve_p2(&[scaled, b], bc, rt));
        assert_eq!(s1.len(), s2.len());
        for (x, y) in s1.iter().zip(&s2) {
            assert!(yaw_err_deg(x.yaw, y.yaw) < 1e-9 && pos_err(x.t, y.t) < 1e-9);
        }
    }

    #[test]
    fn p2_random_pairs_obey_the_invariants() {
        // Arbitrary (inconsistent) pairs: whenever solutions are returned they reproduce both bearings exactly with
        // positive depths; many pairs have no real root at all.
        let rig = rig();
        let mut rng = SplitMix64::new(8);
        let mut hist = [0usize; 3];
        for _ in 0..3000 {
            let truth = random_truth(&mut rng);
            let a = outlier(&mut rng, &rig, &truth, &[0, 1, 2, 3]);
            let b = outlier(&mut rng, &rig, &truth, &[0, 1, 2, 3]);
            let sols = solve_p2(&[a, b], &rig.body_from_cam, &truth.r_tilt);
            assert!(sols.len() <= 2);
            hist[sols.len()] += 1;
            for s in &sols {
                assert!(s.yaw.is_finite() && s.yaw.abs() <= PI + 1e-12 && finite3(s.t));
                assert_valid_solution(&rig, &truth.r_tilt, s, [&a, &b], "random pair");
            }
        }
        println!(
            "solve_p2 random pairs: {} with 0, {} with 1, {} with 2 solutions",
            hist[0], hist[1], hist[2]
        );
        assert!(hist[0] > 100 && hist[1] + hist[2] > 100, "{hist:?}");
    }

    #[test]
    fn p2_rejects_points_behind_the_camera() {
        let rig = rig();
        let mut rng = SplitMix64::new(6);
        let mut checked = 0;
        while checked < 100 {
            let truth = random_truth(&mut rng);
            let a = visible(&mut rng, &rig, &truth, &[0, 1], 0.0);
            let b = visible(&mut rng, &rig, &truth, &[0, 1], 0.0);
            if pair_is_degenerate(&rig, &truth, &a, &b) {
                continue;
            }
            // Flip one bearing: under the true pose that point is now behind its camera and must not be returned.
            let bf = with_bearing(b, scale(b.bearing, -1.0));
            let sols = solve_p2(&[a, bf], &rig.body_from_cam, &truth.r_tilt);
            for s in &sols {
                assert_valid_solution(&rig, &truth.r_tilt, s, [&a, &bf], "flipped");
                assert!(
                    !is_truth(s, &truth, 1e-4, 1e-6),
                    "behind-the-camera pose returned"
                );
            }
            checked += 1;
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // RANSAC
    // ---------------------------------------------------------------------------------------------------------

    /// `n_in` noisy inliers and `n_out` outliers in random order.
    fn scene(
        rng: &mut SplitMix64,
        rig: &Rig,
        truth: &Truth,
        cams: &[usize],
        n_in: usize,
        n_out: usize,
        noise_px: f64,
    ) -> (Vec<Corr>, Vec<bool>) {
        let mut v: Vec<(Corr, bool)> = Vec::with_capacity(n_in + n_out);
        for _ in 0..n_in {
            v.push((visible(rng, rig, truth, cams, noise_px), true));
        }
        for _ in 0..n_out {
            v.push((outlier(rng, rig, truth, cams), false));
        }
        for i in (1..v.len()).rev() {
            v.swap(i, rng.below(i + 1));
        }
        v.into_iter().unzip()
    }

    /// Landmarks seen by cameras 0 and 1 at once (adjacent correspondences share the map point), as in a real
    /// relocalisation: `n_in` good landmarks, `n_out` landmarks whose two observations are both wrong.
    fn landmark_scene(
        rng: &mut SplitMix64,
        rig: &Rig,
        truth: &Truth,
        n_in: usize,
        n_out: usize,
    ) -> (Vec<Corr>, Vec<bool>) {
        let (mut corrs, mut is_in) = (Vec::new(), Vec::new());
        while is_in.len() < 2 * n_in {
            let x = room_point(rng);
            if let (Some(a), Some(b)) = (
                observe(rng, rig, truth, 0, x, NOISE_PX),
                observe(rng, rig, truth, 1, x, NOISE_PX),
            ) {
                corrs.extend([a, b]);
                is_in.extend([true, true]);
            }
        }
        for _ in 0..n_out {
            let a = outlier(rng, rig, truth, &[0]);
            let b = outlier(rng, rig, truth, &[1]);
            corrs.extend([a, with_point(b, a.point)]);
            is_in.extend([false, false]);
        }
        (corrs, is_in)
    }

    struct Trial {
        yaw_err: f64,
        t_err: f64,
        recall: f64,
        precision: f64,
        rms: f64,
        iterations: usize,
        ok: bool,
    }

    /// Run `ransac` and score it against the truth; also checks that the report is self-consistent.
    fn evaluate(rig: &Rig, truth: &Truth, corrs: &[Corr], is_in: &[bool], p: &PnpParams) -> Trial {
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        let n_in = is_in.iter().filter(|&&b| b).count();
        let Some(r) = ransac(corrs, bc, rt, p) else {
            return Trial {
                yaw_err: f64::INFINITY,
                t_err: f64::INFINITY,
                recall: 0.0,
                precision: 0.0,
                rms: f64::NAN,
                iterations: p.max_iters,
                ok: false,
            };
        };
        // inliers: ascending, exactly the correspondences within the threshold of the returned pose, and the
        // reported RMS is theirs
        assert!(r.inliers.windows(2).all(|w| w[0] < w[1]));
        let mut sum2 = 0.0;
        for (i, c) in corrs.iter().enumerate() {
            let res = residual_deg(c, bc, rt, &r.pose);
            if r.inliers.binary_search(&i).is_ok() {
                assert!(res <= p.thresh_deg + 1e-9, "listed inlier with {res} deg");
                sum2 += res * res;
            } else {
                assert!(res > p.thresh_deg - 1e-9, "unlisted inlier with {res} deg");
            }
        }
        assert!((r.rms_deg - (sum2 / r.inliers.len() as f64).sqrt()).abs() < 1e-9);
        assert!((1..=p.max_iters).contains(&r.iterations));
        let hits = r.inliers.iter().filter(|&&i| is_in[i]).count();
        let yaw_err = yaw_err_deg(r.pose.yaw, truth.pose.yaw);
        let t_err = pos_err(r.pose.t, truth.pose.t);
        let recall = hits as f64 / n_in as f64;
        Trial {
            yaw_err,
            t_err,
            recall,
            precision: hits as f64 / r.inliers.len() as f64,
            rms: r.rms_deg,
            iterations: r.iterations,
            ok: yaw_err < 0.3 && t_err < 0.02 && recall >= 0.9,
        }
    }

    fn report(label: &str, ts: &[Trial]) -> usize {
        let ok = ts.iter().filter(|t| t.ok).count();
        let col = |f: fn(&Trial) -> f64| ts.iter().map(f).collect::<Vec<_>>();
        println!(
            "ransac {label}: {ok}/{} ok\n  yaw error [deg] {}\n  t error [m]     {}\n  recall          min {:.3} mean {:.3}   precision min {:.3}\n  rms [deg]       {}   iterations {}",
            ts.len(),
            summary(&col(|t| t.yaw_err)),
            summary(&col(|t| t.t_err)),
            ts.iter().map(|t| t.recall).fold(1.0, f64::min),
            ts.iter().map(|t| t.recall).sum::<f64>() / ts.len() as f64,
            ts.iter().map(|t| t.precision).fold(1.0, f64::min),
            summary(&col(|t| t.rms)),
            summary(&col(|t| t.iterations as f64)),
        );
        ok
    }

    fn run_ransac_trials(
        cams: &[usize],
        n_in: usize,
        n_out: usize,
        first_seed: u64,
        trials: usize,
        label: &str,
    ) -> usize {
        let rig = rig();
        let ts: Vec<Trial> = (0..trials as u64)
            .map(|k| {
                let mut rng = SplitMix64::new(first_seed + k);
                let truth = random_truth(&mut rng);
                let (corrs, is_in) = scene(&mut rng, &rig, &truth, cams, n_in, n_out, NOISE_PX);
                let p = PnpParams {
                    seed: first_seed + k,
                    ..PnpParams::default()
                };
                evaluate(&rig, &truth, &corrs, &is_in, &p)
            })
            .collect();
        report(
            &format!("{label} ({n_in} inliers + {n_out} outliers, {NOISE_PX} px)"),
            &ts,
        )
    }

    #[test]
    fn ransac_150_inliers_40_percent_outliers_stereo_pair() {
        // 40 % of the correspondences are outliers: 150 inliers + 100 outliers.
        let ok = run_ransac_trials(&[0, 1], 150, 100, 1000, 50, "cams 0,1");
        assert!(
            ok >= 48,
            "only {ok}/50 trials within 0.3 deg / 2 cm / 90 % recall"
        );
    }

    #[test]
    fn ransac_150_inliers_40_percent_outliers_four_cameras() {
        let ok = run_ransac_trials(&[0, 1, 2, 3], 150, 100, 2000, 50, "cams 0..3");
        assert!(
            ok >= 48,
            "only {ok}/50 trials within 0.3 deg / 2 cm / 90 % recall"
        );
    }

    #[test]
    fn ransac_landmarks_seen_by_both_cameras() {
        let rig = rig();
        let ts: Vec<Trial> = (0..30u64)
            .map(|k| {
                let mut rng = SplitMix64::new(4000 + k);
                let truth = random_truth(&mut rng);
                let (corrs, is_in) = landmark_scene(&mut rng, &rig, &truth, 75, 50);
                let p = PnpParams {
                    seed: k,
                    ..PnpParams::default()
                };
                evaluate(&rig, &truth, &corrs, &is_in, &p)
            })
            .collect();
        let ok = report("landmark pairs, cams 0,1 (150 inliers + 100 outliers)", &ts);
        assert!(ok >= 29, "only {ok}/30 trials succeeded");
    }

    #[test]
    fn ransac_survives_70_percent_outliers() {
        let ok = run_ransac_trials(
            &[0, 1, 2, 3],
            100,
            233,
            3000,
            30,
            "cams 0..3, 70 % outliers",
        );
        assert!(ok >= 28, "only {ok}/30 trials succeeded");
    }

    #[test]
    #[ignore = "slow: 20 seeds x 50 trials of both rigs; run with --ignored"]
    fn ransac_many_seeds() {
        let mut worst = 50;
        for k in 0..20 {
            let a = run_ransac_trials(&[0, 1], 150, 100, 10_000 + 100 * k, 50, "stress cams 0,1");
            let b = run_ransac_trials(
                &[0, 1, 2, 3],
                150,
                100,
                20_000 + 100 * k,
                50,
                "stress cams 0..3",
            );
            worst = worst.min(a).min(b);
        }
        assert!(worst >= 48, "worst block {worst}/50");
    }

    #[test]
    fn ransac_is_deterministic_per_seed() {
        let rig = rig();
        let mut rng = SplitMix64::new(77);
        let truth = random_truth(&mut rng);
        let (corrs, _) = scene(&mut rng, &rig, &truth, &[0, 1], 120, 60, NOISE_PX);
        let p = PnpParams::default();
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        let r1 = ransac(&corrs, bc, rt, &p).unwrap();
        assert_eq!(r1, ransac(&corrs, bc, rt, &p).unwrap());
        // another seed takes another sampling path but lands on the same pose
        let r3 = ransac(&corrs, bc, rt, &params(|p| p.seed = 1)).unwrap();
        assert!(
            yaw_err_deg(r1.pose.yaw, r3.pose.yaw) < 0.1 && pos_err(r1.pose.t, r3.pose.t) < 0.01
        );
    }

    #[test]
    fn ransac_degenerate_inputs_give_none() {
        let rig = rig();
        let mut rng = SplitMix64::new(51);
        let truth = random_truth(&mut rng);
        let p = PnpParams::default();
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        // fewer correspondences than min_inliers, even if all are inliers
        let (few, _) = scene(
            &mut rng,
            &rig,
            &truth,
            &[0, 1],
            p.min_inliers - 1,
            0,
            NOISE_PX,
        );
        assert!(ransac(&few, bc, rt, &p).is_none());
        assert!(ransac(&[], bc, rt, &p).is_none());
        // exactly min_inliers noisy inliers is enough
        let (just, _) = scene(&mut rng, &rig, &truth, &[0, 1], p.min_inliers, 0, NOISE_PX);
        let r = ransac(&just, bc, rt, &p).expect("min_inliers inliers");
        assert_eq!(r.inliers.len(), p.min_inliers);
        // all outliers
        let (junk, _) = scene(&mut rng, &rig, &truth, &[0, 1, 2, 3], 0, 120, NOISE_PX);
        assert!(ransac(&junk, bc, rt, &p).is_none());
        // invalid correspondences (bad camera, NaN) among good ones are skipped and never reported
        let (mut mixed, _) = scene(&mut rng, &rig, &truth, &[0, 1], 60, 20, NOISE_PX);
        let n_valid = mixed.len();
        mixed.push(with_cam(mixed[0], 12));
        mixed.push(with_bearing(mixed[1], [f64::NAN; 3]));
        mixed.push(with_point(mixed[2], [f64::NAN; 3]));
        let r = ransac(&mixed, bc, rt, &p).unwrap();
        assert!(r.inliers.iter().all(|&i| i < n_valid));
        assert!(r.inliers.len() >= 55);
        // all correspondences identical, a rig without cameras, bad parameters
        let same: Vec<Corr> = (0..40).map(|_| mixed[0]).collect();
        assert!(ransac(&same, bc, rt, &p).is_none());
        assert!(ransac(&mixed, &[], rt, &p).is_none());
        assert!(ransac(&mixed, bc, rt, &params(|p| p.thresh_deg = 0.0)).is_none());
        assert!(ransac(&mixed, bc, rt, &params(|p| p.thresh_deg = f64::NAN)).is_none());
        assert!(ransac(&mixed, bc, rt, &params(|p| p.max_iters = 0)).is_none());
        // min_inliers above what exists
        assert!(ransac(&mixed, bc, rt, &params(|p| p.min_inliers = 70)).is_none());
        // a single sample either fails or reports a consistent pose; odd confidence values do not matter
        if let Some(r) = ransac(&mixed, bc, rt, &params(|p| p.max_iters = 1)) {
            assert_eq!(r.iterations, 1);
        }
        for confidence in [f64::NAN, -1.0, 0.0, 1.0, 2.0] {
            assert!(ransac(&mixed, bc, rt, &params(|p| p.confidence = confidence)).is_some());
        }
    }

    // ---------------------------------------------------------------------------------------------------------
    // refinement
    // ---------------------------------------------------------------------------------------------------------

    #[test]
    fn refine_brings_a_perturbed_pose_to_the_noise_level() {
        let rig = rig();
        for (cams, seed) in [(&[0usize, 1][..], 21u64), (&[0, 1, 2, 3][..], 22)] {
            let mut rng = SplitMix64::new(seed);
            let truth = random_truth(&mut rng);
            let (corrs, _) = scene(&mut rng, &rig, &truth, cams, 120, 0, NOISE_PX);
            let idx: Vec<usize> = (0..corrs.len()).collect();
            let dir = normalize([
                uni(&mut rng, -1.0, 1.0),
                uni(&mut rng, -1.0, 1.0),
                uni(&mut rng, -1.0, 1.0),
            ]);
            let init = Pose4 {
                yaw: truth.pose.yaw + 2f64.to_radians(),
                t: add(truth.pose.t, scale(dir, 0.10)),
            };
            let rms_of = |p: &Pose4| {
                let s: f64 = corrs
                    .iter()
                    .map(|c| residual_deg(c, &rig.body_from_cam, &truth.r_tilt, p).powi(2))
                    .sum();
                (s / corrs.len() as f64).sqrt()
            };
            let (rms_init, rms_truth) = (rms_of(&init), rms_of(&truth.pose));
            let (pose, rms) = refine(&corrs, &idx, &rig.body_from_cam, &truth.r_tilt, &init, 1.0);
            let (ye, te) = (
                yaw_err_deg(pose.yaw, truth.pose.yaw),
                pos_err(pose.t, truth.pose.t),
            );
            println!(
                "refine cams {cams:?}: rms {rms_init:.3} -> {rms:.4} deg (rms at the truth {rms_truth:.4}); yaw err 2.000 -> {ye:.4} deg, t err 0.1000 -> {te:.4} m"
            );
            assert!(rms_init > 5.0 * rms_truth);
            assert!(
                (rms_of(&pose) - rms).abs() < 1e-9,
                "returned rms is that of the returned pose"
            );
            assert!(
                rms <= rms_truth * (1.0 + 1e-9),
                "the fit is at least as good as the truth"
            );
            assert!(
                (rms - rms_truth).abs() < 0.05 * rms_truth,
                "rms {rms} vs noise {rms_truth}"
            );
            assert!(ye < 0.1 && te < 0.01, "yaw err {ye} deg, t err {te} m");
            // 0.5 px of pixel noise is about 0.1-0.2 degree
            assert!(rms > 0.03 && rms < 0.3, "rms {rms}");
            assert!((pose.yaw - wrap_pi(pose.yaw)).abs() < 1e-12);
        }
    }

    #[test]
    fn refine_converges_exactly_on_noise_free_data_and_tolerates_gross_outliers() {
        let rig = rig();
        let mut rng = SplitMix64::new(31);
        let truth = random_truth(&mut rng);
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        let (corrs, _) = scene(&mut rng, &rig, &truth, &[0, 1, 2, 3], 100, 0, 0.0);
        let all: Vec<usize> = (0..corrs.len()).collect();
        let init = Pose4 {
            yaw: truth.pose.yaw - 0.15,
            t: add(truth.pose.t, [0.2, -0.1, 0.15]),
        };
        let (pose, rms) = refine(&corrs, &all, bc, rt, &init, 1.0);
        assert!(
            rms < 1e-6 && is_truth(&pose, &truth, 1e-7, 1e-8),
            "rms {rms}"
        );
        // plain least squares (huber <= 0 or NaN) reaches the same optimum
        for huber in [0.0, -3.0, f64::NAN] {
            let (p_ls, _) = refine(&corrs, &all, bc, rt, &init, huber);
            assert!(is_truth(&p_ls, &truth, 1e-7, 1e-8));
        }
        // 6 gross outliers in the index set barely move the Huber solution
        let (outliers, _) = scene(&mut rng, &rig, &truth, &[0, 1, 2, 3], 0, 6, 0.0);
        let mixed: Vec<Corr> = corrs.iter().chain(&outliers).copied().collect();
        let idx: Vec<usize> = (0..mixed.len()).collect();
        let (p_h, _) = refine(&mixed, &idx, bc, rt, &init, 1.0);
        assert!(is_truth(&p_h, &truth, 0.5, 0.1));
        // out-of-range indices and invalid correspondences are ignored; an empty set returns the init with NaN
        let (_, rms_bad) = refine(&corrs, &[0, 1, 2, 3, 4, 5, 999_999], bc, rt, &init, 1.0);
        assert!(rms_bad.is_finite());
        let invalid = [with_cam(corrs[0], 7)];
        let (p0, r0) = refine(&invalid, &[0], bc, rt, &init, 1.0);
        assert_eq!(p0, init);
        assert!(r0.is_nan());
        let (p0, r0) = refine(&corrs, &[], bc, rt, &init, 1.0);
        assert_eq!(p0, init);
        assert!(r0.is_nan());
        // a non-finite start is handed back as it is
        let nan = Pose4 {
            yaw: f64::NAN,
            ..init
        };
        assert!(refine(&corrs, &all, bc, rt, &nan, 1.0).0.yaw.is_nan());
    }

    #[test]
    fn refine_recovers_from_a_large_initial_error() {
        // Levenberg-Marquardt damping: 20 degrees of heading and half a metre of position error.
        let rig = rig();
        let mut rng = SplitMix64::new(33);
        let mut ok = 0;
        for _ in 0..20 {
            let truth = random_truth(&mut rng);
            let (corrs, _) = scene(&mut rng, &rig, &truth, &[0, 1, 2, 3], 100, 0, NOISE_PX);
            let idx: Vec<usize> = (0..corrs.len()).collect();
            let init = Pose4 {
                yaw: truth.pose.yaw + 20f64.to_radians(),
                t: add(truth.pose.t, [0.3, -0.2, 0.3]),
            };
            let (pose, _) = refine(&corrs, &idx, &rig.body_from_cam, &truth.r_tilt, &init, 1.0);
            ok += usize::from(is_truth(&pose, &truth, 0.2, 0.02));
        }
        println!("refine from 20 deg / 0.47 m off: {ok}/20 converged");
        assert!(ok >= 18, "{ok}/20");
    }

    #[test]
    fn residual_deg_basics() {
        let rig = rig();
        let mut rng = SplitMix64::new(41);
        let truth = random_truth(&mut rng);
        let (bc, rt) = (&rig.body_from_cam, &truth.r_tilt);
        let c = visible(&mut rng, &rig, &truth, &[0, 1, 2, 3], 0.0);
        assert!(residual_deg(&c, bc, rt, &truth.pose) < 1e-8);
        // a heading error moves the predicted ray by a bounded, non-zero amount
        let off = Pose4 {
            yaw: truth.pose.yaw + 1f64.to_radians(),
            ..truth.pose
        };
        let r = residual_deg(&c, bc, rt, &off);
        assert!(r > 1e-3 && r < 60.0, "residual {r}");
        // opposite bearing: 180 degrees; unusable correspondences: infinity
        let back = with_bearing(c, scale(c.bearing, -1.0));
        assert!((residual_deg(&back, bc, rt, &truth.pose) - 180.0).abs() < 1e-6);
        let res_of = |c: &Corr| residual_deg(c, bc, rt, &truth.pose);
        assert!(res_of(&with_cam(c, 4)).is_infinite());
        assert!(res_of(&with_bearing(c, [0.0; 3])).is_infinite());
        // a point on top of the camera has no direction
        let cam_centre = truth.map_from_body().apply(bc[c.cam].t);
        assert!(res_of(&with_point(c, cam_centre)).is_infinite());
    }

    #[test]
    fn jacobian_matches_finite_differences() {
        let ray = Ray {
            p: [0.05, 0.02, -0.03],
            d: normalize([0.2, -0.3, 0.9]),
            x: [1.0, 1.5, 2.5],
        };
        let pose = Pose4 {
            yaw: 0.4,
            t: [0.3, -0.1, 0.2],
        };
        let resid = |q: &Pose4| {
            let (s, c) = q.yaw.sin_cos();
            linearize(&ray, c, s, q.t).unwrap().r
        };
        let (s, c) = pose.yaw.sin_cos();
        let lin = linearize(&ray, c, s, pose.t).unwrap();
        assert!(lin.cos > 0.0);
        let h = 1e-6;
        for k in 0..4 {
            let (mut hi, mut lo) = (pose, pose);
            if k == 0 {
                hi.yaw += h;
                lo.yaw -= h;
            } else {
                hi.t[k - 1] += h;
                lo.t[k - 1] -= h;
            }
            let (rh, rl) = (resid(&hi), resid(&lo));
            for (row, (a, b)) in rh.iter().zip(&rl).enumerate() {
                let num = (a - b) / (2.0 * h);
                assert!(
                    (num - lin.j[row][k]).abs() < 1e-7,
                    "J[{row}][{k}] analytic {} numeric {num}",
                    lin.j[row][k]
                );
            }
        }
    }

    #[test]
    fn solve4_solves_and_flags_singular_systems() {
        let a = [
            [4.0, 1.0, 0.0, 0.0],
            [1.0, 3.0, 1.0, 0.0],
            [0.0, 1.0, 2.0, 1.0],
            [0.0, 0.0, 1.0, 5.0],
        ];
        let x = [1.0, -2.0, 0.5, 3.0];
        let b = a.map(|r| r.iter().zip(&x).map(|(p, q)| p * q).sum::<f64>());
        let s = solve4(a, b).unwrap();
        for (p, q) in s.iter().zip(&x) {
            assert!((p - q).abs() < 1e-12);
        }
        assert!(solve4([[0.0; 4]; 4], [1.0; 4]).is_none());
        assert!(solve4([[f64::NAN; 4]; 4], [1.0; 4]).is_none());
    }

    #[test]
    fn adaptive_iteration_count() {
        assert_eq!(iterations_needed(1.0, 0.999), 1);
        assert_eq!(iterations_needed(0.0, 0.999), usize::MAX);
        let n = iterations_needed(0.6, 0.999);
        assert!((15..=17).contains(&n), "n = {n}");
        assert!(iterations_needed(0.3, 0.999) > n);
        assert!(iterations_needed(0.6, f64::NAN) >= 1);
    }
}
