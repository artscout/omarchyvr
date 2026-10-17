//! `omarchyvr-roommap`: persistent room map (save the room, recognise it next time, restore the world origin).
//!
//! ```text
//! omarchyvr-roommap build      (--rec DIR [--tracking CSV] | --source broker [--seconds 60]) [--name NAME | --map DIR]
//!                          [--every N] [--kf-dist M] [--kf-angle DEG] [--no-loops] [--refine [--keep-unrefined]]
//!                          [--min-keyframes N --extend S] [--append]
//!     (--append: grow the saved map with this session's views, room frame from last-reloc.toml)
//!     map the room: stereo ORB keyframes, loop closure (4-DoF pose graph), optional multi-view point refinement
//!     -> ~/.local/share/omarchyvr/roommap/NAME/map.bin
//! omarchyvr-roommap relocalize (--rec DIR [--tracking CSV] | --source broker [--seconds 15]) [--name NAME | --map DIR]
//!                          [--every N] [--window S] [--threads N] [--result FILE] [--apply [--no-floor]]
//!     recognise the room: room_from_slam (yaw + translation) pooled over the frames of a window
//!     -> NAME/last-reloc.toml (offline also: whole-session fit and per-window consistency)
//! omarchyvr-roommap apply      [--name NAME | --map DIR] [--result FILE] [--pid N] [--dry-run] [--no-floor]
//!     write the Monado world offset ($XDG_RUNTIME_DIR/pimax-anchor.offset, PIMAX_ANCHOR=1 hook)
//! omarchyvr-roommap info       [--name NAME | --map DIR]
//! omarchyvr-roommap export    [--name NAME | --map DIR] [--out FILE.json]
//!     the map as JSON (merged points with times-seen, keyframe path, floor, anchors, metadata); default NAME/map.json
//! omarchyvr-roommap view      [--name NAME | --map DIR] [--out FILE.html] [--open]
//!     ONE self-contained HTML5 page (WebGL, no network): 3D orbit view, floor plan, height colours;
//!     default NAME/map.html; --open shows it with xdg-open
//! omarchyvr-roommap floor      detect|drift|touch|set|show|follow|preview ...   (omarchyvr-roommap floor: details)
//!     the floor: from the map points, a controller on the floor, or typed in; stored as NAME/floor.toml and
//!     applied with the room (relocalize --apply / apply put the floor on Monado's STAGE floor; --no-floor: not)
//! omarchyvr-roommap setup      fast|full|show [--name NAME] [--label TEXT]   (omarchyvr-roommap setup: details)
//!     guided room setup in the headset: floor, play area (boundary), centre and forward -> NAME/room-setup.toml;
//!     the room offset then puts the play area's centre at the STAGE origin, forward = -Z (docs/ROOM-SETUP.md)
//! omarchyvr-roommap auto       [--name NAME] [--wait S]
//!     session start: recognise the saved room (retrying) and apply it; $XDG_RUNTIME_DIR/omarchyvr-room.status
//! omarchyvr-roommap light      [--source broker|FILE.pxcam] [--seconds 5] [--every N] [--no-stretch] [--darken F [--noise G]]
//!     can the room be mapped now? brightness, exposure/gain, ORB corners and stereo points per frame set (no pose)
//! omarchyvr-roommap cycle      RESULT.toml RESULT.toml [RESULT.toml ...]
//!     compose relocalisation results around a cycle of sessions (A->B, B->A, ...); identity = consistent
//! common: --calib XML (default: the headset calibration)
//! ```
//! Offline input: a recording of `run_slam.sh live|record` and its Basalt trajectory (`run_slam.sh replay DIR`
//! writes DIR/replay-*/tracking.csv; the newest one is used). Live input: `pimax-cam broker` frames and Monado's
//! head ring (`PIMAX_ANCHOR=1`). Nothing is sent to the headset.

use pimax_cam::calib::DeviceCalib;
use omarchyvr_roommap::align::{self, first_fix, joint_fit, window_fits, AcceptParams, FrameObs, JointFit, JointParams, ObserveParams};
use omarchyvr_roommap::pipeline::{self, build_params};
use omarchyvr_roommap::rec::Recording;
use omarchyvr_roommap::loopclose::{close_loops, correction_at, LoopParams};
use omarchyvr_roommap::refine::{refine_map, RefineParams, RefineStats};
use omarchyvr_roommap::store::{self, RelocResult};
use omarchyvr_roommap::live;
use omarchyvr_room::map::RoomMap;
use omarchyvr_room::orb::OrbParams;
use omarchyvr_room::session::{Alignment, RoomSession};
use omarchyvr_room::stereo::Rig;
use std::path::{Path, PathBuf};

mod floorcmd;
mod setupcmd;

type R<T> = Result<T, String>;

/// Who this process writes the room offset as (the writer lease, omarchyvr_anchor::world): set once by `setup`,
/// `auto` and `floor follow`; `relocalize --apply` and `apply` on their own are user actions.
static WRITER_ROLE: std::sync::OnceLock<omarchyvr_anchor::world::Role> = std::sync::OnceLock::new();

fn writer_role(default: omarchyvr_anchor::world::Role) -> omarchyvr_anchor::world::Role {
    *WRITER_ROLE.get().unwrap_or(&default)
}

