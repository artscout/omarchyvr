//! The calibration pipeline on recorded frames:
//!
//! 1. rectify every camera pair of every scene with the current calibration ([`crate::rect`]) and match it
//!    ([`crate::matcher`]); the matches become ray pairs in the two cameras' own frames;
//! 2. solve the rotation-only bundle adjustment over all pairs and scenes at once ([`crate::ba`]);
//! 3. repeat with the corrected calibration (better rectification gives better matches), then match once more.
//!
//! "Before" and "after" are both **measured** by matching images rectified with the respective calibration; they
//! are not the BA's model residuals. [`holdout`] measures the correction on scenes it was not estimated from.

use crate::ba::{self, BaOptions, BaResult, Obs, RigModel};
use crate::diag;
use crate::matcher::{self, MatchConfig, MatchSet};
use crate::rect::{self, RectGeom, RectImage};
use crate::scene::Scene;
use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::frame::{TILE_H, TILE_W};
use pimax_cam::geom::{self, Mat3, Vec3};
use pimax_cam::overlay::{CalibOverlay, CameraRotation};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

/// Geometry of the rectified pairs (the reported `dy` is in these pixels). The defaults equal
/// `pimax-cam rectify` / `hypotheses`, so the numbers are comparable.
#[derive(Clone, Copy, Debug)]
pub struct RectParams {
    /// Width.
    pub width: usize,
    /// Height.
    pub height: usize,
    /// Focal length (pixels).
    pub focal: f64,
    /// Usable fisheye radius around the principal point (pixels).
    pub mask_radius: f64,
}

impl Default for RectParams {
    fn default() -> Self {
        RectParams {
            width: 640,
            height: 480,
            focal: 220.0,
            mask_radius: rect::DEFAULT_MASK_RADIUS_PX,
        }
    }
}

/// Matches of one camera pair in one scene.
#[derive(Clone, Debug)]
pub struct PairMatches {
    /// Scene index.
    pub scene: usize,
    /// Left camera.
    pub a: usize,
    /// Right camera.
    pub b: usize,
    /// Rectification used.
    pub geom: RectGeom,
    /// Matches.
    pub set: MatchSet,
    /// Rectified images (8-bit, invalid pixels 0), kept on request for pictures.
    pub images: Option<(Vec<u8>, Vec<u8>)>,
}

impl PairMatches {
    /// Vertical-disparity summary.
    pub fn dy_stats(&self) -> DyStats {
        DyStats::of(&self.dys())
    }

    /// All `dy` values.
    pub fn dys(&self) -> Vec<f64> {
        self.set.matches.iter().map(|m| m.dy).collect()
    }
}

/// Summary of vertical disparities `dy` (rectified pixels; 0 for a perfect calibration).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DyStats {
    /// Matches.
    pub n: usize,
    /// Median `dy` (signed).
    pub median: f64,
    /// Median `|dy|`.
    pub median_abs: f64,
    /// Root mean square.
    pub rms: f64,
    /// Fraction with `|dy| <= 0.5 px`.
    pub within_05: f64,
    /// Fraction with `|dy| <= 1 px`.
    pub within_1: f64,
    /// Fraction with `|dy| <= 2 px`.
    pub within_2: f64,
}

fn median_of(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let n = v.len();
    if n % 2 == 1 {
        v[n / 2]
    } else {
        (v[n / 2 - 1] + v[n / 2]) / 2.0
    }
}

impl DyStats {
    /// Statistics of a set of `dy`.
    pub fn of(dy: &[f64]) -> DyStats {
        let n = dy.len();
        if n == 0 {
            return DyStats::default();
        }
        let frac = |t: f64| dy.iter().filter(|d| d.abs() <= t).count() as f64 / n as f64;
        let mut s = dy.to_vec();
        let mut a: Vec<f64> = dy.iter().map(|d| d.abs()).collect();
        DyStats {
            n,
            median: median_of(&mut s),
            median_abs: median_of(&mut a),
            rms: (dy.iter().map(|d| d * d).sum::<f64>() / n as f64).sqrt(),
            within_05: frac(0.5),
            within_1: frac(1.0),
            within_2: frac(2.0),
        }
    }

    /// One line for reports.
    pub fn line(&self) -> String {
        if self.n == 0 {
            return "no matches".into();
        }
        format!(
            "{:4} matches, median dy {:+.2} px, median |dy| {:.2}, rms {:.2}, |dy|<=0.5 {:3.0}%, <=1 {:3.0}%, <=2 {:3.0}%",
            self.n,
            self.median,
            self.median_abs,
            self.rms,
            self.within_05 * 100.0,
            self.within_1 * 100.0,
            self.within_2 * 100.0
        )
    }
}

/// Rectify cameras `a` (left) and `b` of a scene with calibration `cal` (tile index = camera id).
pub fn rectify_pair(
    cal: &DeviceCalib,
    scene: &Scene,
    a: usize,
    b: usize,
    rp: &RectParams,
) -> (RectGeom, RectImage, RectImage) {
    let pose = cal.stereo(a, b, RigConvention::CamFromCam0);
    let g = RectGeom::new(&pose, rp.width, rp.height, rp.focal);
    let one = |which: usize, cam: usize| {
        rect::rectify_one(
            &g,
            which,
            &cal.cameras[cam].intr,
            &scene.tiles[cam],
            TILE_W,
            TILE_H,
            rp.mask_radius,
        )
    };
    let (l, r) = (one(0, a), one(1, b));
    (g, l, r)
}

