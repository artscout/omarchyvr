//! `omarchyvr info`: headset serial (0x16), version (0x03), local info (0x50), interface map. Read-only.
//! Runs a passive session (no wake, no keepalive), so it is safe next to `omarchyvr keepalived`.
//! `--controllers` also asks both controller slots for serial/version (only answered when they are awake).
use crate::args::Args;
use crate::{hex, CliError, CliResult};
use pimax::{discovery, Config, Session};
use pimax_proto::cmd;
use std::time::Duration;

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let ctl = a.flag("--controllers");
    a.no_unknown_flags()?;
    let roots = discovery::Roots::from_env();
    let ifs = discovery::find_interfaces(&roots);
    for (i, p) in ifs.iter().enumerate() {
        println!(
            "IF{i}: {}",
            p.as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| "-".into())
        );
    }
    if let Some(b) = discovery::find_bridge_log(&roots) {
        println!("bridge log (34a4:0043): {}", b.display());
    }
    let s = Session::open(Config::passive(), pimax::NoopHandler)?;
    s.wait_connected(Duration::from_secs(10))
        .map_err(|_| CliError::new("headset not found (34a4:0044)"))?;
    let t = Duration::from_millis(1000);
    let mut rc = Ok(());
    match s.read_serial(0, t) {
        Ok(v) => println!("serial: {v}"),
        Err(e) => {
            println!("serial: error {e}");
            rc = Err(CliError::new("serial read failed"));
        }
    }
    match s.read_version(0, t) {
        Ok(v) => println!("version: {}", hex(&v)),
        Err(e) => {
            println!("version: error {e}");
            rc = Err(CliError::new("version read failed"));
        }
    }
    match s.request(cmd::INFO, 0, &[], t) {
        Ok(v) => println!("info50: {}", hex(&v)),
        Err(e) => println!("info50: error {e}"),
    }
    if ctl {
        for slot in [2u8, 3] {
            match s.read_serial(slot, t) {
                Ok(v) => println!("controller {slot} serial: {v}"),
                Err(_) => println!("controller {slot}: no answer (asleep?)"),
            }
        }
    }
    println!(
        "usb devnum: {}",
        discovery::usb_devnum().unwrap_or_else(|| "?".into())
    );
    rc
}
