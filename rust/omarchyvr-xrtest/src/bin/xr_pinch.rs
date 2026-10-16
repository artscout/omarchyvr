//! xr-pinchtest: headless OpenXR check that the Pimax controllers work as XR_EXT_hand_interaction hands
//! (pinch = trigger), for apps that only suggest hand-interaction bindings.
//!
//! Binds `/user/hand/{left,right}/input/pinch_ext/value` and `.../pinch_ext/ready_ext` on
//! `/interaction_profiles/ext/hand_interaction_ext` ONLY (no Touch bindings, so the runtime has to use the profile the
//! Pimax driver maps), syncs at 20 Hz and checks: current interaction profile = hand_interaction for both hands, the
//! action is active, and with `--demo` (service with PIMAX_CTRL_HANDS_DEMO=1: the scripted trigger sweeps 0 -> 1 -> 0)
//! that the value covers the whole range and ready_ext turns on. Without --demo, pull the triggers while it runs.
//!
//! Exit status: 0 pass, 1 check failed, 2 could not run.

use openxr as xr;
use omarchyvr_xrtest::pinch::PinchStats;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const HELP: &str = "xr-pinchtest [--seconds N] [--demo] [--wait-runtime N] [--quiet]
  --seconds N        run time (default 10; with --demo at least 8 so a trigger sweep is seen)
  --demo             the service runs with PIMAX_CTRL_HANDS_DEMO=1: expect the scripted trigger sweep
  --wait-runtime N   keep trying to reach the runtime for N seconds (default 15)
  --quiet            only the summary
Exit status: 0 pass, 1 check failed, 2 setup problem.";

struct Args {
    seconds: f64,
    demo: bool,
    wait_runtime: f64,
    quiet: bool,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        seconds: 10.0,
        demo: false,
        wait_runtime: 15.0,
        quiet: false,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut num = |name: &str| -> Result<f64, String> {
            it.next()
                .ok_or(format!("{name} needs a value"))?
                .parse::<f64>()
                .map_err(|e| format!("{name}: {e}"))
        };
        match k.as_str() {
            "--seconds" => a.seconds = num("--seconds")?,
            "--wait-runtime" => a.wait_runtime = num("--wait-runtime")?,
            "--demo" => a.demo = true,
            "--quiet" => a.quiet = true,
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            other => return Err(format!("unknown argument '{other}'\n{HELP}")),
        }
    }
    if a.demo {
        a.seconds = a.seconds.max(8.0);
    }
    Ok(Some(a))
}

