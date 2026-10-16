//! Rotation-only bundle adjustment of the camera rig.
//!
//! Unknowns: a rotation correction `w_i` per *free* camera (about its own centre, `R_i' = Exp(w_i) R_i`, the
//! [`pimax_cam::overlay`] convention) and one inverse depth `rho` per two-view observation, anchored on camera a's
//! ray. Camera translations (baselines) stay at the factory values, which also fixes the scale; at least one
//! camera stays fixed (the gauge: the rig's overall rotation cannot be seen from camera-to-camera matches).
//!
//! Residual of observation k (cameras a, b): the predicted ray in b,
//! `u = R_b' (R_a'^T ray_a + rho (c_a - c_b))`, is projected onto the tangent plane at the *measured* ray `n`:
//! `r = f [e1.u / n.u, e2.u / n.u]` with `e2` normal to the epipolar plane ("vertical" in a rectified pair) and
//! `e1` along the epipolar line. `f` (the rectified focal length) makes the units pixels. The along-line
//! component is absorbed by `rho`; the across component is the vertical disparity a rotation error causes.
//!
//! Solver: Levenberg-Marquardt with a Huber kernel (IRLS), the inverse depths eliminated by a Schur complement
//! (each is 1x1), a weak zero-mean prior on every `w_i` (or a per-camera prior, e.g. the previous learned
//! calibration with its uncertainty), one round of outlier rejection, and the a-posteriori covariance of the `w_i`.

use crate::so3;
use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::geom::{self, Mat3, Vec3};

/// Factory rig: per camera `R_i = cam_i_from_cam0` rotation and centre `c_i` in the camera-0 frame.
#[derive(Clone, Debug)]
pub struct RigModel {
    /// Rotations `cam_i_from_cam0`.
    pub r: Vec<Mat3>,
    /// Centres in the camera-0 frame.
    pub c: Vec<Vec3>,
}

impl RigModel {
    /// From the calibration (rig convention `X_i = R X_0 + t`).
    pub fn from_calib(cal: &DeviceCalib) -> RigModel {
        let n = cal.cameras.len();
        let mut r = Vec::with_capacity(n);
        let mut c = Vec::with_capacity(n);
        for i in 0..n {
            let p = cal.cam_from_cam0(i, RigConvention::CamFromCam0);
            r.push(p.r);
            c.push(p.inverse().t);
        }
        RigModel { r, c }
    }

    /// Corrected rotation of camera i.
    pub fn rot(&self, w: &[Vec3], i: usize) -> Mat3 {
        geom::mat_mul(&so3::exp(w[i]), &self.r[i])
    }
}

/// One two-view observation (a stereo match converted to unit rays).
#[derive(Clone, Copy, Debug)]
pub struct Obs {
    /// Anchor camera.
    pub a: usize,
    /// Second camera.
    pub b: usize,
    /// Unit ray in camera a.
    pub ray_a: Vec3,
    /// Unit ray in camera b (the measurement).
    pub ray_b: Vec3,
    /// Caller's tag (scene/pair index) for statistics.
    pub group: usize,
}

/// Options.
#[derive(Clone, Debug)]
pub struct BaOptions {
    /// Cameras whose rotation is estimated; all others keep their initial value.
    pub free: Vec<usize>,
    /// Measurement noise (pixels at `f_res`).
    pub sigma_px: f64,
    /// Pixel scale of the residuals (use the rectified focal length).
    pub f_res: f64,
    /// Huber threshold (pixels).
    pub huber_px: f64,
    /// Prior standard deviation of each rotation component (radians), around zero (= the factory rig). The
    /// default, 0.3 deg, says that a factory-calibrated rigid headset moves by a few tenths of a degree at
    /// most. It keeps the components that the matches barely determine from wandering (on the real morning
    /// scene a loose prior let the yaw-like component run to 0.7 deg, which made the independent eye-view check
    /// worse) and is negligible against a well-determined estimate (sigma 0.002 deg on the synthetic scenes).
    pub prior_rad: f64,
    /// Per-camera prior `(mean, sigma per component)` in radians, replacing the default one where given. Used to
    /// fold in an earlier estimate (its overlay rotation and sigma, widened for the time since), so that the
    /// calibration is refined over sessions instead of re-estimated from scratch.
    pub priors: Vec<Option<(Vec3, Vec3)>>,
    /// Iteration limit per round.
    pub max_iter: usize,
    /// After the first round, observations with a residual above this (pixels) are dropped and the problem is
    /// solved again.
    pub outlier_px: f64,
}

