//! `pimax-passthrough`: Dream Air camera passthrough.
//!
//! ```text
//! pimax-passthrough render --out DIR [--tiles PREFIX | --source SRC [--index N]] [--depth M|inf]
//!                          [--size WxH] [--fov L,R,U,D] [--ipd MM] [--head-offset X,Y,Z] [--gamma G]
//!                          [--calib XML] [--overlay FILE|default] [--no-sweep]
//!     offline: per-eye passthrough images (pinhole, before lens distortion) from recorded frames, camera
//!     coverage maps, the cam0/cam1 overlap check and a depth sweep (report.txt)
//! pimax-passthrough run    [--source SRC] [--depth M|inf] [--res-div N] [--cam-delay-ms D] [--gamma G]
//!                          [--gain K] [--seconds S] [--off] [--floor] [--head-offset X,Y,Z] [--calib XML] [--stats S]
//!                          [--overlay FILE|default]
//!     live OpenXR client (needs monado-service; the camera must be free, i.e. the service runs without
//!     PIMAX_CAM / PIMAX_HANDS=mercury). Keys: p on/off, +/- depth, [/] camera delay, g gamma, f freeze, o floor grid, q quit
//! pimax-passthrough ctl on|off|toggle|gain K|dim|normal|brighter|darker|status   live control of `run` (socket, see control.rs)
//! pimax-passthrough rig    [--calib XML] [--overlay FILE|default] [--head-offset X,Y,Z]   camera geometry in the head frame
//! ```
//! SRC: empty / `auto` (live camera), `/dev/videoN`, or a `.pxcam` recording. PREFIX: `PREFIX0.png`..`PREFIX3.png`.
//! Env: `PIMAX_PT_HEAD_OFFSET=x,y,z` (mm) is the default for `--head-offset`.
//! `--overlay`: apply a learned calibration overlay (`pimax-selfcal`; `default` = `~/.config/pimax/cam-calib-overlay.json`)
//! on top of the factory calibration. Without it the factory calibration is used unchanged.

use pimax_cam::calib::DeviceCalib;
use pimax_cam::geom;
use pimax_cam::png;
use pimax_passthrough::check;
use pimax_passthrough::frames::{self, TileSet};
use pimax_passthrough::lut::{Lut, LutParams};
use pimax_passthrough::remap;
use pimax_passthrough::rig::{self, Rig};
use pimax_passthrough::timing::CamTiming;
use pimax_passthrough::view::{EyeView, Fov};
use pimax_passthrough::xr;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::time::Instant;

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
    fn done(&self) -> R<()> {
        match self.0.first() {
            Some(a) => Err(format!("unexpected argument {a}")),
            None => Ok(()),
        }
    }
}

fn parse_depth(s: &str) -> R<f64> {
    if s == "inf" || s == "infinite" {
        return Ok(f64::INFINITY);
    }
    s.parse::<f64>()
        .ok()
        .filter(|d| *d > 0.05)
        .ok_or_else(|| format!("bad depth {s} (metres > 0.05 or inf)"))
}

fn head_offset(a: &mut Args) -> R<geom::Vec3> {
    let s = match a.opt("--head-offset")? {
        Some(s) => Some(s),
        None => std::env::var("PIMAX_PT_HEAD_OFFSET").ok(),
    };
    let mm = match s {
        Some(s) => rig::parse_mm3(&s).ok_or_else(|| format!("bad head offset {s} (x,y,z mm)"))?,
        None => rig::DEFAULT_HEAD_OFFSET_MM,
    };
    Ok(geom::scale(mm, 1e-3))
}

fn load_rig(a: &mut Args) -> R<(Rig, PathBuf)> {
    let calib = a.opt("--calib")?.map(PathBuf::from);
    let overlay = a.opt("--overlay")?;
    let off = head_offset(a)?;
    let (mut c, p) = DeviceCalib::load_default(calib.as_deref()).map_err(|e| e.to_string())?;
    if let Some(spec) = overlay {
        let (c2, log) = rig::apply_overlay(&c, &spec)?;
        eprintln!("{log}");
        c = c2;
    }
    Ok((Rig::from_calib(&c, off)?, p))
}

fn cmd_rig(mut a: Args) -> R<()> {
    let (rig, p) = load_rig(&mut a)?;
    a.done()?;
    println!("calibration {}", p.display());
    print!("{}", rig.report());
    println!(
        "left eye camera order {:?}, right eye {:?}",
        rig.preference(0),
        rig.preference(1)
    );
    Ok(())
}

fn tiles_from_source(spec: &str, index: usize) -> R<TileSet> {
    let mut cam = frames::open_source(spec, false, false).map_err(|e| format!("{spec}: {e}"))?;
    let mut t = TileSet::black();
    for _ in 0..=index {
        if !frames::next_tracking(&mut cam, 2000, &mut t).map_err(|e| e.to_string())? {
            return Err(format!("{spec}: fewer than {} tracking frames", index + 1));
        }
    }
    Ok(t)
}