/// Take (or keep) this process's writer lease of Monado session `pid`; Err in plain words when another writer has
/// it. The first role taken stays this process's role.
fn take_lease(pid: u32, role: omarchyvr_anchor::world::Role) -> R<()> {
    let role = *WRITER_ROLE.get_or_init(|| role);
    match omarchyvr_anchor::world::take_lease(&omarchyvr_anchor::world::offset_path(), pid, role) {
        Ok(Some(note)) => {
            println!("{note}");
            Ok(())
        }
        Ok(None) => Ok(()),
        Err(e) => Err(format!("{e}; offset unchanged")),
    }
}

fn opt(v: &mut Vec<String>, name: &str) -> R<Option<String>> {
    match v.iter().position(|a| a == name) {
        Some(i) if i + 1 < v.len() => {
            let x = v.remove(i + 1);
            v.remove(i);
            Ok(Some(x))
        }
        Some(_) => Err(format!("{name} needs a value")),
        None => Ok(None),
    }
}

fn num<T: std::str::FromStr>(v: &mut Vec<String>, name: &str, d: T) -> R<T> {
    match opt(v, name)? {
        Some(x) => x.parse().map_err(|_| format!("{name}: bad number {x:?}")),
        None => Ok(d),
    }
}

fn flag(v: &mut Vec<String>, name: &str) -> bool {
    match v.iter().position(|a| a == name) {
        Some(i) => {
            v.remove(i);
            true
        }
        None => false,
    }
}

fn done(v: &[String]) -> R<()> {
    match v.first() {
        Some(a) => Err(format!("unexpected argument {a:?}")),
        None => Ok(()),
    }
}

fn rig(v: &mut Vec<String>) -> R<(DeviceCalib, Rig)> {
    let p = opt(v, "--calib")?;
    let (c, used) = DeviceCalib::load_default(p.as_deref().map(Path::new)).map_err(|e| e.to_string())?;
    eprintln!("calibration {} ({})", used.display(), c.uid);
    let r = Rig::from_calib(&c, &[0, 1])?;
    Ok((c, r))
}

