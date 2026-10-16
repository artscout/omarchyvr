//! xr-handtest: headless OpenXR check of the controller-driven hand tracking of the Pimax Monado driver.
//!
//! Creates a hand tracker per hand (XR_EXT_hand_tracking, data source CONTROLLER when XR_EXT_hand_tracking_data_source
//! is there), prints joint validity, the wrist pose and the per-finger curl 2 times a second for N seconds, and exits
//! non-zero if no valid joints arrive. No graphics: the session is headless (XR_MND_headless).
//!
//! Checks on top of "joints are valid" (all on by default, see --help):
//!  * data source is CONTROLLER
//!  * the wrist joint sits at (grip pose of the Touch controller) * (grip->wrist offset of pimax-hands), and the grip
//!    pose rebuilt from the joints (Monado's ht_ctrl_emu rule) agrees with the real grip pose: the hand follows the
//!    controller
//!  * with --demo (service started with PIMAX_CTRL_HANDS_DEMO=1) the fingers follow the scripted pattern
//!
//! Exit status: 0 pass, 1 a check failed / no valid joints, 2 could not run (no runtime, extension missing).

use openxr as xr;
use openxr::sys;
use openxr::sys::Handle as _;
use pimax_hands::pose::{grip_to_wrist, Hand, Pose};
use pimax_xrtest::checks::{check_demo, DemoSample, MaxErr, Tolerances};
use pimax_xrtest::joints::{
    curl_estimates, flexion, grip_from_joint_set, pose_error, Finger, Joint, Joints, JOINT_COUNT,
    WRIST,
};
use std::ffi::c_void;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const HELP: &str = "xr-handtest [options]
  --seconds N        run time (default 10)
  --print-hz F       lines per second per hand (default 2)
  --require WHO      which hands must deliver valid joints: any (default), both, left, right
  --source S         controller (default: ask for the CONTROLLER data source) or any
  --demo             the service runs with PIMAX_CTRL_HANDS_DEMO=1: check the fingers against the scripted pattern
                     (needs at least 22 s to cover a pattern cycle; default seconds become 24)
  --no-grip-check    skip the 'hand follows the controller grip pose' check
  --require-grip     fail if the grip pose cannot be located (default: warn and skip)
  --min-valid F      fraction of samples that must have valid joints (default 0.5)
  --wait-runtime N   keep trying to reach the runtime for N seconds (default 15)
  --quiet            only the summary
  --help
Exit status: 0 pass, 1 check failed or no valid joints, 2 setup problem.";

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Require {
    Any,
    Both,
    Left,
    Right,
}

struct Args {
    seconds: f64,
    print_hz: f64,
    require: Require,
    controller_source: bool,
    demo: bool,
    grip_check: bool,
    require_grip: bool,
    min_valid: f64,
    wait_runtime: f64,
    quiet: bool,
}

fn parse_args() -> Result<Args, String> {
    let mut a = Args {
        seconds: 10.0,
        print_hz: 2.0,
        require: Require::Any,
        controller_source: true,
        demo: false,
        grip_check: true,
        require_grip: false,
        min_valid: 0.5,
        wait_runtime: 15.0,
        quiet: false,
    };
    let mut seconds_given = false;
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = |name: &str| it.next().ok_or_else(|| format!("{name} needs a value"));
        match arg.as_str() {
            "--seconds" => {
                a.seconds = val("--seconds")?
                    .parse()
                    .map_err(|e| format!("--seconds: {e}"))?;
                seconds_given = true;
            }
            "--print-hz" => {
                a.print_hz = val("--print-hz")?
                    .parse()
                    .map_err(|e| format!("--print-hz: {e}"))?
            }
            "--require" => {
                a.require = match val("--require")?.as_str() {
                    "any" => Require::Any,
                    "both" => Require::Both,
                    "left" => Require::Left,
                    "right" => Require::Right,
                    o => return Err(format!("--require {o}: want any|both|left|right")),
                }
            }
            "--source" => {
                a.controller_source = match val("--source")?.as_str() {
                    "controller" => true,
                    "any" => false,
                    o => return Err(format!("--source {o}: want controller|any")),
                }
            }
            "--demo" => a.demo = true,
            "--no-grip-check" => a.grip_check = false,
            "--require-grip" => a.require_grip = true,
            "--min-valid" => {
                a.min_valid = val("--min-valid")?
                    .parse()
                    .map_err(|e| format!("--min-valid: {e}"))?
            }
            "--wait-runtime" => {
                a.wait_runtime = val("--wait-runtime")?
                    .parse()
                    .map_err(|e| format!("--wait-runtime: {e}"))?
            }
            "--quiet" => a.quiet = true,
            "--help" | "-h" => {
                println!("{HELP}");
                std::process::exit(0);
            }
            o => return Err(format!("unknown option {o} (try --help)")),
        }
    }
    if a.demo && !seconds_given {
        a.seconds = 24.0;
    }
    if !(a.seconds > 0.0 && a.print_hz > 0.0) {
        return Err("--seconds and --print-hz must be positive".into());
    }
    Ok(a)
}