/// Rectify and match one pair of one scene.
#[allow(clippy::too_many_arguments)]
pub fn match_scene_pair(
    cal: &DeviceCalib,
    scene_index: usize,
    scene: &Scene,
    a: usize,
    b: usize,
    rp: &RectParams,
    mc: &MatchConfig,
    keep_images: bool,
) -> PairMatches {
    let (g, l, r) = rectify_pair(cal, scene, a, b, rp);
    let set = matcher::match_pair(&g, &l, &r, mc);
    PairMatches {
        scene: scene_index,
        a,
        b,
        geom: g,
        set,
        images: keep_images.then(|| (l.to_u8(), r.to_u8())),
    }
}

/// Run `f` on every job index in parallel (all cores), results in job order.
fn par_map<T: Send, F: Fn(usize) -> T + Sync>(n: usize, f: F) -> Vec<T> {
    let threads = std::thread::available_parallelism()
        .map(|t| t.get())
        .unwrap_or(4)
        .min(n)
        .max(1);
    let next = AtomicUsize::new(0);
    let out: Mutex<Vec<(usize, T)>> = Mutex::new(Vec::with_capacity(n));
    std::thread::scope(|s| {
        for _ in 0..threads {
            s.spawn(|| loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= n {
                    break;
                }
                let r = f(i);
                out.lock().unwrap().push((i, r));
            });
        }
    });
    let mut v = out.into_inner().unwrap();
    v.sort_by_key(|x| x.0);
    v.into_iter().map(|x| x.1).collect()
}

/// Match every pair of every scene (in parallel). `scene_base` is added to the scene indices.
pub fn match_all(
    cal: &DeviceCalib,
    scenes: &[Scene],
    scene_base: usize,
    pairs: &[(usize, usize)],
    rp: &RectParams,
    mc: &MatchConfig,
    keep_images: bool,
) -> Vec<PairMatches> {
    let jobs: Vec<(usize, usize, usize)> = (0..scenes.len())
        .flat_map(|s| pairs.iter().map(move |&(a, b)| (s, a, b)))
        .collect();
    par_map(jobs.len(), |i| {
        let (s, a, b) = jobs[i];
        match_scene_pair(cal, scene_base + s, &scenes[s], a, b, rp, mc, keep_images)
    })
}

/// BA observations of a set of pair matches (`group` = position in the iteration).
pub fn observations<'a>(pm: impl IntoIterator<Item = &'a PairMatches>) -> Vec<Obs> {
    pm.into_iter()
        .enumerate()
        .flat_map(|(g, p)| {
            p.set.matches.iter().map(move |m| Obs {
                a: p.a,
                b: p.b,
                ray_a: m.ray_a,
                ray_b: m.ray_b,
                group: g,
            })
        })
        .collect()
}

/// The factory calibration with rotation corrections `w` (one per camera, radians, overlay semantics:
/// `R_i' = Exp(w_i) R_i` about the camera centre).
pub fn corrected(factory: &DeviceCalib, w: &[Vec3]) -> DeviceCalib {
    let mut ov = CalibOverlay::for_calib(factory, "pimax-selfcal");
    ov.rotations = w
        .iter()
        .enumerate()
        .filter(|(_, x)| geom::norm(**x) > 0.0)
        .map(|(id, x)| CameraRotation {
            id,
            rotation_vector: *x,
            sigma: None,
        })
        .collect();
    factory
        .with_overlay(&ov, 180.0)
        .expect("an overlay made for this calibration applies")
}

/// Cameras connected to `reference` through `pairs`, without the reference itself (the cameras a BA can
/// estimate when the reference is held fixed).
pub fn connected_free(pairs: &[(usize, usize)], reference: usize, n: usize) -> Vec<usize> {
    let mut seen = vec![false; n];
    if reference < n {
        seen[reference] = true;
    }
    loop {
        let mut grew = false;
        for &(a, b) in pairs {
            if a < n && b < n && seen[a] != seen[b] {
                seen[a] = true;
                seen[b] = true;
                grew = true;
            }
        }
        if !grew {
            break;
        }
    }
    (0..n).filter(|&c| seen[c] && c != reference).collect()
}

/// Parse `"0,1;0,2"` (or `all` = every pair of `n` cameras).
pub fn parse_pairs(s: &str, n: usize) -> Result<Vec<(usize, usize)>, String> {
    if s == "all" {
        return Ok((0..n)
            .flat_map(|a| (a + 1..n).map(move |b| (a, b)))
            .collect());
    }
    let mut out = Vec::new();
    for p in s.split(';').filter(|p| !p.trim().is_empty()) {
        let (a, b) = p
            .split_once(',')
            .ok_or_else(|| format!("bad pair {p:?} (want A,B)"))?;
        let (a, b): (usize, usize) = (
            a.trim().parse().map_err(|_| format!("bad camera {a:?}"))?,
            b.trim().parse().map_err(|_| format!("bad camera {b:?}"))?,
        );
        if a == b || a >= n || b >= n {
            return Err(format!("bad pair {a},{b} for {n} cameras"));
        }
        out.push((a, b));
    }
    if out.is_empty() {
        return Err("no pairs".into());
    }
    Ok(out)
}

