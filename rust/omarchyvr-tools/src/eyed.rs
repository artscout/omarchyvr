//! `omarchyvr eyed`: runs the eye tracker on its own (no Monado) and publishes the gaze in shared memory
//! (`pimax_eye::shm`, `/dev/shm/omarchyvr-eye-gaze`) for the SteamVR driver: the light setup of the Crystal Super
//! (SteamVR Lighthouse + CustomHeadsetOpenVR, docs/CRYSTAL-SUPER.md), whose SteamVR eye tracking component then serves
//! OpenXR `XR_EXT_eye_gaze_interaction`.
//!
//! Options: `--frames FILE` (the start list made by `pimax-eye frames` from a Windows capture of this headset; needed
//! for headsets without a built-in list, the Crystal Super), `--profile FILE` (the user's current eye profile, made by
//! `pimax-eye profile` from Pimax's calibration.setpm, uploaded in place of the list's own; also PIMAX_EYE_PROFILE; a
//! missing file is skipped), `--no-profile` (no eye profile upload at all), `--shm PATH`. The environment of `pimax-eye live` applies (PIMAX_EYE_*, eye-calibration.toml). The eye module
//! is powered with the captured enable when the headset is there and the chip is not. Monado's own eye tracking must
//! not run at the same time (one owner of the chip). SIGTERM/SIGINT stop it; the file then says "no gaze".
//!
//! When the stream goes on but shows no eye for [`NO_EYES_ENABLE`] (the headset was taken off and put on again: the
//! Crystal Super switches its eye module off when it is taken off, and Pimax Play sends the eye-module enable again
//! when it is put back on), the captured enable `58 05 3c 05 00 01` is sent once for that stretch. Losing and finding
//! the eyes is logged.
//!
//! `--follow PROCESS` (SteamVR's `vrserver`): the eye tracker runs only while PROCESS runs, like `omarchyvr
//! keepalived --follow`: it starts with the runtime and stops (unsubscribes, releases the chip) 5 s after it is gone.
use crate::args::Args;
use crate::follow::{process_running, Change, Follow};
use crate::{CliError, CliResult};
use pimax_eye::clock::monotonic_ns;
use pimax_eye::live::{Config, State, Tracker};
use pimax_eye::shm::{self, Publisher};
use std::io::Write;
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// No eye for this long while streaming: send the eye-module enable once.
const NO_EYES_ENABLE: Duration = Duration::from_secs(4);
/// The eyes count as lost after this long without one valid sample.
const EYES_LOST_AFTER: Duration = Duration::from_millis(500);

/// Send the captured eye-module enable to the headset; a log line.
fn enable_eye_module() -> String {
    use pimax_eye::power::{headset, headset_command_node, send_enable};
    let Some(h) = headset(std::path::Path::new("/sys/bus/usb/devices")) else {
        return "no headset on USB".into();
    };
    let Some(node) = headset_command_node(std::path::Path::new("/sys/class/hidraw"), std::path::Path::new("/dev"))
    else {
        return "no hidraw node for the headset".into();
    };
    match send_enable(&node) {
        Ok(()) => format!("sent the captured eye-module enable 58 05 3c 05 00 01 to {} ({})", node.display(), h.name),
        Err(e) => format!("eye module enable to {} failed: {e}", node.display()),
    }
}

fn say(msg: &str) {
    println!("{} {msg}", pimax::sys::local_hms());
    let _ = std::io::stdout().flush();
}

