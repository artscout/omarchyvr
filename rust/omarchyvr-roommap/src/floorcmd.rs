//! `omarchyvr-roommap floor ...`: detect, touch, set, show, follow, drift (see `analysis/floor.md`).

use crate::{flag, map_dir, num, opt, rig, unix_now, R};
use omarchyvr_anchor::world::{self, HeadRing};
use pimax_cam::geom::{self, Pose, Vec3};
use omarchyvr_roommap::floor::{self, Floor, FloorParams, FloorSource, Follower, StoredFloor};
use omarchyvr_roommap::pipeline::{self, PointFrame};
use omarchyvr_roommap::rec::Recording;
use omarchyvr_roommap::store::RelocResult;
use omarchyvr_room::map::RoomMap;
use omarchyvr_room::session::Alignment;
use std::path::Path;

pub const HELP: &str = "\
omarchyvr-roommap floor detect  [--name NAME | --map DIR] [--rec DIR] [--max-range M] [--save]
    floor from the map points (saved as NAME/floor.toml + preview.txt with --save); with --rec alone from a
    recording's stereo points in its tracker world; with --rec and a map, from the recording the map was built
    from (dense points through the map's loop-closure correction; what build does)
omarchyvr-roommap floor drift   --rec DIR [--window S] [--every N] [--csv FILE]
    floor height per window over a recording: the tracker's vertical drift, and the residual with the continuous
    floor lock (Follower) simulated causally
omarchyvr-roommap floor touch   [--name NAME] [--hand left|right] [--bottom M] [--seconds S] [--save]
    live, read-only: a controller lying still on the floor for 1 s (optical pose + Monado head ring)
omarchyvr-roommap floor set     [--name NAME] (--y Y | --standing EYE_HEIGHT_M)
    manual floor (room frame), or from the current head height while standing upright (live head ring)
omarchyvr-roommap floor show    [--name NAME]
    the stored floor, and live (read-only): head height above it, the offset's floor shift
omarchyvr-roommap floor follow  [--seconds S] [--rate M_PER_S] [--window S] [--no-snap] [--dry-run]
    live floor lock: re-measure the floor from the broker's stereo points every few seconds and move the
    offset's y smoothly (at most RATE m/s, default 0.01) so the floor stays at Monado's STAGE y = 0; the first
    measurement places it at once unless --no-snap
omarchyvr-roommap floor preview [--name NAME | --map DIR]
    (re)write NAME/preview.txt (top-down points for the app)";

fn print_floor(label: &str, f: &Floor) {
    println!(
        "{label}: floor y {:+.4} m (+-{:.1} mm, spread {:.1} mm, {} points, {:.1} m^2, tilt {:.2} deg) | head {:.3} m above | confidence {:.2}",
        f.layer.y,
        f.layer.sigma_m * 1e3,
        f.layer.spread_m * 1e3,
        f.layer.inliers,
        f.layer.area_m2,
        f.layer.tilt_deg,
        f.head_above_m,
        f.confidence
    );
}

fn print_layers(f: &Floor) {
    for l in &f.layers {
        println!(
            "    layer {:+.3} m  {:5} points  {:5.1} m^2  tilt {:.2} deg  spread {:.1} mm{}",
            l.y,
            l.inliers,
            l.area_m2,
            l.tilt_deg,
            l.spread_m * 1e3,
            if (l.y - f.layer.y).abs() < 1e-9 { "  <- floor" } else { "" }
        );
    }
    for r in &f.rejected {
        println!("    not the floor: {r}");
    }
}

fn stored(f: &Floor) -> StoredFloor {
    StoredFloor {
        y: f.layer.y,
        source: FloorSource::Points,
        confidence: f.confidence,
        sigma_m: f.layer.sigma_m,
        inliers: f.layer.inliers,
        head_above_m: f.head_above_m,
        when: unix_now(),
    }
}

fn write_preview(dir: &Path, map: &RoomMap, floor_y: Option<f64>) -> R<()> {
    let (pts, heads) = floor::map_points(map, 4.0);
    std::fs::write(dir.join("preview.txt"), floor::preview_text(&pts, &heads, floor_y, 6000)).map_err(|e| e.to_string())
}

/// The floor of a new map from the mapping session's frames (dense stereo points, far more floor than the map's
/// keyframe points): each frame's points moved by the loop-closure correction at its time into the room frame.
/// Saves `floor.toml` and `preview.txt`; a failure only prints (the map is still good).
pub fn floor_at_build(dir: &Path, map: &RoomMap, frames: &[PointFrame], corr: &[omarchyvr_roommap::loopclose::Correction]) {
    let mut pts = Vec::new();
    let mut heads = Vec::new();
    for f in frames {
        let c = omarchyvr_roommap::loopclose::correction_at(corr, f.ts_ns).pose();
        heads.push(c.apply(f.pose.t));
        pts.extend(f.pts.iter().map(|p| c.apply(*p)));
    }
    match floor::detect(&pts, &heads, &FloorParams::default()) {
        Ok(f) => {
            print_floor("floor", &f);
            if let Err(e) = stored(&f).save(dir) {
                eprintln!("floor.toml: {e}");
            }
            let _ = write_preview(dir, map, Some(f.layer.y));
        }
        Err(e) => {
            println!("floor not found in the mapping walk ({e}): look at the floor around you while mapping, or set it (omarchyvr-roommap floor touch|set)");
            let _ = write_preview(dir, map, None);
        }
    }
}

fn cmd_detect(mut v: Vec<String>) -> R<()> {
    let rec = opt(&mut v, "--rec")?;
    let max_range: f64 = num(&mut v, "--max-range", 3.5)?;
    let save = flag(&mut v, "--save");
    let threads: usize = num(&mut v, "--threads", 4)?;
    let every: usize = num(&mut v, "--every", 2)?;
    if let Some(r) = rec {
        let (_c, rig) = rig(&mut v)?;
        let has_map = v.iter().any(|a| a == "--map" || a == "--name");
        let dir = map_dir(&mut v)?;
        crate::done(&v)?;
        let rec = Recording::open(Path::new(&r), None)?;
        let fr = pipeline::recording_points(&rec, &rig, every, max_range, threads)?;
        if has_map {
            // the recording the map was built from: its frames through the map's loop-closure correction
            let map = RoomMap::load(&dir)?;
            println!("{}: {} frames into map {}", rec.name(), fr.len(), dir.display());
            if save {
                floor_at_build(&dir, &map, &fr, &omarchyvr_roommap::store::load_corrections(&dir));
            } else {
                let corr = omarchyvr_roommap::store::load_corrections(&dir);
                let mut pts = Vec::new();
                let mut heads = Vec::new();
                for f in &fr {
                    let c = omarchyvr_roommap::loopclose::correction_at(&corr, f.ts_ns).pose();
                    heads.push(c.apply(f.pose.t));
                    pts.extend(f.pts.iter().map(|p| c.apply(*p)));
                }
                let f = floor::detect(&pts, &heads, &FloorParams::default())?;
                print_floor("mapping walk floor (room frame)", &f);
                print_layers(&f);
            }
            return Ok(());
        }
        let pts: Vec<Vec3> = fr.iter().flat_map(|f| f.pts.iter().copied()).collect();
        let heads: Vec<Vec3> = fr.iter().map(|f| f.pose.t).collect();
        println!("{}: {} frames, {} stereo points", rec.name(), fr.len(), pts.len());
        let f = floor::detect(&pts, &heads, &FloorParams::default())?;
        print_floor("whole session (tracker world)", &f);
        print_layers(&f);
        return Ok(());
    }
    let dir = map_dir(&mut v)?;
    crate::done(&v)?;
    let map = RoomMap::load(&dir)?;
    let (pts, heads) = floor::map_points(&map, max_range);
    println!("map {}: {} keyframes, {} points within {max_range} m", dir.display(), heads.len(), pts.len());
    let f = floor::detect(&pts, &heads, &FloorParams::default())?;
    print_floor("map floor (room frame)", &f);
    print_layers(&f);
    if save {
        let s = stored(&f);
        // a touch or manual floor is the user's word: keep it unless the points agree with it
        if let Ok(old) = StoredFloor::load(&dir) {
            if old.source != FloorSource::Points && (old.y - s.y).abs() > 0.03 {
                println!("kept the {} floor {:+.4} m (points say {:+.4} m)", old.source.name(), old.y, s.y);
                write_preview(&dir, &map, Some(old.y))?;
                return Ok(());
            }
        }
        s.save(&dir).map_err(|e| e.to_string())?;
        write_preview(&dir, &map, Some(s.y))?;
        println!("saved {} and preview.txt", dir.join("floor.toml").display());
    }
    Ok(())
}

/// Window measurements of the floor (tracker world): (window end time s, floor y, points, head above).
/// With a `prior` (the floor known from the whole session / the map) the layer near it is followed ([`floor::track`]).
pub fn window_floors(fr: &[PointFrame], win_s: f64, step_s: f64, prior: Option<f64>) -> Vec<(f64, f64, usize, f64)> {
    let Some(t0) = fr.first().map(|f| f.ts_ns) else { return Vec::new() };
    let t_end = (fr.last().unwrap().ts_ns - t0) as f64 / 1e9;
    let p = FloorParams { min_support: 40, min_area_m2: 0.25, ..FloorParams::default() };
    let mut out = Vec::new();
    let mut t = win_s;
    while t <= t_end + 1e-9 {
        let sel: Vec<&PointFrame> = fr
            .iter()
            .filter(|f| {
                let s = (f.ts_ns - t0) as f64 / 1e9;
                s > t - win_s && s <= t
            })
            .collect();
        let pts: Vec<Vec3> = sel.iter().flat_map(|f| f.pts.iter().copied()).collect();
        let heads: Vec<Vec3> = sel.iter().map(|f| f.pose.t).collect();
        let f = match prior {
            Some(y) => floor::track(&pts, &heads, y, 0.15, &p),
            None => floor::detect(&pts, &heads, &p).ok(),
        };
        if let Some(f) = f {
            out.push((t, f.layer.y, f.layer.inliers, f.head_above_m));
        }
        t += step_s;
    }
    out
}

fn stats(v: &[f64]) -> (f64, f64, f64) {
    let mut a: Vec<f64> = v.iter().map(|x| x.abs()).collect();
    a.sort_by(|x, y| x.total_cmp(y));
    if a.is_empty() {
        return (f64::NAN, f64::NAN, f64::NAN);
    }
    let q = |f: f64| a[((a.len() - 1) as f64 * f).round() as usize];
    (q(0.5), q(0.9), q(1.0))
}

fn slope_cm_per_min(t: &[f64], y: &[f64]) -> f64 {
    let n = t.len() as f64;
    if t.len() < 3 {
        return f64::NAN;
    }
    let (mt, my) = (t.iter().sum::<f64>() / n, y.iter().sum::<f64>() / n);
    let num: f64 = t.iter().zip(y).map(|(a, b)| (a - mt) * (b - my)).sum();
    let den: f64 = t.iter().map(|a| (a - mt) * (a - mt)).sum();
    num / den * 100.0 * 60.0
}

fn cmd_drift(mut v: Vec<String>) -> R<()> {
    let (_c, rig) = rig(&mut v)?;
    let rec = opt(&mut v, "--rec")?.ok_or("--rec DIR")?;
    let win: f64 = num(&mut v, "--window", 6.0)?;
    let step: f64 = num(&mut v, "--step", 2.0)?;
    let every: usize = num(&mut v, "--every", 1)?;
    let threads: usize = num(&mut v, "--threads", 4)?;
    let rate: f64 = num(&mut v, "--rate", 0.01)?;
    let csv = opt(&mut v, "--csv")?;
    crate::done(&v)?;
    let rec = Recording::open(Path::new(&rec), None)?;
    let fr = pipeline::recording_points(&rec, &rig, every, 3.5, threads)?;
    let pts: Vec<Vec3> = fr.iter().flat_map(|f| f.pts.iter().copied()).collect();
    let heads: Vec<Vec3> = fr.iter().map(|f| f.pose.t).collect();
    println!("{}: {} frames, {} stereo points", rec.name(), fr.len(), pts.len());
    let prior = match floor::detect(&pts, &heads, &FloorParams::default()) {
        Ok(f) => {
            print_floor("whole session", &f);
            print_layers(&f);
            Some(f.layer.y)
        }
        Err(e) => {
            println!("whole session: no floor ({e})");
            None
        }
    };
    let w = window_floors(&fr, win, step, prior);
    println!("{} of the {:.0} s windows found the floor", w.len(), (fr.last().map(|f| f.ts_ns).unwrap_or(0) - fr[0].ts_ns) as f64 / 1e9 / step);
    if w.len() < 2 {
        return Ok(());
    }
    let y0 = w[0].1;
    // causal correction: the follower sees each window at its end; the residual of window k is measured with the
    // correction applied before window k's own measurement
    let mut fol = Follower::new();
    fol.max_rate = rate;
    let mut before = Vec::new();
    let mut after = Vec::new();
    let mut ts = Vec::new();
    let mut ys = Vec::new();
    let mut lines = String::from("t_s,floor_y,points,head_above,shift_applied,residual_uncorrected,residual_corrected\n");
    let mut t_prev = w[0].0;
    for (k, (t, y, n, head)) in w.iter().enumerate() {
        // advance the applied value to this window's time at 90 Hz
        let mut tt = t_prev;
        while tt < *t {
            fol.step(1.0 / 90.0);
            tt += 1.0 / 90.0;
        }
        t_prev = *t;
        let shift = fol.applied.unwrap_or(-y0);
        let r_before = y - y0;
        let r_after = y + shift;
        if k > 0 {
            before.push(r_before);
            after.push(r_after);
        }
        ts.push(*t);
        ys.push(*y);
        lines += &format!("{t:.2},{y:.5},{n},{head:.4},{shift:.5},{r_before:.5},{r_after:.5}\n");
        fol.measure(-y, if k == 0 { 0.0 } else { step });
        if k == 0 {
            fol.step(0.0);
        }
    }
    let (bm, b9, bx) = stats(&before);
    let (am, a9, ax) = stats(&after);
    println!(
        "vertical drift (floor height vs the first window): median {:.1} mm, p90 {:.1} mm, max {:.1} mm, trend {:+.2} cm/min",
        bm * 1e3,
        b9 * 1e3,
        bx * 1e3,
        slope_cm_per_min(&ts, &ys)
    );
    println!(
        "with the floor lock ({:.0} mm/s, tau {:.0} s): median {:.1} mm, p90 {:.1} mm, max {:.1} mm",
        rate * 1e3,
        fol.tau_s,
        am * 1e3,
        a9 * 1e3,
        ax * 1e3
    );
    let mut hd: Vec<f64> = w.iter().map(|x| x.3).collect();
    hd.sort_by(|a, b| a.total_cmp(b));
    println!("head above the floor: min {:.2} m, median {:.2} m, max {:.2} m", hd[0], hd[hd.len() / 2], hd[hd.len() - 1]);
    if let Some(c) = csv {
        std::fs::write(&c, lines).map_err(|e| e.to_string())?;
        println!("wrote {c}");
    }
    Ok(())
}

/// The room fix of the running Monado (the map's last relocalisation for this pid).
fn session_fix(dir: &Path, pid: u32) -> Option<Alignment> {
    let r = RelocResult::load(&dir.join("last-reloc.toml")).ok()?;
    (r.pid == pid && pid != 0).then_some(r.align)
}

fn ring() -> R<HeadRing> {
    HeadRing::read(&world::ring_path()).ok_or_else(|| {
        format!("no Monado head ring at {} (desk mode with --anchor, PIMAX_ANCHOR=1)", world::ring_path().display())
    })
}

fn cmd_touch(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let hand = opt(&mut v, "--hand")?.unwrap_or_else(|| "right".into());
    let bottom: f64 = num(&mut v, "--bottom", 0.035)?;
    let seconds: f64 = num(&mut v, "--seconds", 20.0)?;
    let save = flag(&mut v, "--save");
    crate::done(&v)?;
    let hi = match hand.as_str() {
        "left" => 0,
        "right" => 1,
        _ => return Err("--hand left|right".into()),
    };
    let reader = pimax_ctrltrack::shm::PoseReader::open(&pimax_ctrltrack::shm::default_path())
        .map_err(|e| format!("controller poses ({}): {e} (desk mode with --ctrl-track)", pimax_ctrltrack::shm::default_path().display()))?;
    println!("Lay the {hand} controller flat on the floor in view of the headset and let go; keep looking at it.");
    let t0 = std::time::Instant::now();
    let mut buf: Vec<(u64, Vec3)> = Vec::new();
    let mut pid = 0;
    let mut last_frame = 0;
    let mut result = None;
    while t0.elapsed().as_secs_f64() < seconds {
        std::thread::sleep(std::time::Duration::from_millis(20));
        let Some(h) = reader.read() else { continue };
        let h = h[hi];
        if h.frame_ns == 0 || h.frame_ns == last_frame {
            continue;
        }
        last_frame = h.frame_ns;
        let Some(r) = HeadRing::read(&world::ring_path()) else { continue };
        pid = r.pid;
        let Some(head) = r.at(h.frame_ns, 100_000_000) else { continue };
        let p = head.apply([h.pos[0] as f64, h.pos[1] as f64, h.pos[2] as f64]);
        buf.push((h.frame_ns, p));
        buf.retain(|(t, _)| h.frame_ns - t <= 1_000_000_000);
        // 1 s still, at least 0.5 m under the head
        if buf.len() >= 10 && h.frame_ns - buf[0].0 >= 900_000_000 && head.t[1] - p[1] > 0.5 {
            let o: Vec<Vec3> = buf.iter().map(|x| x.1).collect();
            if let Ok(x) = floor::touch_floor(&o, bottom, 0.006) {
                result = Some((x, head.t));
                break;
            }
        }
    }
    let Some(((y_slam, spread), head)) = result else {
        return Err(format!("no still controller on the floor within {seconds:.0} s"));
    };
    println!("controller on the floor: floor y {:+.4} m in the tracker world (still within {:.1} mm, bottom offset {:.0} mm)", y_slam, spread * 1e3, bottom * 1e3);
    println!("head is {:.3} m above it now", head[1] - y_slam);
    let Some(fix) = session_fix(&dir, pid) else {
        println!("no room fix for this Monado session (relocalize first): floor not stored in the map");
        return Ok(());
    };
    let y_room = fix.pose().apply([head[0], y_slam, head[2]])[1];
    let points = StoredFloor::load(&dir).ok().filter(|f| f.source == FloorSource::Points);
    let (y, sig) = floor::combine(points.as_ref().map(|f| (f.y, f.sigma_m.max(0.003))), Some((y_room, 0.008)), 0.04).unwrap();
    if let Some(p) = &points {
        println!("map points said {:+.4} m, touch {:+.4} m (room frame): using {:+.4} m", p.y, y_room, y);
    }
    if save {
        let s = StoredFloor { y, source: FloorSource::Touch, confidence: 0.95, sigma_m: sig, inliers: 0, head_above_m: head[1] - y_slam, when: unix_now() };
        s.save(&dir).map_err(|e| e.to_string())?;
        println!("saved {}", dir.join("floor.toml").display());
    }
    Ok(())
}

fn cmd_set(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    let y = opt(&mut v, "--y")?;
    let standing = opt(&mut v, "--standing")?;
    crate::done(&v)?;
    let s = match (y, standing) {
        (Some(y), None) => {
            let y: f64 = y.parse().map_err(|_| "--y: bad number")?;
            StoredFloor { y, source: FloorSource::Manual, confidence: 0.6, sigma_m: 0.02, inliers: 0, head_above_m: f64::NAN, when: unix_now() }
        }
        (None, Some(h)) => {
            let h: f64 = h.parse().map_err(|_| "--standing: bad number")?;
            if !(1.0..=2.3).contains(&h) {
                return Err("--standing is your eye height standing upright, 1.0-2.3 m".into());
            }
            let r = ring()?;
            let head = r.samples.iter().rev().find(|s| s.2).map(|s| s.1).ok_or("no valid head pose in the ring")?;
            let fix = session_fix(&dir, r.pid).ok_or("no room fix for this Monado session: relocalize first")?;
            let head_room = fix.pose().apply(head.t);
            StoredFloor { y: head_room[1] - h, source: FloorSource::Standing, confidence: 0.7, sigma_m: 0.03, inliers: 0, head_above_m: h, when: unix_now() }
        }
        _ => return Err("floor set needs --y Y or --standing EYE_HEIGHT_M".into()),
    };
    s.save(&dir).map_err(|e| e.to_string())?;
    println!("saved {}: floor y {:+.4} m ({})", dir.join("floor.toml").display(), s.y, s.source.name());
    if let Ok(m) = RoomMap::load(&dir) {
        write_preview(&dir, &m, Some(s.y))?;
    }
    Ok(())
}

fn cmd_show(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    crate::done(&v)?;
    match StoredFloor::load(&dir) {
        Ok(f) => println!(
            "{}: floor y {:+.4} m in the room frame ({}, confidence {:.2}, +-{:.1} mm, head {:.2} m above while mapping)",
            dir.display(),
            f.y,
            f.source.name(),
            f.confidence,
            f.sigma_m * 1e3,
            f.head_above_m
        ),
        Err(e) => println!("no floor stored: {e}"),
    }
    let Ok(r) = ring() else {
        println!("Monado not running with the anchor hook: nothing live to show");
        return Ok(());
    };
    let oh = floor::origin_height(r.pid);
    let off = world::read_offset(&world::offset_path()).filter(|(p, _)| *p == r.pid).map(|(_, o)| o);
    match &off {
        Some(o) => println!("offset of Monado pid {}: t ({:+.3} {:+.3} {:+.3})", r.pid, o.t[0], o.t[1], o.t[2]),
        None => println!("no offset for Monado pid {}", r.pid),
    }
    if let Some(head) = r.samples.iter().rev().find(|s| s.2).map(|s| s.1) {
        let app = off.unwrap_or(Pose { r: geom::rot_x(0.0), t: [0.0; 3] }).compose(&head);
        println!("head now: {:.3} m above Monado's STAGE floor (origin height {oh:.2} m)", app.t[1] + oh);
        if let (Ok(f), Some(fix)) = (StoredFloor::load(&dir), session_fix(&dir, r.pid)) {
            let room = fix.pose().apply(head.t);
            println!("head now: {:.3} m above the room's floor", room[1] - f.y);
        }
    }
    Ok(())
}

/// The offset with its y moved so the floor (at `floor_slam` under the head, tracker world) lands on STAGE y = 0.
pub fn with_floor(off: &Pose, floor_slam: Vec3, origin_height: f64) -> (Pose, f64) {
    let app = off.apply(floor_slam);
    let d = floor::floor_shift(app[1], origin_height);
    (Pose { r: off.r, t: [off.t[0], off.t[1] + d, off.t[2]] }, d)
}

/// The offset for a relocalisation result with the map's floor (if stored): the room frame shifted vertically so
/// the floor lands on Monado's STAGE floor. Used by `relocalize --apply` and `apply`.
pub fn reloc_offset(dir: &Path, a: &Alignment, pid: u32) -> (Alignment, Option<f64>) {
    match StoredFloor::load(dir) {
        Ok(f) => {
            let d = floor::floor_shift(f.y, floor::origin_height(pid));
            (Alignment { yaw: a.yaw, t: [a.t[0], a.t[1] + d, a.t[2]] }, Some(d))
        }
        Err(_) => (*a, None),
    }
}

fn cmd_follow(mut v: Vec<String>) -> R<()> {
    let (_c, rig) = rig(&mut v)?;
    let seconds: f64 = num(&mut v, "--seconds", 1e9)?;
    let rate: f64 = num(&mut v, "--rate", 0.01)?;
    let win: f64 = num(&mut v, "--window", 6.0)?;
    let dry = flag(&mut v, "--dry-run");
    let no_snap = flag(&mut v, "--no-snap");
    let source = opt(&mut v, "--source")?.unwrap_or_else(|| "broker".into());
    crate::done(&v)?;
    let orb = omarchyvr_room::orb::OrbParams::default();
    let mut frames: std::collections::VecDeque<PointFrame> = Default::default();
    let mut fol = Follower::new();
    fol.max_rate = rate;
    let mut last_measure = std::time::Instant::now();
    let mut last_write = std::time::Instant::now();
    let mut k = 0usize;
    let mut known: Option<f64> = None;
    let path = world::offset_path();
    println!("floor lock: measuring every 2 s over {win:.0} s windows, at most {:.0} mm/s{}", rate * 1e3, if dry { " (dry run)" } else { "" });
    omarchyvr_roommap::live::capture(&source, seconds, |f| {
        k += 1;
        // 1 frame in 3 is plenty for the floor and keeps the CPU low
        if k.is_multiple_of(3) {
            let feats = pipeline::frame_features(&rig, [&f.imgs[0], &f.imgs[1]], &orb);
            frames.push_back(PointFrame { ts_ns: f.ts_ns, pose: f.slam_from_body, pts: pipeline::frame_points(&rig, &feats, &f.slam_from_body, 3.5) });
            while frames.front().is_some_and(|x| f.ts_ns - x.ts_ns > (win * 1e9) as i64) {
                frames.pop_front();
            }
        }
        let oh = floor::origin_height(f.pid);
        let off = world::read_offset(&path).filter(|(p, _)| *p == f.pid).map(|(_, o)| o).unwrap_or(Pose { r: geom::rot_x(0.0), t: [0.0; 3] });
        if last_measure.elapsed().as_secs_f64() >= 2.0 && frames.len() >= 10 {
            let dt = last_measure.elapsed().as_secs_f64();
            last_measure = std::time::Instant::now();
            let fv: Vec<PointFrame> = frames.iter().cloned().collect();
            // follow the floor once known (the map's floor, or the first live detection)
            let prior = known;
            if let Some(&(_, y, n, above)) = window_floors(&fv, win, win, prior).last() {
                known = Some(y);
                let h = f.slam_from_body.t;
                // the shift relative to the offset's current y (the follower works on the absolute offset y)
                let (_, d) = with_floor(&off, [h[0], y, h[2]], oh);
                fol.measure(off.t[1] + d, dt);
                println!("floor {:+.4} m (tracker), {n} points, head {above:.2} m above; offset y {:+.4} -> target {:+.4}", y, off.t[1], fol.target.unwrap_or(f64::NAN));
            }
        }
        // write at 4 Hz (Monado re-reads the file at most every 250 ms), never more than rate/4 per write
        let dt = last_write.elapsed().as_secs_f64();
        if dt >= 0.25 {
            last_write = std::time::Instant::now();
            if fol.applied.is_none() && no_snap {
                fol.applied = Some(off.t[1]);
            }
            if let Some(y) = fol.step(dt.min(0.5)) {
                if (y - off.t[1]).abs() >= 0.0002 {
                    let o = Pose { r: off.r, t: [off.t[0], y, off.t[2]] };
                    if !dry {
                        // one writer per session: the follow never takes the offset from a running writer and
                        // stops when a room setup (or another user action) takes it
                        if let Err(e) = crate::take_lease(f.pid, world::Role::Follow) {
                            eprintln!("floor follow stops: {e}");
                            return false;
                        }
                        if let Err(e) = world::write_offset(&path, f.pid, &o) {
                            eprintln!("cannot write {}: {e}", path.display());
                            return false;
                        }
                    }
                }
            }
        }
        true
    })?;
    Ok(())
}

fn cmd_preview(mut v: Vec<String>) -> R<()> {
    let dir = map_dir(&mut v)?;
    crate::done(&v)?;
    let map = RoomMap::load(&dir)?;
    write_preview(&dir, &map, StoredFloor::load(&dir).ok().map(|f| f.y))?;
    println!("wrote {}", dir.join("preview.txt").display());
    Ok(())
}

pub fn cmd_floor(mut v: Vec<String>) -> R<()> {
    if v.is_empty() {
        return Err(format!("floor needs a subcommand:\n{HELP}"));
    }
    let sub = v.remove(0);
    match sub.as_str() {
        "detect" => cmd_detect(v),
        "drift" => cmd_drift(v),
        "touch" => cmd_touch(v),
        "set" => cmd_set(v),
        "show" => cmd_show(v),
        "follow" => cmd_follow(v),
        "preview" => cmd_preview(v),
        _ => Err(format!("unknown floor command {sub:?}\n{HELP}")),
    }
}

