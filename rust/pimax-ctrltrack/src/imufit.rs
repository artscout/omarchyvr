//! Offline controller gyro calibration from a `PIMAX_CTRL_IMU_LOG` log written by the Monado driver (pimax-hands
//! fuse.rs): replays the gyro samples and optical fixes through the same fit as the online one, then checks how well
//! the fitted gyro predicts the optical rotation over 0.25 / 0.5 / 1 s (what out-of-view tracking relies on).

use pimax_hands::fuse::{self, qangle, qexp, qmul, qnorm, Calib, Config, Fuser, FIT_OFFSETS_MS, P};
use std::path::Path;

pub struct Log {
    pub hand: usize,
    /// Lines skipped as unreadable (a take's log slice can start or end inside a line; a log copied while it was
    /// written can hold NUL bytes).
    pub bad_lines: usize,
    /// (host ns, device µs, raw gyro)
    pub imu: Vec<(u64, u32, [i16; 3])>,
    /// (frame ns, world_from_aim, world_from_head, rms px, leds)
    pub fixes: Vec<(u64, P, P, f32, u32)>,
}

pub fn parse(text: &str) -> Result<Log, String> {
    let mut hand = 0;
    let mut imu = Vec::new();
    let mut fixes = Vec::new();
    let mut bad_lines = 0usize;
    for (ln, l) in text.lines().enumerate() {
        if let Some(h) = l.strip_prefix("# pimax-ctrlfuse log v1 hand=") {
            hand = if h.trim() == "right" { 1 } else { 0 };
            continue;
        }
        if l.starts_with('#') || l.trim().is_empty() {
            continue;
        }
        let f: Vec<&str> = l.split(',').collect();
        let _ = ln;
        let bad = || ();
        let r: Result<(), ()> = (|| {
        match f[0] {
            "I" if f.len() >= 6 => {
                let n = |k: usize| f[k].trim().parse::<i64>().map_err(|_| bad());
                imu.push((
                    n(1)? as u64,
                    n(2)? as u32,
                    [n(3)? as i16, n(4)? as i16, n(5)? as i16],
                ));
            }
            "O" if f.len() >= 18 => {
                let x = |k: usize| f[k].trim().parse::<f64>().map_err(|_| bad());
                let t = f[1].trim().parse::<u64>().map_err(|_| bad())?;
                let w = P {
                    p: [x(2)?, x(3)?, x(4)?],
                    q: qnorm([x(5)?, x(6)?, x(7)?, x(8)?]),
                };
                let h = P {
                    p: [x(9)?, x(10)?, x(11)?],
                    q: qnorm([x(12)?, x(13)?, x(14)?, x(15)?]),
                };
                fixes.push((t, w, h, x(16)? as f32, x(17)? as u32));
            }
            "P" | "I" | "O" => {} // short or other lines (shown poses: motion-model data)
            _ => return Err(()),
        }
        Ok(())
        })();
        if r.is_err() {
            bad_lines += 1;
        }
    }
    if imu.is_empty() && fixes.is_empty() && bad_lines > 0 {
        return Err(format!("no readable line ({bad_lines} unreadable)"));
    }
    Ok(Log { hand, bad_lines, imu, fixes })
}

pub struct FitReport {
    pub calib: Option<Calib>,
    /// (offset ms, pairs, moving, r2)
    pub per_offset: Vec<(f64, u32, u32, f64)>,
    /// (horizon s, pairs, median error with the gyro deg, median error holding the orientation deg)
    pub eval: Vec<(f64, usize, f64, f64)>,
    pub total_pairs: u32,
}

/// Replay the log through a fresh fuser (no calibration, online fit on) and evaluate the result.
pub fn fit(log: &Log) -> FitReport {
    let fu = replay_fit(log);
    let per_offset = (0..FIT_OFFSETS_MS.len())
        .map(|k| match fu.fit.robust(k) {
            Some((_, _, r2, kept, moving)) => (FIT_OFFSETS_MS[k], kept, moving, r2),
            None => (FIT_OFFSETS_MS[k], 0, 0, f64::NAN),
        })
        .collect();
    let calib = fu.fit.best(100, 30, 0.8);
    let eval = calib.map(|c| evaluate(log, &c)).unwrap_or_default();
    FitReport {
        total_pairs: fu.fit.pairs(),
        calib,
        per_offset,
        eval,
    }
}