fn map_dir(v: &mut Vec<String>) -> R<PathBuf> {
    Ok(match (opt(v, "--map")?, opt(v, "--name")?) {
        (Some(d), _) => PathBuf::from(d),
        (None, n) => store::map_dir(n.as_deref().unwrap_or("home")),
    })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn fmt_align(a: &Alignment) -> String {
    format!("yaw {:+8.3} deg  t ({:+.4} {:+.4} {:+.4}) m", a.yaw.to_degrees(), a.t[0], a.t[1], a.t[2])
}

fn cmd_build(v: Vec<String>) -> R<()> {
    cmd_build_with(v, None)
}

/// `build` with live progress for the room setup's headset panel (live source only).
pub(crate) fn cmd_build_with(mut v: Vec<String>, mut progress: Option<&mut dyn FnMut(&omarchyvr_roommap::light::MapProgress)>) -> R<()> {
    let (cal, rig) = rig(&mut v)?;
    let dir = map_dir(&mut v)?;
    let every: usize = num(&mut v, "--every", 1)?;
    let kf_dist: f64 = num(&mut v, "--kf-dist", 0.20)?;
    let kf_angle: f64 = num(&mut v, "--kf-angle", 15.0)?;
    let rec = opt(&mut v, "--rec")?;
    let tracking = opt(&mut v, "--tracking")?;
    let source = opt(&mut v, "--source")?;
    let seconds: f64 = num(&mut v, "--seconds", 60.0)?;
    let no_refine = !flag(&mut v, "--refine");
    let no_loops = flag(&mut v, "--no-loops");
    let keep_unrefined = flag(&mut v, "--keep-unrefined");
    let threads: usize = num(&mut v, "--threads", 8)?;
    let min_kf: usize = num(&mut v, "--min-keyframes", 0)?;
    let extend: f64 = num(&mut v, "--extend", 0.0)?;
    let append = flag(&mut v, "--append");
    done(&v)?;
    let rp = RefineParams { drop_unrefined: !keep_unrefined, ..Default::default() };
    let mut sp = build_params(kf_dist, kf_angle);
    // --append: grow the saved map with this session's views in the room frame of last-reloc.toml (a later session
    // at night or with other lights adds its own look of the room; old keyframes are kept, no loop closure)
    if append && rec.is_some() {
        return Err("--append works with --source broker only".into());
    }
    let appending = if append {
        let m = RoomMap::load(&dir)?;
        let r = RelocResult::load(&dir.join("last-reloc.toml"))?;
        sp.kf_per_session = true;
        Some((m, r))
    } else {
        None
    };
    let no_loops = no_loops || appending.is_some();
    let map0 = RoomMap::new(&cal.uid, &[0, 1]);
    let t0 = std::time::Instant::now();
    let mut corrections = Vec::new();
    // stereo points of every 3rd frame (room frame before loop closure) for the floor
    let mut floor_frames: Vec<pipeline::PointFrame> = Vec::new();
    let (map, what) = match (rec, source) {
        (Some(r), _) => {
            let rec = Recording::open(Path::new(&r), tracking.as_deref().map(Path::new))?;
            eprintln!(
                "{}: {} frame sets, {} poses ({})",
                rec.root.display(),
                rec.frames.len(),
                rec.traj.len(),
                rec.tracking.display()
            );
            let id = Alignment { yaw: 0.0, t: [0.0; 3] };
            let (mut m, st) = pipeline::build_from_recording(&rec, &rig, map0, sp, every, id)?;
            floor_frames = pipeline::recording_points(&rec, &rig, 3, 3.5, threads)?;
            eprintln!("{} frames used, {} without a tracker pose", st.frames, st.no_pose);
            if !(no_refine && no_loops) {
                let mut frames = pipeline::map_frames(&rec, &rig, 2, &OrbParams::default(), &id, threads)?;
                if !no_loops {
                    let (ls, corr) = close_loops(&mut m, &rig, &mut frames, &ObserveParams::default(), &LoopParams::default(), threads);
                    println!(
                        "loop closure: {} keyframes, {} loop edges ({} rejected), largest keyframe correction {:.1} cm / {:.2} deg",
                        ls.tried, ls.loops, ls.rejected, ls.max_corr_m * 100.0, ls.max_corr_deg
                    );
                    corrections = corr;
                }
                if !no_refine {
                    let rs = refine_map(&mut m, &rig, &frames, &rp, threads);
                    print_refine(&rs);
                }
            }
            (m, rec.root.display().to_string())
        }
        (None, Some(spec)) => {
            println!("Mapping for {seconds:.0} s: walk slowly through the room, look at every wall, the desk and the floor near you.");
            let need = sp.kf_min_points;
            let mut room_from_slam: Option<Alignment> = appending.as_ref().map(|(_, r)| r.align);
            let mut s = match appending {
                Some((m, r)) => RoomSession::with_alignment(m, rig.clone(), sp, r.align),
                None => RoomSession::new(map0, rig.clone(), sp),
            };
            let mut frames = Vec::new();
            let mut k = 0usize;
            let mut kfs = 0usize;
            let mut since_kf = 0usize;
            let mut t_first: Option<i64> = None;
            let mut qs: Vec<omarchyvr_roommap::light::FrameQuality> = Vec::new();
            let orb = OrbParams::default();
            // a dark or plain room: keep mapping up to --extend s more until --min-keyframes views are in
            let st = live::capture(&spec, seconds + extend.max(0.0), |f| {
                let el = (f.ts_ns - *t_first.get_or_insert(f.ts_ns)) as f64 / 1e9;
                // Room frame = the app world now (the offset already active stays valid).
                let a = *room_from_slam.get_or_insert_with(|| live::current_offset(f.pid));
                let views = [image(&f.imgs[0]), image(&f.imgs[1])];
                let ev = s.on_frame(f.ts_ns, &views, &f.slam_from_body, &a, true);
                let new_kf = ev.iter().filter(|e| matches!(e, omarchyvr_room::session::Event::Keyframe(_))).count();
                kfs += new_kf;
                since_kf = if new_kf > 0 { 0 } else { since_kf + 1 };
                if k % 3 == 0 {
                    let feats = pipeline::frame_features(&rig, [&f.imgs[0], &f.imgs[1]], &orb);
                    let rb = a.pose().compose(&f.slam_from_body);
                    floor_frames.push(pipeline::PointFrame { ts_ns: f.ts_ns, pose: rb, pts: pipeline::frame_points(&rig, &feats, &rb, 3.5) });
                    if k.is_multiple_of(15) {
                        let q = omarchyvr_roommap::light::quality_of(&rig, &feats, [&f.imgs[0], &f.imgs[1]]);
                        qs.push(q);
                        let p = omarchyvr_roommap::light::MapProgress {
                            elapsed_s: el,
                            keyframes: kfs,
                            quality: q,
                            advice: omarchyvr_roommap::light::advice(&q, need),
                            move_more: since_kf > 90,
                        };
                        if let Some(cb) = progress.as_mut() {
                            cb(&p);
                        }
                    }
                    if !no_loops && k.is_multiple_of(6) {
                        frames.push(omarchyvr_roommap::refine::MapFrame::new(f.ts_ns, rb, feats, &rig));
                    }
                } else if !no_loops && k & 1 == 0 {
                    let feats = pipeline::frame_features(&rig, [&f.imgs[0], &f.imgs[1]], &orb);
                    frames.push(omarchyvr_roommap::refine::MapFrame::new(f.ts_ns, a.pose().compose(&f.slam_from_body), feats, &rig));
                }
                k += 1;
                el < seconds || kfs < min_kf
            })?;
            eprintln!("{} tracking frames, {} with a Monado head pose (pid {}), {kfs} new keyframes", st.frames, st.posed, st.pid);
            let lr = omarchyvr_roommap::light::summarise(&qs, need);
            if lr.frames > 0 {
                eprintln!(
                    "image: brightness {:.0}/255, ORB corners {} / {}, stereo points {} (a keyframe needs {need}), {} of {} sampled frame sets usable",
                    lr.mean, lr.corners[0], lr.corners[1], lr.stereo, lr.good, lr.frames
                );
            }
            if st.posed == 0 {
                return Err(no_poses_text("nothing saved"));
            }
            if kfs == 0 && lr.frames > 0 {
                let why = match omarchyvr_roommap::light::advice(&omarchyvr_roommap::light::FrameQuality { mean: lr.mean, stereo: lr.stereo, ..Default::default() }, need) {
                    omarchyvr_roommap::light::Advice::TooDark => format!("too dark: brightness {:.0}/255, {} stereo points per view (a view needs {need}); turn on the room lights", lr.mean, lr.stereo),
                    _ => format!("too little detail: {} stereo points per view (a view needs {need}); look at furniture, not blank walls", lr.stereo),
                };
                return Err(format!("no keyframes ({why}); nothing saved"));
            }
            let mut m = s.finish();
            if !no_loops {
                let (ls, corr) = close_loops(&mut m, &rig, &mut frames, &ObserveParams::default(), &LoopParams::default(), threads);
                println!(
                    "loop closure: {} keyframes, {} loop edges ({} rejected), largest keyframe correction {:.1} cm / {:.2} deg",
                    ls.tried, ls.loops, ls.rejected, ls.max_corr_m * 100.0, ls.max_corr_deg
                );
                corrections = corr;
                if !no_refine {
                    print_refine(&refine_map(&mut m, &rig, &frames, &rp, threads));
                }
            }
            (m, format!("{spec} (Monado pid {})", st.pid))
        }
        _ => return Err("build needs --rec DIR or --source broker".into()),
    };
    if map.keyframes.is_empty() {
        return Err("no keyframes (too dark, too little texture or no stereo overlap); nothing saved".into());
    }
    let p = map.save(&dir).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("roommap.toml"), store::meta_text(&what, map.keyframes.len(), map.points(), unix_now()))
        .map_err(|e| e.to_string())?;
    if !append {
        store::save_corrections(&dir, &corrections).map_err(|e| e.to_string())?;
        let _ = std::fs::remove_file(dir.join("floor.toml"));
        floorcmd::floor_at_build(&dir, &map, &floor_frames, &corrections);
    }
    println!(
        "{}: {} keyframes, {} points ({:.1} s)",
        p.display(),
        map.keyframes.len(),
        map.points(),
        t0.elapsed().as_secs_f64()
    );
    Ok(())
}

