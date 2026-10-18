//! `omarchyvr keepalived`: behaviour of `linux/pimax_keepalived.py`.
//! Waits for the headset, sends the wake, then `58 04 39 00 RR 00` every 213 ms (RR from `$XDG_RUNTIME_DIR/pimax-rate`),
//! keeps IF1-IF3 open and drained, and starts over whenever the device re-enumerates. SIGTERM/SIGINT stop it.
//! Options: `--no-wake` (skip the wake packet), `--rate-file PATH`, `--video FPS` (switch the panel on at every
//! connect, like `video-on FPS`: for SteamVR drivers that do not do it themselves, e.g. the Crystal Super with its
//! Lighthouse faceplate under SteamVR's lighthouse driver; Monado's Pimax driver sends it itself).
//!
//! `--follow PROCESS` (with `--video`): busy only while PROCESS runs (SteamVR's `vrserver`), the way Pimax Play is
//! busy only while it runs (Crystal Super Windows capture of 2026-10-17, take shutdown): while it does not run, one
//! keepalive with rate 0 and then none (the rate file says `off`; the session keeps reading the headset), and the
//! headset goes to its green standby. When it starts: wake, rate FPS and the video frames (`58 05 5b 05 02 02`,
//! `58 04 29 00 01`, `58 0d 1a 05 02 FPS`, `58 0d 1a 05 05`); the LED turns blue. It goes idle again 5 s after the
//! process is gone. The headset is watched while idle: it is logged if it re-enumerates (lesson 40).
//!
//! `--audio` (with `--follow`): while PROCESS runs, the headset's USB audio is the default output and input; when it
//! is gone the previous defaults come back, unless the user picked another device meanwhile ([`crate::audio`]).
use crate::args::Args;
use crate::follow::{process_running, Change, Follow};
use crate::{rate_file, CliError, CliResult};
use pimax::{discovery, Config, Handler, RateSource, Session, State};
use std::io::Write;
use std::sync::{mpsc, Arc};
use std::time::Duration;

/// Wait between the connect and the video-prepare frames: Pimax Play announces the rate about 4 s after its wake
/// (Crystal Super cold start, take cold2), after its identity and calibration reads; the wake itself needs about 1 s.
const VIDEO_DELAY: Duration = Duration::from_secs(2);
/// Write the keepalive rate atomically (the session re-reads the file on every keepalive); `None` = `off`.
fn write_rate(path: &std::path::Path, fps: Option<u8>) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, fps.map_or("off\n".to_string(), |f| format!("{f}\n")))?;
    std::fs::rename(&tmp, path)
}

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
    let follow = a.opt("--follow")?;
    let audio = a.flag("--audio");
    a.no_unknown_flags()?;
    if follow.is_some() && video.is_none() {
        return Err(CliError::usage("--follow needs --video FPS"));
    }
    if audio && follow.is_none() {
        return Err(CliError::usage("--audio needs --follow PROCESS"));
    }
    pimax::sys::block_term_signals(); // inherited by the session and video threads; the main thread sits in sigwait
    let busy_at_start = follow.as_deref().is_none_or(process_running);
    if let Some(fps) = video {
        // the keepalive reads its rate from this file on every keepalive
        write_rate(&rate_path, busy_at_start.then_some(fps))
            .map_err(|e| CliError::new(format!("{}: {e}", rate_path.display())))?;
    }
    if let Some(p) = &follow {
        say(&format!(
            "following {p}: {}",
            if busy_at_start { "running, headset on" } else { "not running, headset idle until it starts" }
        ));
    }
    say("waiting for headset 34a4:0044 (or Crystal Super 34a4:0040)");
    let (tx, rx) = mpsc::channel();
    let s = Arc::new(Session::open(
        Config {
            wake: !no_wake,
            rate: RateSource::File(rate_path.clone()),
            ..Config::default()
        },
        H { connected: video.map(|_| tx) },
    )?);
    if let Some(fps) = video {
        // a Weak, so dropping the session below ends the thread (its sender goes away with the session's handler)
        let weak = Arc::downgrade(&s);
        std::thread::spawn(move || {
            let mut fol = follow.as_ref().map(|_| Follow::new(busy_at_start));
            let mut audio_before = None;
            if audio && busy_at_start {
                let (m, b) = crate::audio::to_headset();
                say(&m);
                audio_before = Some(b);
            }
            let busy = |f: &Option<Follow>| f.as_ref().is_none_or(Follow::active);
            let wake = |s: &Session| match s.video_prepare(fps) {
                Ok(()) => say(&format!("video on at {fps} Hz")),
                Err(e) => say(&format!("video on at {fps} Hz failed: {e}")),
            };
            loop {
                match rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(()) => {
                        // a (re)connect: the panel only while busy; idle, the rate file already says off
                        if busy(&fol) {
                            std::thread::sleep(VIDEO_DELAY);
                            let Some(s) = weak.upgrade() else { break };
                            wake(&s);
                        } else {
                            say("idle: one keepalive with rate 0, then none (standby)");
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
                let (Some(f), Some(p)) = (fol.as_mut(), follow.as_deref()) else { continue };
                let Some(change) = f.step(std::time::Instant::now(), process_running(p)) else { continue };
                let Some(s) = weak.upgrade() else { break };
                match change {
                    Change::Active => {
                        say(&format!("{p} started: waking the headset"));
                        // out of the green standby the way Pimax Play starts: the wake frame first, then the rate
                        if let Err(e) = s.send_report(&pimax_proto::wake()) {
                            say(&format!("wake failed: {e}"));
                        }
                        let _ = write_rate(&rate_path, Some(fps));
                        std::thread::sleep(VIDEO_DELAY);
                        wake(&s);
                        if audio {
                            let (m, b) = crate::audio::to_headset();
                            say(&m);
                            audio_before = Some(b);
                        }
                    }
                    Change::Idle => {
                        say(&format!("{p} gone for 5 s: the headset goes idle (one keepalive with rate 0, then none)"));
                        let _ = write_rate(&rate_path, None);
                        if let Some(b) = audio_before.take() {
                            say(&crate::audio::restore(&b));
                        }
                    }
                }
            }
        });
    }
    pimax::sys::wait_for_term();
    drop(s);
    say("stopped");
    Ok(())
}
