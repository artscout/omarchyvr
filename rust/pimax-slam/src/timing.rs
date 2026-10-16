//! Camera timing from a `.pxcam` recording (frame metadata only): frame kinds and alternation, device-clock frame
//! intervals, USB/driver delivery latency jitter (host arrival minus device stamp), exposure and gain per kind.

use pimax_cam::camera::{Camera, FrameInfo};
use pimax_cam::source::ReplayOptions;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Per-frame metadata of a recording (up to `max` frames, 0 = all).
pub fn read_infos(path: &str, max: usize) -> Result<Vec<FrameInfo>, String> {
    let mut cam =
        Camera::open(path, 0, ReplayOptions::default()).map_err(|e| format!("open {path}: {e}"))?;
    let mut v = Vec::new();
    loop {
        match cam.next_frame(100).map_err(|e| e.to_string())? {
            Some(i) => v.push(i),
            None if cam.source().finished() => break,
            None => {}
        }
        if max > 0 && v.len() >= max {
            break;
        }
    }
    Ok(v)
}

fn pct(v: &mut [f64], p: f64) -> f64 {
    if v.is_empty() {
        return f64::NAN;
    }
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[((v.len() - 1) as f64 * p).round() as usize]
}

fn kind_name(k: u32) -> &'static str {
    match k {
        1 => "tracking",
        2 => "led",
        _ => "unknown",
    }
}

/// Text report.
pub fn report(infos: &[FrameInfo]) -> String {
    let mut s = String::new();
    if infos.len() < 3 {
        return "fewer than 3 frames".into();
    }
    let mut kinds: BTreeMap<u32, usize> = BTreeMap::new();
    for i in infos {
        *kinds.entry(i.kind).or_default() += 1;
    }
    let span = (infos[infos.len() - 1].dev_ts_us - infos[0].dev_ts_us) as f64 / 1e6;
    let _ = writeln!(
        s,
        "{} frames over {:.2} s (device clock): {}",
        infos.len(),
        span,
        kinds
            .iter()
            .map(|(k, n)| format!("{} {} ({:.2}/s)", kind_name(*k), n, *n as f64 / span))
            .collect::<Vec<_>>()
            .join(", ")
    );
    // Kind sequence pattern (first 20).
    let pat: String = infos
        .iter()
        .take(20)
        .map(|i| match i.kind {
            1 => 'T',
            2 => 'L',
            _ => '?',
        })
        .collect();
    let repeats = infos.windows(2).filter(|w| w[0].kind == w[1].kind).count();
    let _ = writeln!(
        s,
        "kind sequence (first 20): {pat}; same-kind neighbours {repeats}"
    );
    let mut dt: Vec<f64> = infos
        .windows(2)
        .map(|w| (w[1].dev_ts_us as f64 - w[0].dev_ts_us as f64) / 1e3)
        .collect();
    let lost: u64 = infos.iter().map(|i| i.dropped as u64).sum();
    let _ = writeln!(
        s,
        "frame interval (device us clock): median {:.3} ms, p5 {:.3}, p95 {:.3}, max {:.3}; frames lost {lost}",
        pct(&mut dt, 0.5),
        pct(&mut dt, 0.05),
        pct(&mut dt, 0.95),
        pct(&mut dt, 1.0)
    );
    let tr: Vec<&FrameInfo> = infos.iter().filter(|i| i.kind == 1).collect();
    if tr.len() > 2 {
        let mut d: Vec<f64> = tr
            .windows(2)
            .map(|w| (w[1].dev_ts_us as f64 - w[0].dev_ts_us as f64) / 1e3)
            .collect();
        let _ = writeln!(
            s,
            "tracking frames: interval median {:.3} ms ({:.2} Hz), max {:.1} ms",
            pct(&mut d, 0.5),
            1e3 / pct(&mut d, 0.5),
            pct(&mut d, 1.0)
        );
    }
    // host arrival - device stamp: constant offset + delivery latency.
    let off: Vec<f64> = infos
        .iter()
        .map(|i| i.host_ts_ns as f64 / 1e6 - i.dev_ts_us as f64 / 1e3)
        .collect();
    let min = off.iter().cloned().fold(f64::INFINITY, f64::min);
    let mut lat: Vec<f64> = off.iter().map(|o| o - min).collect();
    let n = lat.len() as f64;
    let first = &lat[..lat.len() / 4];
    let last = &lat[lat.len() * 3 / 4..];
    let drift = (last.iter().sum::<f64>() / last.len() as f64
        - first.iter().sum::<f64>() / first.len() as f64)
        / (span * 0.75).max(1e-3);
    let _ = writeln!(
        s,
        "delivery jitter above the least-delayed frame: median {:.2} ms, p95 {:.2}, max {:.2} (n {n}); clock drift host vs device ~{:.1} us/s",
        pct(&mut lat, 0.5),
        pct(&mut lat, 0.95),
        pct(&mut lat, 1.0),
        drift * 1e3
    );
    for k in [1u32, 2] {
        let mut exp: BTreeMap<(u16, u16), usize> = BTreeMap::new();
        for i in infos.iter().filter(|i| i.kind == k) {
            *exp.entry((i.exposure[0], i.gain[0])).or_default() += 1;
        }
        if exp.is_empty() {
            continue;
        }
        let mut e: Vec<_> = exp.into_iter().collect();
        e.sort_by_key(|x| std::cmp::Reverse(x.1));
        let _ = writeln!(
            s,
            "{} exposure/gain (tile 0, raw units): {}{}",
            kind_name(k),
            e.iter()
                .take(5)
                .map(|((x, g), n)| format!("{x}/{g} x{n}"))
                .collect::<Vec<_>>()
                .join(", "),
            if e.len() > 5 {
                format!(" (+{} more)", e.len() - 5)
            } else {
                String::new()
            }
        );
    }
    let per_tile: Vec<String> = (0..4)
        .map(|t| {
            let v: Vec<u16> = tr.iter().map(|i| i.exposure[t]).collect();
            let (lo, hi) = (
                v.iter().min().copied().unwrap_or(0),
                v.iter().max().copied().unwrap_or(0),
            );
            format!("tile {t} {lo}..{hi}")
        })
        .collect();
    let _ = writeln!(s, "tracking exposure range: {}", per_tile.join(", "));
    s
}
