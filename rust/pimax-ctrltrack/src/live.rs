//! `pimax-ctrltrack live`: controller poses from the live LED frames (camera broker), published for Monado.
//!
//! Each LED frame (25 Hz): blobs per camera (`BlobParams::live`), then the shared tracker (`tracker.rs`): every
//! controller seen recently is tracked from its velocity-predicted prior (12 px gate, 24 px when it moves fast;
//! 3-4 LED fixes with the orientation tied to the prediction); a controller that is not tracked is searched by
//! brute force (about 10 ms over 4 threads). Poses go to the seqlock file read by Monado's Pimax controller driver
//! (PIMAX_CTRL_TRACK=1). `--baseline` runs the tracker and blob detection as before 2026-10-15 (A/B).

use crate::track::{arg, LedFrame, Rig};
use crate::tracker::{TrackCfg, Tracker};
use pimax_cam::frame::{self, FrameKind, Meta, TILE_COUNT, TILE_H, TILE_W};
use pimax_cam::geom::{self, Pose};
use pimax_cam::source::{self, ReplayOptions};
use pimax_ctrltrack::blob::{self, BlobParams};
use pimax_ctrltrack::shm::{self, Beat, HandPose, PoseWriter};
use pimax_ctrltrack::solve::quat_to_mat;
use std::io::Write;
use std::path::PathBuf;
use std::time::Instant;

/// Default pitch of the aim frame relative to the LED-model frame (deg): with it, the aim ray of a controller held
/// the way the user held them in `ctrl-test.pxcam` points level and straight ahead (mean model orientation 0-40 s,
/// about 52 deg about +x for both hands; analysis/controller-6dof.md).
pub const DEFAULT_AIM_PITCH_DEG: f64 = 52.0;

