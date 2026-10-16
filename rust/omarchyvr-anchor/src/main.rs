#![allow(clippy::needless_range_loop, clippy::too_many_arguments, clippy::type_complexity)]
use omarchyvr_anchor::config::{Anchor, WindowRel};
use omarchyvr_anchor::detect::{self, CamRect, Detection, Detector};
use omarchyvr_anchor::tag::{self, Family};
use omarchyvr_anchor::{rig, world};
use pimax_cam::frame::{self, FrameKind, Meta, TILE_COUNT, TILE_H, TILE_W};
use pimax_cam::geom::{self, Pose};
use pimax_cam::source::{self, ReplayOptions};
use pimax_ctrltrack::shm::monotonic_ns;
use std::path::{Path, PathBuf};
use std::time::Instant;

fn usage() -> ! {
    eprintln!(
        "omarchyvr-anchor: room anchor (AprilTag tag36h11 on the desk) for the Dream Air

  omarchyvr-anchor tag [--id 0] [--size-mm 150] [--out-dir DIR]   write the printable SVG + PDF
  omarchyvr-anchor detect [opts]      tag pose in the head frame (and in the Monado world when Monado runs with PIMAX_ANCHOR=1)
  omarchyvr-anchor calibrate [opts] [--window NAME=px,py,pz,qx,qy,qz,qw ...]   save the reference to ~/.config/omarchyvr/anchor.toml
  omarchyvr-anchor apply [opts] [--full] [--dry-run]   measure the tag now and write the Monado world offset
  omarchyvr-anchor status             config, head ring, offset file
  omarchyvr-anchor clear              remove the offset (identity)

common opts: --source broker|FILE (default broker)  --seconds N (default 4)  --cams 0,1,2,3  --size-mm N (default: config or 150)
             --id N  --calib XML  --extrinsics JSON | --factory  --png OUT (debug mosaic of the last frame)
apply: yaw + translation only by default (the SLAM world is gravity aligned); --full also corrects roll/pitch."
    );
    std::process::exit(2)
}

fn arg<'a>(a: &'a [String], k: &str) -> Option<&'a str> {
    a.iter().position(|s| s == k).and_then(|i| a.get(i + 1)).map(|s| s.as_str())
}
fn has(a: &[String], k: &str) -> bool {
    a.iter().any(|s| s == k)
}
fn die(m: String) -> ! {
    eprintln!("omarchyvr-anchor: {m}");
    std::process::exit(1)
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    if a.len() < 2 {
        usage();
    }
    let r = &a[2..];
    match a[1].as_str() {
        "tag" => cmd_tag(r),
        "detect" => cmd_detect(r),
        "calibrate" => cmd_calibrate(r),
        "apply" => cmd_apply(r),
        "status" => cmd_status(),
        "clear" => {
            match std::fs::remove_file(world::offset_path()) {
                Ok(()) => println!("offset removed ({})", world::offset_path().display()),
                Err(e) => println!("nothing to remove: {e}"),
            }
        }
        _ => usage(),
    }
}

fn cmd_tag(a: &[String]) {
    let id: u32 = arg(a, "--id").map(|v| v.parse().unwrap()).unwrap_or(0);
    let size: f64 = arg(a, "--size-mm").map(|v| v.parse().unwrap()).unwrap_or(150.0);
    let dir = PathBuf::from(arg(a, "--out-dir").unwrap_or("."));
    std::fs::create_dir_all(&dir).unwrap();
    let fam = Family::tag36h11();
    let base = format!("tag36h11-id{id}-{size}mm");
    std::fs::write(dir.join(format!("{base}.svg")), tag::svg(&fam, id, size)).unwrap();
    std::fs::write(dir.join(format!("{base}.pdf")), tag::pdf(&fam, id, size)).unwrap();
    println!("wrote {0}/{base}.svg and .pdf (A4, print at 100%, the black square must measure {size} mm)", dir.display());
}

struct Sample {
    ts_ns: u64,
    fix: detect::Fix,
    /// raw SLAM world_from_tag (Monado ring), when available
    world_from_tag: Option<Pose>,
}

struct Capture {
    samples: Vec<Sample>,
    frames: usize,
    seen: usize,
    ring_pid: Option<u32>,
}

