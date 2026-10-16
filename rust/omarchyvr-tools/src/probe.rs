//! `omarchyvr probe [seconds]`: port of `linux/pimax_probe.py`. Wake + keepalive, identity reads (0x16, 0x03, 0x50),
//! then prints replies, firmware log lines and a one-line IMU summary per second.
//! `--passive` skips the wake/keepalive (use it while `omarchyvr keepalived` or the Python service runs).
use crate::args::Args;
use crate::{hex, CliError, CliResult};
use pimax::{discovery, Config, Event, Session};
use pimax_proto::cmd;
use std::time::{Duration, Instant};

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let passive = a.flag("--passive");
    a.no_unknown_flags()?;
    let secs: f64 = a.pos(0)?.unwrap_or(15.0);
    let roots = discovery::Roots::from_env();
    let ifs = discovery::find_interfaces(&roots);
    let map: Vec<String> = ifs
        .iter()
        .enumerate()
        .filter_map(|(i, p)| p.as_ref().map(|p| format!("{i}: '{}'", p.display())))
        .collect();
    println!(
        "headset hidraw interfaces: {{{}}} | usb devnum: {}",
        map.join(", "),
        discovery::usb_devnum().unwrap_or_else(|| "?".into())
    );
    if ifs[0].is_none() || ifs[2].is_none() {
        return Err(CliError::new(
            "headset IF0/IF2 not found - is 34a4:0044 passed through?",
        ));
    }
    let cfg = if passive {
        Config::passive()
    } else {
        Config::default()
    };
    let (s, rx) = Session::open_channel(cfg, 65536, true)?;
    s.wait_connected(Duration::from_secs(5))?;
    for (c, opt, d) in [
        (cmd::SERIAL, 0u8, &[0x1eu8][..]),
        (cmd::VERSION, 0, &[]),
        (cmd::INFO, 0, &[]),
    ] {
        std::thread::sleep(Duration::from_millis(50));
        s.send(c, opt, d)?;
    }
    let (mut imu_n, mut ctrl_n) = (0u64, 0u64);
    let (mut first_ts, mut last_ts, mut last) = (None, None, None);
    let t0 = Instant::now();
    let mut next_print = 1.0;
    while t0.elapsed().as_secs_f64() < secs {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(Event::Imu(i)) => {
                if i.sample_in_report == 0 {
                    imu_n += 1;
                    first_ts.get_or_insert(i.ts_us);
                    last_ts = Some(i.ts_us);
                    last = Some(i);
                }
            }
            Ok(Event::Controller(_)) => ctrl_n += 1,
            Ok(Event::Reply(r)) => println!("  reply  {}", hex(&r[..12])),
            Ok(Event::Log(l)) => println!("  fwlog  {}", l.trim()),
            Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
            Err(_) => break,
        }
        let el = t0.elapsed().as_secs_f64();
        if el >= next_print {
            next_print += 1.0;
            if let Some(i) = last {
                let g = (i
                    .raw_accel
                    .iter()
                    .map(|&x| f64::from(x).powi(2))
                    .sum::<f64>())
                .sqrt()
                    / 2048.0;
                let st = s.stats();
                println!(
                    "IMU {:6.1} rep/s | |a|={g:5.3} g a=({:6},{:6},{:6}) gyro dps=({:7.2},{:7.2},{:7.2}) | ipd {:5.2} mm | ctrl reports {ctrl_n} | keepalive ok={} err=0",
                    imu_n as f64 / el.max(1e-3), i.raw_accel[0], i.raw_accel[1], i.raw_accel[2], i.gyro_dps[0], i.gyro_dps[1], i.gyro_dps[2], i.ipd_mm, st.keepalives
                );
            }
        }
    }
    let span = match (first_ts, last_ts) {
        (Some(f), Some(l)) => f64::from(l.wrapping_sub(f)) / 1e6,
        _ => 0.0,
    };
    println!(
        "done: {imu_n} IMU reports, device clock span {span:.2} s, usb devnum now {}",
        discovery::usb_devnum().unwrap_or_else(|| "?".into())
    );
    Ok(())
}
