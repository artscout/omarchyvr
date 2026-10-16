//! `omarchyvr keepalived`: behaviour of `linux/pimax_keepalived.py`.
//! Waits for the headset, sends the wake, then `58 04 39 00 RR 00` every 213 ms (RR from `$XDG_RUNTIME_DIR/pimax-rate`),
//! keeps IF1-IF3 open and drained, and starts over whenever the device re-enumerates. SIGTERM/SIGINT stop it.
//! Options: `--no-wake` (skip the wake packet), `--rate-file PATH`.
use crate::args::Args;
use crate::{rate_file, CliResult};
use pimax::{discovery, Config, Handler, RateSource, Session, State};
use std::io::Write;
use std::time::Duration;

fn say(msg: &str) {
    println!("{} {msg}", pimax::sys::local_hms());
    let _ = std::io::stdout().flush();
}

struct H;

impl Handler for H {
    fn on_state(&mut self, s: State) {
        if s == State::Connected {
            let node = discovery::find_interfaces(&discovery::Roots::from_env())[0].clone();
            say(&format!(
                "headset on {}, sending wake + keepalive",
                node.map(|p| p.display().to_string()).unwrap_or_default()
            ));
        }
    }
    fn on_link_closed(&mut self, lived: Duration, keepalives: u64) {
        say(&format!(
            "headset gone after {:.1} s, {keepalives} keepalives",
            lived.as_secs_f64()
        ));
    }
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let no_wake = a.flag("--no-wake");
    let rate_path = a
        .opt("--rate-file")?
        .map(Into::into)
        .unwrap_or_else(rate_file);
    a.no_unknown_flags()?;
    pimax::sys::block_term_signals(); // inherited by the session thread; the main thread sits in sigwait
    say("waiting for headset 34a4:0044");
    let s = Session::open(
        Config {
            wake: !no_wake,
            rate: RateSource::File(rate_path),
            ..Config::default()
        },
        H,
    )?;
    pimax::sys::wait_for_term();
    drop(s);
    say("stopped");
    Ok(())
}