impl Default for BaOptions {
    fn default() -> Self {
        BaOptions {
            free: vec![1],
            sigma_px: 0.3,
            f_res: 220.0,
            huber_px: 1.0,
            prior_rad: 0.3f64.to_radians(),
            priors: Vec::new(),
            max_iter: 50,
            outlier_px: 3.0,
        }
    }
}

/// Result.
#[derive(Clone, Debug)]
pub struct BaResult {
    /// Rotation correction per camera (zero for fixed cameras unless given as initial value).
    pub w: Vec<Vec3>,
    /// Covariance (rad^2) of each free camera's `w` (a-posteriori, scaled by the residual variance).
    pub cov: Vec<Option<Mat3>>,
    /// Inverse depth per observation (1/m).
    pub rho: Vec<f64>,
    /// Final residuals `[along, across]` in pixels, **measured minus predicted** (the across component has the
    /// sign of a rectified `dy`).
    pub res: Vec<[f64; 2]>,
    /// Residuals with the initial rotations and optimised depths (the "before" picture).
    pub res_initial: Vec<[f64; 2]>,
    /// Observations kept (not rejected as outliers).
    pub inlier: Vec<bool>,
    /// LM iterations (all rounds).
    pub iterations: usize,
    /// Robust cost at the start and at the end (inliers of the last round).
    pub cost: (f64, f64),
    /// A-posteriori residual standard deviation of the inliers (pixels).
    pub sigma_post_px: f64,
}

#[derive(Clone, Copy)]
struct Basis {
    n: Vec3,
    e1: Vec3,
    e2: Vec3,
}

fn basis(rig: &RigModel, w: &[Vec3], o: &Obs) -> Basis {
    let n = geom::normalize(o.ray_b);
    // rectified x axis (from a's centre to b's) in b's frame
    let xr = geom::normalize(geom::mat_vec(
        &rig.rot(w, o.b),
        geom::sub(rig.c[o.b], rig.c[o.a]),
    ));
    let e2 = geom::normalize(geom::cross(n, xr));
    let e1 = geom::cross(e2, n);
    Basis { n, e1, e2 }
}

/// Predicted direction `u` (not normalised) and its Jacobians wrt (delta_a, delta_b, rho).
struct Pred {
    u: Vec3,
    du_da: Mat3,
    du_db: Mat3,
    du_drho: Vec3,
}

fn predict(rig: &RigModel, w: &[Vec3], o: &Obs, rho: f64) -> Pred {
    let ra = rig.rot(w, o.a);
    let rb = rig.rot(w, o.b);
    let rbra_t = geom::mat_mul(&rb, &geom::transpose(&ra));
    let base = geom::sub(rig.c[o.a], rig.c[o.b]);
    let u = geom::add(
        geom::mat_vec(&rbra_t, o.ray_a),
        geom::scale(geom::mat_vec(&rb, base), rho),
    );
    Pred {
        u,
        du_da: geom::mat_mul(&rbra_t, &so3::skew(o.ray_a)),
        du_db: so3::mat_scale(&so3::skew(u), -1.0),
        du_drho: geom::mat_vec(&rb, base),
    }
}

/// Residual (prediction minus measurement, pixels) and d r / d u (2x3).
fn project(b: &Basis, u: Vec3, f: f64) -> Option<([f64; 2], [[f64; 3]; 2])> {
    let nu = geom::dot(b.n, u);
    if nu <= 1e-9 * geom::norm(u).max(1e-12) {
        return None;
    }
    let (p1, p2) = (geom::dot(b.e1, u) / nu, geom::dot(b.e2, u) / nu);
    let row = |e: Vec3, p: f64| {
        let v = geom::scale(geom::sub(e, geom::scale(b.n, p)), f / nu);
        [v[0], v[1], v[2]]
    };
    Some(([f * p1, f * p2], [row(b.e1, p1), row(b.e2, p2)]))
}

