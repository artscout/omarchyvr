//! Constellation solve: blobs -> LED identities -> pose, per camera and jointly over all four cameras.
//!
//! Acquisition is brute force without a prior: each blob plus two of its nearest neighbours is tried against every
//! ordered triple of neighbouring, co-visible LEDs; Grunert P3P gives up to four poses; each pose is scored by how
//! many other facing LEDs land on a blob (angular test on unit bearings, no distortion maths in the inner loop).
//! The winner is refined with Levenberg-Marquardt on the pixel reprojection error (KB4 fisheye), re-matched and
//! refined again. Tracking with a prior (project last pose, nearest blob, refine) skips the brute force.

use crate::model::LedModel;
use crate::p3p;
use pimax_cam::geom::{self, Pose, Vec3};
use pimax_cam::kb4::Kb4;

/// One camera of the rig.
#[derive(Clone, Debug)]
pub struct Cam {
    pub intr: Kb4,
    /// `cam_from_body` (body = OpenXR head axes, from the factory calibration).
    pub cam_from_body: Pose,
}

/// Blobs of one camera in one LED frame.
#[derive(Clone, Debug, Default)]
pub struct CamObs {
    pub px: Vec<(f64, f64)>,
    /// Unit bearings in the camera frame (`None` outside the model's disc).
    pub bearing: Vec<Option<Vec3>>,
}

impl CamObs {
    pub fn new(cam: &Cam, px: Vec<(f64, f64)>) -> CamObs {
        let bearing = px.iter().map(|&(u, v)| cam.intr.unproject(u, v)).collect();
        CamObs { px, bearing }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Params {
    /// Angular inlier tolerance for hypothesis scoring (radians).
    pub tol_rad: f64,
    /// Pixel inlier tolerance after refinement.
    pub tol_px: f64,
    /// Pixel tolerance when matching from a prior pose.
    pub prior_tol_px: f64,
    /// An LED counts as visible when its normal is within this angle of the direction to the camera.
    pub facing_deg: f64,
    pub min_dist: f64,
    pub max_dist: f64,
    /// Nearest neighbours per anchor blob used to form blob triples.
    pub knn: usize,
    /// Minimum matched LEDs to accept a single-camera brute-force solve.
    pub min_inliers: usize,
    /// Stop the brute-force search once a hypothesis matches this many LEDs.
    pub good_enough: usize,
}

impl Default for Params {
    fn default() -> Self {
        Params {
            tol_rad: 3.0 / 230.0,
            tol_px: 2.5,
            prior_tol_px: 12.0,
            facing_deg: 80.0,
            min_dist: 0.07,
            max_dist: 1.5,
            knn: 5,
            min_inliers: 5,
            good_enough: 9,
        }
    }
}

/// A pose with its LED <-> blob matches in one camera.
#[derive(Clone, Debug)]
pub struct CamSolve {
    pub cam: usize,
    pub cam_from_model: Pose,
    /// (led index, blob index)
    pub matches: Vec<(usize, usize)>,
    pub rms_px: f64,
}

fn facing(cam_from_model: &Pose, led: &crate::model::Led, cos_lim: f64) -> Option<Vec3> {
    let pc = cam_from_model.apply(led.p);
    if pc[2] <= 0.0 {
        return None;
    }
    let d = geom::normalize(pc);
    let n = geom::mat_vec(&cam_from_model.r, led.n);
    (geom::dot(n, geom::scale(d, -1.0)) >= cos_lim).then_some(d)
}

/// Greedy one-to-one angular matching of facing LEDs to unused blobs.
fn match_angular(
    pose: &Pose,
    model: &LedModel,
    obs: &CamObs,
    used: &[bool],
    cos_face: f64,
    cos_tol: f64,
) -> (Vec<(usize, usize)>, f64) {
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for (li, led) in model.leds.iter().enumerate() {
        let Some(d) = facing(pose, led, cos_face) else {
            continue;
        };
        for (bi, b) in obs.bearing.iter().enumerate() {
            if used[bi] {
                continue;
            }
            if let Some(b) = b {
                let c = geom::dot(d, *b);
                if c >= cos_tol {
                    cand.push((c, li, bi));
                }
            }
        }
    }
    cand.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut lu = [false; 64];
    let mut bu = vec![false; obs.px.len()];
    let mut m = Vec::new();
    let mut err = 0.0;
    for (c, li, bi) in cand {
        if !lu[li] && !bu[bi] {
            lu[li] = true;
            bu[bi] = true;
            m.push((li, bi));
            err += 1.0 - c;
        }
    }
    (m, err)
}

/// Pixel-space matching from a pose (used after refinement and for priors).
pub fn match_pixels(
    pose: &Pose,
    model: &LedModel,
    cam: &Cam,
    obs: &CamObs,
    used: &[bool],
    cos_face: f64,
    tol_px: f64,
) -> Vec<(usize, usize)> {
    let mut cand: Vec<(f64, usize, usize)> = Vec::new();
    for (li, led) in model.leds.iter().enumerate() {
        if facing(pose, led, cos_face).is_none() {
            continue;
        }
        let Some((u, v)) = cam.intr.project(pose.apply(led.p)) else {
            continue;
        };
        for (bi, &(bu, bv)) in obs.px.iter().enumerate() {
            if used[bi] {
                continue;
            }
            let d = ((u - bu).powi(2) + (v - bv).powi(2)).sqrt();
            if d <= tol_px {
                cand.push((d, li, bi));
            }
        }
    }
    cand.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());
    let mut lu = [false; 64];
    let mut bu = vec![false; obs.px.len()];
    let mut m = Vec::new();
    for (_, li, bi) in cand {
        if !lu[li] && !bu[bi] {
            lu[li] = true;
            bu[bi] = true;
            m.push((li, bi));
        }
    }
    m
}

fn perturb(p: &Pose, d: &[f64; 6]) -> Pose {
    Pose {
        r: geom::mat_mul(&geom::rodrigues([d[0], d[1], d[2]]), &p.r),
        t: geom::add(p.t, [d[3], d[4], d[5]]),
    }
}

fn solve6(a: &mut [[f64; 6]; 6], b: &mut [f64; 6]) -> Option<[f64; 6]> {
    for c in 0..6 {
        let piv = (c..6).max_by(|&i, &j| a[i][c].abs().partial_cmp(&a[j][c].abs()).unwrap())?;
        if a[piv][c].abs() < 1e-18 {
            return None;
        }
        a.swap(c, piv);
        b.swap(c, piv);
        for r in c + 1..6 {
            let f = a[r][c] / a[c][c];
            for k in c..6 {
                a[r][k] -= f * a[c][k];
            }
            b[r] -= f * b[c];
        }
    }
    let mut x = [0.0; 6];
    for c in (0..6).rev() {
        let mut s = b[c];
        for k in c + 1..6 {
            s -= a[c][k] * x[k];
        }
        x[c] = s / a[c][c];
    }
    Some(x)
}

/// A pixel observation for joint refinement: camera, model point, measured pixel.
pub struct Obs<'a> {
    pub cam: &'a Cam,
    pub p: Vec3,
    pub px: (f64, f64),
}