/// Options of [`estimate`].
#[derive(Clone, Debug)]
pub struct EstimateOptions {
    /// Camera pairs `(left, right)` to match.
    pub pairs: Vec<(usize, usize)>,
    /// Camera held fixed (the gauge; the rig's overall rotation is not visible in camera-to-camera matches).
    pub reference: usize,
    /// Cameras to estimate; `None` = all cameras connected to the reference through `pairs`.
    pub free: Option<Vec<usize>>,
    /// Match + BA rounds (the first on the starting calibration, later ones on the corrected one).
    pub rounds: usize,
    /// Rectification.
    pub rect: RectParams,
    /// Matcher.
    pub matcher: MatchConfig,
    /// Bundle adjustment (its `free` and `f_res` are set from the fields above).
    pub ba: BaOptions,
    /// Keep the rectified images of the before/after matching (for pictures).
    pub keep_images: bool,
    /// A pair feeds the bundle adjustment only if a rotation brings its vertical disparities down to this
    /// robust scale (pixels) and its diagnosis is usable ([`diag::Verdict::usable`]). A pair that no rotation
    /// explains must not drag the cameras it shares with good pairs.
    pub accept_sigma_px: f64,
    /// Use every pair regardless of its diagnosis.
    pub accept_all: bool,
    /// Folds of the spatial cross-validation (0 = off).
    pub cv_folds: usize,
    /// A scene's matches of a pair feed the adjustment only if there are at least this many: a handful of
    /// matches in a frame are more often mismatches than signal, and they sit at positions with a lot of
    /// leverage on the weakly determined rotation components.
    pub min_scene_matches: usize,
}

impl Default for EstimateOptions {
    fn default() -> Self {
        EstimateOptions {
            pairs: vec![(0, 1), (0, 2), (1, 3), (2, 3)],
            reference: 0,
            free: None,
            rounds: 2,
            rect: RectParams::default(),
            matcher: MatchConfig::default(),
            ba: BaOptions::default(),
            keep_images: false,
            accept_sigma_px: 1.2,
            accept_all: false,
            cv_folds: 5,
            min_scene_matches: 8,
        }
    }
}

/// Diagnosis of one camera pair on the starting calibration (all scenes pooled).
#[derive(Clone, Debug)]
pub struct PairDiag {
    /// Left camera.
    pub a: usize,
    /// Right camera.
    pub b: usize,
    /// Matches.
    pub n: usize,
    /// Model fits ([`diag::compare`]).
    pub fits: Vec<diag::Fit>,
    /// What they say.
    pub verdict: diag::Verdict,
    /// Fraction of matches near the vertical search limit ([`diag::band_saturation`]).
    pub saturation: f64,
    /// Whether the bundle adjustment uses the pair.
    pub accepted: bool,
    /// Why not (empty when accepted).
    pub why: String,
}

impl PairDiag {
    /// Robust residual scale of the rotation model (pixels), if it could be fitted.
    pub fn rotation_sigma(&self) -> Option<f64> {
        self.fits
            .iter()
            .find(|f| f.model == diag::Model::Rotation)
            .map(|f| f.sigma)
    }
}

/// Diagnose every pair in `pm` and decide which ones a rotation-only adjustment may use.
pub fn diagnose_pairs(
    pm: &[PairMatches],
    focal: f64,
    band: f64,
    accept_sigma_px: f64,
) -> Vec<PairDiag> {
    pairs_of(pm)
        .into_iter()
        .map(|(a, b)| {
            let smp = pair_samples(pm, a, b);
            let fits = diag::compare(&smp, focal);
            let verdict = diag::verdict(&fits, DIAG_NOISE_PX);
            let mut d = PairDiag {
                a,
                b,
                n: smp.len(),
                saturation: diag::band_saturation(&smp, band),
                fits,
                verdict,
                accepted: false,
                why: String::new(),
            };
            match (verdict.usable(), d.rotation_sigma()) {
                (false, _) => d.why = verdict.text().to_string(),
                (true, Some(sg)) if sg <= accept_sigma_px => d.accepted = true,
                (true, Some(sg)) => {
                    d.why = format!(
                        "a rotation leaves {sg:.2} px of scatter (limit {accept_sigma_px} px)"
                    )
                }
                (true, None) => d.why = "no rotation fit".into(),
            }
            d
        })
        .collect()
}

/// Spatial cross-validation of an estimate: the matches are split into folds by image block (96 px), each fold
/// is scored with rotations estimated from the other folds.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CrossVal {
    /// Folds that could be evaluated.
    pub folds: usize,
    /// Held-out across-epipolar residuals with the starting calibration.
    pub before: DyStats,
    /// The same observations with the rotations estimated without them.
    pub after: DyStats,
}

/// One match + BA round.
#[derive(Clone, Debug)]
pub struct Round {
    /// Corrections after the round.
    pub w: Vec<Vec3>,
    /// Observations used.
    pub n_obs: usize,
    /// Inlier observations through each camera.
    pub inliers_per_camera: Vec<usize>,
    /// The BA result.
    pub ba: BaResult,
}

/// Result of [`estimate`].
#[derive(Clone, Debug)]
pub struct Estimate {
    /// Cameras estimated.
    pub free: Vec<usize>,
    /// Reference camera.
    pub reference: usize,
    /// Starting corrections.
    pub w0: Vec<Vec3>,
    /// Final corrections (radians, per camera, relative to the factory rig).
    pub w: Vec<Vec3>,
    /// Their covariance (rad^2), for the free cameras.
    pub cov: Vec<Option<Mat3>>,
    /// The rounds.
    pub rounds: Vec<Round>,
    /// Matches on the starting calibration.
    pub before: Vec<PairMatches>,
    /// Matches on the corrected calibration.
    pub after: Vec<PairMatches>,
    /// Diagnosis of every requested pair on the starting calibration.
    pub pairs: Vec<PairDiag>,
    /// The pairs the bundle adjustment used.
    pub used_pairs: Vec<(usize, usize)>,
    /// Cameras that appear in the requested pairs but could not be estimated (no accepted pair connects them
    /// to the reference).
    pub not_estimated: Vec<usize>,
    /// Spatial cross-validation (when enabled and there are enough matches).
    pub cv: Option<CrossVal>,
}

impl Estimate {
    /// One-sigma uncertainty of each component of camera `c`'s correction (radians).
    pub fn sigma(&self, c: usize) -> Option<Vec3> {
        self.cov[c].map(|m| [m[0][0].sqrt(), m[1][1].sqrt(), m[2][2].sqrt()])
    }

