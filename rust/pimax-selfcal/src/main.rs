//! `pimax-selfcal`: self-calibration of the Dream Air tracking-camera rig from recorded frames.
//!
//! ```text
//! pimax-selfcal estimate SCENES... [--pairs 0,1;0,2;1,3;2,3|all] [--reference 0] [--free 1,2,3] [--rounds 2]
//!                        [--calib XML] [--from OVERLAY|default] [--out DIR] [--holdout] [--no-eye]
//!                        [--all-pairs] [--accept-sigma PX] [--prior-deg D] [--write [FILE|default]] [--force]
//!     estimate rotation corrections; print the report; with --out also write report.txt, summary.json, the
//!     candidate overlay, match lists and before/after pictures; with --write, install the overlay (default
//!     ~/.config/omarchyvr/cam-calib-overlay.json; the old one is kept as .prev) if the sanity gate passes
//! pimax-selfcal check SCENES... [--overlay [FILE|default]] [--pairs ...] [--calib XML] [--no-eye]
//!     measure the vertical disparity per pair and the passthrough eye-view shift with the factory calibration
//!     and, when an overlay is given, with the overlay on top
//! pimax-selfcal pairs SCENES... [--overlay [FILE|default]] [--pairs ...] [--out DIR]
//!     per scene and pair: matches, rejections, vertical disparity; with --out match lists (CSV) and pictures
//! pimax-selfcal lens SCENES...             image-circle centre of every tile vs the calibration's principal point
//! pimax-selfcal selftest [--out DIR] [--angle DEG]
//!     render a synthetic room through a rig rotated by known angles and check that the pipeline recovers them
//! pimax-selfcal show [FILE|default]        print an overlay and whether it applies to the calibration
//! pimax-selfcal rollback [FILE|default]    swap the overlay with its .prev copy
//! pimax-selfcal clear [FILE|default]       delete the overlay and its .prev copy (back to the factory calibration)
//! ```
//! SCENES: a directory of `*tile0.png`..`*tile3.png` sets, a tile prefix (`.../cam01_ctr49470_tile`), or
//! `FILE.pxcam[@FIRST[:COUNT[:STEP]]]` (tracking frames). Matcher options for estimate/check/pairs:
//! `--dy-search N --max-disp N --min-disp N --cell N --half N --lk-half N --min-zncc X --margin X --min-eig X
//! --max-level X`. Offline only: nothing here opens the headset.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom::{self, Vec3};
use pimax_cam::json;
use pimax_cam::overlay::{self, CalibOverlay};
use pimax_selfcal::eyecheck::{self, EyeParams, EyeShift};
use pimax_selfcal::matcher::MatchConfig;
use pimax_selfcal::pipeline::{self, DyStats, Estimate, EstimateOptions, GateLimits, PairMatches};
use pimax_selfcal::scene::{self, Scene};
use pimax_selfcal::synth;
use pimax_selfcal::viz;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type R<T> = Result<T, String>;

/// Vertical errors are drawn this many times enlarged in the pictures.
const NEEDLE: f64 = 10.0;

/// The eye-view check of one scene: its index and (before, after) per eye.
type EyeData = Option<(usize, Vec<(EyeShift, EyeShift)>)>;

struct Args(Vec<String>);

impl Args {
    fn flag(&mut self, n: &str) -> bool {
        match self.0.iter().position(|a| a == n) {
            Some(i) => {
                self.0.remove(i);
                true
            }
            None => false,
        }
    }
    fn opt(&mut self, n: &str) -> R<Option<String>> {
        if let Some(i) = self.0.iter().position(|a| a == n) {
            if i + 1 >= self.0.len() {
                return Err(format!("{n} needs a value"));
            }
            let v = self.0.remove(i + 1);
            self.0.remove(i);
            return Ok(Some(v));
        }
        let pre = format!("{n}=");
        Ok(self
            .0
            .iter()
            .position(|a| a.starts_with(&pre))
            .map(|i| self.0.remove(i)[pre.len()..].to_string()))
    }
    /// `--name` alone (value `default`) or `--name VALUE` when the next word is `default` or a `.json` path.
    fn opt_or_flag(&mut self, n: &str) -> Option<String> {
        let i = self.0.iter().position(|a| a == n)?;
        self.0.remove(i);
        if i < self.0.len() && !self.0[i].starts_with("--") && looks_like_path(&self.0[i]) {
            return Some(self.0.remove(i));
        }
        Some("default".into())
    }
    fn num<T: std::str::FromStr>(&mut self, n: &str, d: T) -> R<T> {
        match self.opt(n)? {
            Some(v) => v.parse().map_err(|_| format!("bad value for {n}: {v}")),
            None => Ok(d),
        }
    }
    fn rest(&mut self) -> R<Vec<String>> {
        if let Some(a) = self.0.iter().find(|a| a.starts_with("--")) {
            return Err(format!("unknown option {a}"));
        }
        Ok(std::mem::take(&mut self.0))
    }
}

