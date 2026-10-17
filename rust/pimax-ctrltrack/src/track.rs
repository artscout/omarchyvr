//! `pimax-ctrltrack track`: offline controller poses from a `.pxcam` recording.

use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::frame::{self, FrameKind, Meta, TILE_COUNT, TILE_H, TILE_W};
use pimax_cam::geom::{self, Pose};
use pimax_cam::record::Reader;
use pimax_ctrltrack::blob::{self, BlobParams};
use pimax_ctrltrack::extrinsics;
use pimax_ctrltrack::model::LedModel;
use pimax_ctrltrack::solve::{self, Cam, CamObs, CamSolve, JointSolve, Params};
use std::io::Write;
use std::path::Path;

pub struct LedFrame {
    pub rec: usize,
    pub ts_us: u64,
    pub px: Vec<Vec<(f64, f64)>>,
}

#[derive(Clone)]
pub struct Found {
    pub model: usize,
    pub joint: JointSolve,
    /// Joint fit of the other hand's model from the same start (L/R discrimination).
    pub(crate) alt: Option<JointSolve>,
    /// Largest distance between a single-camera solve and the joint pose (m), for camera pairs.
    pub(crate) cam_spread_m: f64,
    pub(crate) single: usize,
}

/// Everything the solver needs: calibration, (corrected) cameras, LED models, hypothesis triples.
pub struct Rig {
    pub calib: DeviceCalib,
    pub cams: Vec<Cam>,
    pub models: [LedModel; 2],
    pub triples: Vec<Vec<[usize; 3]>>,
    pub prm: Params,
}

impl Rig {
    /// [`Rig::try_load`], or a plain message and exit code 2 (never a panic: a missing input is a reason, not a crash;
    /// the launcher then runs the session without optical controller tracking).
    pub fn load(a: &[String]) -> Rig {
        match Rig::try_load(a) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("pimax-ctrltrack: {e}");
                eprintln!("pimax-ctrltrack: optical controller tracking is off; the controllers still work (buttons, gyro).");
                std::process::exit(2);
            }
        }
    }

    /// `--calib XML --left JSON --right JSON --extrinsics JSON|--factory`. The calibration and the LED models fall
    /// back to the shipped Dream Air reference when the user has none of their own (the log line says which is used).
    pub fn try_load(a: &[String]) -> Result<Rig, String> {
        let (calib, cpath) = DeviceCalib::load_default(arg(a, "--calib").map(Path::new))
            .map_err(|e| format!("headset calibration: {e}"))?;
        if calib.cameras.len() < TILE_COUNT {
            return Err(format!("headset calibration {}: {} cameras, the tracker needs {TILE_COUNT}", cpath.display(), calib.cameras.len()));
        }
        let env = |k: &str| std::env::var(k).ok();
        let (models, source) = LedModel::resolve(
            arg(a, "--left").map(Path::new),
            arg(a, "--right").map(Path::new),
            &env,
        )?;
        let mut cams: Vec<Cam> = Vec::with_capacity(TILE_COUNT);
        for i in 0..TILE_COUNT {
            let body_from_cam = calib
                .body_from_cam(i, RigConvention::CamFromCam0)
                .ok_or_else(|| format!("headset calibration {}: no camera/IMU extrinsic (ombc/tbc)", cpath.display()))?;
            cams.push(Cam {
                intr: calib.cameras[i].intr.clone(),
                cam_from_body: body_from_cam.inverse(),
            });
        }
        let mut ext_note = "factory extrinsics".to_string();
        if !a.iter().any(|s| s == "--factory") {
            // the user's own fit, else the shipped fit for the reference headset (its factory extrinsics of cams 1-3
            // are off by up to 3.2 deg, which tilts the controllers; lost once by a clean reinstall, 2026-10-17)
            let (v, note) = extrinsics::resolve(&calib, arg(a, "--extrinsics").map(Path::new), &env);
            for (id, pose) in &v {
                if *id < cams.len() {
                    cams[*id].cam_from_body = *pose;
                }
            }
            ext_note = note;
        }
        let triples: Vec<Vec<[usize; 3]>> = models
            .iter()
            .map(|m| m.neighbour_triples(0.045, 110.0))
            .collect();
        eprintln!(
            "calibration {}{} ({ext_note}) | models {} ({} LEDs) / {} ({} LEDs) | LED triples {} / {}",
            cpath.display(),
            if calib.is_reference() { " (the shipped Dream Air reference)" } else { "" },
            models[0].hand,
            models[0].leds.len(),
            models[1].hand,
            models[1].leds.len(),
            triples[0].len(),
            triples[1].len()
        );
        eprintln!("controller models: {}", source.describe());
        Ok(Rig {
            calib,
            cams,
            models,
            triples,
            prm: Params::default(),
        })
    }
}