/// Levenberg-Marquardt over `base_from_model`; each observation's camera sees `cam_from_base * base_from_model`
/// (`cam_from_base` = identity for a single-camera solve, `cam_from_body` for a joint one). Returns (pose, rms px).
pub fn refine(pose: Pose, obs: &[Obs], joint: bool, iters: usize) -> (Pose, f64) {
    refine_with(pose, obs, joint, iters, None)
}

/// Rotation vector (axis * angle, rad) of a rotation matrix.
pub fn rotvec(r: &geom::Mat3) -> Vec3 {
    let c = ((r[0][0] + r[1][1] + r[2][2] - 1.0) / 2.0).clamp(-1.0, 1.0);
    let ang = c.acos();
    if ang < 1e-12 {
        return [0.0; 3];
    }
    let k = ang / (2.0 * ang.sin());
    [
        k * (r[2][1] - r[1][2]),
        k * (r[0][2] - r[2][0]),
        k * (r[1][0] - r[0][1]),
    ]
}

/// `refine` with an optional rotation prior `(r_prior, w)`: three extra residuals `w * rotvec(R r_prior^T)` (w in
/// px per rad), so 3-4 LED solves stay near the predicted orientation (the constrained few-LED track). The
/// returned rms is over the pixel residuals only.
pub fn refine_with(
    pose: Pose,
    obs: &[Obs],
    joint: bool,
    iters: usize,
    rot_prior: Option<(geom::Mat3, f64)>,
) -> (Pose, f64) {
    let rp_t = rot_prior.map(|(r, w)| (geom::transpose(&r), w));
    let res = |p: &Pose, out: &mut Vec<f64>| -> bool {
        out.clear();
        if let Some((rt, w)) = &rp_t {
            let d = rotvec(&geom::mat_mul(&p.r, rt));
            out.extend(d.iter().map(|x| x * w));
        }
        for o in obs {
            let x = p.apply(o.p);
            let xc = if joint {
                o.cam.cam_from_body.apply(x)
            } else {
                x
            };
            match o.cam.intr.project(xc) {
                Some((u, v)) => {
                    out.push(u - o.px.0);
                    out.push(v - o.px.1);
                }
                None => return false,
            }
        }
        true
    };
    let cost = |r: &[f64]| r.iter().map(|x| x * x).sum::<f64>();
    let mut p = pose;
    let mut r0 = Vec::new();
    if !res(&p, &mut r0) {
        return (p, f64::INFINITY);
    }
    let mut c0 = cost(&r0);
    let mut lambda = 1e-3;
    let mut r1 = Vec::new();
    for _ in 0..iters {
        // numeric Jacobian
        let mut jac = vec![[0.0f64; 6]; r0.len()];
        let mut ok = true;
        for k in 0..6 {
            let mut d = [0.0; 6];
            let h = if k < 3 { 1e-6 } else { 1e-7 };
            d[k] = h;
            if !res(&perturb(&p, &d), &mut r1) {
                ok = false;
                break;
            }
            for (i, row) in jac.iter_mut().enumerate() {
                row[k] = (r1[i] - r0[i]) / h;
            }
        }
        if !ok {
            break;
        }
        let mut jtj = [[0.0; 6]; 6];
        let mut jtr = [0.0; 6];
        for (i, row) in jac.iter().enumerate() {
            for a in 0..6 {
                jtr[a] -= row[a] * r0[i];
                for b in 0..6 {
                    jtj[a][b] += row[a] * row[b];
                }
            }
        }
        let mut improved = false;
        for _ in 0..8 {
            let mut a = jtj;
            for (d, row) in a.iter_mut().enumerate() {
                row[d] += lambda * (jtj[d][d] + 1e-9);
            }
            let mut b = jtr;
            let Some(dx) = solve6(&mut a, &mut b) else {
                break;
            };
            let pn = perturb(&p, &dx);
            if res(&pn, &mut r1) {
                let c1 = cost(&r1);
                if c1 < c0 {
                    p = pn;
                    std::mem::swap(&mut r0, &mut r1);
                    let rel = (c0 - c1) / c0.max(1e-30);
                    c0 = c1;
                    lambda = (lambda * 0.3).max(1e-9);
                    improved = rel > 1e-10;
                    break;
                }
            }
            lambda *= 10.0;
        }
        if !improved {
            break;
        }
    }
    let npx = if rp_t.is_some() {
        r0.len().saturating_sub(3)
    } else {
        r0.len()
    };
    let prior_cost: f64 = if rp_t.is_some() {
        r0[..3.min(r0.len())].iter().map(|x| x * x).sum()
    } else {
        0.0
    };
    let rms = ((c0 - prior_cost).max(0.0) / (npx / 2).max(1) as f64).sqrt();
    // re-orthonormalise
    let q = geom::quat_from_mat(&p.r);
    p.r = quat_to_mat(q);
    (p, rms)
}