fn looks_like_path(s: &str) -> bool {
    s == "default" || s.ends_with(".json")
}

fn overlay_path(spec: &str) -> R<PathBuf> {
    if spec == "default" {
        overlay::default_path().ok_or_else(|| "no default overlay path (HOME unset)".into())
    } else {
        Ok(PathBuf::from(spec))
    }
}

fn load_calib(p: Option<String>) -> R<DeviceCalib> {
    let (c, used) =
        DeviceCalib::load_default(p.as_deref().map(Path::new)).map_err(|e| e.to_string())?;
    eprintln!("calibration: {} (device {})", used.display(), c.uid);
    Ok(c)
}

fn load_scenes(specs: &[String]) -> R<Vec<Scene>> {
    if specs.is_empty() {
        return Err("give at least one scene (tile directory, tile prefix or .pxcam)".into());
    }
    let mut v = Vec::new();
    for s in specs {
        let got = scene::load(s)?;
        eprintln!("scenes from {s}: {}", got.len());
        v.extend(got);
    }
    Ok(v)
}

/// Rotations of an overlay as a per-camera vector (after checking that it applies to `cal`).
fn overlay_w(cal: &DeviceCalib, ov: &CalibOverlay) -> R<Vec<Vec3>> {
    if let Some(why) = ov.refusal(cal, overlay::DEFAULT_MAX_ANGLE_DEG) {
        return Err(format!("overlay does not apply: {why}"));
    }
    Ok((0..cal.cameras.len()).map(|c| ov.rotation(c)).collect())
}

/// Matcher overrides shared by the commands.
fn matcher_opts(a: &mut Args, m: &mut MatchConfig) -> R<()> {
    m.cell = a.num("--cell", m.cell)?;
    m.min_zncc = a.num("--min-zncc", m.min_zncc)?;
    m.min_margin = a.num("--margin", m.min_margin)?;
    m.min_eig = a.num("--min-eig", m.min_eig)?;
    m.max_level = a.num("--max-level", m.max_level)?;
    m.dy_search = a.num("--dy-search", m.dy_search)?;
    m.min_disp = a.num("--min-disp", m.min_disp)?;
    m.max_disp = a.num("--max-disp", m.max_disp)?;
    m.half = a.num("--half", m.half)?;
    m.lk_half = a.num("--lk-half", m.lk_half)?;
    if m.half < 2 || m.lk_half < 2 || m.cell < 2 || m.dy_search < 0 || m.max_disp <= m.min_disp {
        return Err("bad matcher options".into());
    }
    Ok(())
}

fn tool_line() -> String {
    format!("pimax-selfcal {}", env!("CARGO_PKG_VERSION"))
}

/// Per-camera priors from an earlier overlay: its rotation, with its uncertainty widened by a random walk of
/// 0.02 deg per sqrt(day) since it was written (at least 0.02 deg). Cameras without a stored sigma get none.
fn priors_from(ov: &CalibOverlay, n: usize) -> Vec<Option<(Vec3, Vec3)>> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(ov.created_unix);
    let days = now.saturating_sub(ov.created_unix) as f64 / 86400.0;
    let walk = 0.02f64.to_radians() * days.sqrt();
    (0..n)
        .map(|c| {
            let r = ov.rotations.iter().find(|r| r.id == c)?;
            let sg = r.sigma?;
            let widen = |s: f64| (s * s + walk * walk).sqrt().max(0.02f64.to_radians());
            Some((
                r.rotation_vector,
                [widen(sg[0]), widen(sg[1]), widen(sg[2])],
            ))
        })
        .collect()
}

/// Eye-view check of the scene with the most pair 0-1 matches: text for the report and the numbers.
fn eye_check(
    before: &DeviceCalib,
    after: &DeviceCalib,
    scenes: &[Scene],
    pm: &[PairMatches],
) -> R<(String, EyeData)> {
    let Some(si) = pipeline::richest_scene(pm, 0, 1) else {
        return Ok((String::new(), None));
    };
    let p = EyeParams::default();
    let v = eyecheck::before_after(before, after, &scenes[si], &p)?;
    if v.is_empty() {
        return Ok((String::new(), None));
    }
    let mut s = format!(
        "\neye-view check, scene {} (passthrough geometry: one eye's view rendered only from camera 0 and only from\ncamera 1, reprojection depth {} m; the vertical shift between the two is what the two eyes get as vertical disparity):\n",
        scenes[si].name,
        if v[0].0.depth_m.is_finite() {
            format!("{}", v[0].0.depth_m)
        } else {
            "inf".into()
        }
    );
    for (b, a) in &v {
        s += &format!(
            "  {} eye: vertical {:+.2} deg -> {:+.2} deg, horizontal {:+.2} -> {:+.2} deg, correlation at the best shift {:.3} -> {:.3} ({} px)\n",
            if b.eye == 0 { "left " } else { "right" },
            b.dy_deg,
            a.dy_deg,
            b.dx_deg,
            a.dx_deg,
            b.ncc,
            a.ncc,
            b.pixels
        );
    }
    Ok((s, Some((si, v))))
}