pub fn arg<'a>(a: &'a [String], k: &str) -> Option<&'a str> {
    a.iter()
        .position(|s| s == k)
        .and_then(|i| a.get(i + 1))
        .map(|s| s.as_str())
}

pub fn track(a: &[String]) {
    let path = &a[0];
    let start: usize = arg(a, "--start").map(|v| v.parse().unwrap()).unwrap_or(0);
    let count: usize = arg(a, "--count")
        .map(|v| v.parse().unwrap())
        .unwrap_or(usize::MAX);
    let every: usize = arg(a, "--every").map(|v| v.parse().unwrap()).unwrap_or(1);
    let use_prior = !a.iter().any(|s| s == "--no-prior");
    let rig = Rig::load(a);
    let (cams, models, triples, prm) = (rig.cams.clone(), &rig.models, &rig.triples, rig.prm);
    let calib = &rig.calib;

    // pass 1: blobs of every LED frame
    let t0 = std::time::Instant::now();
    let mut r = Reader::open(Path::new(path)).expect("open");
    let bp = BlobParams::default();
    let mut tile = vec![0u8; TILE_W * TILE_H];
    let mut frames = Vec::new();
    let (mut i, mut nled) = (0usize, 0usize);
    while let Some(rec) = r.next_record().expect("read") {
        i += 1;
        let Some(m) = Meta::parse(&rec.data[..frame::META_LEN]) else {
            continue;
        };
        if m.kind() != FrameKind::Led {
            continue;
        }
        nled += 1;
        if nled <= start || (nled - start - 1) % every != 0 {
            continue;
        }
        if frames.len() >= count {
            break;
        }
        let rows = frame::tile_rows(Some(&m), &rec.data, rec.stride);
        let mut px = Vec::new();
        for t in 0..TILE_COUNT {
            frame::copy_tile(&rec.data, rec.stride, t, rows.y[t], &mut tile, TILE_W);
            px.push(
                blob::detect(&tile, TILE_W, TILE_H, TILE_W, &bp)
                    .iter()
                    .map(|b| (b.x, b.y))
                    .collect(),
            );
        }
        frames.push(LedFrame {
            rec: i,
            ts_us: m.ts_us,
            px,
        });
    }
    eprintln!(
        "pass 1: {} LED frames, blobs in {:.1} s",
        frames.len(),
        t0.elapsed().as_secs_f64()
    );

    // pass 2: brute-force acquisition per frame, in parallel
    let t1 = std::time::Instant::now();
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let mut acq: Vec<Vec<Found>> = vec![Vec::new(); frames.len()];
    let results = std::sync::Mutex::new(&mut acq);
    std::thread::scope(|sc| {
        for _ in 0..nthreads {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if k >= frames.len() {
                    break;
                }
                let f = solve_frame(&frames[k], &cams, models, triples, &prm, None, &[], false);
                results.lock().unwrap()[k] = f;
            });
        }
    });
    eprintln!(
        "pass 2: acquisition in {:.1} s on {nthreads} threads",
        t1.elapsed().as_secs_f64()
    );

    // pass 3: temporal pass with priors (tracking): keep the acquisition result unless the prior gives more matches
    let mut out: Vec<Vec<Found>> = acq.clone();
    if use_prior {
        let mut prev: Vec<Option<(usize, Pose, u64)>> = vec![None, None];
        for (k, f) in frames.iter().enumerate() {
            let mut tracked = solve_frame(f, &cams, models, triples, &prm, Some(&prev), &[], false);
            // fill hands that the prior lost from acquisition
            for fa in &acq[k] {
                if !tracked.iter().any(|t| same_object(t, fa)) {
                    tracked.push(fa.clone());
                }
            }
            tracked.truncate(2);
            prev = vec![None, None];
            for (slot, t) in tracked.iter().enumerate() {
                prev[slot] = Some((t.model, t.joint.body_from_model, f.ts_us));
            }
            out[k] = tracked;
        }
    }

    // optional: refit the extrinsics of cameras 1..3 (camera 0 fixed) on the controller poses, then re-solve
    let mut cams = cams;
    if a.iter().any(|s| s == "--fit-extrinsics") {
        let orig: Vec<Pose> = cams.iter().map(|c| c.cam_from_body).collect();
        for round in 0..4 {
            for c in 1..TILE_COUNT {
                let mut pts = Vec::new();
                for (k, f) in frames.iter().enumerate() {
                    for fo in &out[k] {
                        if fo.joint.cams() < 2 {
                            continue;
                        }
                        let cfm = cams[c].cam_from_body.compose(&fo.joint.body_from_model);
                        let ob = CamObs::new(&cams[c], f.px[c].clone());
                        let none = vec![false; ob.px.len()];
                        for (l, b) in solve::match_pixels(
                            &cfm,
                            &models[fo.model],
                            &cams[c],
                            &ob,
                            &none,
                            prm.facing_deg.to_radians().cos(),
                            8.0,
                        ) {
                            pts.push((
                                fo.joint.body_from_model.apply(models[fo.model].leds[l].p),
                                ob.px[b],
                            ));
                        }
                    }
                }
                if pts.len() < 30 {
                    eprintln!(
                        "extrinsics: cam{c}: only {} points, not refitted",
                        pts.len()
                    );
                    continue;
                }
                let raw = Cam {
                    intr: cams[c].intr.clone(),
                    cam_from_body: Pose::IDENTITY,
                };
                let o: Vec<solve::Obs> = pts
                    .iter()
                    .map(|(p, px)| solve::Obs {
                        cam: &raw,
                        p: *p,
                        px: *px,
                    })
                    .collect();
                let (np, rms) = solve::refine(cams[c].cam_from_body, &o, false, 30);
                eprintln!(
                    "extrinsics round {round}: cam{c}: {} points, rms {rms:.2} px",
                    pts.len()
                );
                cams[c].cam_from_body = np;
            }
            for (k, f) in frames.iter().enumerate() {
                let obs: Vec<CamObs> = cams
                    .iter()
                    .zip(&f.px)
                    .map(|(c, px)| CamObs::new(c, px.clone()))
                    .collect();
                let mut used: Vec<Vec<bool>> =
                    obs.iter().map(|o| vec![false; o.px.len()]).collect();
                for fo in out[k].iter_mut() {
                    if let Some(j) = solve::joint_from_prior(
                        fo.joint.body_from_model,
                        &models[fo.model],
                        &cams,
                        &obs,
                        &used,
                        prm.tol_px * 2.0,
                        prm.facing_deg,
                    )
                    .and_then(|j| {
                        solve::joint_from_prior(
                            j.body_from_model,
                            &models[fo.model],
                            &cams,
                            &obs,
                            &used,
                            prm.tol_px,
                            prm.facing_deg,
                        )
                    }) {
                        claim(&mut used, &j);
                        fo.joint = j;
                    }
                }
            }
        }
        if let Some(sp) = arg(a, "--save-extrinsics") {
            let v: Vec<(usize, Pose)> = (1..TILE_COUNT)
                .map(|c| (c, cams[c].cam_from_body))
                .collect();
            let note = format!(
                "fitted on controller poses of {path} ({} LED frames)",
                frames.len()
            );
            match extrinsics::save(Path::new(sp), &calib.uid, &v, &note) {
                Ok(()) => println!("extrinsics saved to {sp}"),
                Err(e) => eprintln!("could not save {sp}: {e}"),
            }
        }
        for c in 1..TILE_COUNT {
            // correction about the camera's own centre: cam_from_body' = D * cam_from_body
            let d = cams[c].cam_from_body.compose(&orig[c].inverse());
            let ang = geom::angle_deg([0.0, 0.0, 1.0], geom::mat_vec(&d.r, [0.0, 0.0, 1.0]));
            let centre_shift = geom::sub(cams[c].cam_from_body.inverse().t, orig[c].inverse().t);
            println!(
                "extrinsic correction cam{c}: optical axis moved {ang:.2} deg, rotation {:.2} deg, centre moved ({:+.1} {:+.1} {:+.1}) mm in the body frame",
                geom::norm(rotvec(&d.r)).to_degrees(),
                centre_shift[0] * 1e3,
                centre_shift[1] * 1e3,
                centre_shift[2] * 1e3
            );
        }
    }

    // report
    let mut csv: Box<dyn Write> = match arg(a, "--csv") {
        Some(p) => Box::new(std::io::BufWriter::new(
            std::fs::File::create(p).expect("csv"),
        )),
        None => Box::new(std::io::sink()),
    };
    writeln!(csv, "record,ts_s,hand,lr_by_position,cams,leds,rms_px,alt_rms_px,alt_leds,cam_spread_mm,x_m,y_m,z_m,qx,qy,qz,qw").unwrap();
    let mut rms_all = Vec::new();
    let mut leds_all = Vec::new();
    let mut spread = Vec::new();
    let mut nposes = [0usize; 3];
    let mut acq_n = [0usize; 3];
    let mut lr_agree = (0usize, 0usize);
    let mut steps: Vec<f64> = Vec::new();
    let mut last: [Option<(geom::Vec3, u64)>; 2] = [None, None];
    for (k, f) in frames.iter().enumerate() {
        nposes[out[k].len().min(2)] += 1;
        acq_n[acq[k].len().min(2)] += 1;
        let mut found = out[k].clone();
        found.sort_by(|a, b| {
            a.joint.body_from_model.t[0]
                .partial_cmp(&b.joint.body_from_model.t[0])
                .unwrap()
        });
        for (pi, fo) in found.iter().enumerate() {
            let by_pos = if found.len() == 2 {
                ["L", "R"][pi]
            } else {
                "-"
            };
            let hand = &models[fo.model].hand[..1];
            if found.len() == 2 {
                lr_agree.1 += 1;
                if hand == by_pos {
                    lr_agree.0 += 1;
                }
            }
            let p = &fo.joint.body_from_model;
            let q = geom::quat_from_mat(&p.r);
            rms_all.push(fo.joint.rms_px);
            leds_all.push(fo.joint.total() as f64);
            if fo.single >= 2 {
                spread.push(fo.cam_spread_m * 1000.0);
            }
            let slot = if by_pos == "R" { 1 } else { 0 };
            if found.len() == 2 {
                if let Some((lp, lt)) = last[slot] {
                    if f.ts_us - lt < 50_000 {
                        steps.push(geom::norm(geom::sub(p.t, lp)) * 1000.0);
                    }
                }
                last[slot] = Some((p.t, f.ts_us));
            }
            writeln!(
                csv,
                "{},{:.6},{},{},{},{},{:.3},{},{},{:.1},{:.4},{:.4},{:.4},{:.5},{:.5},{:.5},{:.5}",
                f.rec,
                f.ts_us as f64 * 1e-6,
                models[fo.model].hand,
                by_pos,
                fo.joint.cams(),
                fo.joint.total(),
                fo.joint.rms_px,
                fo.alt
                    .as_ref()
                    .map(|x| format!("{:.3}", x.rms_px))
                    .unwrap_or_default(),
                fo.alt
                    .as_ref()
                    .map(|x| x.total().to_string())
                    .unwrap_or_default(),
                fo.cam_spread_m * 1000.0,
                p.t[0],
                p.t[1],
                p.t[2],
                q[0],
                q[1],
                q[2],
                q[3]
            )
            .unwrap();
            if k % 25 == 0 {
                println!(
                    "rec {:5} t {:8.3}  {}({}) cams {} LEDs {:2} rms {:.2} px  pos ({:+.3} {:+.3} {:+.3}) m  alt {}",
                    f.rec,
                    f.ts_us as f64 * 1e-6,
                    models[fo.model].hand,
                    by_pos,
                    fo.joint.cams(),
                    fo.joint.total(),
                    fo.joint.rms_px,
                    p.t[0],
                    p.t[1],
                    p.t[2],
                    fo.alt.as_ref().map(|x| format!("{} LEDs {:.2} px", x.total(), x.rms_px)).unwrap_or("-".into())
                );
            }
        }
    }
    // per-camera bias: blobs within 15 px of the projected facing LEDs of each final pose
    let mut bias: Vec<(Vec<f64>, Vec<f64>, usize, usize)> =
        vec![(Vec::new(), Vec::new(), 0, 0); TILE_COUNT];
    for (k, f) in frames.iter().enumerate() {
        for fo in &out[k] {
            for t in 0..TILE_COUNT {
                let cfm = cams[t].cam_from_body.compose(&fo.joint.body_from_model);
                let ob = CamObs::new(&cams[t], f.px[t].clone());
                let none = vec![false; ob.px.len()];
                let m = solve::match_pixels(
                    &cfm,
                    &models[fo.model],
                    &cams[t],
                    &ob,
                    &none,
                    prm.facing_deg.to_radians().cos(),
                    15.0,
                );
                bias[t].3 += fo.joint.matches[t].len();
                for (l, b) in m {
                    let (u, v) = cams[t]
                        .intr
                        .project(cfm.apply(models[fo.model].leds[l].p))
                        .unwrap();
                    bias[t].0.push(ob.px[b].0 - u);
                    bias[t].1.push(ob.px[b].1 - v);
                    bias[t].2 += 1;
                }
            }
        }
    }
    for (t, (du, dv, n, used)) in bias.iter_mut().enumerate() {
        du.sort_by(|a, b| a.partial_cmp(b).unwrap());
        dv.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = |v: &Vec<f64>| {
            if v.is_empty() {
                f64::NAN
            } else {
                v[v.len() / 2]
            }
        };
        println!("cam{t}: blobs within 15 px of a projected LED {n}, used in the joint fit {used}, median offset blob-proj ({:+.2}, {:+.2}) px", med(du), med(dv));
    }
    if let Some(dir) = arg(a, "--overlay") {
        let oe: usize = arg(a, "--overlay-every")
            .map(|v| v.parse().unwrap())
            .unwrap_or(50);
        overlay(path, dir, oe, &frames, &out, &cams, models, &prm);
    }
    let pct = |v: &mut Vec<f64>, q: f64| {
        if v.is_empty() {
            return f64::NAN;
        }
        v.sort_by(|a, b| a.partial_cmp(b).unwrap());
        v[((v.len() - 1) as f64 * q).round() as usize]
    };
    let n = frames.len().max(1) as f64;
    println!("== summary over {} LED frames", frames.len());
    println!(
        "controllers found per frame (tracking): 0: {:.1} %  1: {:.1} %  2: {:.1} %",
        100.0 * nposes[0] as f64 / n,
        100.0 * nposes[1] as f64 / n,
        100.0 * nposes[2] as f64 / n
    );
    println!(
        "controllers found per frame (acquisition only, no prior): 0: {:.1} %  1: {:.1} %  2: {:.1} %",
        100.0 * acq_n[0] as f64 / n,
        100.0 * acq_n[1] as f64 / n,
        100.0 * acq_n[2] as f64 / n
    );
    println!(
        "joint reprojection RMS px: median {:.2}  p90 {:.2}  p99 {:.2}",
        pct(&mut rms_all, 0.5),
        pct(&mut rms_all, 0.9),
        pct(&mut rms_all, 0.99)
    );
    println!(
        "matched LEDs per pose (all cameras): median {:.0}  p10 {:.0}",
        pct(&mut leds_all, 0.5),
        pct(&mut leds_all, 0.1)
    );
    println!(
        "single-camera solve vs joint pose (seen by >=2 cams): median {:.1} mm  p90 {:.1} mm  (n={})",
        pct(&mut spread.clone(), 0.5),
        pct(&mut spread, 0.9),
        spread.len()
    );
    println!(
        "frame-to-frame step (40 ms): median {:.1} mm  p90 {:.1} mm  p99 {:.1} mm",
        pct(&mut steps.clone(), 0.5),
        pct(&mut steps.clone(), 0.9),
        pct(&mut steps, 0.99)
    );
    println!(
        "left/right: shape-fit label agrees with the left/right position in {}/{} poses",
        lr_agree.0, lr_agree.1
    );
}

