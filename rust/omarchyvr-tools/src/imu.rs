//! `omarchyvr imu`: Mahony orientation from IF2, read-only (sends nothing), port of `linux/pimax_imu_orient.py`.
//! Options: `--json`, `--rate HZ`, `--no-calib`, `--calib FILE`, `--auto-bias S`, `--still N`, `--seconds T`,
//! `--kp`, `--ki`, `--axis-mode 0|1` (default 1: X and Z negated, what the Monado driver uses).
use crate::args::Args;
use crate::orient::{axis_map, correct, parse_calib, Calib, Mahony, Sample, Unwrap};
use crate::{CliError, CliResult};
use pimax::{discovery, hid, sys};
use std::fs::File;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// Locate `headset-calibration.xml`: `--calib`, `$PIMAX_CALIB`, `./analysis/`, `~/.config/omarchyvr/`, `<repo>/analysis/`.
pub fn find_calib_file(explicit: Option<String>) -> Option<PathBuf> {
    let mut c: Vec<PathBuf> = vec![];
    if let Some(e) = explicit {
        c.push(e.into());
    }
    if let Some(e) = std::env::var_os("PIMAX_CALIB") {
        c.push(e.into());
    }
    c.push("analysis/headset-calibration.xml".into());
    c.push("../analysis/headset-calibration.xml".into());
    if let Some(h) = std::env::var_os("HOME") {
        c.extend(omarchyvr_config::paths::headset_calibration_candidates(std::path::Path::new(&h)));
    }
    c.into_iter().find(|p| p.is_file())
}

fn load_cal(explicit: Option<String>) -> Option<Calib> {
    parse_calib(&std::fs::read_to_string(find_calib_file(explicit)?).ok()?)
}

/// Blocking sample source over one hidraw file.
struct Source {
    f: File,
    unwrap: Unwrap,
    buf: std::collections::VecDeque<Sample>,
    stop: &'static std::sync::atomic::AtomicBool,
}

impl Source {
    fn next(&mut self) -> Option<Sample> {
        loop {
            if let Some(s) = self.buf.pop_front() {
                return Some(s);
            }
            if self.stop.load(std::sync::atomic::Ordering::Relaxed) {
                return None;
            }
            match sys::wait_readable(&[&self.f], Some(Duration::from_secs(1))) {
                Ok(r) if r[0] => {}
                Ok(_) => continue,
                Err(_) => return None,
            }
            let mut d = [0u8; 64];
            loop {
                match hid::read_report(&self.f, &mut d) {
                    Ok(Some(n)) if n >= 28 => {
                        if let Some(s) = self.unwrap.push(&d) {
                            self.buf.extend(s);
                        }
                    }
                    Ok(Some(_)) => {}
                    Ok(None) => break,
                    Err(_) => return None,
                }
            }
        }
    }
}

fn mean(v: &[[f64; 3]], i: usize) -> f64 {
    v.iter().map(|r| r[i]).sum::<f64>() / v.len() as f64
}

fn std(v: &[[f64; 3]], i: usize) -> f64 {
    let m = mean(v, i);
    (v.iter().map(|r| (r[i] - m).powi(2)).sum::<f64>() / v.len() as f64).sqrt()
}