fn cmd_light(mut v: Vec<String>) -> R<()> {
    let (_, rig) = rig(&mut v)?;
    let spec = opt(&mut v, "--source")?.unwrap_or_else(|| "broker".into());
    let seconds: f64 = num(&mut v, "--seconds", 5.0)?;
    let every: usize = num(&mut v, "--every", 3)?;
    let no_stretch = flag(&mut v, "--no-stretch");
    let darken = match opt(&mut v, "--darken")? {
        Some(x) => Some((x.parse::<f32>().map_err(|_| format!("--darken: bad number {x:?}"))?, num(&mut v, "--noise", 3.0f32)?)),
        None => None,
    };
    done(&v)?;
    let need = build_params(0.2, 15.0).kf_min_points;
    let o = omarchyvr_roommap::light::MeasureOpts { every, low: (!no_stretch).then_some(omarchyvr_room::stereo::LOW_LIGHT), darken };
    let qs = omarchyvr_roommap::light::measure_source(&spec, seconds, &o, &rig, &OrbParams::default())?;
    let r = omarchyvr_roommap::light::summarise(&qs, need);
    println!(
        "{} frame sets: brightness {:.0}/255, exposure {} gain {}, ORB corners {} / {}, stereo points {} (a keyframe needs {need}); {} of {} frame sets usable",
        r.frames, r.mean, r.exposure, r.gain, r.corners[0], r.corners[1], r.stereo, r.good, r.frames
    );
    if let Some(q) = qs.get(qs.len() / 2) {
        println!("{}", omarchyvr_roommap::light::advice_text(omarchyvr_roommap::light::advice(q, need)));
    }
    Ok(())
}

fn print_refine(rs: &RefineStats) {
    println!(
        "refined {} of {} points from multi-view observations (median {:.0} views, median move {:.1} cm), dropped {}",
        rs.refined,
        rs.points,
        rs.median_obs,
        rs.median_move_m * 100.0,
        rs.dropped
    );
}

fn image(g: &omarchyvr_room::pngread::Grey) -> omarchyvr_room::orb::Image<'_> {
    omarchyvr_room::orb::Image { w: g.w, h: g.h, stride: g.w, data: &g.data }
}

fn print_fit(label: &str, f: &JointFit) {
    println!(
        "{label}: {} | {} frames, {} inliers, rms {:.3} deg, formal 1-sigma {:.3} deg / {:.1} mm, frame spread {:.1} mm / {:.2} deg",
        fmt_align(&f.align),
        f.frames,
        f.inliers,
        f.rms_deg,
        f.sigma_deg,
        f.sigma_m * 1e3,
        f.hyp_rms_m * 1e3,
        f.hyp_rms_deg
    );
}