fn r3(x: f64) -> String {
    json::num((x * 1000.0).round() / 1000.0)
}

fn stats_json(d: &DyStats) -> String {
    let z = |x: f64| if d.n == 0 { 0.0 } else { x };
    format!(
        "{{\"n\": {}, \"median\": {}, \"median_abs\": {}, \"rms\": {}, \"within_0_5\": {}, \"within_1\": {}, \"within_2\": {}}}",
        d.n,
        r3(z(d.median)),
        r3(z(d.median_abs)),
        r3(d.rms),
        r3(d.within_05),
        r3(d.within_1),
        r3(d.within_2)
    )
}

fn deg3(v: Vec3) -> String {
    json::num_array(&v.map(|x| (x.to_degrees() * 1e5).round() / 1e5))
}

/// Machine-readable summary of an estimate (for the report page).
fn summary_json(
    factory: &DeviceCalib,
    est: &Estimate,
    scenes: &[Scene],
    gate: &[String],
    eye: &EyeData,
    truth: Option<&[Vec3]>,
) -> String {
    let mut o = String::from("{\n");
    o += &format!("  \"tool\": {},\n", json::quote(&tool_line()));
    o += &format!("  \"device_uid\": {},\n", json::quote(&factory.uid));
    o += &format!(
        "  \"scenes\": [{}],\n",
        scenes
            .iter()
            .map(|s| json::quote(&s.name))
            .collect::<Vec<_>>()
            .join(", ")
    );
    o += &format!("  \"reference_camera\": {},\n", est.reference);
    o += &format!("  \"not_estimated\": {:?},\n", est.not_estimated);
    o += "  \"cameras\": [";
    for (k, &c) in est.free.iter().enumerate() {
        o += if k == 0 { "\n" } else { ",\n" };
        o += &format!(
            "    {{\"id\": {c}, \"name\": {}, \"angle_deg\": {}, \"w_deg\": {}, \"sigma_deg\": {}, \"inliers\": {}",
            json::quote(&factory.cameras[c].name),
            json::num((geom::norm(est.w[c]).to_degrees() * 1e4).round() / 1e4),
            deg3(est.w[c]),
            deg3(est.sigma(c).unwrap_or([0.0; 3])),
            est.inliers_of(c)
        );
        if let Some(h) = pipeline::body_angles_deg(factory, est.w[c], c) {
            o += &format!(
                ", \"head_pitch_yaw_roll_deg\": {}",
                json::num_array(&h.map(|x| (x * 1e4).round() / 1e4))
            );
        }
        if let Some(t) = truth {
            o += &format!(
                ", \"truth_w_deg\": {}, \"error_deg\": {}",
                deg3(t[c]),
                json::num((geom::norm(geom::sub(est.w[c], t[c])).to_degrees() * 1e5).round() / 1e5)
            );
        }
        o += "}";
    }
    o += "\n  ],\n  \"pairs\": [";
    for (k, p) in est.pairs.iter().enumerate() {
        o += if k == 0 { "\n" } else { ",\n" };
        let used = est.used_pairs.contains(&(p.a, p.b));
        let ((fb, nb), (fa, na)) = (
            pipeline::depth_split(&est.before, p.a, p.b, pipeline::DEPTH_SPLIT_M),
            pipeline::depth_split(&est.after, p.a, p.b, pipeline::DEPTH_SPLIT_M),
        );
        o += &format!(
            "    {{\"a\": {}, \"b\": {}, \"matches\": {}, \"verdict\": {}, \"used\": {used}, \"why_not\": {},\n     \"before\": {}, \"after\": {},\n     \"far_before\": {}, \"far_after\": {}, \"near_before\": {}, \"near_after\": {},\n     \"fits\": [",
            p.a,
            p.b,
            p.n,
            json::quote(p.verdict.text()),
            json::quote(&p.why),
            stats_json(&pipeline::pooled(&est.before, p.a, p.b)),
            stats_json(&pipeline::pooled(&est.after, p.a, p.b)),
            stats_json(&fb),
            stats_json(&fa),
            stats_json(&nb),
            stats_json(&na)
        );
        for (j, f) in p.fits.iter().enumerate() {
            o += if j == 0 { "" } else { ", " };
            o += &format!(
                "{{\"model\": {}, \"sigma_px\": {}, \"inliers\": {}, \"w_deg\": {}, \"t\": {}}}",
                json::quote(f.model.name()),
                r3(f.sigma),
                f.inliers,
                deg3([f.params[0], f.params[1], f.params[2]]),
                json::num_array(&[
                    (f.params[3] * 1e4).round() / 1e4,
                    (f.params[4] * 1e4).round() / 1e4
                ])
            );
        }
        let dys = |pm: &[PairMatches]| {
            let v: Vec<f64> = pm
                .iter()
                .filter(|q| q.a == p.a && q.b == p.b)
                .flat_map(|q| q.dys())
                .map(|d| (d * 100.0).round() / 100.0)
                .collect();
            json::num_array(&v)
        };
        o += &format!(
            "],\n     \"dy_before\": {},\n     \"dy_after\": {}}}",
            dys(&est.before),
            dys(&est.after)
        );
    }
    o += "\n  ],\n";
    o += &format!(
        "  \"used\": {{\"before\": {}, \"after\": {}}},\n",
        stats_json(&pipeline::used_stats(est, &est.before)),
        stats_json(&pipeline::used_stats(est, &est.after))
    );
    if let Some(cv) = &est.cv {
        o += &format!(
            "  \"cross_validation\": {{\"folds\": {}, \"before\": {}, \"after\": {}}},\n",
            cv.folds,
            stats_json(&cv.before),
            stats_json(&cv.after)
        );
    }
    if let Some((si, v)) = eye {
        o += &format!(
            "  \"eye_view\": {{\"scene\": {}, \"eyes\": [",
            json::quote(&scenes[*si].name)
        );
        for (k, (b, a)) in v.iter().enumerate() {
            o += if k == 0 { "" } else { ", " };
            o += &format!(
                "{{\"eye\": {}, \"depth_m\": {}, \"dy_deg_before\": {}, \"dy_deg_after\": {}, \"dx_deg_before\": {}, \"dx_deg_after\": {}, \"ncc0_before\": {}, \"ncc0_after\": {}}}",
                b.eye,
                if b.depth_m.is_finite() {
                    r3(b.depth_m)
                } else {
                    "null".into()
                },
                r3(b.dy_deg),
                r3(a.dy_deg),
                r3(b.dx_deg),
                r3(a.dx_deg),
                r3(b.ncc0),
                r3(a.ncc0)
            );
        }
        o += "]},\n";
    }
    if let Some(r) = est.rounds.last() {
        o += &format!(
            "  \"ba\": {{\"observations\": {}, \"residual_px\": {}}},\n",
            r.n_obs,
            r3(r.ba.sigma_post_px)
        );
    }
    o += &format!(
        "  \"gate\": {{\"pass\": {}, \"reasons\": [{}]}}\n}}\n",
        gate.is_empty(),
        gate.iter()
            .map(|g| json::quote(g))
            .collect::<Vec<_>>()
            .join(", ")
    );
    o
}