pub fn quat_to_mat(q: [f64; 4]) -> geom::Mat3 {
    // q = [x, y, z, w] (as `geom::quat_from_mat`)
    let n = (q[0] * q[0] + q[1] * q[1] + q[2] * q[2] + q[3] * q[3]).sqrt();
    let (x, y, z, w) = (q[0] / n, q[1] / n, q[2] / n, q[3] / n);
    [
        [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y - w * z),
            2.0 * (x * z + w * y),
        ],
        [
            2.0 * (x * y + w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z - w * x),
        ],
        [
            2.0 * (x * z - w * y),
            2.0 * (y * z + w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    ]
}

/// Refine a single-camera pose on its matches, re-match in pixels and refine again.
pub fn polish(
    pose: Pose,
    model: &LedModel,
    cam: &Cam,
    cam_idx: usize,
    obs: &CamObs,
    used: &[bool],
    prm: &Params,
    matches: &[(usize, usize)],
) -> Option<CamSolve> {
    let cos_face = prm.facing_deg.to_radians().cos();
    let mk = |m: &[(usize, usize)]| -> Vec<Obs> {
        m.iter()
            .map(|&(l, b)| Obs {
                cam,
                p: model.leds[l].p,
                px: obs.px[b],
            })
            .collect()
    };
    let (p1, _) = refine(pose, &mk(matches), false, 15);
    let m2 = match_pixels(&p1, model, cam, obs, used, cos_face, prm.tol_px * 2.0);
    if m2.len() < 4 {
        return None;
    }
    let (p2, _) = refine(p1, &mk(&m2), false, 20);
    let m3 = match_pixels(&p2, model, cam, obs, used, cos_face, prm.tol_px);
    if m3.len() < 4 {
        return None;
    }
    let (p3, rms) = refine(p2, &mk(&m3), false, 20);
    Some(CamSolve {
        cam: cam_idx,
        cam_from_model: p3,
        matches: m3,
        rms_px: rms,
    })
}

/// Brute-force acquisition of one controller in one camera (no prior). `used` marks blobs already claimed.
pub fn acquire(
    model: &LedModel,
    triples: &[[usize; 3]],
    cam: &Cam,
    cam_idx: usize,
    obs: &CamObs,
    used: &[bool],
    prm: &Params,
) -> Option<CamSolve> {
    let n = obs.px.len();
    let free: Vec<usize> = (0..n)
        .filter(|&i| !used[i] && obs.bearing[i].is_some())
        .collect();
    if free.len() < prm.min_inliers.max(3) {
        return None;
    }
    let cos_face = prm.facing_deg.to_radians().cos();
    let cos_tol = prm.tol_rad.cos();
    // blob triples: anchor + 2 of its k nearest free neighbours
    let mut btri: Vec<[usize; 3]> = Vec::new();
    for &a in &free {
        let mut nb: Vec<(f64, usize)> = free
            .iter()
            .filter(|&&b| b != a)
            .map(|&b| {
                (
                    (obs.px[a].0 - obs.px[b].0).powi(2) + (obs.px[a].1 - obs.px[b].1).powi(2),
                    b,
                )
            })
            .collect();
        nb.sort_by(|x, y| x.0.partial_cmp(&y.0).unwrap());
        nb.truncate(prm.knn);
        for i in 0..nb.len() {
            for j in i + 1..nb.len() {
                let mut t = [a, nb[i].1, nb[j].1];
                t.sort();
                btri.push(t);
            }
        }
    }
    btri.sort();
    btri.dedup();
    let mut best: Option<(usize, f64, Pose, Vec<(usize, usize)>)> = None;
    'search: for bt in &btri {
        let y = bt.map(|i| obs.bearing[i].unwrap());
        for lt in triples {
            let x = lt.map(|i| model.leds[i].p);
            for pose in p3p::p3p(y, x) {
                let dist = geom::norm(pose.t);
                if dist < prm.min_dist || dist > prm.max_dist {
                    continue;
                }
                if lt
                    .iter()
                    .any(|&l| facing(&pose, &model.leds[l], cos_face).is_none())
                {
                    continue;
                }
                let (m, err) = match_angular(&pose, model, obs, used, cos_face, cos_tol);
                let better = match &best {
                    None => true,
                    Some((bn, be, _, _)) => m.len() > *bn || (m.len() == *bn && err < *be),
                };
                if better && m.len() >= 3 {
                    let n = m.len();
                    best = Some((n, err, pose, m));
                    if n >= prm.good_enough {
                        break 'search;
                    }
                }
            }
        }
    }
    let (cnt, _, pose, m) = best?;
    if cnt < prm.min_inliers {
        return None;
    }
    let s = polish(pose, model, cam, cam_idx, obs, used, prm, &m)?;
    (s.matches.len() >= prm.min_inliers && s.rms_px <= prm.tol_px).then_some(s)
}