fn spread(fits: &[JointFit], center: &Alignment) -> (f64, f64, f64, f64) {
    let n = fits.len().max(1) as f64;
    let dm: Vec<f64> = fits.iter().map(|f| align::diff(&f.align, center).0).collect();
    let dd: Vec<f64> = fits.iter().map(|f| align::wrap(f.align.yaw - center.yaw).to_degrees()).collect();
    let rms_m = (dm.iter().map(|x| x * x).sum::<f64>() / n).sqrt();
    let rms_d = (dd.iter().map(|x| x * x).sum::<f64>() / n).sqrt();
    let max_m = dm.iter().cloned().fold(0.0, f64::max);
    let max_d = dd.iter().map(|x| x.abs()).fold(0.0, f64::max);
    (rms_m, rms_d, max_m, max_d)
}

fn cmd_relocalize(mut v: Vec<String>) -> R<()> {
    let (_cal, rig) = rig(&mut v)?;
    let dir = map_dir(&mut v)?;
    let every: usize = num(&mut v, "--every", 2)?;
    let window: f64 = num(&mut v, "--window", 4.0)?;
    let threads: usize = num(&mut v, "--threads", 8)?;
    let rec = opt(&mut v, "--rec")?;
    let tracking = opt(&mut v, "--tracking")?;
    let source = opt(&mut v, "--source")?;
    let seconds: f64 = num(&mut v, "--seconds", 15.0)?;
    let result = opt(&mut v, "--result")?;
    let apply = flag(&mut v, "--apply");
    let no_floor = flag(&mut v, "--no-floor");
    done(&v)?;
    let map = RoomMap::load(&dir)?;
    println!("map {}: {} keyframes, {} points", dir.display(), map.keyframes.len(), map.points());
    let bfc = rig.body_from_cams();
    let jp = JointParams::default();
    let ap = AcceptParams { window_ns: (window * 1e9) as i64, ..Default::default() };
    let op = ObserveParams::default();
    let orb = OrbParams::default();
    let (obs, tried, t_first, src_name, pid): (Vec<FrameObs>, usize, i64, String, u32) = match (rec, source) {
        (Some(r), _) => {
            let rec = Recording::open(Path::new(&r), tracking.as_deref().map(Path::new))?;
            let t0 = std::time::Instant::now();
            let (obs, n) = pipeline::observe_recording(&rec, &map, &rig, every, &orb, &op, threads)?;
            eprintln!("matched {} of {} frame sets in {:.1} s", obs.len(), n, t0.elapsed().as_secs_f64());
            let tf = rec.frames.first().map(|f| f.ts_ns).unwrap_or(0);
            (obs, n, tf, rec.name(), 0)
        }
        (None, Some(spec)) => {
            println!("Look around the mapped room for up to {seconds:.0} s (slow head turns).");
            let mut obs = Vec::new();
            let mut n = 0usize;
            let mut t_first = None;
            let mut fixed = false;
            let st = live::capture(&spec, seconds, |f| {
                n += 1;
                t_first.get_or_insert(f.ts_ns);
                let feats = pipeline::frame_features(&rig, [&f.imgs[0], &f.imgs[1]], &orb);
                if let Some(o) = align::observe(&map, &rig, &feats, f.ts_ns, &f.slam_from_body, &op) {
                    obs.push(o);
                    if first_fix(&obs[obs.len().saturating_sub(200)..], &bfc, &jp, &ap).is_some() {
                        fixed = true;
                        return false;
                    }
                }
                true
            })?;
            let _ = fixed;
            if st.frames > 0 && st.posed == 0 {
                return Err(no_poses_text("offset unchanged"));
            }
            (obs, n, t_first.unwrap_or(0), spec.clone(), st.pid)
        }
        _ => return Err("relocalize needs --rec DIR or --source broker".into()),
    };
    println!("{} of {} frame sets matched the map", obs.len(), tried);
    let Some((i, fix)) = first_fix(&obs, &bfc, &jp, &ap) else {
        return Err(format!(
            "room NOT recognised ({} matching frames, need {} agreeing within {:.0} s); offset unchanged",
            obs.len(),
            ap.min_frames,
            window
        ));
    };
    let fix_after_s = (obs[i].ts_ns - t_first) as f64 / 1e9;
    print_fit(&format!("first fix after {fix_after_s:.2} s"), &fix);
    let refs: Vec<&FrameObs> = obs.iter().collect();
    if refs.len() > obs[..=i].len() {
        if let Some(all) = joint_fit(&refs, &bfc, &jp) {
            print_fit("whole session", &all);
            let (dm, dd) = align::diff(&all.align, &fix.align);
            println!("  first fix vs whole session: {:.1} mm, {:.2} deg", dm * 1e3, dd);
            let ws = window_fits(&obs, &bfc, &jp, ap.window_ns, ap.min_frames);
            if ws.len() >= 2 {
                let (rm, rd, mm, md) = spread(&ws, &all.align);
                println!(
                    "  {} windows of {:.0} s: spread around the whole-session fit {:.1} mm / {:.2} deg RMS, max {:.1} mm / {:.2} deg (tracker drift + relocalisation noise)",
                    ws.len(),
                    window,
                    rm * 1e3,
                    rd,
                    mm * 1e3,
                    md
                );
                for w in &ws {
                    println!(
                        "    {:6.1}-{:5.1} s  {}  {:3} frames {:5} inl",
                        (w.span_ns.0 - t_first) as f64 / 1e9,
                        (w.span_ns.1 - t_first) as f64 / 1e9,
                        fmt_align(&w.align),
                        w.frames,
                        w.inliers
                    );
                }
            }
        }
    }
    let res = RelocResult {
        map: dir.display().to_string(),
        source: src_name.clone(),
        align: fix.align,
        frames: fix.frames,
        inliers: fix.inliers,
        rms_deg: fix.rms_deg,
        sigma_deg: fix.sigma_deg,
        sigma_m: fix.sigma_m,
        fix_after_s,
        pid,
    };
    let safe: String = src_name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let out = result.map(PathBuf::from).unwrap_or_else(|| dir.join(format!("reloc-{safe}.toml")));
    res.save(&out).map_err(|e| e.to_string())?;
    res.save(&dir.join("last-reloc.toml")).map_err(|e| e.to_string())?;
    println!("saved {} (and last-reloc.toml)", out.display());
    if apply {
        write_offset(&dir, &res, None, false, no_floor, omarchyvr_anchor::world::Role::Reloc)?;
    }
    Ok(())
}

