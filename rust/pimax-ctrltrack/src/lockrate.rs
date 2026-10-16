//! `pimax-ctrltrack lockrate FILE.pxcam [--baseline] [--oracle] [--start N] [--count N] [--csv OUT]`: replay a
//! recording through the live tracker (as fast as possible, the same blob detection and per-frame logic as
//! `live`) and report the optical lock rate, few-LED share, gaps, frame-to-frame steps and jumps, and frame work.
//!
//! `--oracle` also runs the brute-force search on every frame and classifies each lost (frame, hand) where the
//! hand had a fix in the last second (analysis/ctrl-lock-rate.md):
//! * `oracle` — the search finds the controller near its last position: a tracker loss (prior / gate / LED count);
//! * `out-of-view` — under 3 facing LEDs of the last pose project into any camera image;
//! * `undetected` — LEDs should be in view but under 3 blobs lie within 25 px of them (occlusion, the LED facing
//!   away, blur or a dim LED below the blob threshold);
//! * `few-blobs` — 3+ blobs there, but no solve with the tracker (too few / ambiguous LEDs).
//!
//! Lost frames more than 1 s after the last fix count as `long-gap`.

use crate::track::{arg, solve_frame, LedFrame, Rig};
use crate::tracker::{visibility, TrackCfg, Tracker};
use pimax_cam::frame::{self, FrameKind, Meta, TILE_COUNT, TILE_H, TILE_W};
use pimax_cam::geom;
use pimax_cam::record::Reader;
use pimax_ctrltrack::blob::{self, BlobParams};
use std::collections::BTreeMap;
use std::io::Write;
use std::path::Path;
use std::time::Instant;