/// Entry point.
pub fn run(mut a: Args) -> CliResult {
    let frames = a.opt("--frames")?;
    let profile = a.opt("--profile")?;
    let no_profile = a.flag("--no-profile");
    let path: PathBuf = a.opt("--shm")?.map(Into::into).unwrap_or_else(shm::default_path);
    let follow = a.opt("--follow")?;
    a.no_unknown_flags()?;

    let mut cfg = Config::from_env();
    if let Some(f) = &frames {
        let text = std::fs::read_to_string(f).map_err(|e| CliError::new(format!("{f}: {e}")))?;
        let steps = pimax_eye::frames::parse(&text).map_err(|e| CliError::new(format!("{f}: {e}")))?;
        say(&format!("start list {f}: {} steps", steps.len()));
        cfg.frames = Some(text.into());
        // the list's own profile upload is this user's Pimax Play eye profile
        cfg.with_cal = !no_profile;
    }
    if let Some(f) = &profile {
        match std::fs::read(f) {
            Ok(b) => {
                pimax_eye::frames::check_profile(&b).map_err(|e| CliError::new(format!("{f}: {e}")))?;
                cfg.profile = Some(b.into());
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => say(&format!("no eye profile {f}: the list's own")),
            Err(e) => return Err(CliError::new(format!("{f}: {e}"))),
        }
    }
    if no_profile {
        cfg.profile = None;
    }
    if let Some(b) = &cfg.profile {
        say(&format!("eye profile: {} bytes (the user's calibration)", b.len()));
    }
    let mut out = Publisher::create(&path).map_err(|e| CliError::new(format!("{}: {e}", path.display())))?;
    out.publish(None, State::Searching as u32, monotonic_ns())
        .map_err(|e| CliError::new(format!("{}: {e}", path.display())))?;
    say(&format!("publishing the gaze to {}", path.display()));

    pimax_eye::sigint::install();
    let running_now = follow.as_deref().is_none_or(process_running);
    let mut fol = follow.as_ref().map(|_| Follow::new(running_now));
    if let Some(p) = &follow {
        say(&format!("following {p}: {}", if running_now { "running" } else { "not running, waiting for it" }));
    }
    let mut tr = running_now.then(|| Tracker::start(cfg.clone()));
    let mut last_follow_check = Instant::now();
    let mut last_state = None;
    let mut last_seq = None;
    let mut last_write = Instant::now();
    let mut streamed = 0u64;
    let mut since = Instant::now();
    // eyes seen or not (any eye's direction valid), and since when; whether the enable went out for this stretch
    let mut eyes: Option<(bool, Instant)> = None;
    let mut enable_sent = false;
    let mut last_valid: Option<Instant> = None;
    while !pimax_eye::sigint::stopped() {
        std::thread::sleep(Duration::from_millis(2));
        if let (Some(f), Some(p)) = (fol.as_mut(), follow.as_deref()) {
            if last_follow_check.elapsed() >= Duration::from_millis(500) {
                last_follow_check = Instant::now();
                match f.step(Instant::now(), process_running(p)) {
                    Some(Change::Active) => {
                        say(&format!("{p} started: starting the eye tracker"));
                        tr = Some(Tracker::start(cfg.clone()));
                    }
                    Some(Change::Idle) => {
                        say(&format!("{p} gone for 5 s: stopping the eye tracker"));
                        tr = None; // drop: unsubscribes and releases the chip
                    }
                    None => {}
                }
            }
        }
        let Some(tr) = tr.as_ref() else {
            if last_write.elapsed() >= Duration::from_millis(500) {
                let _ = out.publish(None, State::Stopped as u32, monotonic_ns());
                last_write = Instant::now();
            }
            last_state = None;
            continue;
        };
        let st = tr.state();
        if last_state != Some(st) {
            say(&format!("eye tracker: {st:?}"));
            last_state = Some(st);
        }
        let s = if st == State::Streaming { tr.latest() } else { None };
        let fresh = s.map(|x| x.seq) != last_seq;
        // every new sample, and twice a second without one (readers see the state and that the writer lives)
        if fresh || last_write.elapsed() >= Duration::from_millis(500) {
            if let Err(e) = out.publish(s.as_ref(), st as u32, monotonic_ns()) {
                say(&format!("{}: {e}", path.display()));
            }
            last_write = Instant::now();
            last_seq = s.map(|x| x.seq);
            if fresh && s.is_some() {
                streamed += 1;
            }
        }
        if let (true, Some(x)) = (fresh, s.as_ref()) {
            if x.eyes.iter().any(|e| e.direction_valid) {
                last_valid = Some(Instant::now());
            }
            // about half the samples carry no eye even when worn (blinks, alternating frames): "lost" only after
            // EYES_LOST_AFTER without a single valid one
            let seen = last_valid.is_some_and(|t| t.elapsed() < EYES_LOST_AFTER);
            match eyes {
                Some((was, t)) if was == seen => {
                    if !seen && !enable_sent && t.elapsed() >= NO_EYES_ENABLE && cfg.power_on {
                        say(&format!("no eye for {:.0} s: {}", t.elapsed().as_secs_f64(), enable_eye_module()));
                        enable_sent = true;
                    }
                }
                Some((_, t)) => {
                    say(&format!(
                        "{} after {:.1} s",
                        if seen { "eyes seen again" } else { "eyes lost" },
                        t.elapsed().as_secs_f64()
                    ));
                    eyes = Some((seen, Instant::now()));
                    if seen {
                        enable_sent = false;
                    }
                }
                None => {
                    say(if seen { "eyes seen" } else { "no eye seen yet" });
                    eyes = Some((seen, Instant::now()));
                }
            }
        }
        if since.elapsed() >= Duration::from_secs(60) && streamed > 0 {
            say(&format!("{:.0} gaze samples/s", streamed as f64 / since.elapsed().as_secs_f64()));
            streamed = 0;
            since = Instant::now();
        }
    }
    drop(tr);
    let _ = out.publish(None, State::Searching as u32, monotonic_ns());
    say("stopped");
    Ok(())
}