fn capture(a: &[String], size: f64, id: i32) -> Capture {
    let spec = arg(a, "--source").unwrap_or("broker").to_string();
    let secs: f64 = arg(a, "--seconds").map(|v| v.parse().unwrap()).unwrap_or(4.0);
    let cams_used: Vec<usize> = arg(a, "--cams")
        .map(|v| v.split(',').map(|x| x.parse().unwrap()).collect())
        .unwrap_or_else(|| (0..TILE_COUNT).collect());
    let cams = rig::load(
        arg(a, "--calib").map(Path::new),
        arg(a, "--extrinsics").map(Path::new),
        has(a, "--factory"),
    )
    .unwrap_or_else(|e| die(e));
    let rects: Vec<CamRect> = cams.iter().map(|c| CamRect::new(&c.intr)).collect();
    let rr: Vec<&CamRect> = rects.iter().collect();
    let det = Detector::new(2);
    let mut src = source::open(&spec, 0, ReplayOptions { looping: false, realtime: false })
        .unwrap_or_else(|e| die(format!("cannot open source {spec:?}: {e}")));
    eprintln!("source {} | cameras {:?} | tag36h11 id {id}, {:.0} mm | {secs} s", src.describe(), cams_used, size * 1e3);
    let png = arg(a, "--png").map(PathBuf::from);
    let ring_path = world::ring_path();
    let t0 = Instant::now();
    let mut tile = vec![0u8; TILE_W * TILE_H];
    let mut out = Capture { samples: Vec::new(), frames: 0, seen: 0, ring_pid: None };
    let mut last_mosaic: Option<(Vec<u8>, Vec<(usize, Detection)>)> = None;
    while t0.elapsed().as_secs_f64() < secs {
        let fr = match src.next(500) {
            Ok(Some(f)) => f,
            Ok(None) => {
                if src.finished() {
                    break;
                }
                continue;
            }
            Err(e) => die(format!("source: {e}")),
        };
        let Some(m) = Meta::parse(&fr.data[..frame::META_LEN]) else { continue };
        if m.kind() != FrameKind::Tracking {
            continue;
        }
        out.frames += 1;
        let rows = frame::tile_rows(Some(&m), fr.data, fr.stride);
        let mut dets: Vec<(usize, Detection)> = Vec::new();
        let mut mosaic = png.as_ref().map(|_| vec![0u8; TILE_W * TILE_COUNT * TILE_H]);
        for &ci in &cams_used {
            frame::copy_tile(fr.data, fr.stride, ci, rows.y[ci], &mut tile, TILE_W);
            if let Some(mo) = mosaic.as_mut() {
                for y in 0..TILE_H {
                    let o = y * TILE_W * TILE_COUNT + ci * TILE_W;
                    mo[o..o + TILE_W].copy_from_slice(&tile[y * TILE_W..(y + 1) * TILE_W]);
                }
            }
            for d in det.detect(rr[ci], &tile) {
                if d.id == id {
                    dets.push((ci, d));
                }
            }
        }
        if let Some(mo) = mosaic {
            last_mosaic = Some((mo, dets.clone()));
        }
        if dets.is_empty() {
            continue;
        }
        out.seen += 1;
        let Some(fix) = detect::solve(&cams, &rr, &dets, size) else { continue };
        if fix.rms_px > 2.5 {
            continue;
        }
        let ts_ns = fr.host_ts_ns;
        let mut wft = None;
        if let Some(ring) = world::HeadRing::read(&ring_path) {
            out.ring_pid = Some(ring.pid);
            if let Some(h) = ring.at(ts_ns, 100_000_000) {
                wft = Some(h.compose(&fix.body_from_tag));
            }
        }
        out.samples.push(Sample { ts_ns, fix, world_from_tag: wft });
    }
    if let (Some(p), Some((mo, dets))) = (png, last_mosaic) {
        let w = TILE_W * TILE_COUNT;
        let mut rgb = vec![0u8; w * TILE_H * 3];
        for (i, v) in mo.iter().enumerate() {
            rgb[i * 3..i * 3 + 3].copy_from_slice(&[*v, *v, *v]);
        }
        for (ci, d) in &dets {
            for k in 0..4 {
                for (dx, dy) in [(-3i64, 0i64), (3, 0), (0, -3), (0, 3), (0, 0)] {
                    let (x, y) = (d.px[k].0.round() as i64 + dx + (*ci * TILE_W) as i64, d.px[k].1.round() as i64 + dy);
                    if x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < TILE_H {
                        let o = (y as usize * w + x as usize) * 3;
                        rgb[o..o + 3].copy_from_slice(&[255, 0, 0]);
                    }
                }
            }
        }
        let _ = pimax_cam::png::write_rgb(&p, w, TILE_H, &rgb);
        eprintln!("wrote {}", p.display());
    }
    out
}