/// Track from a prior `body_from_model`: match in every camera and refine jointly.
pub fn joint_from_prior(
    body_from_model: Pose,
    model: &LedModel,
    cams: &[Cam],
    obs: &[CamObs],
    used: &[Vec<bool>],
    tol_px: f64,
    facing_deg: f64,
) -> Option<JointSolve> {
    let cos_face = facing_deg.to_radians().cos();
    let mut per_cam: Vec<Vec<(usize, usize)>> = Vec::new();
    for (ci, cam) in cams.iter().enumerate() {
        let cfm = cam.cam_from_body.compose(&body_from_model);
        per_cam.push(match_pixels(
            &cfm, model, cam, &obs[ci], &used[ci], cos_face, tol_px,
        ));
    }
    let total: usize = per_cam.iter().map(|m| m.len()).sum();
    if total < 4 {
        return None;
    }
    let mut o = Vec::new();
    for (ci, m) in per_cam.iter().enumerate() {
        for &(l, b) in m {
            o.push(Obs {
                cam: &cams[ci],
                p: model.leds[l].p,
                px: obs[ci].px[b],
            });
        }
    }
    let (pose, rms) = refine(body_from_model, &o, true, 25);
    Some(JointSolve {
        body_from_model: pose,
        matches: per_cam,
        rms_px: rms,
    })
}

#[derive(Clone, Debug)]
pub struct JointSolve {
    pub body_from_model: Pose,
    /// Matches per camera.
    pub matches: Vec<Vec<(usize, usize)>>,
    pub rms_px: f64,
}