fn huber_w(r: f64, k: f64) -> f64 {
    let a = r.abs();
    if a <= k {
        1.0
    } else {
        k / a
    }
}

fn huber_cost(r: f64, k: f64) -> f64 {
    let a = r.abs();
    if a <= k {
        0.5 * a * a
    } else {
        k * (a - 0.5 * k)
    }
}

fn init_rho(rig: &RigModel, w: &[Vec3], o: &Obs, b: &Basis) -> f64 {
    let p0 = predict(rig, w, o, 0.0);
    let den = geom::dot(b.e1, p0.du_drho);
    if den.abs() < 1e-12 {
        return 0.0;
    }
    (-geom::dot(b.e1, p0.u) / den).clamp(-0.5, 20.0)
}

/// Solve `A x = b` (small dense, partial pivoting). `None` if singular.
pub fn solve_dense(a: &[Vec<f64>], b: &[f64]) -> Option<Vec<f64>> {
    let n = b.len();
    let mut m: Vec<Vec<f64>> = a.to_vec();
    let mut v = b.to_vec();
    for c in 0..n {
        let p = (c..n).max_by(|&i, &j| m[i][c].abs().partial_cmp(&m[j][c].abs()).unwrap())?;
        if m[p][c].abs() < 1e-300 {
            return None;
        }
        m.swap(c, p);
        v.swap(c, p);
        for r in c + 1..n {
            let f = m[r][c] / m[c][c];
            if f != 0.0 {
                for k in c..n {
                    m[r][k] -= f * m[c][k];
                }
                v[r] -= f * v[c];
            }
        }
    }
    let mut x = vec![0.0; n];
    for c in (0..n).rev() {
        let mut s = v[c];
        for k in c + 1..n {
            s -= m[c][k] * x[k];
        }
        x[c] = s / m[c][c];
    }
    Some(x)
}

struct Normal {
    s: Vec<Vec<f64>>,
    g: Vec<f64>,
    // per observation: H_wrho (dense over free params), H_rhorho, g_rho
    hwr: Vec<Vec<f64>>,
    hrr: Vec<f64>,
    gr: Vec<f64>,
}

struct Problem<'a> {
    rig: &'a RigModel,
    obs: &'a [Obs],
    opt: &'a BaOptions,
    bases: Vec<Basis>,
    slot: Vec<Option<usize>>, // camera -> parameter block index
    np: usize,
}