/// CLOCK_MONOTONIC in nanoseconds: the clock Monado stamps its data with and the demo pattern runs on.
fn mono_ns() -> u64 {
    let mut ts = libc::timespec {
        tv_sec: 0,
        tv_nsec: 0,
    };
    // SAFETY: ts is a valid timespec.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as u64 * 1_000_000_000 + ts.tv_nsec as u64
}

fn pose_of(p: &xr::Posef) -> Pose {
    Pose {
        q: [
            f64::from(p.orientation.x),
            f64::from(p.orientation.y),
            f64::from(p.orientation.z),
            f64::from(p.orientation.w),
        ],
        p: [
            f64::from(p.position.x),
            f64::from(p.position.y),
            f64::from(p.position.z),
        ],
    }
}

/// Result of one xrLocateHandJointsEXT call.
struct Located {
    active: bool,
    /// Data source state, `None` when the extension is not enabled.
    source: Option<(bool, sys::HandTrackingDataSourceEXT)>,
    loc: [sys::HandJointLocationEXT; JOINT_COUNT],
}

fn locate_hand(
    session: &xr::Session<xr::Headless>,
    tracker: &xr::HandTracker,
    base: &xr::Space,
    time: xr::Time,
    with_source: bool,
) -> Result<Located, sys::Result> {
    let fp = session
        .instance()
        .exts()
        .ext_hand_tracking
        .as_ref()
        .ok_or(sys::Result::ERROR_EXTENSION_NOT_PRESENT)?;
    let info = sys::HandJointsLocateInfoEXT {
        ty: sys::HandJointsLocateInfoEXT::TYPE,
        next: std::ptr::null(),
        base_space: base.as_raw(),
        time,
    };
    let mut loc = [sys::HandJointLocationEXT::default(); JOINT_COUNT];
    let mut src = sys::HandTrackingDataSourceStateEXT {
        ty: sys::HandTrackingDataSourceStateEXT::TYPE,
        next: std::ptr::null_mut(),
        is_active: false.into(),
        data_source: sys::HandTrackingDataSourceEXT::from_raw(0),
    };
    let mut out = sys::HandJointLocationsEXT {
        ty: sys::HandJointLocationsEXT::TYPE,
        next: if with_source {
            &mut src as *mut _ as *mut c_void
        } else {
            std::ptr::null_mut()
        },
        is_active: false.into(),
        joint_count: JOINT_COUNT as u32,
        joint_locations: loc.as_mut_ptr(),
    };
    // SAFETY: valid handles, `out` points at JOINT_COUNT locations that outlive the call.
    let r = unsafe { (fp.locate_hand_joints)(tracker.as_raw(), &info, &mut out) };
    if r.into_raw() < 0 {
        return Err(r);
    }
    Ok(Located {
        active: out.is_active.into(),
        source: with_source.then(|| (src.is_active.into(), src.data_source)),
        loc,
    })
}

