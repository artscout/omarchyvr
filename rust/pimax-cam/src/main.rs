//! `pimax-cam`: Dream Air camera tool. Opening the live camera needs the headset lock
//! (`flock -w 3600 /tmp/pimax-headset.lock pimax-cam ...`); replays and `calib` do not.
//!
//! ```text
//! pimax-cam info      [--source S]                       device, formats, first frames' metadata
//! pimax-cam stats     [--source S] [--seconds N] [--fps F]
//! pimax-cam record    OUT.pxcam [--seconds N] [--fps F] [--zlib L] [--source S]
//! pimax-cam tiles     SRC --out DIR [--index N] [--kind tracking|led|any] [--count K]
//! pimax-cam rectify   SRC --out DIR [--pair A,B] [--index N] [--conv 1|2] [--kb6] [--focal F] [--calib XML] [--overlay F]
//! pimax-cam hypotheses SRC [--frames N] [--calib XML] [--overlay F]   rank calibration readings by epipolar error
//! pimax-cam calib     [XML] [--conv 1|2] [--overlay F]   parsed calibration and derived geometry
//! pimax-cam cpu       PID [--seconds N]                  per-thread CPU use of a process
//! pimax-cam import    IN.raw OUT.pxcam [--zlib L]        raw YUYV frames (v4l2-ctl dump) -> recording
//! pimax-cam broker    [--source S] [--fps F] [--socket P] [--slots N] [--loop]   share one camera stream with
//!                     any number of processes (they open the source `broker`; see src/broker.rs). The live camera
//!                     is reopened after an unplug or a 5 s stall (src/resilient.rs); the broker keeps serving.
//! pimax-cam broker-status [--socket P] [--window S] [--json]   probe a running broker: camera state, frames/s,
//!                     reopens (exit 0 streaming, 1 no broker, 2 camera reopening / no frames)
//! ```
//! SRC / S: empty or `auto` (live camera), `/dev/videoN`, `broker[:SOCKET]` (frames of a running broker), or a
//! `.pxcam` recording.
//! `--overlay F`: apply a learned calibration overlay (`pimax-selfcal`; `default` = `~/.config/pimax/cam-calib-overlay.json`)
//! on top of the factory calibration. Without it the factory calibration is used unchanged.

use pimax_cam::calib::{DeviceCalib, RigConvention};
use pimax_cam::camera::{Camera, FrameInfo};
use pimax_cam::check::{self, Hypothesis};
use pimax_cam::frame::{FrameKind, TILE_H, TILE_W};
use pimax_cam::rectify::{MatchParams, RectifyParams};
use pimax_cam::source::ReplayOptions;
use pimax_cam::stats::CaptureStats;
use pimax_cam::{clock, png};
use std::path::{Path, PathBuf};
use std::process::ExitCode;

type R<T> = Result<T, String>;

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
    fn num<T: std::str::FromStr>(&mut self, n: &str, d: T) -> R<T> {
        match self.opt(n)? {
            Some(v) => v.parse().map_err(|_| format!("bad value for {n}: {v}")),
            None => Ok(d),
        }
    }
    fn pos(&self, i: usize) -> Option<String> {
        self.0
            .iter()
            .filter(|a| !a.starts_with("--"))
            .nth(i)
            .cloned()
    }
    fn done(&self) -> R<()> {
        match self.0.iter().find(|a| a.starts_with("--")) {
            Some(a) => Err(format!("unknown option {a}")),
            None => Ok(()),
        }
    }
}

fn conv_of(v: u32) -> R<RigConvention> {
    match v {
        1 => Ok(RigConvention::Cam0FromCam),
        2 => Ok(RigConvention::CamFromCam0),
        _ => Err("--conv must be 1 or 2".into()),
    }
}

fn open(spec: &str, fps: u32) -> R<Camera> {
    let cam = Camera::open(spec, fps, ReplayOptions::default())
        .map_err(|e| format!("open {spec:?}: {e}"))?;
    eprintln!("source: {}", cam.describe());
    Ok(cam)
}

