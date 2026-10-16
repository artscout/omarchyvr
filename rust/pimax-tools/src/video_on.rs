//! `pimax video-on <fps> [seconds]`: port of `linux/pimax_video_on.py`. Switches the panel on: this is the one subcommand
//! that changes the headset state (5b, 29, 1a x2 at 150 ms spacing), then tails both logs. Needs `pimax keepalived`
//! (or the Python service) running; writes `<fps>` into `$XDG_RUNTIME_DIR/pimax-rate` so the keepalive carries it.
use crate::args::Args;
use crate::logtail::{open_tails, pump};
use crate::{hex, rate_file, CliError, CliResult};
use pimax::{Config, Event, Session};
use pimax_proto::{build_command, cmd};
use std::time::{Duration, Instant};

/// Entry point.
pub fn run(a: Args) -> CliResult {
    a.no_unknown_flags()?;
    let fps: u8 = a.pos(0)?.unwrap_or(72);
    let secs: f64 = a.pos(1)?.unwrap_or(40.0);
    let (s, rx) = Session::open_channel(Config::passive(), 65536, true)?;
    s.wait_connected(Duration::from_secs(5))
        .map_err(|_| CliError::new("headset not found"))?;
    let mut tails = open_tails();
    println!(
        "headset connected, log sources: {:?}",
        tails.iter().map(|t| t.name).collect::<Vec<_>>()
    );
    if let Err(e) = std::fs::write(rate_file(), fps.to_string()) {
        println!("could not write rate file: {e}");
    }
    let steps: [(String, u8, u8, Vec<u8>); 4] = [
        (
            "ept app_unlock".into(),
            cmd::APP_UNLOCK,
            0x05,
            vec![0x02, 0x02],
        ),
        ("panel-on request".into(), cmd::PANEL_ON, 0x00, vec![0x01]),
        (
            format!("announce {fps} Hz"),
            cmd::ANNOUNCE,
            0x05,
            [&[0x02, fps][..], &[0; 8]].concat(),
        ),
        (
            "GPU cfg nvidia".into(),
            cmd::ANNOUNCE,
            0x05,
            [&[0x05, 0x00][..], &[0; 8]].concat(),
        ),
    ];
    for (name, c, opt, d) in &steps {
        s.send(*c, *opt, d)?;
        let r = build_command(*c, *opt, d).map_err(|e| CliError::new(e.to_string()))?;
        println!(">> {name}: {}", hex(&r[..4 + d.len().min(2)]));
        std::thread::sleep(Duration::from_millis(150));
    }
    let stop = pimax::sys::install_term_flag();
    let t0 = Instant::now();
    while t0.elapsed().as_secs_f64() < secs && !stop.load(std::sync::atomic::Ordering::Relaxed) {
        let t = t0.elapsed().as_secs_f64();
        for e in rx.try_iter() {
            if let Event::Reply(r) = e {
                if r[2] != 0x39 {
                    println!("{t:6.2}  reply  {}", hex(&r[..12]));
                }
            }
        }
        if !pump(&mut tails, Duration::from_millis(200), |name, text| {
            println!("{t:6.2}  {name:6} {text}")
        }) {
            println!("!! device gone");
            break;
        }
    }
    Ok(())
}