fn write_offset(dir: &Path, res: &RelocResult, pid: Option<u32>, dry: bool, no_floor: bool, role: omarchyvr_anchor::world::Role) -> R<()> {
    let ring = omarchyvr_anchor::world::HeadRing::read(&omarchyvr_anchor::world::ring_path());
    let pid = pid.or((res.pid != 0).then_some(res.pid)).or(ring.as_ref().map(|r| r.pid)).unwrap_or(0);
    if let (Some(r), true) = (&ring, res.pid != 0) {
        if r.pid != res.pid {
            return Err(format!(
                "the result belongs to Monado pid {} but pid {} runs now: relocalise again; offset unchanged",
                res.pid, r.pid
            ));
        }
    }
    let (align, shift) = if no_floor { (res.align, None) } else { floorcmd::reloc_offset(dir, &res.align, pid) };
    match shift {
        Some(d) => println!("floor: {}/floor.toml puts the room {:+.3} m vertically onto Monado's STAGE floor", dir.display(), d),
        None if !no_floor => println!("floor: none stored for this map (omarchyvr-roommap floor detect --save): height left as mapped"),
        None => {}
    }
    // the room setup: the play area's centre becomes the STAGE origin, forward = -Z (docs/ROOM-SETUP.md)
    let room_setup = omarchyvr_roommap::setup::RoomSetup::load(dir).ok();
    let align = match &room_setup {
        Some(st) => {
            println!(
                "room setup: centre ({:+.2}, {:+.2}) m and forward {:.1} deg of the map become the STAGE origin and -Z; {}",
                st.centre[0],
                st.centre[1],
                st.yaw.to_degrees(),
                st.describe()
            );
            store::compose(&st.setup_from_room(), &align)
        }
        None => align,
    };
    println!("offset (app_from_slam): {}", fmt_align(&align));
    if pimax_cam::geom::norm(align.t) > 20.0 {
        return Err("offset larger than 20 m: wrong map? not written".into());
    }
    if dry {
        println!("dry run: nothing written");
        return Ok(());
    }
    if pid == 0 {
        println!("note: no Monado pid known (Monado not running with PIMAX_ANCHOR=1?): Monado ignores the file until a matching pid is written");
    }
    // one writer per Monado session: a process that set its role (setup, auto, follow) keeps it
    take_lease(pid, writer_role(role))?;
    let p = store::write_offset(&align, pid).map_err(|e| e.to_string())?;
    println!("wrote {} for Monado pid {pid}", p.display());
    if let (Some(st), true) = (&room_setup, pid != 0) {
        match setupcmd::write_stage_bounds(pid, st.play_area) {
            Ok(b) => println!("STAGE bounds {:.2} x {:.2} m: {}", st.play_area[0], st.play_area[1], b.display()),
            Err(e) => eprintln!("cannot write the STAGE bounds: {e}"),
        }
    }
    Ok(())
}

fn cmd_apply(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let result = opt(&mut v, "--result")?.map(PathBuf::from).unwrap_or_else(|| dir.join("last-reloc.toml"));
    let pid: Option<u32> = opt(&mut v, "--pid")?.map(|x| x.parse().map_err(|_| "--pid: bad number")).transpose()?;
    let dry = flag(&mut v, "--dry-run");
    let no_floor = flag(&mut v, "--no-floor");
    done(&v)?;
    let res = RelocResult::load(&result)?;
    println!(
        "{}: {} from {} ({} frames, {} inliers, rms {:.3} deg)",
        result.display(),
        res.map,
        res.source,
        res.frames,
        res.inliers,
        res.rms_deg
    );
    write_offset(&dir, &res, pid, dry, no_floor, omarchyvr_anchor::world::Role::Apply)
}

fn cmd_info(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    done(&v)?;
    let m = RoomMap::load(&dir)?;
    let (mut lo, mut hi) = ([f64::MAX; 3], [f64::MIN; 3]);
    for k in &m.keyframes {
        for i in 0..3 {
            lo[i] = lo[i].min(k.pose.t[i]);
            hi[i] = hi[i].max(k.pose.t[i]);
        }
    }
    println!(
        "{}: device {} | {} sessions, {} keyframes, {} points | keyframes x {:.2}..{:.2} y {:.2}..{:.2} z {:.2}..{:.2} m",
        dir.display(),
        m.device_uid,
        m.sessions,
        m.keyframes.len(),
        m.points(),
        lo[0],
        hi[0],
        lo[1],
        hi[1],
        lo[2],
        hi[2]
    );
    if let Ok(t) = std::fs::read_to_string(dir.join("roommap.toml")) {
        print!("{t}");
    }
    if let Ok(r) = RelocResult::load(&dir.join("last-reloc.toml")) {
        println!("last relocalisation: {} from {} ({:.2} s to fix)", fmt_align(&r.align), r.source, r.fix_after_s);
    }
    Ok(())
}