/// Report, summary, candidate overlay, match lists and pictures of an estimate.
#[allow(clippy::too_many_arguments)]
fn write_outputs(
    dir: &Path,
    factory: &DeviceCalib,
    est: &Estimate,
    scenes: &[Scene],
    rep: &str,
    gate: &[String],
    eye: &EyeData,
    truth: Option<&[Vec3]>,
) -> R<()> {
    let wr = |name: &str, data: &[u8]| {
        std::fs::write(dir.join(name), data)
            .map_err(|e| format!("{}: {e}", dir.join(name).display()))
    };
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    wr("report.txt", rep.as_bytes())?;
    wr(
        "summary.json",
        summary_json(factory, est, scenes, gate, eye, truth).as_bytes(),
    )?;
    let ov = pipeline::to_overlay(factory, est, &tool_line(), "estimate (not installed)");
    wr("cam-calib-overlay.json", ov.to_json().as_bytes())?;
    for (name, pm) in [
        ("matches_before.csv", &est.before),
        ("matches_after.csv", &est.after),
    ] {
        let mut csv = String::from("scene,cam_a,cam_b,x,y,disparity,dy,zncc\n");
        for p in pm.iter() {
            for m in &p.set.matches {
                csv += &format!(
                    "{},{},{},{:.1},{:.1},{:.3},{:.3},{:.3}\n",
                    scenes[p.scene].name, p.a, p.b, m.x, m.y, m.disp, m.dy, m.zncc
                );
            }
        }
        wr(name, csv.as_bytes())?;
    }
    for p in &est.pairs {
        let Some(si) = pipeline::richest_scene(&est.before, p.a, p.b) else {
            continue;
        };
        let (Some(b), Some(a)) = (
            pipeline::find(&est.before, si, p.a, p.b),
            pipeline::find(&est.after, si, p.a, p.b),
        ) else {
            continue;
        };
        let stem = format!("pair{}{}_{}", p.a, p.b, scenes[si].name);
        if let Some(img) = viz::before_after(b, a, NEEDLE) {
            img.save(&dir.join(format!("{stem}_before_after.png")))?;
        }
        if let Some(d) = viz::details_before_after(b, a, 6, 40, 5) {
            d.save(&dir.join(format!("{stem}_details.png")))?;
        }
    }
    if let Some((si, v)) = eye {
        let p = EyeParams::default();
        let depth = v[0].0.depth_m;
        let before = pipeline::corrected(factory, &est.w0);
        let after = pipeline::corrected(factory, &est.w);
        let (b0, b1) = eyecheck::renders(&before, &scenes[*si], 0, (0, 1), depth, &p)?;
        let (a0, a1) = eyecheck::renders(&after, &scenes[*si], 0, (0, 1), depth, &p)?;
        let name = &scenes[*si].name;
        viz::anaglyph(&b0, &b1, p.width, p.height)
            .save(&dir.join(format!("eye_left_{name}_before.png")))?;
        viz::anaglyph(&a0, &a1, p.width, p.height)
            .save(&dir.join(format!("eye_left_{name}_after.png")))?;
        let c = viz::textured_centres(&b0, &b1, p.width, p.height, 5, 48);
        if !c.is_empty() {
            viz::stack(
                &[
                    &viz::zoom_pair(&b0, &b1, p.width, p.height, &c, 48, 4),
                    &viz::zoom_pair(&a0, &a1, p.width, p.height, &c, 48, 4),
                ],
                6,
            )
            .save(&dir.join(format!("eye_left_{name}_details.png")))?;
        }
    }
    eprintln!("wrote {}", dir.display());
    Ok(())
}