/// Create a hand tracker, asking for the CONTROLLER data source when `controller_only`, else for both sources
/// (the locate call chains XrHandTrackingDataSourceStateEXT, which the spec only allows for trackers created with
/// requested sources).
fn create_tracker(
    session: &xr::Session<xr::Headless>,
    hand: xr::Hand,
    controller_only: bool,
) -> Result<xr::HandTracker, sys::Result> {
    let fp = session
        .instance()
        .exts()
        .ext_hand_tracking
        .as_ref()
        .ok_or(sys::Result::ERROR_EXTENSION_NOT_PRESENT)?;
    let mut controller = [sys::HandTrackingDataSourceEXT::CONTROLLER];
    let mut both = [
        sys::HandTrackingDataSourceEXT::UNOBSTRUCTED,
        sys::HandTrackingDataSourceEXT::CONTROLLER,
    ];
    let sources: &mut [sys::HandTrackingDataSourceEXT] = if controller_only {
        &mut controller
    } else {
        &mut both
    };
    let ds = sys::HandTrackingDataSourceInfoEXT {
        ty: sys::HandTrackingDataSourceInfoEXT::TYPE,
        next: std::ptr::null(),
        requested_data_source_count: sources.len() as u32,
        requested_data_sources: sources.as_mut_ptr(),
    };
    let info = sys::HandTrackerCreateInfoEXT {
        ty: sys::HandTrackerCreateInfoEXT::TYPE,
        next: &ds as *const _ as *const c_void,
        hand,
        hand_joint_set: sys::HandJointSetEXT::DEFAULT,
    };
    let mut handle = sys::HandTrackerEXT::NULL;
    // SAFETY: valid session, `info` and its chain outlive the call.
    let r = unsafe { (fp.create_hand_tracker)(session.as_raw(), &info, &mut handle) };
    if r.into_raw() < 0 {
        return Err(r);
    }
    // SAFETY: handle was just created for this session; the wrapper destroys it.
    Ok(unsafe { xr::HandTracker::from_raw(session, handle) })
}

fn source_name(s: sys::HandTrackingDataSourceEXT) -> &'static str {
    if s == sys::HandTrackingDataSourceEXT::CONTROLLER {
        "CONTROLLER"
    } else if s == sys::HandTrackingDataSourceEXT::UNOBSTRUCTED {
        "UNOBSTRUCTED"
    } else {
        "?"
    }
}

#[derive(Default)]
struct Stats {
    samples: u32,
    active: u32,
    valid: u32,
    pos_tracked_all: u32,
    ori_tracked_all: u32,
    src_controller: u32,
    src_other: u32,
    src_missing: u32,
    grip_pos: MaxErr,
    grip_ang: MaxErr,
    derived_pos: MaxErr,
    derived_ang: MaxErr,
    grip_unavailable: u32,
    demo: Vec<DemoSample>,
    last: Option<LastSample>,
}

#[derive(Clone)]
struct LastSample {
    active: bool,
    valid_joints: usize,
    src: String,
    wrist: Option<xr::Posef>,
    curls: Option<[f64; 5]>,
    grip_err: Option<(f64, f64)>,
}

struct HandCtx {
    name: &'static str,
    hand: Hand,
    tracker: xr::HandTracker,
    grip_space: Option<xr::Space>,
    stats: Stats,
}