fn print_info(i: &FrameInfo) {
    println!(
        "frame ctr {} seq {} kind {} type {:#04x} idx {} dev {} us host {} ns mono {} ns exp {:?} gain {:?} rows {:?} dropped {} flags {:#x}",
        i.counter,
        i.sequence,
        kind_name(i.kind),
        i.frame_type,
        i.stream_index,
        i.dev_ts_us,
        i.host_ts_ns,
        i.mono_ts_ns,
        i.exposure,
        i.gain,
        i.y_offset,
        i.dropped,
        i.flags
    );
}

fn kind_name(k: u32) -> &'static str {
    match k {
        1 => FrameKind::Tracking.name(),
        2 => FrameKind::Led.name(),
        _ => FrameKind::Unknown.name(),
    }
}

fn cmd_info(mut a: Args) -> R<()> {
    let src = a.opt("--source")?.unwrap_or_default();
    let n: usize = a.num("--frames", 6)?;
    a.done()?;
    if src.is_empty() {
        println!("device: {:?}", pimax_cam::v4l2::find_device());
    }
    let mut cam = open(&src, 0)?;
    for _ in 0..n {
        match cam.next_frame(1000).map_err(|e| e.to_string())? {
            Some(i) => print_info(&i),
            None => return Err("no frame within 1 s".into()),
        }
    }
    Ok(())
}

fn cmd_stats(mut a: Args) -> R<()> {
    let src = a.opt("--source")?.unwrap_or_default();
    let secs: f64 = a.num("--seconds", 10.0)?;
    let fps: u32 = a.num("--fps", 0)?;
    a.done()?;
    let mut cam = open(&src, fps)?;
    let mut st = CaptureStats::default();
    let t0 = clock::monotonic_ns();
    let mut timeouts = 0;
    let mut next_print = 1.0;
    while (clock::monotonic_ns() - t0) as f64 / 1e9 < secs {
        match cam.next_frame(500).map_err(|e| e.to_string())? {
            Some(i) => st.add(&i),
            None if cam.source().finished() => break,
            None => timeouts += 1,
        }
        let el = (clock::monotonic_ns() - t0) as f64 / 1e9;
        if el >= next_print {
            eprintln!(
                "  {:5.1} s: {} frames ({} tracking, {} led), lost {}",
                el, st.frames, cam.stats.tracking, cam.stats.led, st.dropped
            );
            next_print += 5.0;
        }
    }
    st.finish();
    print!("{}", st.report());
    let s = &cam.stats;
    println!(
        "  kinds: tracking {} led {} unknown {}; non-alternating {}; rows scanned {}; row/picture mismatch {}; corrupt buffers {}; poll timeouts {}",
        s.tracking,
        s.led,
        s.unknown,
        s.kind_repeats,
        s.scanned_rows,
        s.row_mismatch,
        cam.corrupt_frames(),
        timeouts
    );
    Ok(())
}

