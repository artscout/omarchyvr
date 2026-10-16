//! xr-gaze: headless OpenXR check of XR_EXT_eye_gaze_interaction on the Pimax Monado driver.
//!
//! Enables the extension, binds a pose action to /user/eyes_ext/input/gaze_ext/pose on
//! /interaction_profiles/ext/eye_gaze_interaction, and prints the gaze pose at 10 Hz for N seconds: located in VIEW
//! space (= in the head frame, the number to check the axis conventions with) and in LOCAL space. Draws nothing: the
//! session is headless (XR_MND_headless).
//!
//! The service must run with PIMAX_EYE=1. The EyeChip is opened when this client creates the gaze space and released
//! when it exits, so the first lines show "inactive" for a second or two (init sequence, ~2 s).
//!
//! What to look for (frame check, all in the VIEW-space columns, OpenXR axes x right, y up, -z forward):
//!   look straight ahead  -> dir ~ (0, 0, -1)
//!   look to your right   -> dir.x > 0       (if negative: PIMAX_EYE_FLIP_X=1 in the service environment)
//!   look up              -> dir.y > 0       (if negative: PIMAX_EYE_FLIP_Y=1)
//!   look far away        -> dir.z < 0       (if positive: PIMAX_EYE_FLIP_Z=0)
//!
//! Exit status: 0 gaze seen, 1 session ran but the gaze was never valid, 2 could not run.

use openxr as xr;
use openxr::sys;
use std::ffi::c_void;
use std::process::ExitCode;
use std::time::{Duration, Instant};

const HELP: &str = "xr-gaze [--seconds N] [--hz F] [--wait-runtime N]
  --seconds N        run time (default 30)
  --hz F             lines per second (default 10)
  --wait-runtime N   keep trying to reach the runtime for N seconds (default 15)
Run the service with PIMAX_EYE=1. Exit status: 0 gaze seen, 1 never valid, 2 setup problem.";

struct Args {
    seconds: f64,
    hz: f64,
    wait_runtime: f64,
}

fn parse_args() -> Result<Option<Args>, String> {
    let mut a = Args {
        seconds: 30.0,
        hz: 10.0,
        wait_runtime: 15.0,
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
            "--hz" => a.hz = num("--hz")?.clamp(0.1, 100.0),
            "--wait-runtime" => a.wait_runtime = num("--wait-runtime")?,
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            other => return Err(format!("unknown argument '{other}'\n{HELP}")),
        }
    }
    Ok(Some(a))
}

/// Direction of the pose's -Z axis (OpenXR "forward").
fn forward(q: xr::Quaternionf) -> [f32; 3] {
    // rotate (0,0,-1) by q: v' = v + 2w(u x v) + 2 u x (u x v)
    let (u, w) = ([q.x, q.y, q.z], q.w);
    let v = [0.0, 0.0, -1.0];
    let cross = |a: [f32; 3], b: [f32; 3]| {
        [
            a[1] * b[2] - a[2] * b[1],
            a[2] * b[0] - a[0] * b[2],
            a[0] * b[1] - a[1] * b[0],
        ]
    };
    let t = cross(u, v);
    let t2 = cross(u, t);
    [
        v[0] + 2.0 * (w * t[0] + t2[0]),
        v[1] + 2.0 * (w * t[1] + t2[1]),
        v[2] + 2.0 * (w * t[2] + t2[2]),
    ]
}