/// One fit over several logs of the same hand (their fix pairs pooled), the time offset refined at 4 ms over all of
/// them. Returns the calibration and, per log, the 1 s gyro prediction error with it (median deg, with hold).
pub fn fit_pooled(logs: &[Log]) -> Option<(Calib, Vec<(f64, f64)>)> {
    let mut pool = fuse::Fit::new();
    for l in logs {
        for (xs, y) in replay_fit(l).fit.pairs {
            pool.add(xs, y);
        }
    }
    let mut c = pool.best(200, 60, 0.8)?;
    let mut best = (f64::INFINITY, c.time_offset_ms);
    for k in -6..=6 {
        let mut cc = c;
        cc.time_offset_ms = c.time_offset_ms + 4.0 * k as f64;
        let (mut sum, mut n) = (0.0, 0.0);
        for l in logs {
            if let Some(&(_, m, err, _)) = evaluate(l, &cc).first() {
                if err.is_finite() {
                    sum += err * m as f64;
                    n += m as f64;
                }
            }
        }
        if n > 0.0 && sum / n < best.0 {
            best = (sum / n, cc.time_offset_ms);
        }
    }
    c.time_offset_ms = best.1;
    let per = logs
        .iter()
        .map(|l| {
            evaluate(l, &c)
                .iter()
                .find(|e| e.0 == 1.0)
                .map(|e| (e.2, e.3))
                .unwrap_or((f64::NAN, f64::NAN))
        })
        .collect();
    Some((c, per))
}

/// The log replayed through a fresh fuser (gyro off, no calibration): its fit holds every fix pair.
fn replay_fit(log: &Log) -> Fuser {
    let cfg = Config {
        use_imu: Some(false),
        ..Config::default()
    };
    let mut fu = Fuser::new(log.hand, cfg, None);
    let (mut i, mut j) = (0, 0);
    while i < log.imu.len() || j < log.fixes.len() {
        let take_imu =
            j >= log.fixes.len() || (i < log.imu.len() && log.imu[i].0 <= log.fixes[j].0);
        if take_imu {
            let (t, d, g) = log.imu[i];
            fu.sample(t, d, g);
            i += 1;
        } else {
            let (t, w, h, r, n) = log.fixes[j];
            fu.optical(t, w, &h, r, n);
            j += 1;
        }
    }
    fu
}

/// Gyro prediction of the optical rotation over fixed horizons.
pub fn evaluate(log: &Log, c: &Calib) -> Vec<(f64, usize, f64, f64)> {
    // integrated orientation at every IMU sample (device dt)
    let mut q = [0.0, 0.0, 0.0, 1.0];
    let mut qs: Vec<(u64, [f64; 4])> = Vec::with_capacity(log.imu.len());
    let mut last: Option<u32> = None;
    for &(t, d, g) in &log.imu {
        let dt = last
            .map(|l| d.wrapping_sub(l) as f64 * 1e-6)
            .unwrap_or(0.0)
            .clamp(0.0, 0.02);
        last = Some(d);
        let w = c.omega([g[0] as f64, g[1] as f64, g[2] as f64]);
        q = qnorm(qmul(q, qexp([w[0] * dt, w[1] * dt, w[2] * dt])));
        qs.push((t, q));
    }
    let at = |t: u64| -> Option<[f64; 4]> {
        let i = qs.partition_point(|(t0, _)| *t0 <= t);
        if i == 0 || i >= qs.len() {
            return None;
        }
        Some(qs[i - 1].1)
    };
    let off = (c.time_offset_ms * 1e6) as i64;
    let mut out = Vec::new();
    for h in [0.25, 0.5, 1.0] {
        let mut e_g = Vec::new();
        let mut e_h = Vec::new();
        let hn = (h * 1e9) as u64;
        let mut k = 0;
        for a in 0..log.fixes.len() {
            let (ta, wa, ..) = log.fixes[a];
            while k < log.fixes.len() && log.fixes[k].0 < ta + hn {
                k += 1;
            }
            if k >= log.fixes.len() {
                break;
            }
            let (tb, wb, ..) = log.fixes[k];
            if tb - ta > hn + 30_000_000 || log.fixes[a].4 < 6 || log.fixes[k].4 < 6 {
                continue;
            }
            let (Some(qa), Some(qb)) = (at((ta as i64 + off) as u64), at((tb as i64 + off) as u64))
            else {
                continue;
            };
            let pred = qnorm(qmul(wa.q, qmul(fuse::qconj(qa), qb)));
            e_g.push(qangle(pred, wb.q).to_degrees());
            e_h.push(qangle(wa.q, wb.q).to_degrees());
        }
        let med = |v: &mut Vec<f64>| {
            if v.is_empty() {
                return f64::NAN;
            }
            v.sort_by(|a, b| a.partial_cmp(b).unwrap());
            v[v.len() / 2]
        };
        let n = e_g.len();
        out.push((h, n, med(&mut e_g), med(&mut e_h)));
    }
    out
}