fn still(src: &mut Source, secs: f64, cal: Option<&Calib>) {
    let (mut acc, mut gyr) = (vec![], vec![]);
    let mut t0 = None;
    while let Some(s) = src.next() {
        let t0v = *t0.get_or_insert(s.t_us);
        if (s.t_us - t0v) as f64 > secs * 1e6 {
            break;
        }
        acc.push(s.acc);
        gyr.push(s.gyro);
    }
    let n = acc.len();
    if n < 4 {
        println!("{n} samples: not enough data");
        return;
    }
    let h = n / 2;
    let nm = ["x", "y", "z"];
    println!("{n} samples over {secs} s");
    let fmt3 = |f: &dyn Fn(usize) -> String| {
        (0..3)
            .map(|i| format!("{}={}", nm[i], f(i)))
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mall: Vec<f64> = (0..3).map(|i| mean(&gyr, i)).collect();
    let m1: Vec<f64> = (0..3).map(|i| mean(&gyr[..h], i)).collect();
    let m2: Vec<f64> = (0..3).map(|i| mean(&gyr[h..], i)).collect();
    println!(
        "gyro raw mean (dps) : {}",
        fmt3(&|i| format!("{:+.4}", mall[i]))
    );
    println!(
        "gyro noise 1-sigma  : {} dps/sample (1 kHz)",
        fmt3(&|i| format!("{:.4}", std(&gyr, i)))
    );
    println!(
        "ARW estimate        : {} dps/sqrt(s) (std/sqrt(fs), i.e. deg/sqrt(s))",
        fmt3(&|i| format!("{:.4}", std(&gyr, i) / 1000f64.sqrt()))
    );
    if let Some(c) = cal {
        let wb: Vec<f64> = (0..3)
            .map(|i| c.w_bias[i] / (std::f64::consts::PI / 180.0))
            .collect();
        println!(
            "calib wBias (dps)   : {}",
            fmt3(&|i| format!("{:+.4}", wb[i]))
        );
        println!(
            "drift/min with calib bias removed (deg): {}",
            fmt3(&|i| format!("{:+.2}", (mall[i] - wb[i]) * 60.0))
        );
    }
    println!(
        "drift/min no bias removal (deg)        : {}",
        fmt3(&|i| format!("{:+.2}", mall[i] * 60.0))
    );
    println!(
        "drift/min, bias from 1st half, applied to 2nd half (deg): {}",
        fmt3(&|i| format!("{:+.3}", (m2[i] - m1[i]) * 60.0))
    );
    let am: Vec<f64> = (0..3).map(|i| mean(&acc, i)).collect();
    println!(
        "accel mean (g)      : {} |a|={:.4}",
        fmt3(&|i| format!("{:+.4}", am[i])),
        am.iter().map(|x| x * x).sum::<f64>().sqrt()
    );
    let ac = if cal.is_some() {
        correct([am[0], am[1], am[2]], [mall[0], mall[1], mall[2]], cal).0
    } else {
        [am[0], am[1], am[2]]
    };
    for (lab, v) in [("raw", [am[0], am[1], am[2]]), ("calib", ac)] {
        let pitch = v[2].atan2(v[0].hypot(v[1])).to_degrees();
        let roll = v[0].atan2(v[1]).to_degrees();
        println!("static tilt from accel ({lab}): pitch(nose-up, -Z fwd)={:+.2} deg  roll(+X side up)={roll:+.2} deg", -pitch);
    }
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let json = a.flag("--json");
    let rate: f64 = a.opt_parse("--rate")?.unwrap_or(10.0);
    let no_calib = a.flag("--no-calib");
    let auto_bias: f64 = a.opt_parse("--auto-bias")?.unwrap_or(0.0);
    let still_s: f64 = a.opt_parse("--still")?.unwrap_or(0.0);
    let seconds: f64 = a.opt_parse("--seconds")?.unwrap_or(0.0);
    let kp: f64 = a.opt_parse("--kp")?.unwrap_or(1.0);
    let ki: f64 = a.opt_parse("--ki")?.unwrap_or(0.0);
    let mode: i32 = a.opt_parse("--axis-mode")?.unwrap_or(1);
    let calib_path = a.opt("--calib")?;
    a.no_unknown_flags()?;
    let cal = if no_calib { None } else { load_cal(calib_path) };
    let stop = sys::install_term_flag();
    let if2 = discovery::find_interfaces(&discovery::Roots::from_env())[2]
        .clone()
        .ok_or_else(|| CliError::new("IF2 hidraw not found"))?;
    let mut src = Source {
        f: hid::open(&if2, false)?,
        unwrap: Unwrap::default(),
        buf: Default::default(),
        stop,
    };
    if still_s > 0.0 {
        still(&mut src, still_s, cal.as_ref());
        return Ok(());
    }
    let mut bias = [0.0; 3];
    if auto_bias > 0.0 {
        let (mut acc, mut t0) = (vec![], None);
        while let Some(s) = src.next() {
            let t0v = *t0.get_or_insert(s.t_us);
            if (s.t_us - t0v) as f64 > auto_bias * 1e6 {
                break;
            }
            acc.push(correct(s.acc, s.gyro, cal.as_ref()).1);
        }
        if acc.is_empty() {
            return Err(CliError::new("no IMU samples"));
        }
        for (i, b) in bias.iter_mut().enumerate() {
            *b = acc.iter().map(|x| x[i]).sum::<f64>() / acc.len() as f64;
        }
        eprintln!(
            "auto gyro bias (dps, after calib): {:?}",
            bias.iter()
                .map(|b| (b * 1e4).round() / 1e4)
                .collect::<Vec<_>>()
        );
    }
    let mut f = Mahony::new(kp, ki);
    let (mut prev, mut nxt, mut init) = (0u64, 0u64, false);
    let wall0 = Instant::now();
    while let Some(s) = src.next() {
        let (ac, g) = correct(s.acc, s.gyro, cal.as_ref());
        let g = axis_map(mode, [g[0] - bias[0], g[1] - bias[1], g[2] - bias[2]]);
        let ac = axis_map(mode, ac);
        if !init {
            f.seed(ac);
            init = true;
            prev = s.t_us;
            continue;
        }
        let dt = (s.t_us as i64 - prev as i64) as f64 * 1e-6;
        prev = s.t_us;
        if !(0.0 < dt && dt < 0.05) {
            continue;
        }
        f.update(g, ac, dt);
        if s.t_us >= nxt {
            nxt = s.t_us + (1e6 / rate) as u64;
            if json {
                let q: Vec<String> =
                    f.q.iter()
                        .map(|x| format!("{:.6}", (x * 1e6).round() / 1e6))
                        .collect();
                println!("{{\"t_us\": {}, \"q\": [{}]}}", s.t_us, q.join(", "));
            } else {
                let (y, p, r) = f.euler();
                println!(
                    "t={:9.3}s yaw={y:+8.2} pitch={p:+7.2} roll={r:+7.2} deg",
                    s.t_us as f64 * 1e-6
                );
            }
        }
        if seconds > 0.0 && wall0.elapsed().as_secs_f64() > seconds {
            break;
        }
    }
    Ok(())
}