impl Problem<'_> {
    fn cost_of(&self, w: &[Vec3], rho: &[f64], active: &[bool]) -> f64 {
        let k = self.opt.huber_px / self.opt.sigma_px;
        let mut c = 0.0;
        for (i, o) in self.obs.iter().enumerate() {
            if !active[i] {
                continue;
            }
            let p = predict(self.rig, w, o, rho[i]);
            match project(&self.bases[i], p.u, self.opt.f_res) {
                Some((r, _)) => {
                    for x in r {
                        c += huber_cost(x / self.opt.sigma_px, k);
                    }
                }
                None => c += 1e6,
            }
        }
        c + self.prior_cost(w)
    }

    /// Prior mean and per-component sigma of camera `cam`.
    fn prior_of(&self, cam: usize) -> (Vec3, Vec3) {
        match self.opt.priors.get(cam).copied().flatten() {
            Some(p) => p,
            None => ([0.0; 3], [self.opt.prior_rad; 3]),
        }
    }

    fn prior_cost(&self, w: &[Vec3]) -> f64 {
        let mut c = 0.0;
        for (cam, s) in self.slot.iter().enumerate() {
            if s.is_some() {
                let (m, sg) = self.prior_of(cam);
                for k in 0..3 {
                    c += 0.5 * ((w[cam][k] - m[k]) / sg[k]).powi(2);
                }
            }
        }
        c
    }

    fn normal(&self, w: &[Vec3], rho: &[f64], active: &[bool]) -> Normal {
        let np = self.np;
        let k = self.opt.huber_px / self.opt.sigma_px;
        let mut s = vec![vec![0.0; np]; np];
        let mut g = vec![0.0; np];
        let n_obs = self.obs.len();
        let mut hwr = vec![vec![0.0; np]; n_obs];
        let mut hrr = vec![0.0; n_obs];
        let mut gr = vec![0.0; n_obs];
        for (i, o) in self.obs.iter().enumerate() {
            if !active[i] {
                continue;
            }
            let p = predict(self.rig, w, o, rho[i]);
            let Some((r, dr_du)) = project(&self.bases[i], p.u, self.opt.f_res) else {
                continue;
            };
            for comp in 0..2 {
                let rn = r[comp] / self.opt.sigma_px;
                let wt = huber_w(rn, k) / self.opt.sigma_px.powi(2);
                let d = dr_du[comp];
                // Jacobian row over free params and rho
                let mut jw = vec![0.0; np];
                for (cam, m) in [(o.a, &p.du_da), (o.b, &p.du_db)] {
                    if let Some(sl) = self.slot[cam] {
                        for c in 0..3 {
                            jw[3 * sl + c] += d[0] * m[0][c] + d[1] * m[1][c] + d[2] * m[2][c];
                        }
                    }
                }
                let jr = geom::dot(d, p.du_drho);
                for a in 0..np {
                    if jw[a] == 0.0 {
                        continue;
                    }
                    g[a] += wt * jw[a] * r[comp];
                    hwr[i][a] += wt * jw[a] * jr;
                    for b in 0..np {
                        s[a][b] += wt * jw[a] * jw[b];
                    }
                }
                hrr[i] += wt * jr * jr;
                gr[i] += wt * jr * r[comp];
            }
        }
        // prior
        for (cam, sl) in self.slot.iter().enumerate() {
            if let Some(sl) = sl {
                let (m, sg) = self.prior_of(cam);
                for c in 0..3 {
                    s[3 * sl + c][3 * sl + c] += 1.0 / sg[c].powi(2);
                    g[3 * sl + c] += (w[cam][c] - m[c]) / sg[c].powi(2);
                }
            }
        }
        Normal { s, g, hwr, hrr, gr }
    }

    /// One damped step. Returns the new (w, rho).
    fn step(
        &self,
        nm: &Normal,
        w: &[Vec3],
        rho: &[f64],
        lambda: f64,
        active: &[bool],
    ) -> Option<(Vec<Vec3>, Vec<f64>)> {
        let np = self.np;
        let mut red = nm.s.clone();
        let mut b = nm.g.clone();
        for a in 0..np {
            red[a][a] *= 1.0 + lambda;
        }
        for i in 0..self.obs.len() {
            if !active[i] || nm.hrr[i] <= 0.0 {
                continue;
            }
            let hrr = nm.hrr[i] * (1.0 + lambda) + 1e-9;
            let hw = &nm.hwr[i];
            for a in 0..np {
                if hw[a] == 0.0 {
                    continue;
                }
                b[a] -= hw[a] * nm.gr[i] / hrr;
                for c in 0..np {
                    red[a][c] -= hw[a] * hw[c] / hrr;
                }
            }
        }
        let dw = if np > 0 {
            solve_dense(&red, &b.iter().map(|x| -x).collect::<Vec<_>>())?
        } else {
            vec![]
        };
        let mut w2 = w.to_vec();
        for (cam, sl) in self.slot.iter().enumerate() {
            if let Some(sl) = sl {
                let d = [dw[3 * sl], dw[3 * sl + 1], dw[3 * sl + 2]];
                w2[cam] = so3::log(&geom::mat_mul(&so3::exp(d), &so3::exp(w[cam])));
            }
        }
        let mut rho2 = rho.to_vec();
        for i in 0..self.obs.len() {
            if !active[i] || nm.hrr[i] <= 0.0 {
                continue;
            }
            let hrr = nm.hrr[i] * (1.0 + lambda) + 1e-9;
            let mut t = nm.gr[i];
            for a in 0..np {
                t += nm.hwr[i][a] * dw[a];
            }
            rho2[i] = (rho[i] - t / hrr).clamp(-0.5, 20.0);
        }
        Some((w2, rho2))
    }

    fn lm(&self, w: &mut Vec<Vec3>, rho: &mut Vec<f64>, active: &[bool]) -> usize {
        let mut lambda = 1e-4;
        let mut it = 0;
        let mut cost = self.cost_of(w, rho, active);
        while it < self.opt.max_iter {
            it += 1;
            let nm = self.normal(w, rho, active);
            let mut improved = false;
            for _ in 0..10 {
                let Some((w2, rho2)) = self.step(&nm, w, rho, lambda, active) else {
                    lambda *= 10.0;
                    continue;
                };
                let c2 = self.cost_of(&w2, &rho2, active);
                if c2 < cost {
                    let rel = (cost - c2) / cost.max(1e-12);
                    *w = w2;
                    *rho = rho2;
                    cost = c2;
                    lambda = (lambda * 0.3).max(1e-9);
                    improved = true;
                    if rel < 1e-10 {
                        return it;
                    }
                    break;
                }
                lambda *= 10.0;
            }
            if !improved {
                break;
            }
        }
        it
    }

    fn residuals(&self, w: &[Vec3], rho: &[f64]) -> Vec<[f64; 2]> {
        self.obs
            .iter()
            .enumerate()
            .map(|(i, o)| {
                let p = predict(self.rig, w, o, rho[i]);
                match project(&self.bases[i], p.u, self.opt.f_res) {
                    // measured minus predicted
                    Some((r, _)) => [-r[0], -r[1]],
                    None => [f64::NAN, f64::NAN],
                }
            })
            .collect()
    }
}