fn run() -> Result<ExitCode, String> {
    let Some(args) = parse_args()? else {
        return Ok(ExitCode::SUCCESS);
    };
    // SAFETY: loads the system OpenXR loader.
    let entry = unsafe { xr::Entry::load(&()) }
        .map_err(|e| format!("cannot load libopenxr_loader: {e}"))?;
    let avail = entry
        .enumerate_extensions()
        .map_err(|e| format!("enumerate extensions: {e}"))?;
    for (name, ok) in [
        ("XR_MND_headless", avail.mnd_headless),
        ("XR_EXT_hand_interaction", avail.ext_hand_interaction),
    ] {
        if !ok {
            return Err(format!("the runtime does not offer {name}"));
        }
    }
    let mut exts = xr::ExtensionSet::default();
    exts.mnd_headless = true;
    exts.ext_hand_interaction = true;
    let app = xr::ApplicationInfo {
        application_name: "xr-pinchtest",
        application_version: 1,
        engine_name: "omarchyvr-xrtest",
        engine_version: 1,
        api_version: xr::Version::new(1, 0, 0),
    };
    let deadline = Instant::now() + Duration::from_secs_f64(args.wait_runtime);
    let instance = loop {
        match entry.create_instance(&app, &exts, &[], &()) {
            Ok(i) => break i,
            Err(e) if Instant::now() < deadline => {
                eprintln!("waiting for the runtime: {e}");
                std::thread::sleep(Duration::from_secs(1));
            }
            Err(e) => return Err(format!("xrCreateInstance: {e}")),
        }
    };
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .map_err(|e| format!("no HMD system: {e}"))?;
    // SAFETY: headless sessions need no graphics objects.
    let (session, _waiter, _stream) = unsafe {
        instance.create_session::<xr::Headless>(system, &xr::headless::SessionCreateInfo {})
    }
    .map_err(|e| format!("xrCreateSession (headless): {e}"))?;

    let p = |s: &str| {
        instance
            .string_to_path(s)
            .map_err(|e| format!("path {s}: {e}"))
    };
    let hands = [p("/user/hand/left")?, p("/user/hand/right")?];
    let set = instance
        .create_action_set("pinchtest", "pinch test", 0)
        .map_err(|e| format!("action set: {e}"))?;
    let pinch = set
        .create_action::<f32>("pinch", "pinch value", &hands)
        .map_err(|e| format!("action: {e}"))?;
    let ready = set
        .create_action::<bool>("ready", "pinch ready", &hands)
        .map_err(|e| format!("action: {e}"))?;
    let profile = p(omarchyvr_xrtest::pinch::HAND_INTERACTION_PROFILE)?;
    instance
        .suggest_interaction_profile_bindings(
            profile,
            &[
                xr::Binding::new(&pinch, p("/user/hand/left/input/pinch_ext/value")?),
                xr::Binding::new(&pinch, p("/user/hand/right/input/pinch_ext/value")?),
                xr::Binding::new(&ready, p("/user/hand/left/input/pinch_ext/ready_ext")?),
                xr::Binding::new(&ready, p("/user/hand/right/input/pinch_ext/ready_ext")?),
            ],
        )
        .map_err(|e| format!("suggest bindings: {e}"))?;
    session
        .attach_action_sets(&[&set])
        .map_err(|e| format!("attach: {e}"))?;

    let mut stats = [PinchStats::default(), PinchStats::default()];
    let mut ev = xr::EventDataBuffer::new();
    let (mut running, mut began, mut focused) = (false, false, false);
    let start = Instant::now();
    let mut next_print = 0.0;
    'main: loop {
        while let Some(e) = instance
            .poll_event(&mut ev)
            .map_err(|e| format!("poll event: {e}"))?
        {
            match e {
                xr::Event::SessionStateChanged(s) => match s.state() {
                    xr::SessionState::READY if !running => {
                        session
                            .begin(xr::ViewConfigurationType::PRIMARY_STEREO)
                            .map_err(|e| format!("xrBeginSession: {e}"))?;
                        running = true;
                        began = true;
                    }
                    xr::SessionState::FOCUSED => focused = true,
                    xr::SessionState::STOPPING => {
                        let _ = session.end();
                        running = false;
                        break 'main;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => break 'main,
                    _ => {}
                },
                xr::Event::InteractionProfileChanged(_) => {
                    for (i, h) in hands.iter().enumerate() {
                        if let Ok(pr) = session.current_interaction_profile(*h) {
                            if pr != xr::Path::NULL {
                                stats[i].profile = instance.path_to_string(pr).ok();
                            }
                        }
                    }
                }
                _ => {}
            }
        }
        let t = start.elapsed().as_secs_f64();
        if t >= args.seconds {
            break;
        }
        if running
            && focused
            && session
                .sync_actions(&[xr::ActiveActionSet::new(&set)])
                .is_ok()
        {
            for (i, h) in hands.iter().enumerate() {
                let v = pinch
                    .state(&session, *h)
                    .map_err(|e| format!("pinch state: {e}"))?;
                let r = ready
                    .state(&session, *h)
                    .map_err(|e| format!("ready state: {e}"))?;
                stats[i].add(v.current_state, v.is_active, r.is_active && r.current_state);
                if stats[i].profile.is_none() {
                    if let Ok(pr) = session.current_interaction_profile(*h) {
                        if pr != xr::Path::NULL {
                            stats[i].profile = instance.path_to_string(pr).ok();
                        }
                    }
                }
            }
            if !args.quiet && t >= next_print {
                next_print += 0.5;
                let s = |i: usize| {
                    let st = &stats[i];
                    format!(
                        "{} {:.2}",
                        if st.samples > 0 && st.active > 0 {
                            "on "
                        } else {
                            "off"
                        },
                        pinch
                            .state(&session, hands[i])
                            .map(|v| v.current_state)
                            .unwrap_or(f32::NAN)
                    )
                };
                println!("t={t:5.1} pinch L {} | R {}", s(0), s(1));
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !began {
        return Err("the session never reached READY; is a service running?".into());
    }
    if running {
        let _ = session.request_exit();
        let until = Instant::now() + Duration::from_secs(3);
        'stop: while Instant::now() < until {
            while let Ok(Some(e)) = instance.poll_event(&mut ev) {
                if let xr::Event::SessionStateChanged(s) = e {
                    if s.state() == xr::SessionState::STOPPING {
                        let _ = session.end();
                        break 'stop;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    println!("\n== summary ==");
    let mut pass = true;
    for (name, st) in ["L", "R"].iter().zip(stats.iter()) {
        match st.verdict(args.demo) {
            Ok(s) => println!("{name}: {s}"),
            Err(e) => {
                pass = false;
                println!("{name}: FAIL {e}");
            }
        }
    }
    println!("RESULT: {}", if pass { "PASS" } else { "FAIL" });
    Ok(if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("xr-pinchtest: {e}");
            ExitCode::from(2)
        }
    }
}