    /// Largest one-sigma axis of camera `c`'s correction (degrees).
    pub fn sigma_max_deg(&self, c: usize) -> Option<f64> {
        self.sigma(c)
            .map(|s| s.iter().copied().fold(0.0, f64::max).to_degrees())
    }

    /// Inlier observations through camera `c` in the last round.
    pub fn inliers_of(&self, c: usize) -> usize {
        self.rounds
            .last()
            .and_then(|r| r.inliers_per_camera.get(c).copied())
            .unwrap_or(0)
    }
}

/// Inlier observations through each of `n` cameras.
fn inliers_per_camera(obs: &[Obs], inlier: &[bool], n: usize) -> Vec<usize> {
    let mut v = vec![0; n];
    for (o, &i) in obs.iter().zip(inlier) {
        if i {
            v[o.a] += 1;
            v[o.b] += 1;
        }
    }
    v
}

/// Estimate rotation corrections from `scenes`, starting from corrections `w0` (zero = the factory calibration;
/// an earlier overlay to refine it).
pub fn estimate(
    factory: &DeviceCalib,
    w0: &[Vec3],
    scenes: &[Scene],
    opt: &EstimateOptions,
) -> Result<Estimate, String> {
    let n = factory.cameras.len();
    if w0.len() != n {
        return Err(format!("{} initial corrections for {n} cameras", w0.len()));
    }
    if scenes.is_empty() {
        return Err("no scenes".into());
    }
    if let Some(&(a, b)) = opt.pairs.iter().find(|(a, b)| a == b || *a >= n || *b >= n) {
        return Err(format!("bad pair {a},{b}"));
    }
    let before = match_all(
        &corrected(factory, w0),
        scenes,
        0,
        &opt.pairs,
        &opt.rect,
        &opt.matcher,
        opt.keep_images,
    );
    let mut pairs = diagnose_pairs(
        &before,
        opt.rect.focal,
        opt.matcher.dy_search as f64,
        opt.accept_sigma_px,
    );
    if opt.accept_all {
        for p in pairs.iter_mut() {
            p.accepted = true;
            p.why.clear();
        }
    }
    let accepted: Vec<(usize, usize)> = pairs
        .iter()
        .filter(|p| p.accepted)
        .map(|p| (p.a, p.b))
        .collect();
    let reachable = connected_free(&accepted, opt.reference, n);
    // pairs between cameras that no accepted pair ties to the reference would only add constant residuals
    let tied = |c: usize| c == opt.reference || reachable.contains(&c);
    let used_pairs: Vec<(usize, usize)> = accepted
        .into_iter()
        .filter(|&(a, b)| tied(a) && tied(b))
        .collect();
    let free: Vec<usize> = match &opt.free {
        Some(f) => f
            .iter()
            .copied()
            .filter(|c| reachable.contains(c))
            .collect(),
        None => reachable,
    };
    if free.is_empty() {
        let why: Vec<String> = pairs
            .iter()
            .map(|p| {
                format!(
                    "pair {}-{}: {} matches, {}",
                    p.a,
                    p.b,
                    p.n,
                    if p.accepted { "usable" } else { p.why.as_str() }
                )
            })
            .collect();
        return Err(format!(
            "no camera can be estimated: no usable pair connects one to reference camera {} ({})",
            opt.reference,
            why.join("; ")
        ));
    }
    let rig = RigModel::from_calib(factory);
    let ba_opt = BaOptions {
        free: free.clone(),
        f_res: opt.rect.focal,
        ..opt.ba.clone()
    };
    let used = |p: &&PairMatches| {
        used_pairs.contains(&(p.a, p.b)) && p.set.matches.len() >= opt.min_scene_matches
    };
    let mut w = w0.to_vec();
    let mut rounds = Vec::new();
    for it in 0..opt.rounds.max(1) {
        let fresh;
        let pm = if it == 0 {
            &before
        } else {
            fresh = match_all(
                &corrected(factory, &w),
                scenes,
                0,
                &used_pairs,
                &opt.rect,
                &opt.matcher,
                false,
            );
            &fresh
        };
        let obs = observations(pm.iter().filter(used));
        if obs.len() < 20 {
            return Err(format!(
                "only {} matches in round {} (light, texture, overlap?)",
                obs.len(),
                it + 1
            ));
        }
        let r = ba::solve(&rig, &obs, &w, &ba_opt);
        w = r.w.clone();
        rounds.push(Round {
            w: w.clone(),
            n_obs: obs.len(),
            inliers_per_camera: inliers_per_camera(&obs, &r.inlier, n),
            ba: r,
        });
    }
    let after = match_all(
        &corrected(factory, &w),
        scenes,
        0,
        &opt.pairs,
        &opt.rect,
        &opt.matcher,
        opt.keep_images,
    );
    let cov = rounds.last().map(|r| r.ba.cov.clone()).unwrap_or_default();
    let cv = (opt.cv_folds >= 2)
        .then(|| crossval(&rig, w0, before.iter().filter(used), &ba_opt, opt.cv_folds))
        .flatten();
    let mut not_estimated: Vec<usize> = opt
        .pairs
        .iter()
        .flat_map(|&(a, b)| [a, b])
        .filter(|c| *c != opt.reference && !free.contains(c))
        .collect();
    not_estimated.sort();
    not_estimated.dedup();
    Ok(Estimate {
        free,
        reference: opt.reference,
        w0: w0.to_vec(),
        w,
        cov,
        rounds,
        before,
        after,
        pairs,
        used_pairs,
        not_estimated,
        cv,
    })
}

