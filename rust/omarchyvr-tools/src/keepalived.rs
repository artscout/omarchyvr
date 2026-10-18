//! `omarchyvr keepalived`: behaviour of `linux/pimax_keepalived.py`.
//! Waits for the headset, sends the wake, then `58 04 39 00 RR 00` every 213 ms (RR from `$XDG_RUNTIME_DIR/pimax-rate`),
//! keeps IF1-IF3 open and drained, and starts over whenever the device re-enumerates. SIGTERM/SIGINT stop it.
//! Options: `--no-wake` (skip the wake packet), `--rate-file PATH`, `--video FPS` (switch the panel on at every
//! connect, like `video-on FPS`: for SteamVR drivers that do not do it themselves, e.g. the Crystal Super with its
//! Lighthouse faceplate under SteamVR's lighthouse driver; Monado's Pimax driver sends it itself).
use crate::args::Args;
use crate::{rate_file, CliError, CliResult};
use pimax::{discovery, Config, Handler, RateSource, Session, State};
use std::io::Write;
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Wait between the connect and the video-prepare frames: Pimax Play announces the rate about 4 s after its wake
/// (Crystal Super cold start, take cold2), after its identity and calibration reads; the wake itself needs about 1 s.
const VIDEO_DELAY: Duration = Duration::from_secs(2);

fn say(msg: &str) {
    println!("{} {msg}", pimax::sys::local_hms());
    let _ = std::io::stdout().flush();
}

struct H {
    /// Tells the video thread about every connect (`--video`).
    connected: Option<mpsc::Sender<()>>,
}

impl Handler for H {
    fn on_state(&mut self, s: State) {
        if s == State::Connected {
            let found = discovery::find_headset(&discovery::Roots::from_env());
            let node = found.as_ref().and_then(|(_, f)| f[0].clone());
            let model = found.map_or(String::new(), |(m, _)| format!(" ({})", m.name));
            say(&format!(
                "headset on {}{model}, sending wake + keepalive",
                node.map(|p| p.display().to_string()).unwrap_or_default()
            ));
            if let Some(tx) = &self.connected {
                let _ = tx.send(());
            }
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
    let video: Option<u8> = match a.opt("--video")? {
        None => None,
        Some(v) => match v.parse::<u8>() {
            Ok(f @ (72 | 90)) => Some(f),
            _ => return Err(CliError::usage(format!("--video wants 72 or 90, not {v:?}"))),
        },
    };
    a.no_unknown_flags()?;
    pimax::sys::block_term_signals(); // inherited by the session and video threads; the main thread sits in sigwait
    if let Some(fps) = video {
        // the keepalive reads its rate from this file on every keepalive
        std::fs::write(&rate_path, format!("{fps}\n"))
            .map_err(|e| CliError::new(format!("{}: {e}", rate_path.display())))?;
    }
    say("waiting for headset 34a4:0044 (or Crystal Super 34a4:0040)");
    let (tx, rx) = mpsc::channel();
    let s = Arc::new(Session::open(
        Config {
            wake: !no_wake,
            rate: RateSource::File(rate_path),
            ..Config::default()
        },
        H { connected: video.map(|_| tx) },
    )?);
    if let Some(fps) = video {
        // a Weak, so dropping the session below ends the thread (its sender goes away with the session's handler)
        let weak = Arc::downgrade(&s);
        std::thread::spawn(move || {
            while rx.recv().is_ok() {
                std::thread::sleep(VIDEO_DELAY);
                let Some(s) = weak.upgrade() else { break };
                match s.video_prepare(fps) {
                    Ok(()) => say(&format!("video on at {fps} Hz")),
                    Err(e) => say(&format!("video on at {fps} Hz failed: {e}")),
                }
            }
        });
    }
    pimax::sys::wait_for_term();
    drop(s);
    say("stopped");
    Ok(())
}