fn side_by_side(l: &[u8], r: &[u8], w: usize, h: usize, gap: usize) -> (Vec<u8>, usize) {
    let ow = 2 * w + gap;
    let mut out = vec![0u8; ow * h];
    for y in 0..h {
        out[y * ow..y * ow + w].copy_from_slice(&l[y * w..(y + 1) * w]);
        out[y * ow + w + gap..y * ow + 2 * w + gap].copy_from_slice(&r[y * w..(y + 1) * w]);
    }
    (out, ow)
}

fn write_png(path: &Path, w: usize, h: usize, ch: usize, px: &[u8]) -> R<()> {
    std::fs::write(path, png::encode(w, h, ch, px)).map_err(|e| format!("{}: {e}", path.display()))
}

fn cmd_render(mut a: Args) -> R<()> {
    let out = PathBuf::from(a.opt("--out")?.ok_or("--out DIR is required")?);
    let tiles_prefix = a.opt("--tiles")?;
    let source = a.opt("--source")?;
    let index: usize = a.num("--index", 0)?;
    let depth = parse_depth(&a.opt("--depth")?.unwrap_or_else(|| "2".into()))?;
    let size = a.opt("--size")?.unwrap_or_else(|| "888x960".into());
    let (w, h) = size
        .split_once('x')
        .and_then(|(x, y)| Some((x.parse::<usize>().ok()?, y.parse::<usize>().ok()?)))
        .filter(|&(x, y)| (16..=8192).contains(&x) && (16..=8192).contains(&y))
        .ok_or_else(|| format!("bad --size {size}"))?;
    let fov = match a.opt("--fov")? {
        Some(s) => {
            let v: Vec<f64> = s.split(',').filter_map(|t| t.trim().parse().ok()).collect();
            if v.len() != 4 || v.iter().any(|x| !(1.0..89.0).contains(x)) {
                return Err(format!(
                    "bad --fov {s} (left,right,up,down half angles in degrees)"
                ));
            }
            Fov::from_half_deg(v[0], v[1], v[2], v[3])
        }
        None => Fov::dream_air_default(),
    };
    let ipd_mm: f64 = a.num("--ipd", 63.0)?;
    let gamma: f64 = a.num("--gamma", 1.0)?;
    let sweep = !a.flag("--no-sweep");
    let (rig, calib_path) = load_rig(&mut a)?;
    a.done()?;

    let (tiles, what) = match (tiles_prefix, source) {
        (Some(p), None) => (TileSet::from_pngs(&p)?, format!("tiles {p}[0-3].png")),
        (None, Some(s)) => (
            tiles_from_source(&s, index)?,
            format!("source {s}, tracking frame #{index}"),
        ),
        (None, None) => return Err("give --tiles PREFIX or --source SRC".into()),
        _ => return Err("--tiles and --source are exclusive".into()),
    };
    std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
    let mut rep = String::new();
    let _ = writeln!(rep, "pimax-passthrough render: {what}");
    let _ = writeln!(rep, "calibration {}", calib_path.display());
    rep.push_str(&rig.report());
    let hd = fov.half_deg();
    let _ =
        writeln!(
        rep,
        "eyes {w}x{h}, FOV half angles L{:.1} R{:.1} U{:.1} D{:.1} deg, IPD {ipd_mm} mm, depth {}",
        hd[0],
        hd[1],
        hd[2],
        hd[3],
        if depth.is_finite() { format!("{depth} m") } else { "inf".into() }
    );
    let views = EyeView::default_pair(w, h, ipd_mm * 1e-3, fov);
    let p = LutParams {
        depth_m: depth,
        ..LutParams::default()
    };
    let t0 = Instant::now();
    let luts = [
        Lut::build(&rig, &views[0], 0, &p),
        Lut::build(&rig, &views[1], 1, &p),
    ];
    let build_ms = t0.elapsed().as_secs_f64() * 1e3;
    let tone = remap::tone_table(gamma, 1.0);
    let mut eyes = [vec![0u8; w * h], vec![0u8; w * h]];
    let t1 = Instant::now();
    for e in 0..2 {
        remap::remap_grey(&luts[e], &tiles.view(), &mut eyes[e], 0);
    }
    let remap_ms = t1.elapsed().as_secs_f64() * 1e3;
    for e in eyes.iter_mut() {
        for v in e.iter_mut() {
            *v = tone[*v as usize];
        }
    }
    let _ = writeln!(rep, "left  map: {}", luts[0].summary());
    let _ = writeln!(rep, "right map: {}", luts[1].summary());
    let _ = writeln!(
        rep,
        "map build {build_ms:.0} ms (both eyes), remap {remap_ms:.2} ms (both eyes, all cores)"
    );
    write_png(&out.join("left.png"), w, h, 1, &eyes[0])?;
    write_png(&out.join("right.png"), w, h, 1, &eyes[1])?;
    let (sbs, sw) = side_by_side(&eyes[0], &eyes[1], w, h, 16);
    write_png(&out.join("sbs.png"), sw, h, 1, &sbs)?;
    write_png(
        &out.join("coverage_left.png"),
        w,
        h,
        3,
        &luts[0].coverage_rgb(),
    )?;
    write_png(
        &out.join("coverage_right.png"),
        w,
        h,
        3,
        &luts[1].coverage_rgb(),
    )?;

    // overlap picture of the left eye: red = trackingA only, cyan = trackingB only
    let single = |cam: usize| {
        let l = Lut::build(
            &rig,
            &views[0],
            0,
            &LutParams {
                depth_m: depth,
                order: Some(vec![cam]),
                ..LutParams::default()
            },
        );
        let mut g = vec![0u8; w * h];
        remap::remap_grey(&l, &tiles.view(), &mut g, 0);
        g
    };
    let (ga, gb) = (single(0), single(1));
    let ana: Vec<u8> = ga.iter().zip(&gb).flat_map(|(&r, &c)| [r, c, c]).collect();
    write_png(
        &out.join("overlap_left_cam0_red_cam1_cyan.png"),
        w,
        h,
        3,
        &ana,
    )?;

    if sweep {
        let deg_per_px = ((fov.right.tan() - fov.left.tan()) / w as f64).to_degrees();
        let _ = writeln!(
            rep,
            "\noverlap check, left eye, trackingA (cam 0) vs trackingB (cam 1) single-camera renders\n\
             (best shift = where B's image matches A's; ~{deg_per_px:.3} deg per eye pixel near the centre;\n\
             with a correct calibration dy ~ 0 and dx shrinks when the depth matches the scene):"
        );
        let _ = writeln!(rep, "  depth     pixels   NCC@0   best dx  dy   NCC");
        let small = EyeView::default_pair(w / 2, h / 2, ipd_mm * 1e-3, fov);
        for o in check::depth_sweep(&rig, &small[0], &tiles.view(), (0, 1), 16) {
            let _ = writeln!(
                rep,
                "  {:>6} {:>9} {:>7.3} {:>8} {:>3} {:>6.3}",
                if o.depth_m.is_finite() {
                    format!("{}", o.depth_m)
                } else {
                    "inf".into()
                },
                o.pixels,
                o.ncc0,
                o.best_dx * 2,
                o.best_dy * 2,
                o.best_ncc
            );
        }
        let _ = writeln!(
            rep,
            "  (sweep at half resolution; shifts converted to full-resolution eye pixels)"
        );
    }
    print!("{rep}");
    std::fs::write(out.join("report.txt"), &rep).map_err(|e| e.to_string())?;
    println!("wrote {}", out.display());
    Ok(())
}