/// Optimise only the inverse depths for fixed rotations `w` (1-D Gauss-Newton per observation).
fn fit_depths(p: &Problem, w: &[Vec3], rho: &mut [f64]) {
    for (i, o) in p.obs.iter().enumerate() {
        for _ in 0..10 {
            let pr = predict(p.rig, w, o, rho[i]);
            let Some((r, d)) = project(&p.bases[i], pr.u, p.opt.f_res) else {
                break;
            };
            let j = [geom::dot(d[0], pr.du_drho), geom::dot(d[1], pr.du_drho)];
            let h = j[0] * j[0] + j[1] * j[1];
            if h < 1e-12 {
                break;
            }
            let step = (j[0] * r[0] + j[1] * r[1]) / h;
            rho[i] = (rho[i] - step).clamp(-0.5, 20.0);
            if step.abs() < 1e-10 {
                break;
            }
        }
    }
}

/// Across-epipolar residual of each observation under rotations `w` (pixels at `f_res`, measured minus predicted:
/// the sign and size of a rectified `dy`), with each observation's depth chosen to fit it best. This is how a
/// calibration is scored on observations it was not estimated from.
pub fn across_residuals(rig: &RigModel, obs: &[Obs], w: &[Vec3], f_res: f64) -> Vec<f64> {
    let opt = BaOptions {
        free: Vec::new(),
        f_res,
        ..BaOptions::default()
    };
    let bases: Vec<Basis> = obs.iter().map(|o| basis(rig, w, o)).collect();
    let prob = Problem {
        rig,
        obs,
        opt: &opt,
        bases,
        slot: vec![None; rig.r.len()],
        np: 0,
    };
    let mut rho: Vec<f64> = obs
        .iter()
        .enumerate()
        .map(|(i, o)| init_rho(rig, w, o, &prob.bases[i]))
        .collect();
    fit_depths(&prob, w, &mut rho);
    prob.residuals(w, &rho).iter().map(|r| r[1]).collect()
}