/// Fold of a match: by scene, pair and 96-pixel image block, so that neighbouring matches (which share scene
/// structure and local calibration error) stay together.
fn fold_of(p: &PairMatches, m: &crate::matcher::StereoMatch, folds: usize) -> usize {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for v in [
        p.scene as u64,
        p.a as u64,
        p.b as u64,
        (m.x / 96.0) as u64,
        (m.y / 96.0) as u64,
    ] {
        h = (h ^ v).wrapping_mul(0x0000_0100_0000_01b3);
        h ^= h >> 29;
    }
    (h % folds as u64) as usize
}

/// Spatial cross-validation: for each fold, estimate from the other folds (one BA from `w0`) and score the
/// held-out matches with [`ba::across_residuals`] (angular error times the focal length: off-axis it is smaller
/// than the rectified `dy` of the same match). `None` with fewer than 60 matches or fewer than 2 usable folds.
pub fn crossval<'a>(
    rig: &RigModel,
    w0: &[Vec3],
    pm: impl IntoIterator<Item = &'a PairMatches>,
    ba_opt: &BaOptions,
    folds: usize,
) -> Option<CrossVal> {
    let pm: Vec<&PairMatches> = pm.into_iter().collect();
    let obs = observations(pm.iter().copied());
    let fold: Vec<usize> = pm
        .iter()
        .flat_map(|p| p.set.matches.iter().map(move |m| fold_of(p, m, folds)))
        .collect();
    if obs.len() < 60 {
        return None;
    }
    let (mut before, mut after, mut done) = (Vec::new(), Vec::new(), 0);
    for k in 0..folds {
        let train: Vec<Obs> = obs
            .iter()
            .zip(&fold)
            .filter(|(_, f)| **f != k)
            .map(|(o, _)| *o)
            .collect();
        let test: Vec<Obs> = obs
            .iter()
            .zip(&fold)
            .filter(|(_, f)| **f == k)
            .map(|(o, _)| *o)
            .collect();
        if train.len() < 30 || test.len() < 3 {
            continue;
        }
        let r = ba::solve(rig, &train, w0, ba_opt);
        before.extend(ba::across_residuals(rig, &test, w0, ba_opt.f_res));
        after.extend(ba::across_residuals(rig, &test, &r.w, ba_opt.f_res));
        done += 1;
    }
    let finite = |v: Vec<f64>| -> Vec<f64> { v.into_iter().filter(|x| x.is_finite()).collect() };
    (done >= 2).then(|| CrossVal {
        folds: done,
        before: DyStats::of(&finite(before)),
        after: DyStats::of(&finite(after)),
    })
}

/// Index of the scene with the most matches for pair `(a, b)` in `pm`.
pub fn richest_scene(pm: &[PairMatches], a: usize, b: usize) -> Option<usize> {
    pm.iter()
        .filter(|p| p.a == a && p.b == b && !p.set.matches.is_empty())
        .max_by_key(|p| p.set.matches.len())
        .map(|p| p.scene)
}

/// The matches of pair `(a, b)` in scene `scene`.
pub fn find(pm: &[PairMatches], scene: usize, a: usize, b: usize) -> Option<&PairMatches> {
    pm.iter().find(|p| p.scene == scene && p.a == a && p.b == b)
}

/// Pool the `dy` of all pair matches of camera pair `(a, b)` (all scenes) into one summary.
pub fn pooled(pm: &[PairMatches], a: usize, b: usize) -> DyStats {
    let dy: Vec<f64> = pm
        .iter()
        .filter(|p| p.a == a && p.b == b)
        .flat_map(|p| p.dys())
        .collect();
    DyStats::of(&dy)
}

/// `dy` summaries of camera pair `(a, b)` split by triangulated depth: `(far, near)` with far = at least
/// `depth_m` metres away. A rotation error shows in both; a baseline (translation) error mostly in the near part.
pub fn depth_split(pm: &[PairMatches], a: usize, b: usize, depth_m: f64) -> (DyStats, DyStats) {
    let (mut far, mut near) = (Vec::new(), Vec::new());
    for p in pm.iter().filter(|p| p.a == a && p.b == b) {
        let limit = p.geom.focal * p.geom.baseline / depth_m;
        for m in &p.set.matches {
            if m.disp <= limit {
                far.push(m.dy);
            } else {
                near.push(m.dy);
            }
        }
    }
    (DyStats::of(&far), DyStats::of(&near))
}

/// Pool every `dy` of every pair.
pub fn pooled_all(pm: &[PairMatches]) -> DyStats {
    DyStats::of(&pm.iter().flat_map(|p| p.dys()).collect::<Vec<_>>())
}

/// The camera pairs present in a set of matches, sorted.
pub fn pairs_of(pm: &[PairMatches]) -> Vec<(usize, usize)> {
    let mut pairs: Vec<(usize, usize)> = pm.iter().map(|p| (p.a, p.b)).collect();
    pairs.sort();
    pairs.dedup();
    pairs
}

/// The matches of camera pair `(a, b)` over all scenes as samples for [`crate::diag`].
pub fn pair_samples(pm: &[PairMatches], a: usize, b: usize) -> Vec<diag::Sample> {
    pm.iter()
        .filter(|p| p.a == a && p.b == b)
        .flat_map(|p| diag::samples(&p.geom, &p.set.matches))
        .collect()
}

/// Depth (metres) that separates "far" from "near" matches in the reports.
pub const DEPTH_SPLIT_M: f64 = 1.0;

/// Matcher noise assumed by the diagnosis verdicts (pixels): what the best real pair shows after correction.
pub const DIAG_NOISE_PX: f64 = 0.5;