fn cmd_export(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let out = opt(&mut v, "--out")?.map(PathBuf::from).unwrap_or_else(|| dir.join("map.json"));
    done(&v)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let j = omarchyvr_roommap::export::export_json(&dir, &name)?;
    std::fs::write(&out, &j).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} ({} bytes)", out.display(), j.len());
    Ok(())
}

fn cmd_view(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let out = opt(&mut v, "--out")?.map(PathBuf::from).unwrap_or_else(|| dir.join("map.html"));
    let open = flag(&mut v, "--open");
    done(&v)?;
    let name = dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let j = omarchyvr_roommap::export::export_json(&dir, &name)?;
    let page = omarchyvr_roommap::export::html_page(&j, &name);
    std::fs::write(&out, &page).map_err(|e| format!("{}: {e}", out.display()))?;
    println!("{} ({} KB)", out.display(), page.len() / 1024);
    if open {
        std::process::Command::new("xdg-open")
            .arg(&out)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .map_err(|e| format!("xdg-open: {e}"))?;
    }
    Ok(())
}

fn cmd_cycle(v: Vec<String>) -> R<()> {
    if v.len() < 2 {
        return Err("cycle needs at least two result files".into());
    }
    let mut acc = Alignment { yaw: 0.0, t: [0.0; 3] };
    for f in &v {
        let r = RelocResult::load(Path::new(f))?;
        println!("{f}: {}", fmt_align(&r.align));
        acc = store::compose(&acc, &r.align);
    }
    println!(
        "cycle product (identity if consistent): {} -> {:.1} mm, {:.3} deg",
        fmt_align(&acc),
        pimax_cam::geom::norm(acc.t) * 1e3,
        acc.yaw.to_degrees().abs()
    );
    Ok(())
}

/// Window fits with the mean body position (slam frame) and forward direction of their frames.
fn placed_windows(obs: &[FrameObs], bfc: &[pimax_cam::geom::Pose], jp: &JointParams, win_ns: i64) -> Vec<(JointFit, [f64; 3], [f64; 3])> {
    use pimax_cam::geom;
    window_fits(obs, bfc, jp, win_ns, 6)
        .into_iter()
        .filter_map(|f| {
            let fr: Vec<&FrameObs> = obs.iter().filter(|o| o.ts_ns >= f.span_ns.0 && o.ts_ns <= f.span_ns.1).collect();
            if fr.is_empty() {
                return None;
            }
            let n = fr.len() as f64;
            let p = fr.iter().fold([0.0; 3], |a, o| geom::add(a, geom::scale(o.slam_from_body.t, 1.0 / n)));
            let d = fr.iter().fold([0.0; 3], |a, o| geom::add(a, geom::mat_vec(&o.slam_from_body.r, [0.0, 0.0, -1.0])));
            Some((f, p, geom::normalize(d)))
        })
        .collect()
}