/// Options of the estimate shared by the commands.
fn estimate_opts(a: &mut Args, n: usize) -> R<EstimateOptions> {
    let mut opt = EstimateOptions::default();
    if let Some(p) = a.opt("--pairs")? {
        opt.pairs = pipeline::parse_pairs(&p, n)?;
    }
    opt.reference = a.num("--reference", opt.reference)?;
    if let Some(f) = a.opt("--free")? {
        opt.free = Some(
            f.split(',')
                .map(|x| {
                    x.trim()
                        .parse::<usize>()
                        .map_err(|_| format!("bad --free {f}"))
                })
                .collect::<R<Vec<_>>>()?,
        );
    }
    opt.rounds = a.num("--rounds", opt.rounds)?;
    opt.accept_all = a.flag("--all-pairs");
    opt.accept_sigma_px = a.num("--accept-sigma", opt.accept_sigma_px)?;
    let prior_deg: f64 = a.num("--prior-deg", opt.ba.prior_rad.to_degrees())?;
    if prior_deg.is_nan() || prior_deg <= 0.0 {
        return Err("--prior-deg must be positive".into());
    }
    opt.ba.prior_rad = prior_deg.to_radians();
    matcher_opts(a, &mut opt.matcher)?;
    Ok(opt)
}

fn gate_text(why: &[String]) -> String {
    if why.is_empty() {
        return "\nsanity gate: PASS\n".into();
    }
    let mut s = String::from("\nsanity gate: FAIL (the overlay is not installed)\n");
    for w in why {
        s += &format!("  - {w}\n");
    }
    s
}