/// Run the adjustment from initial rotations `w0` (one per rig camera).
pub fn solve(rig: &RigModel, obs: &[Obs], w0: &[Vec3], opt: &BaOptions) -> BaResult {
    let ncam = rig.r.len();
    assert_eq!(w0.len(), ncam);
    let mut slot = vec![None; ncam];
    let mut np = 0;
    for &c in &opt.free {
        if c < ncam && slot[c].is_none() {
            slot[c] = Some(np / 3);
            np += 3;
        }
    }
    let bases = obs.iter().map(|o| basis(rig, w0, o)).collect();
    let prob = Problem {
        rig,
        obs,
        opt,
        bases,
        slot,
        np,
    };
    let mut w = w0.to_vec();
    let mut rho: Vec<f64> = obs
        .iter()
        .enumerate()
        .map(|(i, o)| init_rho(rig, &w, o, &prob.bases[i]))
        .collect();
    fit_depths(&prob, &w, &mut rho);
    let res_initial = prob.residuals(&w, &rho);
    let mut active: Vec<bool> = res_initial.iter().map(|r| r[1].is_finite()).collect();
    let cost0 = prob.cost_of(&w, &rho, &active);
    let mut iterations = prob.lm(&mut w, &mut rho, &active);
    // one round of outlier rejection, then re-solve
    let res1 = prob.residuals(&w, &rho);
    let mut changed = false;
    for (a, r) in active.iter_mut().zip(&res1) {
        let d = r[0].hypot(r[1]);
        let bad = d.is_nan() || d > opt.outlier_px;
        if *a && bad {
            *a = false;
            changed = true;
        }
    }
    if changed {
        iterations += prob.lm(&mut w, &mut rho, &active);
    }
    let res = prob.residuals(&w, &rho);
    let nm = prob.normal(&w, &rho, &active);
    // a-posteriori variance factor from the inlier residuals (2 per obs, 1 depth each, plus the rotations)
    let n_act = active.iter().filter(|a| **a).count();
    let ss: f64 = res
        .iter()
        .zip(&active)
        .filter(|(_, a)| **a)
        .map(|(r, _)| r[0] * r[0] + r[1] * r[1])
        .sum();
    let dof = (2 * n_act).saturating_sub(n_act + np).max(1) as f64;
    let sigma_post_px = (ss / dof).sqrt();
    let scale = (sigma_post_px / opt.sigma_px).powi(2);
    // covariance: inverse of the reduced information matrix (no damping)
    let mut red = nm.s.clone();
    for i in 0..obs.len() {
        if !active[i] || nm.hrr[i] <= 0.0 {
            continue;
        }
        for a in 0..np {
            for c in 0..np {
                red[a][c] -= nm.hwr[i][a] * nm.hwr[i][c] / (nm.hrr[i] + 1e-9);
            }
        }
    }
    let mut cov = vec![None; ncam];
    if np > 0 {
        let mut inv = vec![vec![0.0; np]; np];
        let mut ok = true;
        for c in 0..np {
            let mut e = vec![0.0; np];
            e[c] = 1.0;
            match solve_dense(&red, &e) {
                Some(x) => {
                    for r in 0..np {
                        inv[r][c] = x[r];
                    }
                }
                None => ok = false,
            }
        }
        if ok {
            for (cam, sl) in prob.slot.iter().enumerate() {
                if let Some(sl) = sl {
                    let mut m = [[0.0; 3]; 3];
                    for (r, row) in m.iter_mut().enumerate() {
                        for (c, v) in row.iter_mut().enumerate() {
                            *v = inv[3 * sl + r][3 * sl + c] * scale;
                        }
                    }
                    cov[cam] = Some(m);
                }
            }
        }
    }
    BaResult {
        cost: (cost0, prob.cost_of(&w, &rho, &active)),
        w,
        cov,
        rho,
        res,
        res_initial,
        inlier: active,
        iterations,
        sigma_post_px,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn calib() -> DeviceCalib {
        DeviceCalib::parse(include_str!(
            "../../pimax-cam/tests/fixtures/headset-calibration.xml"
        ))
        .unwrap()
    }

    /// Points around the rig seen by cameras a and b under the true rig `w_true`.
    fn synth_obs(rig: &RigModel, w_true: &[Vec3], a: usize, b: usize, noise_px: f64) -> Vec<Obs> {
        let mut s = 12345u64;
        let mut rnd = move || {
            s = s
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((s >> 11) as f64) / ((1u64 << 53) as f64) - 0.5
        };
        let ra = rig.rot(w_true, a);
        let rb = rig.rot(w_true, b);
        // mean viewing direction of the pair in camera-0 coordinates
        let za = geom::col(&geom::transpose(&ra), 2);
        let zb = geom::col(&geom::transpose(&rb), 2);
        let mid = geom::normalize(geom::add(za, zb));
        let mut out = Vec::new();
        while out.len() < 300 {
            let dir = geom::normalize(geom::add(mid, [1.4 * rnd(), 1.0 * rnd(), 1.4 * rnd()]));
            let depth = 0.6 + 4.0 * (rnd() + 0.5);
            let x0 = geom::add(
                geom::scale(geom::add(rig.c[a], rig.c[b]), 0.5),
                geom::scale(dir, depth),
            );
            let xa = geom::mat_vec(&ra, geom::sub(x0, rig.c[a]));
            let xb = geom::mat_vec(&rb, geom::sub(x0, rig.c[b]));
            if xa[2] < 0.2 * geom::norm(xa) || xb[2] < 0.2 * geom::norm(xb) {
                continue;
            }
            // pixel-level noise on b's ray (radians = px / 230)
            let nb = geom::add(
                geom::normalize(xb),
                [
                    noise_px / 230.0 * rnd() * 3.4,
                    noise_px / 230.0 * rnd() * 3.4,
                    0.0,
                ],
            );
            out.push(Obs {
                a,
                b,
                ray_a: geom::normalize(xa),
                ray_b: geom::normalize(nb),
                group: 0,
            });
        }
        out
    }

    #[test]
    fn jacobians_match_finite_differences() {
        let rig = RigModel::from_calib(&calib());
        let w = vec![
            [0.0; 3],
            [0.01, -0.02, 0.005],
            [0.003, 0.0, -0.01],
            [0.0; 3],
        ];
        let o = Obs {
            a: 2,
            b: 1,
            ray_a: geom::normalize([0.2, -0.1, 1.0]),
            ray_b: geom::normalize([-0.3, 0.05, 1.0]),
            group: 0,
        };
        let rho = 0.4;
        let p = predict(&rig, &w, &o, rho);
        let eps = 1e-7;
        for (cam, jac) in [(o.a, p.du_da), (o.b, p.du_db)] {
            for c in 0..3 {
                let mut d = [0.0; 3];
                d[c] = eps;
                let mut w2 = w.clone();
                w2[cam] = so3::log(&geom::mat_mul(&so3::exp(d), &so3::exp(w[cam])));
                let p2 = predict(&rig, &w2, &o, rho);
                for r in 0..3 {
                    let num = (p2.u[r] - p.u[r]) / eps;
                    assert!(
                        (num - jac[r][c]).abs() < 1e-5,
                        "cam {cam} c {c} r {r}: {num} vs {}",
                        jac[r][c]
                    );
                }
            }
        }
        let p3 = predict(&rig, &w, &o, rho + eps);
        for r in 0..3 {
            assert!(((p3.u[r] - p.u[r]) / eps - p.du_drho[r]).abs() < 1e-6);
        }
        // projection Jacobian
        let b = basis(&rig, &w, &o);
        let (r0, d) = project(&b, p.u, 220.0).unwrap();
        for c in 0..3 {
            let mut u2 = p.u;
            u2[c] += eps;
            let (r1, _) = project(&b, u2, 220.0).unwrap();
            for k in 0..2 {
                assert!(((r1[k] - r0[k]) / eps - d[k][c]).abs() < 1e-3);
            }
        }
    }

    #[test]
    fn recovers_a_known_rotation() {
        let rig = RigModel::from_calib(&calib());
        let truth = [0.004, -0.0025, 0.003]; // 0.31 deg
        let w_true = vec![[0.0; 3], truth, [0.0; 3], [0.0; 3]];
        let obs = synth_obs(&rig, &w_true, 0, 1, 0.2);
        let opt = BaOptions::default();
        let r = solve(&rig, &obs, &[[0.0; 3]; 4], &opt);
        let err = geom::norm(geom::sub(r.w[1], truth)).to_degrees();
        assert!(err < 0.02, "error {err} deg, w {:?}", r.w[1]);
        // the "before" residuals show the vertical error, the "after" ones do not
        let rms =
            |v: &[[f64; 2]]| (v.iter().map(|x| x[1] * x[1]).sum::<f64>() / v.len() as f64).sqrt();
        assert!(rms(&r.res_initial) > 0.5, "before {}", rms(&r.res_initial));
        assert!(rms(&r.res) < 0.3, "after {}", rms(&r.res));
        let cov = r.cov[1].unwrap();
        for i in 0..3 {
            let s = cov[i][i].sqrt().to_degrees();
            assert!(s > 0.0 && s < 0.05, "sigma {i} {s} deg");
        }
        assert!(r.cov[0].is_none());
        assert!(r.cost.1 < r.cost.0);
        // scoring a calibration on observations: large with the factory rig, noise only with the estimate
        let med = |w: &[Vec3]| {
            let mut a: Vec<f64> = across_residuals(&rig, &obs, w, 220.0)
                .iter()
                .map(|x| x.abs())
                .collect();
            a.sort_by(|p, q| p.partial_cmp(q).unwrap());
            a[a.len() / 2]
        };
        let (before, after) = (med(&[[0.0; 3]; 4]), med(&r.w));
        assert!(
            before > 0.4 && after < 0.25 && after < 0.5 * before,
            "{before} -> {after}"
        );
    }

    #[test]
    fn all_pairs_jointly_and_outliers() {
        let rig = RigModel::from_calib(&calib());
        let w_true = vec![
            [0.0; 3],
            [0.003, 0.001, -0.002],
            [-0.004, 0.002, 0.001],
            [0.001, -0.003, 0.002],
        ];
        let mut obs = Vec::new();
        for (a, b) in [(0, 1), (0, 2), (1, 3), (2, 3)] {
            obs.extend(synth_obs(&rig, &w_true, a, b, 0.2));
        }
        // 5 % gross outliers, ~11 px across the epipolar line (an error *along* the line is only a wrong depth
        // and cannot be detected by two views)
        let mut bad = vec![false; obs.len()];
        for (i, o) in obs.iter_mut().enumerate() {
            if i % 20 == 0 {
                let e2 = basis(&rig, &w_true, o).e2;
                o.ray_b = geom::normalize(geom::add(o.ray_b, geom::scale(e2, 0.05)));
                bad[i] = true;
            }
        }
        let opt = BaOptions {
            free: vec![1, 2, 3],
            ..BaOptions::default()
        };
        let r = solve(&rig, &obs, &[[0.0; 3]; 4], &opt);
        for c in 1..4 {
            let err = geom::norm(geom::sub(r.w[c], w_true[c])).to_degrees();
            assert!(err < 0.03, "camera {c}: error {err} deg");
        }
        let caught = bad
            .iter()
            .zip(&r.inlier)
            .filter(|(b, i)| **b && !**i)
            .count();
        let false_alarms = bad
            .iter()
            .zip(&r.inlier)
            .filter(|(b, i)| !**b && !**i)
            .count();
        let n_bad = bad.iter().filter(|b| **b).count();
        assert_eq!(caught, n_bad, "outliers caught {caught} of {n_bad}");
        assert!(
            false_alarms * 100 <= obs.len(),
            "{false_alarms} good observations rejected"
        );
    }

    #[test]
    fn dense_solver() {
        let a = vec![
            vec![4.0, 1.0, 0.0],
            vec![1.0, 3.0, 1.0],
            vec![0.0, 1.0, 2.0],
        ];
        let x = solve_dense(&a, &[1.0, 2.0, 3.0]).unwrap();
        for (r, row) in a.iter().enumerate() {
            let s: f64 = row.iter().zip(&x).map(|(p, q)| p * q).sum();
            assert!((s - [1.0, 2.0, 3.0][r]).abs() < 1e-12);
        }
        assert!(solve_dense(&[vec![0.0, 0.0], vec![0.0, 0.0]], &[1.0, 1.0]).is_none());
    }
}