pub fn lockrate(a: &[String]) {
    let path = &a[0];
    let start: usize = arg(a, "--start").map(|v| v.parse().unwrap()).unwrap_or(0);
    let count: usize = arg(a, "--count")
        .map(|v| v.parse().unwrap())
        .unwrap_or(usize::MAX);
    let rig = Rig::load(a);
    let mut cfg = if a.iter().any(|s| s == "--baseline") {
        TrackCfg::baseline()
    } else {
        TrackCfg::default()
    };
    if let Some(v) = arg(a, "--few") {
        cfg.few_leds = v.parse().unwrap();
    }
    if let Some(v) = arg(a, "--wide") {
        cfg.wide_gate_px = v.parse().unwrap();
    }
    if a.iter().any(|s| s == "--no-predict") {
        cfg.predict = false;
    }
    if let Some(v) = arg(a, "--rot-w") {
        cfg.rot_w = v.parse().unwrap();
    }

    // pass 1: blobs (timed: part of the live frame work)
    let mut r = Reader::open(Path::new(path)).expect("open");
    let bp = if a.iter().any(|s| s == "--baseline") && arg(a, "--thr").is_none() {
        BlobParams::default()
    } else {
        BlobParams::from_args(a)
    };
    let mut tile = vec![0u8; TILE_W * TILE_H];
    let mut frames: Vec<(LedFrame, u64)> = Vec::new();
    let mut blob_ms = 0.0;
    let mut nled = 0usize;
    while let Some(rec) = r.next_record().expect("read") {
        let Some(m) = Meta::parse(&rec.data[..frame::META_LEN]) else {
            continue;
        };
        if m.kind() != FrameKind::Led {
            continue;
        }
        nled += 1;
        if nled <= start {
            continue;
        }
        if frames.len() >= count {
            break;
        }
        let t0 = Instant::now();
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
        blob_ms += t0.elapsed().as_secs_f64() * 1e3;
        frames.push((
            LedFrame {
                rec: nled,
                ts_us: m.ts_us,
                px,
            },
            rec.host_ts_ns,
        ));
    }
    let n = frames.len();
    eprintln!("{n} LED frames, config {cfg:?}");

    // pass 2: the tracker, frame by frame
    let mut tr = Tracker::new(cfg);
    let mut track_ms = Vec::with_capacity(n);
    // per frame: per slot Some((pos, leds, few, model, pose))
    type Fx = ([f64; 3], usize, bool, usize, geom::Pose);
    let mut fixes: Vec<[Option<Fx>; 2]> = Vec::with_capacity(n);
    for (lf, ns) in &frames {
        let t0 = Instant::now();
        let got = tr.step(&rig, lf, *ns);
        track_ms.push(t0.elapsed().as_secs_f64() * 1e3);
        let mut row = [None, None];
        for f in got {
            let j = &f.found.joint;
            row[f.slot] = Some((
                j.body_from_model.t,
                j.total(),
                f.few,
                f.found.model,
                j.body_from_model,
            ));
        }
        fixes.push(row);
    }
    let s = tr.stats;
    let mean_ms = (blob_ms + track_ms.iter().sum::<f64>()) / n.max(1) as f64;
    let mut sorted = track_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let p95 = sorted.get(n * 95 / 100).copied().unwrap_or(0.0);
    println!(
        "lock: left {:.1} %, right {:.1} % of {n} LED frames | few-LED fixes {} / {} | searches {} | frame work {:.2} ms mean (blobs {:.2}), tracker p95 {:.1} ms",
        100.0 * s.hand[0] as f64 / n as f64,
        100.0 * s.hand[1] as f64 / n as f64,
        s.few[0],
        s.few[1],
        s.searches,
        mean_ms,
        blob_ms / n.max(1) as f64,
        p95
    );
    // steps between consecutive-frame fixes, jumps, gaps
    for slot in 0..2 {
        let mut steps = Vec::new();
        let (mut jumps, mut gaps) = (0, BTreeMap::<&str, usize>::new());
        let mut run = 0usize;
        let mut seen = false;
        for k in 0..n {
            match fixes[k][slot] {
                Some((p, ..)) => {
                    if k > 0 {
                        if let Some((q, ..)) = fixes[k - 1][slot] {
                            let d = geom::norm(geom::sub(p, q));
                            steps.push(d);
                            if d > 0.08 {
                                jumps += 1;
                            }
                        }
                    }
                    if seen && run > 0 {
                        let key = match run {
                            1..=2 => "1-2",
                            3..=7 => "3-7",
                            8..=25 => "8-25",
                            _ => ">25",
                        };
                        *gaps.entry(key).or_default() += 1;
                    }
                    run = 0;
                    seen = true;
                }
                None => run += 1,
            }
        }
        steps.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let q = |f: f64| {
            steps
                .get((steps.len() as f64 * f) as usize)
                .copied()
                .unwrap_or(0.0)
                * 1e3
        };
        println!(
            "  {}: steps p50 {:.1} mm p95 {:.1} mm, jumps >8 cm {jumps}, gaps (LED frames) {:?}",
            ["left", "right"][slot],
            q(0.5),
            q(0.95),
            gaps
        );
    }

    if let Some(p) = arg(a, "--csv") {
        let mut w = std::io::BufWriter::new(std::fs::File::create(p).expect("csv"));
        writeln!(w, "rec,host_ns,slot,x,y,z,leds,few").unwrap();
        for (k, row) in fixes.iter().enumerate() {
            for (slot, f) in row.iter().enumerate() {
                if let Some((p, l, few, ..)) = f {
                    writeln!(
                        w,
                        "{},{},{slot},{:.4},{:.4},{:.4},{l},{}",
                        frames[k].0.rec, frames[k].1, p[0], p[1], p[2], *few as u8
                    )
                    .unwrap();
                }
            }
        }
    }

    if !a.iter().any(|s| s == "--oracle") {
        return;
    }
    // pass 3: brute force on every frame, in parallel
    let nthreads = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4);
    let next = std::sync::atomic::AtomicUsize::new(0);
    let oracle: std::sync::Mutex<Vec<Vec<[f64; 3]>>> = std::sync::Mutex::new(vec![Vec::new(); n]);
    std::thread::scope(|sc| {
        for _ in 0..nthreads {
            sc.spawn(|| loop {
                let k = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                if k >= n {
                    break;
                }
                let f = solve_frame(
                    &frames[k].0,
                    &rig.cams,
                    &rig.models,
                    &rig.triples,
                    &rig.prm,
                    None,
                    &[],
                    false,
                );
                oracle.lock().unwrap()[k] = f.iter().map(|x| x.joint.body_from_model.t).collect();
            });
        }
    });
    let oracle = oracle.into_inner().unwrap();
    let mut cls: [BTreeMap<&str, usize>; 2] = [BTreeMap::new(), BTreeMap::new()];
    for k in 0..n {
        for slot in 0..2 {
            let c = &mut cls[slot];
            if fixes[k][slot].is_some() {
                *c.entry("locked").or_default() += 1;
                continue;
            }
            // the slot's last fix
            let last = (0..k)
                .rev()
                .find_map(|j| fixes[j][slot].map(|f| (j, f.0, f.3, f.4)));
            let Some((j, lp, mi, pose)) = last else {
                *c.entry("never-seen").or_default() += 1;
                continue;
            };
            let age = (frames[k].1.saturating_sub(frames[j].1)) as f64 * 1e-9;
            if age > 1.0 {
                // blobs in the frame beyond what the other hand's fix explains
                let blobs: usize = frames[k].0.px.iter().map(|v| v.len()).sum();
                let other_leds = fixes[k][1 - slot].map(|f| f.1).unwrap_or(0);
                let key = if blobs.saturating_sub(other_leds) < 3 {
                    "long-gap-no-blobs"
                } else {
                    "long-gap-blobs"
                };
                *c.entry(key).or_default() += 1;
                continue;
            }
            let other = fixes[k][1 - slot].map(|f| f.0);
            let near = oracle[k].iter().any(|o| {
                geom::norm(geom::sub(*o, lp)) < 0.25
                    && other.is_none_or(|q| geom::norm(geom::sub(*o, q)) > 0.06)
            });
            if near {
                *c.entry("oracle").or_default() += 1;
                continue;
            }
            // geometry from the slot's last pose
            let (inview, nearb) = visibility(&rig, mi, &pose, &frames[k].0.px, 25.0);
            let key = if inview < 3 {
                "out-of-view"
            } else if nearb < 3 {
                "undetected"
            } else {
                "few-blobs"
            };
            *c.entry(key).or_default() += 1;
        }
    }
    for slot in 0..2 {
        let tot: usize = cls[slot].values().sum();
        let parts: Vec<String> = cls[slot]
            .iter()
            .map(|(k, v)| format!("{k} {:.1} %", 100.0 * *v as f64 / tot.max(1) as f64))
            .collect();
        println!("  {} frames: {}", ["left", "right"][slot], parts.join(", "));
    }
    let both = oracle.iter().filter(|o| o.len() >= 2).count();
    let any = oracle.iter().filter(|o| !o.is_empty()).count();
    println!(
        "  oracle (brute force per frame): any controller {:.1} %, both {:.1} %",
        100.0 * any as f64 / n as f64,
        100.0 * both as f64 / n as f64
    );
}