fn cmd_estimate(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let from = a.opt("--from")?;
    let out = a.opt("--out")?.map(PathBuf::from);
    let do_holdout = a.flag("--holdout");
    let no_eye = a.flag("--no-eye");
    let force = a.flag("--force");
    let write = a.opt_or_flag("--write");
    let factory = load_calib(calp)?;
    let n = factory.cameras.len();
    let mut opt = estimate_opts(&mut a, n)?;
    opt.keep_images = out.is_some();
    let specs = a.rest()?;
    let scenes = load_scenes(&specs)?;
    let w0 = match &from {
        Some(spec) => {
            let p = overlay_path(spec)?;
            let ov = CalibOverlay::load(&p).map_err(|e| e.to_string())?;
            eprintln!(
                "starting from overlay {} (its rotations are the prior)",
                p.display()
            );
            opt.ba.priors = priors_from(&ov, n);
            overlay_w(&factory, &ov)?
        }
        None => vec![[0.0; 3]; n],
    };
    let t0 = std::time::Instant::now();
    let est = pipeline::estimate(&factory, &w0, &scenes, &opt)?;
    eprintln!("estimate: {:.1} s", t0.elapsed().as_secs_f64());
    let mut rep = pipeline::report(&factory, &est, &scenes);
    if do_holdout && scenes.len() > 1 {
        let hs = pipeline::holdout(&factory, &scenes, &est, &opt);
        rep += "\nleave-one-scene-out (estimate without the scene, measure on it):\n";
        for h in &hs {
            let (b, af) = (
                pipeline::pooled_all(&h.before),
                pipeline::pooled_all(&h.after),
            );
            let dw: f64 = est
                .free
                .iter()
                .map(|&c| geom::norm(geom::sub(h.w[c], est.w[c])).to_degrees())
                .fold(0.0, f64::max);
            rep += &format!(
                "  {:10} before: {}\n  {:10} after : {}   (max difference to the all-scene estimate {:.3} deg)\n",
                scenes[h.scene].name,
                b.line(),
                "",
                af.line(),
                dw
            );
        }
        if hs.is_empty() {
            rep += "  (not enough matches outside any single scene)\n";
        }
    }
    let eye = if no_eye {
        None
    } else {
        let (text, data) = eye_check(
            &pipeline::corrected(&factory, &est.w0),
            &pipeline::corrected(&factory, &est.w),
            &scenes,
            &est.before,
        )?;
        rep += &text;
        data
    };
    let why = pipeline::gate(&est, &GateLimits::default());
    rep += &gate_text(&why);
    print!("{rep}");
    if let Some(dir) = &out {
        write_outputs(dir, &factory, &est, &scenes, &rep, &why, &eye, None)?;
    }
    if let Some(spec) = write {
        if !why.is_empty() && !force {
            return Err("sanity gate failed: overlay NOT written (use --force to override)".into());
        }
        let p = overlay_path(&spec)?;
        let note = format!(
            "from {} scene(s): {}",
            scenes.len(),
            scenes
                .iter()
                .map(|s| s.name.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        );
        let ov = pipeline::to_overlay(&factory, &est, &tool_line(), &note);
        overlay::save(&ov, &p).map_err(|e| format!("{}: {e}", p.display()))?;
        println!(
            "overlay written: {} (previous kept as {}; undo with `pimax-selfcal rollback`)",
            p.display(),
            overlay::prev_path(&p).display()
        );
    }
    Ok(ExitCode::SUCCESS)
}

/// The calibration with an optional overlay applied.
fn calib_with(factory: &DeviceCalib, ovp: &Option<String>) -> R<Option<DeviceCalib>> {
    match ovp {
        Some(spec) => {
            let p = overlay_path(spec)?;
            let ov = CalibOverlay::load(&p).map_err(|e| format!("{}: {e}", p.display()))?;
            eprintln!("overlay: {}", p.display());
            Ok(Some(pipeline::corrected(
                factory,
                &overlay_w(factory, &ov)?,
            )))
        }
        None => Ok(None),
    }
}

fn cmd_check(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let ovp = a.opt_or_flag("--overlay");
    let no_eye = a.flag("--no-eye");
    let factory = load_calib(calp)?;
    let opt = estimate_opts(&mut a, factory.cameras.len())?;
    let specs = a.rest()?;
    let with = calib_with(&factory, &ovp)?;
    let scenes = load_scenes(&specs)?;
    let run = |cal: &DeviceCalib| {
        pipeline::match_all(cal, &scenes, 0, &opt.pairs, &opt.rect, &opt.matcher, false)
    };
    let base = run(&factory);
    let over = with.as_ref().map(run);
    println!("vertical disparity per pair (rectified pixels; 0 for a perfect calibration):");
    for (pa, pb) in pipeline::pairs_of(&base) {
        println!(
            "  pair {pa}-{pb} factory: {}",
            pipeline::pooled(&base, pa, pb).line()
        );
        if let Some(o) = &over {
            println!(
                "  pair {pa}-{pb} overlay: {}",
                pipeline::pooled(o, pa, pb).line()
            );
        }
    }
    println!(
        "\nwhat explains the factory calibration's vertical disparities:\n{}",
        pipeline::diagnose(&base, opt.rect.focal)
    );
    if let Some(o) = &over {
        println!(
            "with the overlay:\n{}",
            pipeline::diagnose(o, opt.rect.focal)
        );
    }
    if no_eye {
        return Ok(ExitCode::SUCCESS);
    }
    if let Some(w) = &with {
        print!("{}", eye_check(&factory, w, &scenes, &base)?.0);
    } else if let Some(si) = pipeline::richest_scene(&base, 0, 1) {
        let p = EyeParams::default();
        if let Some(d) = eyecheck::best_depth(&factory, &scenes[si], 0, (0, 1), &p)? {
            for eye in 0..2 {
                let s = eyecheck::shift(&factory, &scenes[si], eye, (0, 1), d, &p)?;
                println!(
                    "eye-view check, scene {}, {} eye, depth {} m: camera 1's picture is {:+.2} deg vertical, {:+.2} deg horizontal from camera 0's (correlation {:.3})",
                    scenes[si].name,
                    if eye == 0 { "left" } else { "right" },
                    d,
                    s.dy_deg,
                    s.dx_deg,
                    s.ncc
                );
            }
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_pairs(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let out = a.opt("--out")?.map(PathBuf::from);
    let ovp = a.opt_or_flag("--overlay");
    let factory = load_calib(calp)?;
    let opt = estimate_opts(&mut a, factory.cameras.len())?;
    let specs = a.rest()?;
    let cal = calib_with(&factory, &ovp)?.unwrap_or_else(|| factory.clone());
    let scenes = load_scenes(&specs)?;
    let pm = pipeline::match_all(
        &cal,
        &scenes,
        0,
        &opt.pairs,
        &opt.rect,
        &opt.matcher,
        out.is_some(),
    );
    for p in &pm {
        println!(
            "{:10} pair {}-{}: {}\n{:22}{}",
            scenes[p.scene].name,
            p.a,
            p.b,
            p.set.summary(),
            "",
            p.dy_stats().line()
        );
        let Some(dir) = &out else { continue };
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        let mut csv = String::from("x,y,disparity,dy,zncc\n");
        for m in &p.set.matches {
            csv += &format!(
                "{:.2},{:.2},{:.3},{:.3},{:.3}\n",
                m.x, m.y, m.disp, m.dy, m.zncc
            );
        }
        let stem = format!("{}_pair{}{}", scenes[p.scene].name, p.a, p.b);
        std::fs::write(dir.join(format!("{stem}.csv")), csv).map_err(|e| e.to_string())?;
        if let Some(img) = viz::match_overlay(p, NEEDLE) {
            img.save(&dir.join(format!("{stem}.png")))?;
        }
    }
    println!(
        "\nwhat explains the vertical disparities (all scenes pooled per pair):\n{}",
        pipeline::diagnose(&pm, opt.rect.focal)
    );
    Ok(ExitCode::SUCCESS)
}

/// `lens`: image-circle centre of every tile against the calibration's principal point.
fn cmd_lens(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let specs = a.rest()?;
    let cal = load_calib(calp)?;
    let scenes = load_scenes(&specs)?;
    let n = cal.cameras.len();
    println!("image circle centre minus the calibration's principal point (pixels; x right, y down in the raw tile)");
    let mut pooled: Vec<Vec<(f64, f64)>> = vec![Vec::new(); n];
    for sc in &scenes {
        for (c, pts) in pooled.iter_mut().enumerate() {
            let k = &cal.cameras[c].intr;
            let (w, h) = (k.width as usize, k.height as usize);
            pts.extend(pimax_selfcal::circle::edge_points(
                &sc.tiles[c],
                w,
                h,
                k.cx,
                k.cy,
            ));
            match pimax_selfcal::circle::fit(&sc.tiles[c], w, h, k.cx, k.cy) {
                Some(f) => println!(
                    "  {:10} cam {c} {:15} dx {:+6.2} dy {:+6.2}  radius {:6.2}  ({} edge points in {} corners, rms {:.2})",
                    sc.name,
                    cal.cameras[c].name,
                    f.cx - k.cx,
                    f.cy - k.cy,
                    f.r,
                    f.points,
                    f.corners,
                    f.rms
                ),
                None => println!(
                    "  {:10} cam {c} {:15} no circle (corners not lit, or lit to the tile border)",
                    sc.name, cal.cameras[c].name
                ),
            }
        }
    }
    println!(
        "all scenes pooled (edge points of every scene in one fit; trust it only with 4 corners):"
    );
    for (c, pts) in pooled.into_iter().enumerate() {
        let k = &cal.cameras[c].intr;
        let npts = pts.len();
        match pimax_selfcal::circle::fit_points(pts, k.cx, k.cy) {
            Some(f) => println!(
                "  cam {c} {:15} dx {:+6.2} dy {:+6.2}  radius {:6.2}  ({} of {npts} edge points in {} corners, rms {:.2})",
                cal.cameras[c].name,
                f.cx - k.cx,
                f.cy - k.cy,
                f.r,
                f.points,
                f.corners,
                f.rms
            ),
            None => println!(
                "  cam {c} {:15} no circle ({npts} edge points)",
                cal.cameras[c].name
            ),
        }
    }
    Ok(ExitCode::SUCCESS)
}

/// `selftest`: synthetic room, rig rotated by known angles, pipeline must recover them.
fn cmd_selftest(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let out = a.opt("--out")?.map(PathBuf::from);
    let angle: f64 = a.num("--angle", 0.5)?;
    if !(0.0..=1.5).contains(&angle) {
        return Err("--angle: 0 .. 1.5 degrees".into());
    }
    let factory = load_calib(calp)?;
    let n = factory.cameras.len();
    let mut opt = estimate_opts(&mut a, n)?;
    opt.keep_images = out.is_some();
    a.rest()?;
    // fixed directions, scaled so that the largest rotation is `angle`
    let dirs: [Vec3; 3] = [
        [0.30, -0.25, 0.20],
        [-0.35, 0.20, 0.30],
        [0.25, 0.30, -0.40],
    ];
    let scale = angle.to_radians() / dirs.iter().map(|d| geom::norm(*d)).fold(0.0, f64::max);
    let mut w_true = vec![[0.0; 3]; n];
    for (c, w) in w_true.iter_mut().enumerate().skip(1) {
        *w = geom::scale(dirs[(c - 1) % 3], scale);
    }
    let truth = pipeline::corrected(&factory, &w_true);
    let room = synth::Room::new(7);
    let scenes: Vec<Scene> = synth::default_poses()
        .iter()
        .enumerate()
        .map(|(i, p)| {
            synth::render(
                &truth,
                &room,
                p,
                1.5,
                100 + i as u64,
                &format!("synthetic{i}"),
            )
        })
        .collect();
    let est = pipeline::estimate(&factory, &vec![[0.0; 3]; n], &scenes, &opt)?;
    let mut rep = pipeline::report(&factory, &est, &scenes);
    rep += "\ninjected rotation -> estimated (degrees):\n";
    let mut worst = 0.0f64;
    for (c, wt) in w_true.iter().enumerate().skip(1) {
        let err = geom::norm(geom::sub(est.w[c], *wt)).to_degrees();
        worst = worst.max(err);
        let (t, e) = (wt.map(f64::to_degrees), est.w[c].map(f64::to_degrees));
        rep += &format!(
            "  camera {c}: ({:+.4}, {:+.4}, {:+.4}) -> ({:+.4}, {:+.4}, {:+.4}), error {:.4} deg\n",
            t[0], t[1], t[2], e[0], e[1], e[2], err
        );
    }
    let (text, eye) = eye_check(
        &factory,
        &pipeline::corrected(&factory, &est.w),
        &scenes,
        &est.before,
    )?;
    rep += &text;
    let why = pipeline::gate(&est, &GateLimits::default());
    rep += &gate_text(&why);
    let ok = worst < 0.03 && why.is_empty() && est.free.len() == n - 1;
    rep += &format!(
        "selftest: {} (largest error {:.4} deg, limit 0.03)\n",
        if ok { "PASS" } else { "FAIL" },
        worst
    );
    print!("{rep}");
    if let Some(dir) = &out {
        write_outputs(
            dir,
            &factory,
            &est,
            &scenes,
            &rep,
            &why,
            &eye,
            Some(&w_true),
        )?;
        let mut mosaic = viz::Rgb::new(4 * 640, 480, [0; 3]);
        for (t, tile) in scenes[0].tiles.iter().enumerate() {
            mosaic.blit(&viz::Rgb::from_grey(tile, 640, 480), t * 640, 0);
        }
        mosaic.save(&dir.join("synthetic0_tiles.png"))?;
    }
    Ok(if ok {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    })
}

fn cmd_show(mut a: Args) -> R<ExitCode> {
    let calp = a.opt("--calib")?;
    let rest = a.rest()?;
    let p = overlay_path(rest.first().map(String::as_str).unwrap_or("default"))?;
    let ov = CalibOverlay::load(&p).map_err(|e| e.to_string())?;
    println!("{}", p.display());
    print!("{}", ov.to_json());
    if let Ok(c) = DeviceCalib::load_default(calp.as_deref().map(Path::new)) {
        match ov.refusal(&c.0, overlay::DEFAULT_MAX_ANGLE_DEG) {
            None => println!("applies to {} ({})", c.1.display(), c.0.uid),
            Some(w) => println!("does NOT apply to {}: {w}", c.1.display()),
        }
    }
    Ok(ExitCode::SUCCESS)
}

fn cmd_rollback(mut a: Args) -> R<ExitCode> {
    let rest = a.rest()?;
    let p = overlay_path(rest.first().map(String::as_str).unwrap_or("default"))?;
    overlay::rollback(&p).map_err(|e| e.to_string())?;
    println!(
        "{} <-> {} swapped",
        p.display(),
        overlay::prev_path(&p).display()
    );
    Ok(ExitCode::SUCCESS)
}

fn cmd_clear(mut a: Args) -> R<ExitCode> {
    let rest = a.rest()?;
    let p = overlay_path(rest.first().map(String::as_str).unwrap_or("default"))?;
    let gone = overlay::clear(&p).map_err(|e| e.to_string())?;
    if gone.is_empty() {
        println!("nothing to delete at {}", p.display());
    }
    for g in gone {
        println!("deleted {}", g.display());
    }
    Ok(ExitCode::SUCCESS)
}

fn usage() -> &'static str {
    "usage: pimax-selfcal estimate|check|pairs|lens|selftest|show|rollback|clear ...\n\
     \x20 estimate SCENES... [--out DIR] [--write [FILE]] [--from OVERLAY] [--pairs A,B;...] [--holdout]\n\
     \x20 check SCENES... [--overlay [FILE]]      factory calibration vs factory + overlay on recorded frames\n\
     \x20 pairs SCENES... [--out DIR]             matches and vertical disparity per scene and pair\n\
     \x20 lens SCENES...                          image circle vs principal point\n\
     \x20 selftest [--out DIR] [--angle DEG]      synthetic end-to-end check\n\
     \x20 show|rollback|clear [FILE]              the installed overlay (default ~/.config/omarchyvr/cam-calib-overlay.json)\n\
     SCENES: tile directory, tile prefix, or FILE.pxcam[@FIRST[:COUNT[:STEP]]]. Offline: never opens the headset."
}

fn main() -> ExitCode {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() || args[0] == "-h" || args[0] == "--help" {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    }
    let cmd = args.remove(0);
    let a = Args(args);
    let r = match cmd.as_str() {
        "estimate" => cmd_estimate(a),
        "check" => cmd_check(a),
        "pairs" => cmd_pairs(a),
        "lens" => cmd_lens(a),
        "selftest" => cmd_selftest(a),
        "show" => cmd_show(a),
        "rollback" => cmd_rollback(a),
        "clear" => cmd_clear(a),
        _ => Err(format!("unknown command {cmd}\n{}", usage())),
    };
    match r {
        Ok(c) => c,
        Err(e) => {
            eprintln!("pimax-selfcal: {e}");
            ExitCode::FAILURE
        }
    }
}