impl JointSolve {
    pub fn total(&self) -> usize {
        self.matches.iter().map(|m| m.len()).sum()
    }
    pub fn cams(&self) -> usize {
        self.matches.iter().filter(|m| !m.is_empty()).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Led;

    fn ring_model() -> LedModel {
        // 15 LEDs on a tilted ring, like the Pimax controller (radius ~4.5 cm), normals outward.
        let mut leds = Vec::new();
        for i in 0..15 {
            let a = i as f64 / 15.0 * std::f64::consts::TAU * 0.9;
            let n = geom::normalize([a.cos(), a.sin(), -1.0 + 0.3 * (2.0 * a).sin()]);
            leds.push(Led {
                p: [0.045 * a.cos(), 0.045 * a.sin(), 0.015 * (3.0 * a).sin()],
                n,
            });
        }
        LedModel {
            hand: "T".into(),
            serial: "t".into(),
            leds,
            imu_to_device: Pose::IDENTITY,
        }
    }

    #[test]
    fn acquires_synthetic_constellation() {
        let model = ring_model();
        let intr = Kb4::new(
            640,
            480,
            232.0,
            232.0,
            320.0,
            240.0,
            [0.06, -0.06, 0.03, -0.008],
            vec![],
        );
        let cam = Cam {
            intr,
            cam_from_body: Pose::IDENTITY,
        };
        let truth = Pose {
            r: geom::rodrigues([0.4, 0.3, -0.2]),
            t: [0.06, 0.05, 0.40],
        };
        let mut px = Vec::new();
        let cos_face = 80f64.to_radians().cos();
        for led in &model.leds {
            if facing(&truth, led, cos_face).is_some() {
                let (u, v) = cam.intr.project(truth.apply(led.p)).unwrap();
                px.push((u + 0.2, v - 0.15));
            }
        }
        px.push((100.0, 100.0)); // a lamp
        let obs = CamObs::new(&cam, px);
        let used = vec![false; obs.px.len()];
        let tri = model.neighbour_triples(0.05, 100.0);
        let s = acquire(&model, &tri, &cam, 0, &obs, &used, &Params::default()).expect("solve");
        assert!(s.matches.len() >= 6, "{} matches", s.matches.len());
        assert!(s.rms_px < 0.5, "rms {}", s.rms_px);
        assert!(geom::norm(geom::sub(s.cam_from_model.t, truth.t)) < 0.01);
    }

    /// Three LEDs and a rotation prior 3 deg off: the constrained refine recovers the position to under 1 cm (7 mm) from a
    /// start 3 cm away (the few-LED track).
    #[test]
    fn three_led_refine_with_rotation_prior() {
        let model = ring_model();
        let cam = Cam {
            intr: Kb4::new(
                640,
                480,
                232.0,
                232.0,
                320.0,
                240.0,
                [0.06, -0.06, 0.03, -0.008],
                vec![],
            ),
            cam_from_body: Pose::IDENTITY,
        };
        let truth = Pose {
            r: geom::rodrigues([0.4, 0.3, -0.2]),
            t: [0.06, 0.05, 0.40],
        };
        let cos_face = 80f64.to_radians().cos();
        let vis: Vec<usize> = (0..model.leds.len())
            .filter(|&l| facing(&truth, &model.leds[l], cos_face).is_some())
            .take(3)
            .collect();
        assert_eq!(vis.len(), 3);
        let obs: Vec<Obs> = vis
            .iter()
            .map(|&l| Obs {
                cam: &cam,
                p: model.leds[l].p,
                px: cam.intr.project(truth.apply(model.leds[l].p)).unwrap(),
            })
            .collect();
        let prior_r = geom::mat_mul(&truth.r, &geom::rodrigues([0.0, 3f64.to_radians(), 0.0]));
        let start = Pose {
            r: prior_r,
            t: geom::add(truth.t, [0.02, -0.015, 0.015]),
        };
        let (p, rms) = refine_with(start, &obs, true, 30, Some((prior_r, 300.0)));
        let err = geom::norm(geom::sub(p.t, truth.t));
        assert!(err < 0.01, "position error {err}");
        assert!(rms < 1.5, "rms {rms}");
    }
}

#[test]
fn quat_roundtrip() {
    let r = geom::rodrigues([0.3, -1.2, 0.7]);
    let r2 = quat_to_mat(geom::quat_from_mat(&r));
    for i in 0..3 {
        for j in 0..3 {
            assert!((r[i][j] - r2[i][j]).abs() < 1e-12);
        }
    }
}