/// Camera-to-IMU time offset at 4 ms resolution (the fit's grid is 16 ms): the offset whose gyro best predicts the
/// optical rotation over 0.25 s. Returns (best offset ms, a one-line table).
pub fn fine_offset(log: &Log, c: &Calib) -> (f64, String) {
    let mut best = (f64::INFINITY, c.time_offset_ms);
    let mut line = Vec::new();
    for k in -6..=6 {
        let o = c.time_offset_ms + 4.0 * k as f64;
        let mut cc = *c;
        cc.time_offset_ms = o;
        let e = evaluate(log, &cc);
        let Some(&(_, _, err, _)) = e.first() else { continue };
        if k % 2 == 0 {
            line.push(format!("{o:+.0}: {err:.2}"));
        }
        if err < best.0 {
            best = (err, o);
        }
    }
    (best.1, line.join(", "))
}

fn fu_pairs(r: &FitReport) -> u32 {
    r.total_pairs
}

/// `imufit --pool LOG...`: one calibration per hand over all logs, printed as JSON lines (the format of
/// `~/.config/omarchyvr/ctrl-imu.json`) and as the built-in default (`fuse::default_calib`).
pub fn run_pooled(paths: &[&Path], save: Option<&Path>) -> Result<(), String> {
    let mut by_hand: [Vec<Log>; 2] = [Vec::new(), Vec::new()];
    let mut names: [Vec<String>; 2] = [Vec::new(), Vec::new()];
    for p in paths {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let l = parse(&text)?;
        names[l.hand].push(p.display().to_string());
        by_hand[l.hand].push(l);
    }
    for h in 0..2 {
        if by_hand[h].is_empty() {
            continue;
        }
        let hand = ["left", "right"][h];
        let Some((c, per)) = fit_pooled(&by_hand[h]) else {
            println!("{hand}: no usable pooled fit");
            continue;
        };
        println!(
            "{hand} (pooled over {} logs): {:.4} deg/s per LSB, axes {}, misalignment {:.1} deg, bias {:.1} {:.1} {:.1} LSB, offset {:+} ms, {} pairs ({} moving), r2 {:.3}",
            by_hand[h].len(),
            c.scale_dps_per_lsb(),
            c.axes(),
            c.axis_misalignment_deg(),
            c.bias[0],
            c.bias[1],
            c.bias[2],
            c.time_offset_ms,
            c.pairs,
            c.moving,
            c.r2
        );
        for (n, (g, hold)) in names[h].iter().zip(&per) {
            println!("  {n}: orientation after 1 s from the gyro alone {g:.1} deg (holding the last pose {hold:.1} deg)");
        }
        let fl: Vec<String> = c.c.iter().flatten().map(|v| format!("{v:e}")).collect();
        println!("  c = [{}]", fl.join(", "));
        if let Some(p) = save {
            fuse::save_calib(p, h, &c).map_err(|e| format!("{}: {e}", p.display()))?;
            println!("  saved the {hand} calibration to {}", p.display());
        }
    }
    Ok(())
}