fn same_object(a: &Found, b: &Found) -> bool {
    geom::norm(geom::sub(
        a.joint.body_from_model.t,
        b.joint.body_from_model.t,
    )) < 0.06
}

/// Solve one LED frame. With `prior`, track each previous controller first; acquisition is skipped.
pub fn solve_frame(
    f: &LedFrame,
    cams: &[Cam],
    models: &[LedModel; 2],
    triples: &[Vec<[usize; 3]>],
    prm: &Params,
    prior: Option<&Vec<Option<(usize, Pose, u64)>>>,
    pre: &[Found],
    parallel: bool,
) -> Vec<Found> {
    let obs: Vec<CamObs> = cams
        .iter()
        .zip(&f.px)
        .map(|(c, px)| CamObs::new(c, px.clone()))
        .collect();
    let mut used: Vec<Vec<bool>> = obs.iter().map(|o| vec![false; o.px.len()]).collect();
    for p in pre {
        claim(&mut used, &p.joint);
    }
    let accept = |j: &JointSolve| j.total() >= 5 && j.rms_px <= prm.tol_px;
    let mut out: Vec<Found> = Vec::new();
    if let Some(prior) = prior {
        for (mi, pose, _) in prior.iter().flatten() {
            let Some(j) = solve::joint_from_prior(
                *pose,
                &models[*mi],
                cams,
                &obs,
                &used,
                prm.prior_tol_px,
                prm.facing_deg,
            ) else {
                continue;
            };
            // tighten: rematch at the normal tolerance from the refined pose
            let Some(j) = solve::joint_from_prior(
                j.body_from_model,
                &models[*mi],
                cams,
                &obs,
                &used,
                prm.tol_px,
                prm.facing_deg,
            ) else {
                continue;
            };
            if accept(&j) {
                claim(&mut used, &j);
                out.push(Found {
                    model: *mi,
                    joint: j,
                    alt: None,
                    cam_spread_m: 0.0,
                    single: 0,
                });
            }
        }
        return out;
    }
    // single-camera acquisition: per camera, the better model first, then the other on the remaining blobs
    let per_cam = |ci: usize| -> Vec<(usize, CamSolve)> {
        let cam = &cams[ci];
        let mut u = used[ci].clone();
        let mut got = Vec::new();
        for _round in 0..2 {
            let mut best: Option<(usize, CamSolve)> = None;
            for (mi, m) in models.iter().enumerate() {
                if let Some(s) = solve::acquire(m, &triples[mi], cam, ci, &obs[ci], &u, prm) {
                    let better = match &best {
                        None => true,
                        Some((_, b)) => {
                            s.matches.len() > b.matches.len()
                                || (s.matches.len() == b.matches.len() && s.rms_px < b.rms_px)
                        }
                    };
                    if better {
                        best = Some((mi, s));
                    }
                }
            }
            if std::env::var_os("CT_DEBUG").is_some() {
                eprintln!(
                    "  cam{ci} round {_round}: blobs {} -> {:?}",
                    obs[ci].px.len(),
                    best.as_ref()
                        .map(|(m, s)| (*m, s.matches.len(), s.rms_px, s.cam_from_model.t))
                );
            }
            let Some((mi, s)) = best else { break };
            for &(_, b) in &s.matches {
                u[b] = true;
            }
            got.push((mi, s));
        }
        got
    };
    let mut cands: Vec<(usize, CamSolve)> = if parallel {
        std::thread::scope(|sc| {
            let hs: Vec<_> = (0..cams.len())
                .map(|ci| sc.spawn(move || per_cam(ci)))
                .collect();
            hs.into_iter().flat_map(|h| h.join().unwrap()).collect()
        })
    } else {
        (0..cams.len()).flat_map(per_cam).collect()
    };
    cands.sort_by(|a, b| {
        b.1.matches
            .len()
            .cmp(&a.1.matches.len())
            .then(a.1.rms_px.partial_cmp(&b.1.rms_px).unwrap())
    });
    let body = |c: &CamSolve| {
        cams[c.cam]
            .cam_from_body
            .inverse()
            .compose(&c.cam_from_model)
    };
    for (mi, c) in &cands {
        if out.len() + pre.len() >= 2 {
            break;
        }
        let start = body(c);
        if out
            .iter()
            .any(|o| geom::norm(geom::sub(o.joint.body_from_model.t, start.t)) < 0.06)
        {
            continue;
        }
        let Some(j) = solve::joint_from_prior(
            start,
            &models[*mi],
            cams,
            &obs,
            &used,
            prm.tol_px * 2.0,
            prm.facing_deg,
        )
        .and_then(|j| {
            solve::joint_from_prior(
                j.body_from_model,
                &models[*mi],
                cams,
                &obs,
                &used,
                prm.tol_px,
                prm.facing_deg,
            )
        }) else {
            continue;
        };
        if !accept(&j) {
            continue;
        }
        let other = 1 - mi;
        let alt = solve::joint_from_prior(
            start,
            &models[other],
            cams,
            &obs,
            &used,
            prm.tol_px * 2.0,
            prm.facing_deg,
        )
        .and_then(|x| {
            solve::joint_from_prior(
                x.body_from_model,
                &models[other],
                cams,
                &obs,
                &used,
                prm.tol_px,
                prm.facing_deg,
            )
        });
        // the same controller's single-camera solves from other cameras
        let mut spread = 0.0f64;
        let mut single = 0;
        for (mj, c2) in &cands {
            let b2 = body(c2);
            if *mj == *mi && geom::norm(geom::sub(b2.t, j.body_from_model.t)) < 0.06 {
                single += 1;
                spread = spread.max(geom::norm(geom::sub(b2.t, j.body_from_model.t)));
            }
        }
        claim(&mut used, &j);
        out.push(Found {
            model: *mi,
            joint: j,
            alt,
            cam_spread_m: spread,
            single,
        });
    }
    out
}