fn describe(p: &Pose) -> String {
    let n = geom::mat_vec(&p.r, [0.0, 0.0, 1.0]);
    format!(
        "pos ({:+.3} {:+.3} {:+.3}) m, tag normal ({:+.2} {:+.2} {:+.2})",
        p.t[0], p.t[1], p.t[2], n[0], n[1], n[2]
    )
}

fn size_and_id(a: &[String], cfg: Option<&Anchor>) -> (f64, i32) {
    let size = arg(a, "--size-mm")
        .map(|v| v.parse::<f64>().unwrap() / 1e3)
        .or(cfg.map(|c| c.size_m))
        .unwrap_or(0.15);
    let id = arg(a, "--id").map(|v| v.parse().unwrap()).or(cfg.map(|c| c.id as i32)).unwrap_or(0);
    (size, id)
}

/// Aggregate world samples; prints the numbers.
fn summarise(c: &Capture, need: usize) -> (Option<(Pose, usize, f64, f64)>, Option<(Pose, usize, f64, f64)>) {
    println!("frames {} | tag seen in {} | solved {} (rms <= 2.5 px)", c.frames, c.seen, c.samples.len());
    let amb = c.samples.iter().filter(|s| s.fix.ambiguous).count();
    if amb > 0 {
        println!("  {amb} samples had an ambiguous planar solution (tag small or head-on): get closer or use more cameras");
    }
    let good: Vec<&Sample> = c.samples.iter().filter(|s| !s.fix.ambiguous).collect();
    let head: Vec<Pose> = good.iter().map(|s| s.fix.body_from_tag).collect();
    let hm = world::robust_mean(&head, 0.02, 3.0);
    if let Some((p, n, st, sr)) = &hm {
        let rms: f64 = good.iter().map(|s| s.fix.rms_px).sum::<f64>() / good.len() as f64;
        println!("head frame: {} | {n} samples, scatter {:.1} mm / {:.2} deg, mean reprojection {rms:.2} px", describe(p), st * 1e3, sr);
        let cams: Vec<usize> = (0..TILE_COUNT).filter(|i| good.iter().any(|s| s.fix.cams.contains(i))).collect();
        println!("  cameras that saw it: {cams:?}");
    }
    let wl: Vec<Pose> = good.iter().filter_map(|s| s.world_from_tag).collect();
    let wm = world::robust_mean(&wl, 0.03, 3.0).filter(|m| m.1 >= need.min(wl.len()));
    match &wm {
        Some((p, n, st, sr)) => println!("Monado world (raw SLAM): {} | {n} samples, scatter {:.1} mm / {:.2} deg", describe(p), st * 1e3, sr),
        None if c.ring_pid.is_none() => println!("no head ring ({}): Monado is not running with PIMAX_ANCHOR=1, only the head-frame pose is known", world::ring_path().display()),
        None => println!("head ring present but no sample matched the frame times"),
    }
    let _ = c.samples.last().map(|s| s.ts_ns);
    (hm, wm)
}

fn cmd_detect(a: &[String]) {
    let cfg = Anchor::load().ok();
    let (size, id) = size_and_id(a, cfg.as_ref());
    let c = capture(a, size, id);
    summarise(&c, 1);
}

fn current_offset(ring_pid: Option<u32>) -> Pose {
    match (world::read_offset(&world::offset_path()), ring_pid) {
        (Some((pid, o)), Some(rp)) if pid == rp => o,
        _ => Pose::IDENTITY,
    }
}