pub fn run(path: &Path, save: Option<&Path>) -> Result<(), String> {
    let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let log = parse(&text)?;
    let hand = ["left", "right"][log.hand];
    println!(
        "{}: {hand} controller, {} gyro samples, {} optical fixes{}",
        path.display(),
        log.imu.len(),
        log.fixes.len(),
        if log.bad_lines > 0 { format!(" ({} unreadable lines skipped)", log.bad_lines) } else { String::new() }
    );
    let r = fit(&log);
    println!("  {} pairs of fixes (9+ LEDs, ~120 ms apart); robust fit per camera-to-IMU time offset:", fu_pairs(&r));
    for (o, n, m, r2) in &r.per_offset {
        println!("  time offset {o:+5.0} ms: {n} pairs kept ({m} moving), r2 {r2:.3}");
    }
    let Some(c) = r.calib else {
        return Err("no usable fit: rotate the controllers in view of the cameras (see analysis/controller-6dof.md)".into());
    };
    println!(
        "fit: {:.4} deg/s per LSB, axes (aim x y z from raw) {}, misalignment {:.1} deg, bias {:.1} {:.1} {:.1} LSB, offset {} ms, {} pairs ({} moving), r2 {:.3}",
        c.scale_dps_per_lsb(),
        c.axes(),
        c.axis_misalignment_deg(),
        c.bias[0],
        c.bias[1],
        c.bias[2],
        c.time_offset_ms,
        c.pairs,
        c.moving,
        c.r2
    );
    for (h, n, g, hold) in &r.eval {
        println!("  orientation after {h:.2} s from the gyro alone: median error {g:.1} deg (holding the last pose: {hold:.1} deg), {n} intervals");
    }
    let (best, line) = fine_offset(&log, &c);
    println!("  time offset, 4 ms steps (median 0.25 s gyro error): {line}; best {best:+.0} ms");
    if let Some(p) = save {
        fuse::save_calib(p, log.hand, &c).map_err(|e| format!("{}: {e}", p.display()))?;
        println!("saved the {hand} calibration to {}", p.display());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use pimax_hands::fuse::{qconj, qrot};

    /// A synthetic log written by the fuser's own logger, then fitted from the file.
    #[test]
    fn fit_from_a_logged_session() {
        let dir = std::env::temp_dir().join(format!("imufit-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("log-right.csv");
        let mut fu = Fuser::new(
            1,
            Config {
                online_fit: false,
                ..Config::default()
            },
            None,
        );
        fu.start_log(&path, 10.0).unwrap();
        let s = (1.0 / 16.4f64).to_radians();
        // true mapping aim = (-raw x, raw z, raw y) * s; bias (5, 0, -4)
        let raw_of = |w: [f64; 3]| [-w[0] / s + 5.0, w[2] / s + 0.0, w[1] / s - 4.0];
        let mut q = [0.0, 0.0, 0.0, 1.0];
        let t0 = 5_000_000_000u64;
        let dt = 0.002;
        for k in 0..(40.0 / dt) as usize {
            let t = k as f64 * dt;
            let w = [
                2.5 * (1.1 * t).sin(),
                2.0 * (0.9 * t + 0.5).cos(),
                1.5 * (2.1 * t).sin(),
            ];
            q = qnorm(qmul(q, qexp([w[0] * dt, w[1] * dt, w[2] * dt])));
            let host = t0 + (t * 1e9) as u64;
            let r = raw_of(w);
            fu.sample(
                host + 1_000_000,
                (t * 1e6) as u32,
                [
                    r[0].round() as i16,
                    r[1].round() as i16,
                    r[2].round() as i16,
                ],
            );
            if k % 20 == 0 {
                let head = P {
                    q: [0.0, 0.0, 0.0, 1.0],
                    p: [0.0; 3],
                };
                fu.optical(
                    host,
                    P {
                        q,
                        p: [0.1, 0.0, -0.4],
                    },
                    &head,
                    0.5,
                    9,
                );
            }
        }
        fu.flush_log();
        drop(fu);
        let log = parse(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(log.hand, 1);
        let r = fit(&log);
        let c = r.calib.expect("fit");
        assert_eq!(c.axes(), "-x +z +y");
        assert!((c.scale_dps_per_lsb() * 16.4 - 1.0).abs() < 0.03);
        let e = r.eval.iter().find(|e| e.0 == 1.0).unwrap();
        assert!(
            e.2 < 1.0 && e.3 > 20.0,
            "gyro {} deg vs hold {} deg",
            e.2,
            e.3
        );
        let _ = (qconj(q), qrot(q, [0.0; 3]));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