/// For every pair: which error model explains its vertical disparities ([`crate::diag`]), as text.
pub fn diagnose(pm: &[PairMatches], focal: f64) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    for (a, b) in pairs_of(pm) {
        let smp = pair_samples(pm, a, b);
        let fits = diag::compare(&smp, focal);
        let _ = writeln!(
            s,
            "  pair {a}-{b}: {} matches -> {}",
            smp.len(),
            diag::verdict(&fits, DIAG_NOISE_PX).text()
        );
        s += &diag::table(&fits);
    }
    s
}

/// Leave-one-scene-out result.
#[derive(Clone, Debug)]
pub struct Holdout {
    /// The scene left out.
    pub scene: usize,
    /// Corrections estimated without it.
    pub w: Vec<Vec3>,
    /// Its matches on the starting calibration.
    pub before: Vec<PairMatches>,
    /// Its matches on the calibration estimated from the other scenes.
    pub after: Vec<PairMatches>,
}

/// For each scene: estimate from the other scenes' matches (one BA round on the starting calibration, the
/// pairs the estimate used) and measure the left-out scene before and after. Scenes without usable matches of
/// their own, or whose complement has fewer than 50 matches, are skipped.
pub fn holdout(
    factory: &DeviceCalib,
    scenes: &[Scene],
    est: &Estimate,
    opt: &EstimateOptions,
) -> Vec<Holdout> {
    let rig = RigModel::from_calib(factory);
    let ba_opt = BaOptions {
        free: est.free.clone(),
        f_res: opt.rect.focal,
        ..opt.ba.clone()
    };
    let used = |p: &&PairMatches| {
        est.used_pairs.contains(&(p.a, p.b)) && p.set.matches.len() >= opt.min_scene_matches
    };
    let mut out = Vec::new();
    for k in 0..scenes.len() {
        let own = est
            .before
            .iter()
            .filter(used)
            .filter(|p| p.scene == k)
            .count();
        let obs = observations(est.before.iter().filter(used).filter(|p| p.scene != k));
        if own == 0 || obs.len() < 50 {
            continue;
        }
        let r = ba::solve(&rig, &obs, &est.w0, &ba_opt);
        let after = match_all(
            &corrected(factory, &r.w),
            &scenes[k..k + 1],
            k,
            &est.used_pairs,
            &opt.rect,
            &opt.matcher,
            false,
        );
        out.push(Holdout {
            scene: k,
            w: r.w,
            before: est
                .before
                .iter()
                .filter(used)
                .filter(|p| p.scene == k)
                .cloned()
                .collect(),
            after,
        });
    }
    out
}

/// Rotation of camera `c`'s correction expressed in the body (IMU / OpenXR head) frame, degrees:
/// `[pitch about +x (right), yaw about +y (up), roll about +z (back)]`. The camera turns by this rotation
/// relative to the head. `None` without the camera-IMU extrinsic.
pub fn body_angles_deg(factory: &DeviceCalib, w: Vec3, c: usize) -> Option<Vec3> {
    // body_from_cam' = body_from_cam * Exp(-w): in body coordinates the camera is rotated by Exp(-R_bc w)
    let r_bc = factory.body_from_cam(c, RigConvention::CamFromCam0)?.r;
    let v = geom::mat_vec(&r_bc, w);
    Some([-v[0].to_degrees(), -v[1].to_degrees(), -v[2].to_degrees()])
}

/// Sanity gate before an estimate may become the learned calibration.
#[derive(Clone, Debug, PartialEq)]
pub struct GateLimits {
    /// Largest correction per camera (degrees).
    pub max_angle_deg: f64,
    /// Largest one-sigma axis per camera (degrees).
    pub max_sigma_deg: f64,
    /// Fewest inlier observations through each free camera.
    pub min_obs: usize,
    /// Largest a-posteriori residual of the bundle adjustment (pixels): above it the rotation model does not
    /// describe the matches.
    pub max_residual_px: f64,
}

impl Default for GateLimits {
    fn default() -> Self {
        GateLimits {
            max_angle_deg: 2.0,
            max_sigma_deg: 0.05,
            min_obs: 100,
            max_residual_px: 0.8,
        }
    }
}

/// Check an estimate. Returns the reasons it must not be used (empty = OK).
pub fn gate(est: &Estimate, lim: &GateLimits) -> Vec<String> {
    let mut why = Vec::new();
    for &c in &est.free {
        let ang = geom::norm(est.w[c]).to_degrees();
        if ang > lim.max_angle_deg {
            why.push(format!(
                "camera {c}: correction {ang:.2} deg > {} deg (mechanical damage? wrong data?)",
                lim.max_angle_deg
            ));
        }
        match est.sigma_max_deg(c) {
            Some(s) if s <= lim.max_sigma_deg => {}
            Some(s) => why.push(format!(
                "camera {c}: uncertainty {s:.3} deg > {} deg (too few or badly spread matches)",
                lim.max_sigma_deg
            )),
            None => why.push(format!("camera {c}: no covariance (degenerate problem)")),
        }
        let n = est.inliers_of(c);
        if n < lim.min_obs {
            why.push(format!(
                "camera {c}: {n} inlier observations < {}",
                lim.min_obs
            ));
        }
    }
    if let Some(r) = est.rounds.last() {
        if r.ba.sigma_post_px > lim.max_residual_px {
            why.push(format!(
                "residual {:.2} px > {} px: rotations do not describe the matches",
                r.ba.sigma_post_px, lim.max_residual_px
            ));
        }
    }
    for p in &est.pairs {
        let touches = est.free.contains(&p.a) || est.free.contains(&p.b);
        if !touches || !est.used_pairs.contains(&(p.a, p.b)) {
            continue;
        }
        if matches!(
            p.verdict,
            diag::Verdict::Ambiguous | diag::Verdict::RotationAndTranslation
        ) {
            why.push(format!("pair {}-{}: {}", p.a, p.b, p.verdict.text()));
        }
        if p.saturation > 0.25 {
            why.push(format!(
                "pair {}-{}: {:.0} % of the matches sit at the vertical search limit (the error is larger than the search range)",
                p.a,
                p.b,
                p.saturation * 100.0
            ));
        }
    }
    let (b, a) = (used_stats(est, &est.before), used_stats(est, &est.after));
    if a.n < 20 || a.within_1 < b.within_1 || a.median_abs > b.median_abs {
        why.push(format!(
            "re-matched pairs did not improve: before [{}], after [{}]",
            b.line(),
            a.line()
        ));
    }
    if let Some(cv) = &est.cv {
        if cv.after.median_abs > cv.before.median_abs {
            why.push(format!(
                "cross-validation: held-out matches got worse ({:.2} -> {:.2} px median |dy|)",
                cv.before.median_abs, cv.after.median_abs
            ));
        }
    }
    why
}