pub fn live(a: &[String]) {
    let spec = arg(a, "--source").unwrap_or("broker").to_string();
    let out = arg(a, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(shm::default_path);
    let secs: f64 = arg(a, "--seconds")
        .map(|v| v.parse().unwrap())
        .unwrap_or(f64::INFINITY);
    let pitch: f64 = arg(a, "--aim-pitch-deg")
        .map(|v| v.parse().unwrap())
        .or_else(|| {
            std::env::var("PIMAX_CTRLTRACK_AIM_PITCH_DEG")
                .ok()
                .and_then(|v| v.parse().ok())
        })
        .unwrap_or(DEFAULT_AIM_PITCH_DEG);
    let stats_every: f64 = arg(a, "--stats").map(|v| v.parse().unwrap()).unwrap_or(5.0);
    let rig = Rig::load(a);
    // device_from_aim: rotation about the model's x axis
    let device_from_aim = Pose {
        r: geom::rot_x(-pitch),
        t: [0.0; 3],
    };
    let aim_from_imu: Vec<[f32; 4]> = rig
        .models
        .iter()
        .map(|m| {
            let r = geom::mat_mul(&geom::transpose(&device_from_aim.r), &m.imu_to_device.r);
            let q = geom::quat_from_mat(&r);
            [q[0] as f32, q[1] as f32, q[2] as f32, q[3] as f32]
        })
        .collect();
    let mut src = match source::open(
        &spec,
        0,
        ReplayOptions {
            looping: false,
            realtime: true,
        },
    ) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("cannot open source {spec:?}: {e}");
            std::process::exit(1);
        }
    };
    let mut w = match PoseWriter::create(&out) {
        Ok(w) => w,
        Err(e) => {
            eprintln!("cannot create {}: {e}", out.display());
            std::process::exit(1);
        }
    };
    eprintln!(
        "live: source {} -> {} (aim pitch {pitch} deg about the model x axis)",
        src.describe(),
        out.display()
    );

    let bp = if a.iter().any(|s| s == "--baseline") {
        BlobParams::default()
    } else {
        BlobParams::from_args(a)
    };
    let mut tile = vec![0u8; TILE_W * TILE_H];
    let t_start = Instant::now();
    let mut cfg = if a.iter().any(|s| s == "--baseline") {
        TrackCfg::baseline()
    } else {
        TrackCfg::default()
    };
    cfg.body = !a.iter().any(|s| s == "--no-body");
    let mut tracker = Tracker::new(cfg);
    let mut trace = arg(a, "--trace")
        .map(|p| std::io::BufWriter::new(std::fs::File::create(p).expect("trace")));
    let mut t_stats = Instant::now();
    let mut solve_ms = 0.0f64;
    // Heartbeat in the pose file (shm.rs): written at least every 0.5 s (the read timeout) while this loop runs, so
    // a probe can tell a live tracker without controllers in view from a hung or dead one. The source reopens
    // itself after a broker restart or a camera drop (pimax_cam::resilient); `reopens` counts those.
    let mut beat = Beat::default();
    let mut gen = src.generation();
    loop {
        if t_start.elapsed().as_secs_f64() > secs {
            break;
        }
        let now = shm::monotonic_ns();
        if now.saturating_sub(beat.beat_ns) >= 100_000_000 {
            let g = src.generation();
            if g != gen {
                gen = g;
                beat.reopens += 1;
            }
            beat.beat_ns = now;
            beat.source_up = src.up();
            w.beat(beat);
        }
        let fr = match src.next(500) {
            Ok(Some(f)) => f,
            Ok(None) => {
                if src.finished() {
                    eprintln!("live: source finished");
                    break;
                }
                continue;
            }
            Err(e) => {
                // only a replay ends this way; a live source reopens by itself
                eprintln!("live: source error: {e}");
                break;
            }
        };
        let Some(m) = Meta::parse(&fr.data[..frame::META_LEN]) else {
            continue;
        };
        if m.kind() != FrameKind::Led {
            continue;
        }
        beat.led_frames += 1;
        let frame_ns = fr.host_ts_ns;
        let t0 = Instant::now();
        let rows = frame::tile_rows(Some(&m), fr.data, fr.stride);
        let mut px = Vec::with_capacity(TILE_COUNT);
        for t in 0..TILE_COUNT {
            frame::copy_tile(fr.data, fr.stride, t, rows.y[t], &mut tile, TILE_W);
            px.push(
                blob::detect(&tile, TILE_W, TILE_H, TILE_W, &bp)
                    .iter()
                    .map(|b| (b.x, b.y))
                    .collect(),
            );
        }
        let lf = LedFrame {
            rec: fr.sequence as usize,
            ts_us: m.ts_us,
            px,
        };
        for fx in tracker.step(&rig, &lf, frame_ns) {
            let (slot, f) = (fx.slot, &fx.found);
            if let Some(tw) = trace.as_mut() {
                let p = f.joint.body_from_model.t;
                let _ = writeln!(
                    tw,
                    "{frame_ns},{slot},{:.4},{:.4},{:.4},{},{}",
                    p[0],
                    p[1],
                    p[2],
                    f.joint.total(),
                    f.model
                );
            }
            let body_from_aim = f.joint.body_from_model.compose(&device_from_aim);
            let q = geom::quat_from_mat(&body_from_aim.r);
            w.set(
                slot,
                HandPose {
                    frame_ns,
                    publish_ns: shm::monotonic_ns(),
                    pos: body_from_aim.t.map(|v| v as f32),
                    quat: q.map(|v| v as f32),
                    rms_px: f.joint.rms_px as f32,
                    leds: f.joint.total() as u32,
                    cams: f.joint.cams() as u32,
                    aim_from_imu: aim_from_imu[slot],
                },
            );
        }
        solve_ms += t0.elapsed().as_secs_f64() * 1e3;
        if t_stats.elapsed().as_secs_f64() >= stats_every {
            let s = tracker.stats;
            let n = s.led_frames.max(1) as f64;
            eprintln!(
                "live: {} LED frames, left {:.0} %, right {:.0} %, prior tracks {}/{}, few-LED {}/{}, searches {}, body-model rejects {}, collapsed {}, frame work {:.1} ms (mean)",
                s.led_frames,
                100.0 * s.hand[0] as f64 / n,
                100.0 * s.hand[1] as f64 / n,
                s.hand[0] + s.hand[1],
                s.tries,
                s.few[0],
                s.few[1],
                s.searches,
                s.unassigned,
                s.collapsed,
                solve_ms / n
            );
            for slot in 0..2 {
                if let Some((_, pose, _)) = tracker.last(slot) {
                    let ba = pose.compose(&device_from_aim);
                    eprintln!(
                        "   {}: ({:+.3} {:+.3} {:+.3}) m",
                        ["left", "right"][slot],
                        ba.t[0],
                        ba.t[1],
                        ba.t[2]
                    );
                }
            }
            t_stats = Instant::now();
            tracker.stats = Default::default();
            solve_ms = 0.0;
        }
    }
}