static REC_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn rec_on_signal(_: libc::c_int) {
    REC_STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn cmd_record(mut a: Args) -> R<()> {
    let src = a.opt("--source")?.unwrap_or_default();
    let secs: f64 = a.num("--seconds", 20.0)?;
    let fps: u32 = a.num("--fps", 0)?;
    let zlib: u8 = a.num("--zlib", 0)?;
    let max_frames: u64 = a.num("--frames", u64::MAX)?;
    let out = a.pos(0).ok_or("record needs an output path")?;
    a.0.retain(|x| x != &out);
    a.done()?;
    let mut cam = open(&src, fps)?;
    cam.start_recording(Path::new(&out), (zlib > 0).then_some(zlib))
        .map_err(|e| format!("{out}: {e}"))?;
    let mut st = CaptureStats::default();
    let t0 = clock::monotonic_ns();
    // SIGINT/SIGTERM end the recording cleanly (the file is flushed): `pimax record tracking stop`
    // SAFETY: the handler only stores to an atomic.
    unsafe {
        libc::signal(libc::SIGINT, rec_on_signal as *const () as libc::sighandler_t);
        libc::signal(libc::SIGTERM, rec_on_signal as *const () as libc::sighandler_t);
    }
    while (clock::monotonic_ns() - t0) as f64 / 1e9 < secs
        && st.frames < max_frames
        && !REC_STOP.load(std::sync::atomic::Ordering::Relaxed)
    {
        match cam.next_frame(500).map_err(|e| e.to_string())? {
            Some(i) => st.add(&i),
            None if cam.source().finished() => break,
            None => {}
        }
    }
    let (frames, bytes) = cam
        .stop_recording()
        .map_err(|e| format!("recording failed: {e}"))?
        .unwrap_or((0, 0));
    st.finish();
    print!("{}", st.report());
    println!(
        "recorded {frames} frames ({} not queued: disk too slow), {:.1} MB -> {out}",
        cam.stats.record_dropped,
        bytes as f64 / 1e6
    );
    Ok(())
}

/// Convert raw frames (concatenated 2129920-byte YUYV buffers, e.g. from `v4l2-ctl --stream-to`) into a
/// `.pxcam` recording with synthetic 50 fps host timestamps.
fn cmd_import(mut a: Args) -> R<()> {
    let zlib: u8 = a.num("--zlib", 6)?;
    let inp = a.pos(0).ok_or("import needs IN.raw OUT.pxcam")?;
    let out = a.pos(1).ok_or("import needs IN.raw OUT.pxcam")?;
    a.0.retain(|x| x != &inp && x != &out);
    a.done()?;
    let data = std::fs::read(&inp).map_err(|e| format!("{inp}: {e}"))?;
    let fb = pimax_cam::frame::FRAME_BYTES;
    if data.len() % fb != 0 || data.is_empty() {
        return Err(format!(
            "{inp}: {} bytes is not a multiple of {fb}",
            data.len()
        ));
    }
    let mut rec = pimax_cam::record::Recorder::create(
        Path::new(&out),
        pimax_cam::frame::FRAME_W,
        (zlib > 0).then_some(zlib),
    )
    .map_err(|e| format!("{out}: {e}"))?;
    for (i, f) in data.chunks(fb).enumerate() {
        let raw = pimax_cam::source::RawFrame {
            data: f,
            stride: pimax_cam::frame::FRAME_W,
            host_ts_ns: 1_000_000_000 + i as u64 * 19_988_000,
            sequence: i as u32,
        };
        rec.write(&raw).map_err(|e| e.to_string())?;
    }
    let (_, n, bytes) = rec.finish().map_err(|e| e.to_string())?;
    println!("imported {n} frames, {:.1} MB -> {out}", bytes as f64 / 1e6);
    Ok(())
}

/// Advance to the `index`-th frame of the requested kind and return the four tiles.
fn grab(cam: &mut Camera, kind: Option<FrameKind>, index: usize) -> R<(FrameInfo, Vec<Vec<u8>>)> {
    let mut seen = 0;
    loop {
        let i = cam
            .next_frame(1000)
            .map_err(|e| e.to_string())?
            .ok_or("no more frames")?;
        if kind.is_none_or(|k| k as u32 == i.kind) {
            if seen == index {
                let tiles = (0..4)
                    .map(|t| cam.tile(t).map_err(|e| e.to_string()))
                    .collect::<R<Vec<_>>>()?;
                return Ok((i, tiles));
            }
            seen += 1;
        }
    }
}

fn parse_kind(s: &str) -> R<Option<FrameKind>> {
    match s {
        "tracking" => Ok(Some(FrameKind::Tracking)),
        "led" => Ok(Some(FrameKind::Led)),
        "any" => Ok(None),
        _ => Err(format!("bad kind {s}")),
    }
}

fn cmd_tiles(mut a: Args) -> R<()> {
    let out = PathBuf::from(a.opt("--out")?.ok_or("--out DIR required")?);
    let index: usize = a.num("--index", 0)?;
    let count: usize = a.num("--count", 1)?;
    let kind = parse_kind(&a.opt("--kind")?.unwrap_or_else(|| "tracking".into()))?;
    let src = a.pos(0).unwrap_or_default();
    a.0.retain(|x| x != &src);
    a.done()?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut cam = open(&src, 0)?;
    for n in 0..count {
        let (i, tiles) = grab(&mut cam, kind, if n == 0 { index } else { 0 })?;
        print_info(&i);
        for (t, img) in tiles.iter().enumerate() {
            let p = out.join(format!(
                "ctr{}_{}_tile{t}.png",
                i.counter,
                kind_name(i.kind)
            ));
            png::write_grey(&p, TILE_W, TILE_H, img).map_err(|e| e.to_string())?;
            let ps = out.join(format!(
                "ctr{}_{}_tile{t}_stretched.png",
                i.counter,
                kind_name(i.kind)
            ));
            png::write_grey(&ps, TILE_W, TILE_H, &png::stretch(img)).map_err(|e| e.to_string())?;
        }
        // mosaic of the four tiles, contrast stretched
        let mut m = vec![0u8; 4 * TILE_W * TILE_H];
        for (t, img) in tiles.iter().enumerate() {
            let s = png::stretch(img);
            for y in 0..TILE_H {
                m[y * 4 * TILE_W + t * TILE_W..y * 4 * TILE_W + (t + 1) * TILE_W]
                    .copy_from_slice(&s[y * TILE_W..(y + 1) * TILE_W]);
            }
        }
        png::write_grey(
            &out.join(format!("ctr{}_{}_mosaic.png", i.counter, kind_name(i.kind))),
            4 * TILE_W,
            TILE_H,
            &m,
        )
        .map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn load_calib(p: Option<String>, overlay: Option<String>) -> R<DeviceCalib> {
    let (c, used) =
        DeviceCalib::load_default(p.as_deref().map(Path::new)).map_err(|e| e.to_string())?;
    eprintln!("calibration: {}", used.display());
    let Some(ov) = overlay else {
        return Ok(c);
    };
    let path = if ov == "default" {
        pimax_cam::overlay::default_path().ok_or("no default overlay path (HOME unset)")?
    } else {
        PathBuf::from(ov)
    };
    let o = pimax_cam::overlay::CalibOverlay::load(&path).map_err(|e| e.to_string())?;
    let c2 = c
        .with_overlay(&o, pimax_cam::overlay::DEFAULT_MAX_ANGLE_DEG)
        .map_err(|e| format!("{}: {e}", path.display()))?;
    for r in &o.rotations {
        eprintln!(
            "overlay {}: camera {} rotated {:.3} deg (rotation vector {:?} rad)",
            path.display(),
            r.id,
            r.angle_deg(),
            r.rotation_vector
        );
    }
    Ok(c2)
}

fn cmd_rectify(mut a: Args) -> R<()> {
    let out = PathBuf::from(a.opt("--out")?.ok_or("--out DIR required")?);
    let pair = a.opt("--pair")?.unwrap_or_else(|| "0,1".into());
    let tiles_opt = a.opt("--tiles")?;
    let index: usize = a.num("--index", 0)?;
    let conv = conv_of(a.num("--conv", 2)?)?;
    let kb6 = a.flag("--kb6");
    let focal: f64 = a.num("--focal", 220.0)?;
    let calp = a.opt("--calib")?;
    let ovp = a.opt("--overlay")?;
    let mp = match_params(&mut a)?;
    let src = a.pos(0).unwrap_or_default();
    a.0.retain(|x| x != &src);
    a.done()?;
    let ab: Vec<usize> = pair
        .split(',')
        .map(|s| s.trim().parse().map_err(|_| "bad --pair"))
        .collect::<Result<_, _>>()?;
    if ab.len() != 2 || ab.iter().any(|&x| x > 3) || ab[0] == ab[1] {
        return Err("--pair A,B with two different cameras 0..3".into());
    }
    let tl: Vec<usize> = match tiles_opt {
        Some(t) => t
            .split(',')
            .map(|s| s.trim().parse().map_err(|_| "bad --tiles"))
            .collect::<Result<_, _>>()?,
        None => ab.clone(),
    };
    let cal = load_calib(calp, ovp)?;
    std::fs::create_dir_all(&out).map_err(|e| e.to_string())?;
    let mut cam = open(&src, 0)?;
    let (i, tiles) = grab(&mut cam, Some(FrameKind::Tracking), index)?;
    print_info(&i);
    let h = Hypothesis {
        cam_a: ab[0],
        cam_b: ab[1],
        tile_a: tl[0],
        tile_b: tl[1],
        conv,
        extra_radial: kb6,
    };
    let rp = RectifyParams {
        width: 640,
        height: 480,
        focal,
    };
    let o = check::evaluate(&cal, &tiles, &h, rp, &mp);
    println!("{}: {}", h.label(), o.report.summary());
    let tag = format!("cam{}{}_ctr{}", ab[0], ab[1], i.counter);
    let w = |name: &str, r: std::io::Result<()>| r.map_err(|e| format!("{name}: {e}"));
    for (t, img) in tiles.iter().enumerate() {
        let n = format!("{tag}_tile{t}.png");
        w(
            &n,
            png::write_grey(&out.join(&n), TILE_W, TILE_H, &png::stretch(img)),
        )?;
    }
    let n = format!("{tag}_rect_left.png");
    w(
        &n,
        png::write_grey(&out.join(&n), rp.width, rp.height, &png::stretch(&o.left)),
    )?;
    let n = format!("{tag}_rect_right.png");
    w(
        &n,
        png::write_grey(&out.join(&n), rp.width, rp.height, &png::stretch(&o.right)),
    )?;
    let n = format!("{tag}_rect_pair_lines.png");
    w(
        &n,
        png::write_rgb(
            &out.join(&n),
            2 * rp.width,
            rp.height,
            &check::pair_with_lines(&o.left, &o.right, rp.width, rp.height, 24),
        ),
    )?;
    let n = format!("{tag}_rect_anaglyph.png");
    w(
        &n,
        png::write_rgb(
            &out.join(&n),
            rp.width,
            rp.height,
            &check::anaglyph(&o.left, &o.right),
        ),
    )?;
    // match list for the record
    let mut csv = String::from("x,y,disparity,dy,zncc\n");
    for m in &o.report.matches {
        csv += &format!("{},{},{:.2},{:.2},{:.3}\n", m.x, m.y, m.disp, m.dy, m.score);
    }
    let n = format!("{tag}_matches.csv");
    w(&n, std::fs::write(out.join(&n), csv))?;
    println!("wrote {}/{tag}_*", out.display());
    Ok(())
}

fn cmd_hypotheses(mut a: Args) -> R<()> {
    let frames: usize = a.num("--frames", 3)?;
    let skip: usize = a.num("--skip", 0)?;
    let every: usize = a.num("--every", 5)?;
    let calp = a.opt("--calib")?;
    let ovp = a.opt("--overlay")?;
    let pairs_s = a
        .opt("--pairs")?
        .unwrap_or_else(|| "0,1;2,3;0,2;1,3".into());
    let mp = match_params(&mut a)?;
    let src = a.pos(0).unwrap_or_default();
    a.0.retain(|x| x != &src);
    a.done()?;
    let cal = load_calib(calp, ovp)?;
    let mut cam = open(&src, 0)?;
    let mut sets = Vec::new();
    for n in 0..frames {
        let (i, tiles) = grab(
            &mut cam,
            Some(FrameKind::Tracking),
            if n == 0 { skip } else { every },
        )?;
        eprintln!("frame ctr {}", i.counter);
        sets.push(tiles);
    }
    let mut pairs = Vec::new();
    for p in pairs_s.split(';') {
        let v: Vec<usize> = p
            .split(',')
            .map(|s| s.trim().parse().map_err(|_| "bad --pairs"))
            .collect::<Result<_, _>>()?;
        if v.len() != 2 || v.iter().any(|&x| x > 3) {
            return Err("bad --pairs".into());
        }
        pairs.push((v[0], v[1]));
    }
    let rp = RectifyParams::default();
    println!(
        "{:58} {:>7} {:>9} {:>6} {:>7} {:>7} {:>7}",
        "hypothesis", "matches", "median_dy", "MAD", "<=1px", "<=2px", "disp"
    );
    let mut verdicts = Vec::new();
    for (ca, cb) in pairs {
        let mut rows = Vec::new();
        for conv in [RigConvention::CamFromCam0, RigConvention::Cam0FromCam] {
            for kb6 in [false, true] {
                // tile mappings: as labelled, swapped within the pair, or one tile replaced by its vertical
                // partner (0<->2, 1<->3: same mounting orientation, so the pictures cannot tell them apart)
                let partner = |t: usize| t ^ 2;
                let mut maps = vec![(ca, cb), (cb, ca), (partner(ca), cb), (ca, partner(cb))];
                maps.dedup();
                for (ta, tb) in maps {
                    if ta == tb {
                        continue;
                    }
                    let h = Hypothesis {
                        cam_a: ca,
                        cam_b: cb,
                        tile_a: ta,
                        tile_b: tb,
                        conv,
                        extra_radial: kb6,
                    };
                    let mut all = pimax_cam::rectify::EpipolarReport::default();
                    for tiles in &sets {
                        let o = check::evaluate(&cal, tiles, &h, rp, &mp);
                        all.candidates += o.report.candidates;
                        all.matches.extend(o.report.matches);
                    }
                    println!(
                        "{:58} {:>7} {:>+9.2} {:>6.2} {:>6.0}% {:>6.0}% {:>7.1}",
                        h.label(),
                        all.matches.len(),
                        all.median_dy(),
                        all.mad_dy(),
                        all.frac_within(1.0) * 100.0,
                        all.frac_within(2.0) * 100.0,
                        all.median_disp()
                    );
                    rows.push((h, all));
                }
            }
        }
        if let Some((_, _, text)) = check::verdict(&rows) {
            verdicts.push(text);
        }
    }
    println!();
    for v in verdicts {
        println!("{v}");
    }
    Ok(())
}

/// `--min-score`, `--max-dy`, `--max-disp`, `--min-std`, `--half`, `--cell` overrides.
fn match_params(a: &mut Args) -> R<MatchParams> {
    let d = MatchParams::default();
    Ok(MatchParams {
        min_score: a.num("--min-score", d.min_score)?,
        max_dy: a.num("--max-dy", d.max_dy)?,
        max_disp: a.num("--max-disp", d.max_disp)?,
        min_std: a.num("--min-std", d.min_std)?,
        half: a.num("--half", d.half)?,
        cell: a.num("--cell", d.cell)?,
        ..d
    })
}

fn cmd_calib(mut a: Args) -> R<()> {
    let conv = conv_of(a.num("--conv", 2)?)?;
    let ovp = a.opt("--overlay")?;
    let p = a.pos(0);
    a.done()?;
    let cal = load_calib(p, ovp)?;
    print!("{}", cal.geometry_report(conv));
    Ok(())
}

fn read_task_cpu(pid: u32) -> std::collections::BTreeMap<u32, (String, u64)> {
    let mut m = std::collections::BTreeMap::new();
    if let Ok(rd) = std::fs::read_dir(format!("/proc/{pid}/task")) {
        for e in rd.flatten() {
            let tid: u32 = match e.file_name().to_string_lossy().parse() {
                Ok(t) => t,
                Err(_) => continue,
            };
            let Ok(stat) = std::fs::read_to_string(e.path().join("stat")) else {
                continue;
            };
            // comm is in parentheses and may contain spaces
            let (Some(l), Some(r)) = (stat.find('('), stat.rfind(')')) else {
                continue;
            };
            let comm = stat[l + 1..r].to_string();
            let f: Vec<&str> = stat[r + 2..].split_whitespace().collect();
            // fields after ')': state(0) ... utime(11) stime(12)
            let ticks = f.get(11).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0)
                + f.get(12).and_then(|v| v.parse::<u64>().ok()).unwrap_or(0);
            m.insert(tid, (comm, ticks));
        }
    }
    m
}

fn cmd_cpu(mut a: Args) -> R<()> {
    let secs: f64 = a.num("--seconds", 10.0)?;
    let pid: u32 = a
        .pos(0)
        .ok_or("cpu needs a PID")?
        .parse()
        .map_err(|_| "bad PID")?;
    a.0.clear();
    // SAFETY: sysconf is always safe to call.
    let hz = unsafe { libc::sysconf(libc::_SC_CLK_TCK) } as f64;
    let before = read_task_cpu(pid);
    if before.is_empty() {
        return Err(format!("no such process {pid}"));
    }
    std::thread::sleep(std::time::Duration::from_secs_f64(secs));
    let after = read_task_cpu(pid);
    let mut rows: Vec<(f64, u32, String)> = after
        .iter()
        .map(|(tid, (comm, t))| {
            let t0 = before.get(tid).map(|b| b.1).unwrap_or(0);
            ((t - t0) as f64 / hz / secs * 100.0, *tid, comm.clone())
        })
        .collect();
    rows.sort_by(|x, y| y.0.partial_cmp(&x.0).unwrap());
    let total: f64 = rows.iter().map(|r| r.0).sum();
    println!(
        "process {pid}: {total:.1}% of one core over {secs:.0} s ({} threads)",
        rows.len()
    );
    for (pct, tid, comm) in rows.iter().filter(|r| r.0 >= 0.05) {
        println!("  {pct:6.1}%  {tid:7}  {comm}");
    }
    Ok(())
}

static BROKER_STOP: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

extern "C" fn broker_on_signal(_: libc::c_int) {
    BROKER_STOP.store(true, std::sync::atomic::Ordering::Relaxed);
}

fn cmd_broker(mut a: Args) -> R<()> {
    use pimax_cam::broker::{self, ServeOptions};
    let src = a.opt("--source")?.unwrap_or_default();
    let fps: u32 = a.num("--fps", 0)?;
    let slots: u32 = a.num("--slots", broker::DEFAULT_SLOTS)?;
    let socket = a
        .opt("--socket")?
        .map(PathBuf::from)
        .unwrap_or_else(broker::default_socket);
    let looping = a.flag("--loop");
    a.done()?;
    if src.trim().starts_with("broker") {
        return Err("a broker cannot serve another broker".into());
    }
    let replay = ReplayOptions {
        looping,
        realtime: true,
    };
    let source =
        pimax_cam::source::open(&src, fps, replay).map_err(|e| format!("open {src:?}: {e}"))?;
    // SAFETY: installing a handler that only stores to an atomic.
    unsafe {
        libc::signal(
            libc::SIGINT,
            broker_on_signal as *const () as libc::sighandler_t,
        );
        libc::signal(
            libc::SIGTERM,
            broker_on_signal as *const () as libc::sighandler_t,
        );
    }
    eprintln!(
        "camera broker: {} -> {} ({} slots); clients open the source `broker`{}; Ctrl-C stops",
        source.describe(),
        socket.display(),
        slots,
        if socket == broker::default_socket() {
            String::new()
        } else {
            format!(" (here: `broker:{}`)", socket.display())
        }
    );
    let opts = ServeOptions {
        socket,
        slots,
        ..ServeOptions::default()
    };
    let mut last = 0u64;
    let mut last_up = true;
    let st = broker::serve(source, &opts, &BROKER_STOP, |s| {
        if s.camera_up != last_up {
            last_up = s.camera_up;
            eprintln!(
                "  camera {} (reopens so far: {}); the broker keeps serving, clients stay connected",
                if s.camera_up { "streaming again" } else { "lost, reopening" },
                s.camera_reopens
            );
            return;
        }
        eprintln!(
            "  camera {}, {:.1} frames/s, {} frames, {} client(s) now ({} ever), {} notices dropped, {} source timeouts",
            if s.camera_up { "up" } else { "REOPENING" },
            (s.frames - last) as f64 / opts.status_every.as_secs_f64(),
            s.frames,
            s.clients,
            s.clients_total,
            s.notices_dropped,
            s.timeouts
        );
        last = s.frames;
    })
    .map_err(|e| e.to_string())?;
    eprintln!(
        "camera broker stopped: {} frames, {} client(s) served, {} notices dropped",
        st.frames, st.clients_total, st.notices_dropped
    );
    Ok(())
}

/// `pimax-cam broker-status [--socket P] [--window S] [--json]`: probe a running broker (exit 0 = camera
/// streaming, 1 = no broker, 2 = broker up but the camera is reopening or no frames in the window).
fn cmd_broker_status(mut a: Args) -> R<()> {
    use pimax_cam::broker;
    let socket = a
        .opt("--socket")?
        .map(PathBuf::from)
        .unwrap_or_else(broker::default_socket);
    let window: f64 = a.num("--window", 1.0)?;
    let json = a.flag("--json");
    a.done()?;
    let p = match broker::probe(&socket, std::time::Duration::from_secs_f64(window.clamp(0.05, 30.0))) {
        Ok(p) => p,
        Err(e) => {
            if json {
                println!("{{\"broker\":false,\"error\":\"{}\"}}", e.to_string().replace('"', "'"));
            } else {
                eprintln!("no broker: {e}");
            }
            std::process::exit(1);
        }
    };
    if json {
        println!("{}", p.to_json());
    } else {
        println!(
            "broker {}: camera {}, {:.1} frames/s, {} frames published, {} camera reopens, heartbeat {}",
            socket.display(),
            match p.camera_up {
                Some(true) => "streaming",
                Some(false) => "REOPENING",
                None => "unknown (older broker)",
            },
            p.fps,
            p.published,
            p.camera_reopens,
            p.heartbeat_age_s
                .map_or("n/a".to_string(), |a| format!("{a:.2} s ago"))
        );
    }
    if p.camera_up == Some(false) || p.fps <= 0.0 {
        std::process::exit(2);
    }
    Ok(())
}

fn usage() -> &'static str {
    "usage: pimax-cam info|stats|record|tiles|rectify|hypotheses|calib|cpu|import|broker|broker-status ... (see the module docs in src/main.rs)"
}

fn main() -> ExitCode {
    let mut argv: Vec<String> = std::env::args().skip(1).collect();
    if argv.is_empty() || argv[0] == "-h" || argv[0] == "--help" {
        eprintln!("{}", usage());
        return ExitCode::from(2);
    }
    let cmd = argv.remove(0);
    let a = Args(argv);
    let r = match cmd.as_str() {
        "info" => cmd_info(a),
        "stats" => cmd_stats(a),
        "record" => cmd_record(a),
        "tiles" => cmd_tiles(a),
        "rectify" => cmd_rectify(a),
        "hypotheses" => cmd_hypotheses(a),
        "calib" => cmd_calib(a),
        "import" => cmd_import(a),
        "cpu" => cmd_cpu(a),
        "broker" => cmd_broker(a),
        "broker-status" => cmd_broker_status(a),
        _ => Err(usage().into()),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pimax-cam {cmd}: {e}");
            ExitCode::FAILURE
        }
    }
}