fn describe(loc: &xr::SpaceLocation) -> Option<String> {
    let f = loc.location_flags;
    if !f.contains(xr::SpaceLocationFlags::ORIENTATION_VALID) {
        return None;
    }
    let d = forward(loc.pose.orientation);
    let p = loc.pose.position;
    let yaw = d[0].atan2(-d[2]).to_degrees();
    let pitch = d[1].atan2(d[0].hypot(d[2])).to_degrees();
    Some(format!(
        "pos=({:+.4},{:+.4},{:+.4}) dir=({:+.3},{:+.3},{:+.3}) yaw={:+6.1} pitch={:+6.1}{}",
        p.x,
        p.y,
        p.z,
        d[0],
        d[1],
        d[2],
        yaw,
        pitch,
        if f.contains(xr::SpaceLocationFlags::POSITION_VALID) {
            ""
        } else {
            " (pos not valid)"
        }
    ))
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
        (
            "XR_EXT_eye_gaze_interaction",
            avail.ext_eye_gaze_interaction,
        ),
        (
            "XR_KHR_convert_timespec_time",
            avail.khr_convert_timespec_time,
        ),
    ] {
        if !ok {
            return Err(format!("the runtime does not offer {name}"));
        }
    }
    let mut exts = xr::ExtensionSet::default();
    exts.mnd_headless = true;
    exts.ext_eye_gaze_interaction = true;
    exts.khr_convert_timespec_time = true;
    let app = xr::ApplicationInfo {
        application_name: "xr-gaze",
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
    let props = instance
        .properties()
        .map_err(|e| format!("instance properties: {e}"))?;
    let system = instance
        .system(xr::FormFactor::HEAD_MOUNTED_DISPLAY)
        .map_err(|e| format!("no HMD system: {e}"))?;

    // supportsEyeGazeInteraction through the system properties chain
    let supports = {
        let mut gaze = sys::SystemEyeGazeInteractionPropertiesEXT {
            ty: sys::SystemEyeGazeInteractionPropertiesEXT::TYPE,
            next: std::ptr::null_mut(),
            supports_eye_gaze_interaction: sys::FALSE,
        };
        // SAFETY: plain C structs; `gaze` outlives the call.
        let mut p: sys::SystemProperties = unsafe { std::mem::zeroed() };
        p.ty = sys::SystemProperties::TYPE;
        p.next = (&mut gaze as *mut sys::SystemEyeGazeInteractionPropertiesEXT).cast::<c_void>();
        // SAFETY: valid instance, system and output struct.
        let r = unsafe { (instance.fp().get_system_properties)(instance.as_raw(), system, &mut p) };
        if r != sys::Result::SUCCESS {
            return Err(format!("xrGetSystemProperties: {r:?}"));
        }
        gaze.supports_eye_gaze_interaction != sys::FALSE
    };
    println!(
        "runtime {} {} | system supportsEyeGazeInteraction: {}",
        props.runtime_name, props.runtime_version, supports
    );
    if !supports {
        return Err(
            "the system reports supportsEyeGazeInteraction = false (service without PIMAX_EYE=1?)"
                .into(),
        );
    }

    // SAFETY: headless sessions need no graphics objects.
    let (session, _waiter, _stream) = unsafe {
        instance.create_session::<xr::Headless>(system, &xr::headless::SessionCreateInfo {})
    }
    .map_err(|e| format!("xrCreateSession (headless): {e}"))?;

    let set = instance
        .create_action_set("gaze", "gaze", 0)
        .map_err(|e| format!("action set: {e}"))?;
    let action = set
        .create_action::<xr::Posef>("gaze_pose", "gaze pose", &[])
        .map_err(|e| format!("action: {e}"))?;
    let profile = instance
        .string_to_path("/interaction_profiles/ext/eye_gaze_interaction")
        .map_err(|e| format!("profile path: {e}"))?;
    let pose_path = instance
        .string_to_path("/user/eyes_ext/input/gaze_ext/pose")
        .map_err(|e| format!("pose path: {e}"))?;
    instance
        .suggest_interaction_profile_bindings(profile, &[xr::Binding::new(&action, pose_path)])
        .map_err(|e| format!("suggest bindings: {e}"))?;
    session
        .attach_action_sets(&[&set])
        .map_err(|e| format!("attach: {e}"))?;
    let gaze_space = action
        .create_space(&session, xr::Path::NULL, xr::Posef::IDENTITY)
        .map_err(|e| format!("gaze space: {e}"))?;
    let view = session
        .create_reference_space(xr::ReferenceSpaceType::VIEW, xr::Posef::IDENTITY)
        .map_err(|e| format!("view space: {e}"))?;
    let local = session
        .create_reference_space(xr::ReferenceSpaceType::LOCAL, xr::Posef::IDENTITY)
        .map_err(|e| format!("local space: {e}"))?;

    let start = Instant::now();
    let period = Duration::from_secs_f64(1.0 / args.hz);
    let (mut running, mut began) = (false, false);
    let (mut lines, mut valid) = (0u64, 0u64);
    let mut ev = xr::EventDataBuffer::new();
    println!(
        "printing the gaze pose {:.0} times a second for {:.0} s (VIEW = head frame)",
        args.hz, args.seconds
    );
    'main: loop {
        let t_loop = Instant::now();
        while let Some(e) = instance
            .poll_event(&mut ev)
            .map_err(|e| format!("poll event: {e}"))?
        {
            if let xr::Event::SessionStateChanged(s) = e {
                println!("session state -> {:?}", s.state());
                match s.state() {
                    xr::SessionState::READY if !running => {
                        session
                            .begin(xr::ViewConfigurationType::PRIMARY_STEREO)
                            .map_err(|e| format!("xrBeginSession: {e}"))?;
                        running = true;
                        began = true;
                    }
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
            let _ = session.sync_actions(&[xr::ActiveActionSet::new(&set)]);
            let active = action.is_active(&session, xr::Path::NULL).unwrap_or(false);
            let time = instance
                .now()
                .map_err(|e| format!("xrConvertTimespecTimeToTime: {e}"))?;
            let in_view = gaze_space
                .locate(&view, time)
                .ok()
                .and_then(|l| describe(&l));
            let in_local = gaze_space
                .locate(&local, time)
                .ok()
                .and_then(|l| describe(&l));
            lines += 1;
            match (&in_view, &in_local) {
                (Some(v), l) => {
                    valid += 1;
                    println!(
                        "[{elapsed:6.2}] active={active} VIEW  {v}{}",
                        l.as_ref().map(|l| format!("\n         LOCAL {l}")).unwrap_or_default()
                    );
                }
                _ => println!("[{elapsed:6.2}] active={active} no valid gaze (EyeChip starting, eyes lost, or tracker not connected)"),
            }
        }
        if let Some(rest) = period.checked_sub(t_loop.elapsed()) {
            std::thread::sleep(rest);
        }
    }
    if !began {
        return Err("the session never reached READY; is a service running?".into());
    }
    if running {
        let _ = session.request_exit();
        let until = Instant::now() + Duration::from_secs(3);
        let mut ended = false;
        while !ended && Instant::now() < until {
            while let Ok(Some(e)) = instance.poll_event(&mut ev) {
                if let xr::Event::SessionStateChanged(s) = e {
                    if s.state() == xr::SessionState::STOPPING {
                        let _ = session.end();
                        ended = true;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
    println!("{valid} of {lines} samples had a valid gaze");
    Ok(if valid > 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    })
}

fn main() -> ExitCode {
    match run() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("xr-gaze: {e}");
            ExitCode::from(2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn identity_looks_down_minus_z_and_yaw_turns_right() {
        let f = forward(xr::Quaternionf {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            w: 1.0,
        });
        assert!((f[2] + 1.0).abs() < 1e-6 && f[0].abs() < 1e-6);
        // -90 degrees about +y (clockwise seen from above) turns -z toward +x: looking right
        let h = std::f32::consts::FRAC_1_SQRT_2;
        let f = forward(xr::Quaternionf {
            x: 0.0,
            y: -h,
            z: 0.0,
            w: h,
        });
        assert!((f[0] - 1.0).abs() < 1e-5, "{f:?}");
    }
}