/// Vertical-disparity summary over the pairs the estimate used.
pub fn used_stats(est: &Estimate, pm: &[PairMatches]) -> DyStats {
    DyStats::of(
        &pm.iter()
            .filter(|p| est.used_pairs.contains(&(p.a, p.b)))
            .flat_map(|p| p.dys())
            .collect::<Vec<_>>(),
    )
}

/// The overlay for an estimate (free cameras only; their sigma from the covariance) with quality figures.
pub fn to_overlay(factory: &DeviceCalib, est: &Estimate, tool: &str, note: &str) -> CalibOverlay {
    let mut ov = CalibOverlay::for_calib(factory, tool);
    ov.reference_camera = est.reference;
    for &c in &est.free {
        ov.rotations.push(CameraRotation {
            id: c,
            rotation_vector: est.w[c],
            sigma: est.sigma(c),
        });
    }
    let (b, a) = (used_stats(est, &est.before), used_stats(est, &est.after));
    let r3 = |x: f64| (x * 1000.0).round() / 1000.0;
    ov.quality = vec![
        ("matches_before".into(), b.n as f64),
        ("matches_after".into(), a.n as f64),
        ("median_abs_dy_before_px".into(), r3(b.median_abs)),
        ("median_abs_dy_after_px".into(), r3(a.median_abs)),
        ("within_1px_before".into(), r3(b.within_1)),
        ("within_1px_after".into(), r3(a.within_1)),
    ];
    if let Some(r) = est.rounds.last() {
        ov.quality
            .push(("ba_sigma_post_px".into(), r3(r.ba.sigma_post_px)));
    }
    if let Some(cv) = &est.cv {
        ov.quality.push((
            "cv_median_abs_dy_before_px".into(),
            r3(cv.before.median_abs),
        ));
        ov.quality
            .push(("cv_median_abs_dy_after_px".into(), r3(cv.after.median_abs)));
    }
    ov.note = note.to_string();
    ov
}