/// Same-place cycle test of two sessions A and B (each with its own map): for every window of B relocalised in
/// map A, find the window of A at the same place (position within 0.5 m, view within 30 deg) relocalised in map B,
/// and compose the two fits. Identity = consistent; the residual is about sqrt(2) x the single relocalisation error
/// (no tracker drift between the two fits, they look at the same part of the room).
fn cmd_eval(mut v: Vec<String>) -> R<()> {
    use pimax_cam::geom;
    let (_cal, rig) = rig(&mut v)?;
    let map_a = store::map_dir(&opt(&mut v, "--map-a")?.ok_or("--map-a NAME")?);
    let map_b = store::map_dir(&opt(&mut v, "--map-b")?.ok_or("--map-b NAME")?);
    let rec_a = opt(&mut v, "--rec-a")?.ok_or("--rec-a DIR")?;
    let rec_b = opt(&mut v, "--rec-b")?.ok_or("--rec-b DIR")?;
    let every: usize = num(&mut v, "--every", 2)?;
    let window: f64 = num(&mut v, "--window", 3.0)?;
    let threads: usize = num(&mut v, "--threads", 8)?;
    done(&v)?;
    let (ma, mb) = (RoomMap::load(&map_a)?, RoomMap::load(&map_b)?);
    let (ra, rb) = (Recording::open(Path::new(&rec_a), None)?, Recording::open(Path::new(&rec_b), None)?);
    let bfc = rig.body_from_cams();
    let (orb, op, jp) = (OrbParams::default(), ObserveParams::default(), JointParams::default());
    let (ob_in_a, _) = pipeline::observe_recording(&rb, &ma, &rig, every, &orb, &op, threads)?;
    let (oa_in_b, _) = pipeline::observe_recording(&ra, &mb, &rig, every, &orb, &op, threads)?;
    let (ca, cb) = (store::load_corrections(&map_a), store::load_corrections(&map_b));
    let w = (window * 1e9) as i64;
    let wb = placed_windows(&ob_in_a, &bfc, &jp, w);
    let wa = placed_windows(&oa_in_b, &bfc, &jp, w);
    println!("{} windows of B in map A, {} windows of A in map B ({:.0} s)", wb.len(), wa.len(), window);
    let mut errs: Vec<(f64, f64)> = Vec::new();
    // A's windows in A's room frame (A's tracker world corrected by A's loop closure)
    let wa_room: Vec<(&JointFit, [f64; 3], [f64; 3], Alignment)> = wa
        .iter()
        .map(|(f, p, d)| {
            let c = correction_at(&ca, (f.span_ns.0 + f.span_ns.1) / 2);
            let cp = c.pose();
            (f, cp.apply(*p), geom::mat_vec(&cp.r, *d), c)
        })
        .collect();
    for (fb, pb, db) in &wb {
        // B's window in A's room frame
        let ab = fb.align.pose();
        let pos = ab.apply(*pb);
        let dir = geom::mat_vec(&ab.r, *db);
        let best = wa_room
            .iter()
            .filter(|(_, pa, da, _)| geom::norm(geom::sub(*pa, pos)) <= 0.5 && geom::angle_deg(*da, dir) <= 30.0)
            .min_by(|x, y| geom::norm(geom::sub(x.1, pos)).total_cmp(&geom::norm(geom::sub(y.1, pos))));
        if let Some((fa, _, _, c_a)) = best {
            // cycle around the place: (A_room <- B_slam) o (B_slam <- B_room) o (B_room <- A_slam) = (A_room <- A_slam)
            let c_b = correction_at(&cb, (fb.span_ns.0 + fb.span_ns.1) / 2);
            let cyc = store::compose(&store::compose(&fb.align, &store::inverse(&c_b)), &fa.align);
            // compared with A's own correction there, at the place (rotation lever arm included)
            let pa_slam = c_a.pose().inverse().apply(pos);
            let at = geom::norm(geom::sub(cyc.pose().apply(pa_slam), c_a.pose().apply(pa_slam)));
            errs.push((at, align::wrap(cyc.yaw - c_a.yaw).to_degrees().abs()));
        }
    }
    if errs.is_empty() {
        println!("no same-place window pairs (the sessions do not overlap)");
        return Ok(());
    }
    let mut t: Vec<f64> = errs.iter().map(|e| e.0).collect();
    let mut d: Vec<f64> = errs.iter().map(|e| e.1).collect();
    t.sort_by(|a, b| a.total_cmp(b));
    d.sort_by(|a, b| a.total_cmp(b));
    let q = |v: &[f64], f: f64| v[((v.len() - 1) as f64 * f).round() as usize];
    println!(
        "same-place cycles: {} pairs | position at the place: median {:.1} mm, p90 {:.1} mm, max {:.1} mm | yaw: median {:.2} deg, p90 {:.2} deg, max {:.2} deg",
        errs.len(),
        q(&t, 0.5) * 1e3,
        q(&t, 0.9) * 1e3,
        q(&t, 1.0) * 1e3,
        q(&d, 0.5),
        q(&d, 0.9),
        q(&d, 1.0)
    );
    Ok(())
}

/// Why a live capture got no head poses, in plain words (who wrote the ring, how old it is).
fn no_poses_text(what: &str) -> String {
    let f = omarchyvr_anchor::world::feed_now();
    match f {
        omarchyvr_anchor::world::Feed::Live { .. } => format!("no head poses matched the camera frames (Monado's feed is live; camera and head clocks apart?); {what}"),
        _ => format!("no head poses: {}; {what}", f.plain()),
    }
}

fn main() {
    let mut v: Vec<String> = std::env::args().skip(1).collect();
    if v.is_empty() || v[0] == "-h" || v[0] == "--help" {
        eprintln!(
            "{}",
            include_str!("main.rs")
                .lines()
                .take_while(|l| l.starts_with("//!"))
                .map(|l| l.trim_start_matches("//!").trim_start_matches(' '))
                .filter(|l| !l.starts_with("```"))
                .collect::<Vec<_>>()
                .join("\n")
        );
        eprintln!("map directory: {}", store::base_dir().display());
        std::process::exit(2);
    }
    let cmd = v.remove(0);
    let r = match cmd.as_str() {
        "build" => cmd_build(v),
        "relocalize" | "reloc" => cmd_relocalize(v),
        "apply" => cmd_apply(v),
        "info" => cmd_info(v),
        "export" => cmd_export(v),
        "view" => cmd_view(v),
        "cycle" => cmd_cycle(v),
        "eval" => cmd_eval(v),
        "floor" => floorcmd::cmd_floor(v),
        "light" => cmd_light(v),
        "setup" => setupcmd::cmd_setup(v),
        "auto" => setupcmd::cmd_auto(v),
        _ => Err(format!("unknown command {cmd:?} (build, relocalize, apply, info, export, view, cycle, eval, floor, setup, auto)")),
    };
    if let Err(e) = r {
        eprintln!("omarchyvr-roommap {cmd}: {e}");
        std::process::exit(1);
    }
}
