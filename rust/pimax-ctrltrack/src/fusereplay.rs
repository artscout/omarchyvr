//! Replay a `PIMAX_CTRL_IMU_LOG` log through the fuser offline: gyro samples and optical fixes go in, the display
//! requests of the session (the `P` lines' times and head poses) are asked again. Writes the log back with the `P`
//! lines replaced by the replayed shown poses, so before/after metrics run on the same input
//! (`analysis/tools/ctrl_motion.py`).

use pimax_hands::fuse::{calib_or_default, calib_path, qnorm, Config, Fuser, P};
use std::io::Write;
use std::path::Path;

fn pose(f: &[&str], k: usize) -> Option<P> {
    let v: Vec<f64> = f
        .get(k..k + 7)?
        .iter()
        .map(|x| x.trim().parse().unwrap_or(f64::NAN))
        .collect();
    Some(P {
        p: [v[0], v[1], v[2]],
        q: qnorm([v[3], v[4], v[5], v[6]]),
    })
}

/// `cfg_mod` adjusts the config (A/B). Returns (fixes accepted, shown samples written).
pub fn replay(
    input: &Path,
    output: &Path,
    cfg_mod: impl Fn(&mut Config),
) -> Result<(u64, u64), String> {
    let text = std::fs::read_to_string(input).map_err(|e| format!("{}: {e}", input.display()))?;
    let hand = if text
        .lines()
        .next()
        .is_some_and(|l| l.contains("hand=right"))
    {
        1
    } else {
        0
    };
    let mut cfg = Config::default();
    cfg_mod(&mut cfg);
    let mut fu = Fuser::new(hand, cfg, calib_or_default(&calib_path(), hand));
    let mut out =
        std::io::BufWriter::new(std::fs::File::create(output).map_err(|e| e.to_string())?);
    let mut shown = 0u64;
    for l in text.lines() {
        let f: Vec<&str> = l.split(',').collect();
        match f[0] {
            "I" if f.len() >= 6 => {
                let n = |k: usize| f[k].trim().parse::<i64>().unwrap_or(0);
                let acc = (f.len() >= 9).then(|| [n(6) as i16, n(7) as i16, n(8) as i16]);
                fu.sample_acc(
                    n(1) as u64,
                    n(2) as u32,
                    [n(3) as i16, n(4) as i16, n(5) as i16],
                    acc,
                );
                writeln!(out, "{l}").ok();
            }
            "O" if f.len() >= 18 => {
                let t: u64 = f[1].trim().parse().unwrap_or(0);
                if let (Some(w), Some(h)) = (pose(&f, 2), pose(&f, 9)) {
                    let rms: f32 = f[16].trim().parse().unwrap_or(1.0);
                    let leds: u32 = f[17].trim().parse().unwrap_or(0);
                    let v = fu.optical_with(t, w, &h, rms, leds, None);
                    // keep the original columns, replace the verdict
                    writeln!(out, "{},{v}", f[..18].join(",")).ok();
                }
            }
            "P" if f.len() >= 17 => {
                let t: u64 = f[1].trim().parse().unwrap_or(0);
                if let Some(h) = pose(&f, 10) {
                    let (s, p) = fu.get(t, &h);
                    writeln!(
                        out,
                        "P,{t},{},{:.5},{:.5},{:.5},{:.6},{:.6},{:.6},{:.6},{}",
                        s as u32,
                        p.p[0],
                        p.p[1],
                        p.p[2],
                        p.q[0],
                        p.q[1],
                        p.q[2],
                        p.q[3],
                        f[10..17].join(",")
                    )
                    .ok();
                    shown += 1;
                }
            }
            _ => {
                writeln!(out, "{l}").ok();
            }
        }
    }
    Ok((fu.stats.fixes, shown))
}