fn run() -> Result<ExitCode, String> {
    let args = parse_args()?;

    // SAFETY: loads the system OpenXR loader.
    let entry = unsafe { xr::Entry::load(&()) }
        .map_err(|e| format!("cannot load libopenxr_loader: {e}"))?;
    let avail = entry
        .enumerate_extensions()
        .map_err(|e| format!("enumerate extensions: {e}"))?;
    for (name, ok) in [
        ("XR_MND_headless", avail.mnd_headless),
        ("XR_EXT_hand_tracking", avail.ext_hand_tracking),
        (
            "XR_KHR_convert_timespec_time",
            avail.khr_convert_timespec_time,
        ),
    ] {
        if !ok {
            return Err(format!("the runtime does not offer {name}"));
        }
    }
    let have_source_ext = avail.ext_hand_tracking_data_source;
    let mut exts = xr::ExtensionSet::default();
    exts.mnd_headless = true;
    exts.ext_hand_tracking = true;
    exts.khr_convert_timespec_time = true;
    exts.ext_hand_tracking_data_source = have_source_ext;

    let app = xr::ApplicationInfo {
        application_name: "xr-handtest",
        application_version: 1,
        engine_name: "pimax-xrtest",
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
    let props = instance
        .properties()
        .map_err(|e| format!("instance properties: {e}"))?;
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .map_err(|e| format!("no HMD system: {e}"))?;
    let supports = instance
        .supports_hand_tracking(system)
        .map_err(|e| format!("system hand tracking: {e}"))?;
    println!(
        "runtime {} {} | hand tracking supported by the system: {} | data source ext: {}",
        props.runtime_name, props.runtime_version, supports, have_source_ext
    );
    if !supports {
        return Err("the system reports supportsHandTracking = false".into());
    }

    // SAFETY: headless sessions need no graphics objects.
    let (session, _waiter, _stream) = unsafe {
        instance.create_session::<xr::Headless>(system, &xr::headless::SessionCreateInfo {})
    }
    .map_err(|e| format!("xrCreateSession (headless): {e}"))?;

    // Grip pose action (Oculus Touch profile) so the hand can be compared with the controller's grip pose.
    let mut grip_action: Option<(xr::ActionSet, xr::Action<xr::Posef>, [xr::Path; 2])> = None;
    if args.grip_check {
        let setup = || -> xr::Result<(xr::ActionSet, xr::Action<xr::Posef>, [xr::Path; 2])> {
            let paths = [
                instance.string_to_path("/user/hand/left")?,
                instance.string_to_path("/user/hand/right")?,
            ];
            let set = instance.create_action_set("handtest", "handtest", 0)?;
            let grip = set.create_action::<xr::Posef>("grip", "grip pose", &paths)?;
            let profile =
                instance.string_to_path("/interaction_profiles/oculus/touch_controller")?;
            let l = instance.string_to_path("/user/hand/left/input/grip/pose")?;
            let r = instance.string_to_path("/user/hand/right/input/grip/pose")?;
            instance.suggest_interaction_profile_bindings(
                profile,
                &[xr::Binding::new(&grip, l), xr::Binding::new(&grip, r)],
            )?;
            session.attach_action_sets(&[&set])?;
            Ok((set, grip, paths))
        };
        match setup() {
            Ok(g) => grip_action = Some(g),
            Err(e) => eprintln!("grip pose action unavailable ({e}); grip check off"),
        }
    }

    let local = session
        .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
        .map_err(|e| format!("local space: {e}"))?;

    let mut hands: Vec<HandCtx> = vec![];
    for (name, hand, xh, idx) in [
        ("L", Hand::Left, xr::Hand::LEFT, 0usize),
        ("R", Hand::Right, xr::Hand::RIGHT, 1usize),
    ] {
        let tracker = create_tracker(&session, xh, args.controller_source && have_source_ext)
            .map_err(|e| format!("xrCreateHandTrackerEXT({name}): {e:?}"))?;
        let grip_space = match &grip_action {
            Some((_, action, paths)) => action
                .create_space(&session, paths[idx], xr::Posef::IDENTITY)
                .ok(),
            None => None,
        };
        hands.push(HandCtx {
            name,
            hand,
            tracker,
            grip_space,
            stats: Stats::default(),
        });
    }

    let tol = Tolerances::default();
    let start = Instant::now();
    let sample_period = Duration::from_millis(50);
    let print_period = 1.0 / args.print_hz;
    let mut next_print = 0.0;
    let mut running = false;
    let mut began = false;
    let mut focused = false;
    let mut state_name = String::from("IDLE");
    let mut ev = xr::EventDataBuffer::new();
    let mut stopping = false;

    println!(
        "sampling {:.0} s, printing {:.1} Hz; hands: L R; grip check: {}; demo check: {}",
        args.seconds,
        args.print_hz,
        if grip_action.is_some() { "on" } else { "off" },
        args.demo
    );

    'main: loop {
        let loop_start = Instant::now();
        while let Some(e) = instance
            .poll_event(&mut ev)
            .map_err(|e| format!("poll event: {e}"))?
        {
            if let xr::Event::SessionStateChanged(s) = e {
                state_name = format!("{:?}", s.state());
                println!("session state -> {state_name}");
                match s.state() {
                    xr::SessionState::READY if !running => {
                        session
                            .begin(xr::ViewConfigurationType::PRIMARY_STEREO)
                            .map_err(|e| format!("xrBeginSession: {e}"))?;
                        running = true;
                        began = true;
                    }
                    xr::SessionState::FOCUSED => focused = true,
                    xr::SessionState::STOPPING => {
                        session.end().map_err(|e| format!("xrEndSession: {e}"))?;
                        running = false;
                        break 'main;
                    }
                    xr::SessionState::EXITING | xr::SessionState::LOSS_PENDING => break 'main,
                    _ => {}
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        if elapsed >= args.seconds {
            break;
        }

        if running {
            if let Some((set, _, _)) = &grip_action {
                // NOT_FOCUSED early on is expected; the grip locate below simply reports nothing then.
                let _ = session.sync_actions(&[xr::ActiveActionSet::new(set)]);
            }
            for h in hands.iter_mut() {
                let t_ns = mono_ns();
                let time = instance
                    .now()
                    .map_err(|e| format!("xrConvertTimespecTimeToTime: {e}"))?;
                let loc = match locate_hand(&session, &h.tracker, &local, time, have_source_ext) {
                    Ok(l) => l,
                    Err(r) => {
                        eprintln!("xrLocateHandJointsEXT({}) failed: {r:?}", h.name);
                        continue;
                    }
                };
                sample_hand(h, &loc, time, t_ns, &local, &args);
            }

            if elapsed >= next_print {
                next_print += print_period;
                if !args.quiet {
                    for h in &hands {
                        print_line(elapsed, h, &state_name, focused);
                    }
                }
            }
        }
        if let Some(rest) = sample_period.checked_sub(loop_start.elapsed()) {
            std::thread::sleep(rest);
        }
    }

    if !began {
        return Err(format!(
            "the session never reached READY (last state {state_name}); is a service running?"
        ));
    }

    // Leave cleanly: ask the runtime to stop the session and wait for STOPPING.
    if running {
        let _ = session.request_exit();
        let until = Instant::now() + Duration::from_secs(3);
        while !stopping && Instant::now() < until {
            while let Ok(Some(e)) = instance.poll_event(&mut ev) {
                if let xr::Event::SessionStateChanged(s) = e {
                    if s.state() == xr::SessionState::STOPPING {
                        let _ = session.end();
                        stopping = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    Ok(summarize(
        &args,
        &tol,
        &mut hands,
        grip_action.is_some(),
        focused,
    ))
}

/// Record one located sample of a hand.
fn sample_hand(
    h: &mut HandCtx,
    loc: &Located,
    time: xr::Time,
    t_ns: u64,
    local: &xr::Space,
    args: &Args,
) {
    let s = &mut h.stats;
    s.samples += 1;
    if loc.active {
        s.active += 1;
    }
    let valid_joints = loc
        .loc
        .iter()
        .filter(|j| {
            j.location_flags.contains(
                xr::SpaceLocationFlags::POSITION_VALID | xr::SpaceLocationFlags::ORIENTATION_VALID,
            )
        })
        .count();
    let valid = loc.active && valid_joints == JOINT_COUNT;
    let src = match loc.source {
        None => "n/a".to_string(),
        Some((false, _)) => "inactive".to_string(),
        Some((true, d)) => source_name(d).to_string(),
    };
    let mut last = LastSample {
        active: loc.active,
        valid_joints,
        src,
        wrist: None,
        curls: None,
        grip_err: None,
    };

    if valid {
        s.valid += 1;
        if loc.loc.iter().all(|j| {
            j.location_flags
                .contains(xr::SpaceLocationFlags::POSITION_TRACKED)
        }) {
            s.pos_tracked_all += 1;
        }
        if loc.loc.iter().all(|j| {
            j.location_flags
                .contains(xr::SpaceLocationFlags::ORIENTATION_TRACKED)
        }) {
            s.ori_tracked_all += 1;
        }
        match loc.source {
            Some((true, d)) if d == sys::HandTrackingDataSourceEXT::CONTROLLER => {
                s.src_controller += 1
            }
            Some((true, _)) => s.src_other += 1,
            _ => s.src_missing += 1,
        }

        let mut joints: Joints = [Joint::default(); JOINT_COUNT];
        for (j, l) in joints.iter_mut().zip(loc.loc.iter()) {
            let p = pose_of(&l.pose);
            *j = Joint { p: p.p, q: p.q };
        }
        last.wrist = Some(loc.loc[WRIST].pose);
        last.curls = Some(curl_estimates(&joints));
        if args.demo {
            let mut fl = [0.0; 5];
            for (k, f) in Finger::ALL.iter().enumerate() {
                fl[k] = flexion(&joints, *f);
            }
            s.demo.push(DemoSample { t_ns, flexion: fl });
        }
        if let Some(gs) = &h.grip_space {
            match gs.locate(local, time) {
                Ok(g)
                    if g.location_flags.contains(
                        xr::SpaceLocationFlags::POSITION_VALID
                            | xr::SpaceLocationFlags::ORIENTATION_VALID,
                    ) =>
                {
                    let grip = pose_of(&g.pose);
                    // 1. wrist joint == grip pose * grip->wrist offset (what the driver composes)
                    let want_wrist = grip.compose(&grip_to_wrist(h.hand));
                    let (dp, da) = pose_error(&joints[WRIST].pose(), &want_wrist);
                    s.grip_pos.add(dp);
                    s.grip_ang.add(da);
                    // 2. grip rebuilt from the joints with Monado's ht_ctrl_emu rule == the controller's grip pose
                    let (dp2, da2) = pose_error(&grip_from_joint_set(&joints, h.hand), &grip);
                    s.derived_pos.add(dp2);
                    s.derived_ang.add(da2);
                    last.grip_err = Some((dp, da));
                }
                _ => s.grip_unavailable += 1,
            }
        }
    }
    s.last = Some(last);
}

fn print_line(t: f64, h: &HandCtx, state: &str, focused: bool) {
    let Some(l) = &h.stats.last else { return };
    let mut line = format!(
        "t={t:5.1} {} {} joints {:2}/{} src={:<10}",
        h.name,
        if l.active { "active  " } else { "inactive" },
        l.valid_joints,
        JOINT_COUNT,
        l.src
    );
    if let Some(w) = &l.wrist {
        line += &format!(
            " | wrist ({:+.3} {:+.3} {:+.3}) q({:+.2} {:+.2} {:+.2} {:+.2})",
            w.position.x,
            w.position.y,
            w.position.z,
            w.orientation.x,
            w.orientation.y,
            w.orientation.z,
            w.orientation.w
        );
    } else {
        line += " | wrist n/a";
    }
    if let Some(c) = &l.curls {
        line += &format!(
            " | curl L{:.2} R{:.2} M{:.2} I{:.2} T{:.2}",
            c[0], c[1], c[2], c[3], c[4]
        );
    }
    if let Some((dp, da)) = l.grip_err {
        line += &format!(
            " | grip err {:.1} mm {:.2} deg",
            dp * 1000.0,
            da.to_degrees()
        );
    }
    if h.name == "L" {
        line += &format!(
            " [session {state}{}]",
            if focused { "" } else { ", not focused" }
        );
    }
    println!("{line}");
}

fn summarize(
    args: &Args,
    tol: &Tolerances,
    hands: &mut [HandCtx],
    grip_enabled: bool,
    focused: bool,
) -> ExitCode {
    println!("\n== summary ==");
    let mut hand_pass = [false; 2];
    let mut hand_fail_reason: [Vec<String>; 2] = [vec![], vec![]];
    for (i, h) in hands.iter().enumerate() {
        let s = &h.stats;
        let frac = if s.samples > 0 {
            f64::from(s.valid) / f64::from(s.samples)
        } else {
            0.0
        };
        println!(
            "{}: {} samples, active {}, valid joints {} ({:.0} %), pos tracked {}, ori tracked {}, source CONTROLLER {} other {} missing {}",
            h.name, s.samples, s.active, s.valid, frac * 100.0, s.pos_tracked_all, s.ori_tracked_all, s.src_controller, s.src_other, s.src_missing
        );
        let mut ok = s.samples > 0 && frac >= args.min_valid;
        if !ok {
            hand_fail_reason[i].push(format!(
                "only {:.0} % of the samples had valid joints (need {:.0} %)",
                frac * 100.0,
                args.min_valid * 100.0
            ));
        }
        // (missing = the data source extension is not there, then nothing is claimed)
        if ok && args.controller_source && s.src_other > 0 {
            ok = false;
            hand_fail_reason[i].push(format!(
                "{} samples came from a data source other than CONTROLLER",
                s.src_other
            ));
        }
        if ok && grip_enabled {
            if s.grip_pos.count > 0 {
                println!(
                    "   grip check: {} samples, wrist vs grip*offset max {:.2} mm / {:.2} deg; grip from joints vs real grip max {:.2} mm / {:.2} deg",
                    s.grip_pos.count,
                    s.grip_pos.max * 1000.0,
                    s.grip_ang.max.to_degrees(),
                    s.derived_pos.max * 1000.0,
                    s.derived_ang.max.to_degrees()
                );
                for (what, v, lim, unit, scale) in [
                    (
                        "wrist vs grip*offset position",
                        s.grip_pos.max,
                        tol.grip_pos_m,
                        "mm",
                        1000.0,
                    ),
                    (
                        "wrist vs grip*offset rotation",
                        s.grip_ang.max,
                        tol.grip_ang_rad,
                        "deg",
                        180.0 / std::f64::consts::PI,
                    ),
                    (
                        "joint-derived grip position",
                        s.derived_pos.max,
                        tol.grip_pos_m,
                        "mm",
                        1000.0,
                    ),
                    (
                        "joint-derived grip rotation",
                        s.derived_ang.max,
                        tol.grip_ang_rad,
                        "deg",
                        180.0 / std::f64::consts::PI,
                    ),
                ] {
                    if v > lim {
                        ok = false;
                        hand_fail_reason[i].push(format!(
                            "{what} off by {:.2} {unit} (limit {:.2})",
                            v * scale,
                            lim * scale
                        ));
                    }
                }
            } else {
                println!(
                    "   grip check: grip pose never located ({} tries, session focused: {focused})",
                    s.grip_unavailable
                );
                if args.require_grip {
                    ok = false;
                    hand_fail_reason[i]
                        .push("grip pose could not be located (--require-grip)".into());
                }
            }
        }
        if ok && args.demo {
            let rep = check_demo(&s.demo, h.hand == Hand::Right, tol);
            let errs: Vec<String> = Finger::ALL
                .iter()
                .enumerate()
                .map(|(k, f)| {
                    format!(
                        "{} {:.2}/{:.2}",
                        f.short(),
                        rep.max_err[k],
                        rep.measured_range[k]
                    )
                })
                .collect();
            println!(
                "   demo check: {} samples, max flexion error / measured range (rad): {}",
                rep.samples,
                errs.join("  ")
            );
            if !rep.ok() {
                ok = false;
                hand_fail_reason[i].extend(rep.failures);
            }
        }
        hand_pass[i] = ok;
        for r in &hand_fail_reason[i] {
            println!("   FAIL: {r}");
        }
    }
    let pass = match args.require {
        Require::Any => hand_pass[0] || hand_pass[1],
        Require::Both => hand_pass[0] && hand_pass[1],
        Require::Left => hand_pass[0],
        Require::Right => hand_pass[1],
    };
    // A hand that delivered valid joints but failed a check is a failure whatever --require says.
    let broken = hands.iter().enumerate().any(|(i, h)| {
        h.stats.valid > 0
            && !hand_pass[i]
            && !hand_fail_reason[i].is_empty()
            && h.stats.valid as f64 >= 0.5 * f64::from(h.stats.samples)
    });
    let pass = pass && !broken;
    println!("RESULT: {}", if pass { "PASS" } else { "FAIL" });
    if pass {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(code) => code,
        Err(e) => {
            eprintln!("xr-handtest: {e}");
            ExitCode::from(2)
        }
    }
}