fn cmd_calibrate(a: &[String]) {
    let (size, id) = size_and_id(a, None);
    let c = capture(a, size, id);
    let (_, wm) = summarise(&c, 8);
    let Some((w_from_tag, n, st, sr)) = wm else {
        die("calibrate needs the tag seen with Monado running (PIMAX_ANCHOR=1) and the headset held still; nothing saved".into())
    };
    if n < 8 || st > 0.01 || sr > 2.0 {
        die(format!("too few or too noisy samples ({n}, scatter {:.1} mm / {:.2} deg): hold still, tag well lit; nothing saved", st * 1e3, sr));
    }
    // app world = SLAM world with the offset that is active now
    let off = current_offset(c.ring_pid);
    let reference = off.compose(&w_from_tag);
    let mut windows = Anchor::load().map(|c| c.windows).unwrap_or_default();
    let mut i = 0;
    while i < a.len() {
        if a[i] == "--window" {
            let spec = a.get(i + 1).unwrap_or_else(|| usage());
            let (name, v) = spec.split_once('=').unwrap_or_else(|| usage());
            let v: Vec<f64> = v.split(',').map(|x| x.parse().unwrap_or_else(|_| usage())).collect();
            if v.len() != 7 {
                usage();
            }
            let app_from_win = world::from_pq([v[0], v[1], v[2]], [v[3], v[4], v[5], v[6]]);
            windows.retain(|w| w.name != name);
            windows.push(WindowRel { name: name.into(), tag_from_window: reference.inverse().compose(&app_from_win) });
        }
        i += 1;
    }
    let cfg = Anchor {
        id: id as u32,
        size_m: size,
        reference,
        saved: format!("unix {}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)),
        windows,
    };
    let p = cfg.save().unwrap_or_else(|e| die(format!("save: {e}")));
    println!("saved {} (tag in the app world: {})", p.display(), describe(&reference));
}

fn cmd_apply(a: &[String]) {
    let cfg = Anchor::load().unwrap_or_else(|e| die(e));
    let (size, id) = size_and_id(a, Some(&cfg));
    let c = capture(a, size, id);
    let (_, wm) = summarise(&c, 6);
    let Some((w_from_tag, n, st, sr)) = wm else { die("cannot apply: tag not seen with a head pose from Monado (PIMAX_ANCHOR=1); offset unchanged".into()) };
    if n < 6 || st > 0.015 || sr > 3.0 {
        die(format!("too few or too noisy samples ({n}, scatter {:.1} mm / {:.2} deg); offset unchanged", st * 1e3, sr));
    }
    let off = world::compute_offset(&cfg.reference, &w_from_tag, !has(a, "--full"));
    let (t, q) = world::pq(&off);
    let ang = 2.0 * (q[0] * q[0] + q[1] * q[1] + q[2] * q[2]).sqrt().atan2(q[3]);
    let ang = if ang > std::f64::consts::PI { ang - 2.0 * std::f64::consts::PI } else { ang };
    println!(
        "offset: translate ({:+.3} {:+.3} {:+.3}) m, rotate {:+.2} deg{}",
        t[0], t[1], t[2], ang.to_degrees(),
        if has(a, "--full") { " (full 3D)" } else { " about up (yaw only)" }
    );
    if geom::norm(t) > 6.0 {
        die("offset larger than 6 m: wrong tag or reference? not written".into());
    }
    if has(a, "--dry-run") {
        return;
    }
    let pid = c.ring_pid.unwrap_or(0);
    world::write_offset(&world::offset_path(), pid, &off).unwrap_or_else(|e| die(format!("write: {e}")));
    println!("wrote {} (Monado picks it up within a second)", world::offset_path().display());
}

fn cmd_status() {
    match Anchor::load() {
        Ok(c) => println!("config {}: tag36h11 id {} size {:.0} mm, {} saved windows, saved {}\n  reference {}", omarchyvr_anchor::config::default_path().display(), c.id, c.size_m * 1e3, c.windows.len(), c.saved, describe(&c.reference)),
        Err(e) => println!("config: {e}"),
    }
    match world::HeadRing::read(&world::ring_path()) {
        Some(r) => {
            let age = (monotonic_ns().saturating_sub(r.newest_ns())) as f64 / 1e9;
            println!("head ring {}: pid {}, newest sample {age:.1} s ago", world::ring_path().display(), r.pid);
            println!("  {}", world::feed_now().plain());
        }
        None => println!("head ring {}: absent (Monado not running with PIMAX_ANCHOR=1)", world::ring_path().display()),
    }
    match world::read_offset(&world::offset_path()) {
        Some((pid, o)) => println!("offset file: pid {pid}, {}", describe(&o)),
        None => println!("offset file: none (identity)"),
    }
}