/// `pimax-ctrltrack show [--out PATH] [--seconds N]`: print what Monado would read.
pub fn show(a: &[String]) {
    let path = arg(a, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(shm::default_path);
    let secs: f64 = arg(a, "--seconds")
        .map(|v| v.parse().unwrap())
        .unwrap_or(10.0);
    let r = match shm::PoseReader::open(&path) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("{}: {e}", path.display());
            std::process::exit(1);
        }
    };
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs {
        if let Some((h, b)) = r.read_all() {
            let now = shm::monotonic_ns();
            let mut line = match b.age_ns(now) {
                Some(a) => format!(
                    "beat {:5.0} ms src {} reopens {}  ",
                    (a as f64 * 1e-6).min(99999.0),
                    if b.source_up { "up" } else { "DOWN" },
                    b.reopens
                ),
                None => String::new(),
            };
            for (i, p) in h.iter().enumerate() {
                let age = if p.frame_ns == 0 {
                    f64::INFINITY
                } else {
                    (now.saturating_sub(p.frame_ns)) as f64 * 1e-6
                };
                let fwd = geom::mat_vec(&quat_to_mat(p.quat.map(|v| v as f64)), [0.0, 0.0, -1.0]);
                line += &format!(
                    "{} age {:6.0} ms pos ({:+.3} {:+.3} {:+.3}) aim fwd ({:+.2} {:+.2} {:+.2}) LEDs {:2} cams {} rms {:.2}   ",
                    ["L", "R"][i],
                    age.min(99999.0),
                    p.pos[0],
                    p.pos[1],
                    p.pos[2],
                    fwd[0],
                    fwd[1],
                    fwd[2],
                    p.leds,
                    p.cams,
                    p.rms_px
                );
            }
            println!("{line}");
        }
        std::thread::sleep(std::time::Duration::from_millis(200));
    }
}

/// `pimax-ctrltrack health [--out POSEFILE] [--max-age S]`: the writer's heartbeat as one JSON line, for pimaxd.
/// Exit 0 = fresh (younger than `--max-age`, default 2 s) with the source up; 1 = no file or no heartbeat (older
/// writer); 2 = stale (writer hung or dead) or source down (broker / camera reopening).
pub fn health(a: &[String]) {
    let path = arg(a, "--out")
        .map(PathBuf::from)
        .unwrap_or_else(shm::default_path);
    let max_age: f64 = arg(a, "--max-age")
        .and_then(|v| v.parse().ok())
        .unwrap_or(2.0);
    let snap = shm::PoseReader::open(&path).ok().and_then(|r| r.read_all());
    let Some((_, b)) = snap else {
        println!("{{\"ok\":false,\"why\":\"no pose file\"}}");
        std::process::exit(1);
    };
    let pid = std::fs::read(&path)
        .ok()
        .filter(|d| d.len() >= 16)
        .map_or(0, |d| u32::from_le_bytes([d[12], d[13], d[14], d[15]]));
    let Some(age) = b.age_ns(shm::monotonic_ns()) else {
        println!("{{\"ok\":false,\"why\":\"no heartbeat (older writer)\",\"pid\":{pid}}}");
        std::process::exit(1);
    };
    let age_s = age as f64 * 1e-9;
    let fresh = age_s <= max_age;
    let ok = fresh && b.source_up;
    let why = if !fresh {
        "stale heartbeat"
    } else if !b.source_up {
        "source reopening"
    } else {
        "ok"
    };
    println!(
        "{{\"ok\":{ok},\"why\":\"{why}\",\"pid\":{pid},\"age_s\":{age_s:.2},\"source_up\":{},\"led_frames\":{},\"reopens\":{}}}",
        b.source_up, b.led_frames, b.reopens
    );
    if !ok {
        std::process::exit(2);
    }
}