/// Multi-line report of an estimate.
pub fn report(factory: &DeviceCalib, est: &Estimate, scenes: &[Scene]) -> String {
    use std::fmt::Write;
    let mut s = String::new();
    let _ = writeln!(
        s,
        "scenes: {}",
        scenes
            .iter()
            .map(|x| x.name.as_str())
            .collect::<Vec<_>>()
            .join(", ")
    );
    let _ = writeln!(
        s,
        "reference camera {} (held fixed), estimated cameras {:?}",
        est.reference, est.free
    );
    let _ = writeln!(
        s,
        "\npairs on the starting calibration (what explains their vertical disparities):"
    );
    for p in &est.pairs {
        let _ = writeln!(
            s,
            "  pair {}-{}: {:4} matches, {} -> {}",
            p.a,
            p.b,
            p.n,
            p.verdict.text(),
            if p.accepted {
                if est.used_pairs.contains(&(p.a, p.b)) {
                    "USED".to_string()
                } else {
                    "usable, but not tied to the reference camera".to_string()
                }
            } else {
                format!("NOT USED ({})", p.why)
            }
        );
        if p.saturation > 0.25 {
            let _ = writeln!(
                s,
                "      {:.0} % of its matches sit at the vertical search limit: the error is larger than the search range (try --dy-search 24)",
                p.saturation * 100.0
            );
        }
        s += &diag::table(&p.fits);
    }
    if !est.not_estimated.is_empty() {
        let _ = writeln!(
            s,
            "cameras NOT estimated (no usable pair ties them to the reference): {:?}",
            est.not_estimated
        );
    }
    let _ = writeln!(s);
    for (i, r) in est.rounds.iter().enumerate() {
        let inl = r.ba.inlier.iter().filter(|&&x| x).count();
        let _ = writeln!(
            s,
            "round {}: {} observations, {} inliers, {} LM iterations, residual sigma {:.3} px, cost {:.1} -> {:.1}",
            i + 1,
            r.n_obs,
            inl,
            r.ba.iterations,
            r.ba.sigma_post_px,
            r.ba.cost.0,
            r.ba.cost.1
        );
    }
    let _ = writeln!(
        s,
        "\ncorrections (rotation about each camera's own centre; camera axes x right, y down, z forward):"
    );
    for &c in &est.free {
        let w = est.w[c];
        let sg = est.sigma(c).unwrap_or([f64::NAN; 3]);
        let _ = write!(
            s,
            "  camera {c} {:15} {:.3} deg  w = ({:+.4}, {:+.4}, {:+.4}) deg  sigma ({:.4}, {:.4}, {:.4}) deg",
            factory.cameras[c].name,
            geom::norm(w).to_degrees(),
            w[0].to_degrees(),
            w[1].to_degrees(),
            w[2].to_degrees(),
            sg[0].to_degrees(),
            sg[1].to_degrees(),
            sg[2].to_degrees()
        );
        if let Some(b) = body_angles_deg(factory, w, c) {
            let _ = write!(
                s,
                "\n      in head axes: pitch {:+.3} (about x, + = nose up), yaw {:+.3} (about y, + = left), roll {:+.3} deg (about z, + = top to the left)",
                b[0], b[1], b[2]
            );
        }
        let _ = writeln!(s);
    }
    let _ = writeln!(
        s,
        "\nvertical disparity per pair, re-matched on rectified images (before = starting calibration, after = corrected):"
    );
    for (a, b) in pairs_of(&est.before) {
        let _ = writeln!(
            s,
            "  pair {a}-{b} before: {}",
            pooled(&est.before, a, b).line()
        );
        let _ = writeln!(
            s,
            "  pair {a}-{b} after : {}",
            pooled(&est.after, a, b).line()
        );
    }
    for &(a, b) in &est.used_pairs {
        let ((fb, nb), (fa, na)) = (
            depth_split(&est.before, a, b, DEPTH_SPLIT_M),
            depth_split(&est.after, a, b, DEPTH_SPLIT_M),
        );
        let _ = writeln!(
            s,
            "  pair {a}-{b} by depth (a rotation error shows at every depth, a baseline error only near):\n    far  (>= {DEPTH_SPLIT_M} m) before: {}\n    far  (>= {DEPTH_SPLIT_M} m) after : {}\n    near (<  {DEPTH_SPLIT_M} m) before: {}\n    near (<  {DEPTH_SPLIT_M} m) after : {}",
            fb.line(),
            fa.line(),
            nb.line(),
            na.line()
        );
    }
    let _ = writeln!(
        s,
        "  used     before: {}",
        used_stats(est, &est.before).line()
    );
    let _ = writeln!(
        s,
        "  used     after : {}",
        used_stats(est, &est.after).line()
    );
    if let Some(cv) = &est.cv {
        let _ = writeln!(
            s,
            "\nspatial cross-validation ({} folds of 96 px image blocks; each fold scored with rotations estimated without it):\n  held out, starting calibration: {}\n  held out, estimated rotations : {}",
            cv.folds,
            cv.before.line(),
            cv.after.line()
        );
    }
    let _ = writeln!(
        s,
        "\nper scene and pair (before -> after: matches, median |dy|, within 1 px):"
    );
    for (bp, ap) in est.before.iter().zip(&est.after) {
        let (x, y) = (bp.dy_stats(), ap.dy_stats());
        let _ = writeln!(
            s,
            "  {:10} pair {}-{}: {:4} {:.2} px {:3.0}%  ->  {:4} {:.2} px {:3.0}%",
            scenes[bp.scene].name,
            bp.a,
            bp.b,
            x.n,
            x.median_abs,
            x.within_1 * 100.0,
            y.n,
            y.median_abs,
            y.within_1 * 100.0
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stats_and_helpers() {
        let d = DyStats::of(&[0.1, -0.2, 1.5, -3.0, 0.4]);
        assert_eq!(d.n, 5);
        assert_eq!(d.median, 0.1);
        assert_eq!(d.median_abs, 0.4);
        assert!((d.within_05 - 0.6).abs() < 1e-12);
        assert!((d.within_1 - 0.6).abs() < 1e-12);
        assert!((d.within_2 - 0.8).abs() < 1e-12);
        assert!(d.line().contains("5 matches"));
        assert_eq!(DyStats::of(&[]).n, 0);
        assert_eq!(connected_free(&[(0, 1), (2, 3)], 0, 4), vec![1]);
        assert_eq!(
            connected_free(&[(0, 1), (1, 3), (2, 3)], 0, 4),
            vec![1, 2, 3]
        );
        assert_eq!(parse_pairs("0,1;2,3", 4).unwrap(), vec![(0, 1), (2, 3)]);
        assert_eq!(parse_pairs("all", 3).unwrap(), vec![(0, 1), (0, 2), (1, 2)]);
        assert!(parse_pairs("0,0", 4).is_err());
        assert!(parse_pairs("0,4", 4).is_err());
        assert!(parse_pairs("x", 4).is_err());
    }

    #[test]
    fn corrected_matches_the_overlay_semantics() {
        let f = DeviceCalib::parse(include_str!(
            "../../pimax-cam/tests/fixtures/headset-calibration.xml"
        ))
        .unwrap();
        let w = vec![[0.0; 3], [0.001, -0.002, 0.003], [0.0; 3], [0.0; 3]];
        let c = corrected(&f, &w);
        let rig0 = RigModel::from_calib(&f);
        let rig1 = RigModel::from_calib(&c);
        // the BA's corrected rotation equals the overlay-corrected calibration's rotation; centres unchanged
        let r = rig0.rot(&w, 1);
        for i in 0..3 {
            for j in 0..3 {
                assert!((r[i][j] - rig1.r[1][i][j]).abs() < 1e-12);
            }
        }
        assert!(geom::norm(geom::sub(rig0.c[1], rig1.c[1])) < 1e-12);
        assert_eq!(c.cameras[0], f.cameras[0]);
        // w about camera 1's own x axis (image right): scene points move up in its image, i.e. the camera
        // pitched down about its own x axis. In head axes the same rotation has the same size, and its
        // direction is camera 1's x axis expressed in the body frame, negated.
        let b = body_angles_deg(&f, [0.01, 0.0, 0.0], 1).unwrap();
        assert!((geom::norm(b) - 0.01f64.to_degrees()).abs() < 1e-9);
        let x1 = geom::col(
            &f.body_from_cam(1, RigConvention::CamFromCam0).unwrap().r,
            0,
        );
        assert!(geom::dot(geom::normalize(b), x1) < -0.999);
    }
}