pub(crate) fn claim(used: &mut [Vec<bool>], j: &JointSolve) {
    for (ci, m) in j.matches.iter().enumerate() {
        for &(_, b) in m {
            used[ci][b] = true;
        }
    }
}

/// Write mosaics of every `oe`-th solved LED frame: blobs red boxes, projected facing LEDs green (matched) or
/// blue (unmatched), the left controller's LEDs as filled dots, the right's as rings.
#[allow(clippy::too_many_arguments)]
fn overlay(
    path: &str,
    dir: &str,
    oe: usize,
    frames: &[LedFrame],
    out: &[Vec<Found>],
    cams: &[Cam],
    models: &[LedModel; 2],
    prm: &Params,
) {
    std::fs::create_dir_all(dir).expect("overlay dir");
    let want: std::collections::HashMap<usize, usize> = frames
        .iter()
        .enumerate()
        .filter(|(k, _)| k % oe == 0)
        .map(|(k, f)| (f.rec, k))
        .collect();
    let mut r = Reader::open(Path::new(path)).expect("open");
    let mut i = 0usize;
    let w = TILE_W * TILE_COUNT;
    let cos_face = prm.facing_deg.to_radians().cos();
    while let Some(rec) = r.next_record().expect("read") {
        i += 1;
        let Some(&k) = want.get(&i) else { continue };
        let m = Meta::parse(&rec.data[..frame::META_LEN]).expect("meta");
        let rows = frame::tile_rows(Some(&m), &rec.data, rec.stride);
        let mut rgb = vec![0u8; w * TILE_H * 3];
        let mut tile = vec![0u8; TILE_W * TILE_H];
        let put = |rgb: &mut Vec<u8>, t: usize, x: i64, y: i64, c: [u8; 3]| {
            if x >= 0 && y >= 0 && (x as usize) < TILE_W && (y as usize) < TILE_H {
                let o = (y as usize * w + t * TILE_W + x as usize) * 3;
                rgb[o..o + 3].copy_from_slice(&c);
            }
        };
        for t in 0..TILE_COUNT {
            frame::copy_tile(&rec.data, rec.stride, t, rows.y[t], &mut tile, TILE_W);
            for y in 0..TILE_H {
                for x in 0..TILE_W {
                    let v = tile[y * TILE_W + x] / 2;
                    put(&mut rgb, t, x as i64, y as i64, [v, v, v]);
                }
            }
            for &(bx, by) in &frames[k].px[t] {
                let (cx, cy) = (bx.round() as i64, by.round() as i64);
                for d in -6i64..=6 {
                    for (x, y) in [
                        (cx + d, cy - 6),
                        (cx + d, cy + 6),
                        (cx - 6, cy + d),
                        (cx + 6, cy + d),
                    ] {
                        put(&mut rgb, t, x, y, [255, 40, 40]);
                    }
                }
            }
            for fo in &out[k] {
                let model = &models[fo.model];
                let cfm = cams[t].cam_from_body.compose(&fo.joint.body_from_model);
                for (li, led) in model.leds.iter().enumerate() {
                    let pc = cfm.apply(led.p);
                    let n = geom::mat_vec(&cfm.r, led.n);
                    if pc[2] <= 0.0
                        || geom::dot(n, geom::scale(geom::normalize(pc), -1.0)) < cos_face
                    {
                        continue;
                    }
                    let Some((u, v)) = cams[t].intr.project(pc) else {
                        continue;
                    };
                    let matched = fo.joint.matches[t].iter().any(|&(l, _)| l == li);
                    let c = if matched {
                        [40, 255, 40]
                    } else {
                        [60, 140, 255]
                    };
                    let (cx, cy) = (u.round() as i64, v.round() as i64);
                    for dy in -3i64..=3 {
                        for dx in -3i64..=3 {
                            let r2 = dx * dx + dy * dy;
                            let ring = (5..=9).contains(&r2);
                            if (fo.model == 0 && r2 <= 4) || (fo.model == 1 && ring) {
                                put(&mut rgb, t, cx + dx, cy + dy, c);
                            }
                        }
                    }
                }
            }
        }
        let p = Path::new(dir).join(format!("rec{:05}.png", i));
        pimax_cam::png::write_rgb(&p, w, TILE_H, &rgb).expect("png");
    }
}

fn rotvec(r: &geom::Mat3) -> geom::Vec3 {
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