fn cmd_run(mut a: Args) -> R<()> {
    let source = a.opt("--source")?.unwrap_or_default();
    let calib = a.opt("--calib")?.map(PathBuf::from);
    let overlay = a.opt("--overlay")?;
    let head_offset_m = head_offset(&mut a)?;
    let depth_m = parse_depth(&a.opt("--depth")?.unwrap_or_else(|| "2".into()))?;
    let res_div: u32 = a.num("--res-div", 3)?;
    let delay_ms: f64 = a.num(
        "--cam-delay-ms",
        pimax_passthrough::timing::DEFAULT_CAM_DELAY_MS,
    )?;
    let gamma: f64 = a.num("--gamma", 1.0)?;
    let gain: f64 = a.num("--gain", 1.0)?;
    let seconds = a
        .opt("--seconds")?
        .map(|s| s.parse::<f64>())
        .transpose()
        .map_err(|e| e.to_string())?;
    let stats_every_s: f64 = a.num("--stats", 5.0)?;
    let start_off = a.flag("--off");
    let floor = a.flag("--floor");
    a.done()?;
    xr::run(xr::RunOpts {
        source,
        calib,
        overlay,
        head_offset_m,
        depth_m,
        res_div,
        timing: CamTiming {
            delay_ns: (delay_ms * 1e6) as i64,
            ..CamTiming::default()
        },
        seconds,
        gamma,
        gain,
        stats_every_s,
        start_off,
        floor,
    })
}

fn main() -> ExitCode {
    let mut v: Vec<String> = std::env::args().skip(1).collect();
    if v.is_empty() || v[0] == "-h" || v[0] == "--help" {
        eprintln!("usage: pimax-passthrough render|run|rig|ctl ... (see the crate docs / analysis/passthrough-design.md)");
        return ExitCode::from(2);
    }
    let cmd = v.remove(0);
    let a = Args(v);
    let r = match cmd.as_str() {
        "render" => cmd_render(a),
        "run" => cmd_run(a),
        "rig" => cmd_rig(a),
        "ctl" => {
            let line = a.0.join(" ");
            pimax_passthrough::control::send(&pimax_passthrough::control::socket_path(), &line).map(|r| println!("{r}"))
        }
        o => Err(format!("unknown command {o}")),
    };
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("pimax-passthrough: {e}");
            ExitCode::FAILURE
        }
    }
}
