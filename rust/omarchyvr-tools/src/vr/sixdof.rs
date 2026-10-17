//! `--6dof` for `omarchyvr vr start` and `omarchyvr desk start`: live 6DoF head tracking (Monado SLAM / Basalt).
//!
//! Same recipe as `analysis/tools/run_slam.sh live` + `run_slam_view.sh` (minus the recording): `pimax-cam broker`
//! first (one camera stream shared by Monado and `omarchyvr-passthrough`), `omarchyvr-slam export` for the Basalt config,
//! and for monado-service `PIMAX_CAM_SOURCE=broker PIMAX_SLAM=1 PIMAX_CAM_AE=1 VIT_SYSTEM_LIBRARY_PATH SLAM_CONFIG`.
//! The broker is stopped by pid after Monado (Monado reads it until the end).
//!
//! `--ctrl-track` (shared by `vr start` and `desk start`, needs `--6dof`): `pimax-ctrltrack live --source broker:SOCK`
//! plus PIMAX_CTRL_TRACK=1 for Monado; the tracker is stopped by pid before Monado.
use super::config::{comm_of, StartOptions};
use super::desk::{stays_alive, stop_client, ClientStop, ProcSpec, Spawner};
use super::host::{Clock, Procs};
use super::session::{self, Deps};
use crate::args::Args;
use crate::{CliError, CliResult};
use std::path::{Path, PathBuf};
use omarchyvr_components::components as comp;
use std::time::Duration;

/// Name of the broker's pid file stem and log.
pub const BROKER: &str = comp::broker::NAME;

/// What `--6dof` needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SixDof {
    /// `pimax-cam` (runs `broker`).
    pub cam_bin: PathBuf,
    /// `omarchyvr-slam` (runs `export`).
    pub slam_bin: PathBuf,
    /// Directory the Basalt config is exported to.
    pub basalt_dir: PathBuf,
    /// Broker socket.
    pub sock: PathBuf,
}

impl SixDof {
    /// The Basalt config file `omarchyvr-slam export` writes.
    pub fn slam_config(&self) -> PathBuf {
        self.basalt_dir.join("pimax-basalt.toml")
    }

    /// The broker as a process to spawn.
    pub fn broker_spec(&self, session_dir: &Path) -> ProcSpec {
        comp::broker::spec(&self.cam_bin, session_dir)
    }

    /// The broker's parts for `omarchyvr_components::components::broker`.
    fn broker_params<'a>(&'a self, session_dir: &'a Path) -> comp::broker::Params<'a> {
        comp::broker::Params {
            cam_bin: &self.cam_bin,
            slam_bin: &self.slam_bin,
            basalt_dir: &self.basalt_dir,
            sock: &self.sock,
            session_dir,
        }
    }
}

/// Parse `--6dof`: returns the plan and pushes the Monado environment onto `vr.extra_env`. `eye_build` is the
/// default build (the merged one) used unless `--monado-build` / `PIMAX_MONADO_BUILD` was given.
pub fn parse(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
    home: &Path,
    build_given: bool,
    default_build: Option<PathBuf>,
) -> Option<SixDof> {
    if !a.flag("--6dof") {
        return None;
    }
    if !build_given {
        if let Some(b) = default_build {
            vr.monado_build = b;
        }
    }
    let rel = super::config::tools_dir(home);
    let runtime = env("XDG_RUNTIME_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/run/user/1000"));
    let s = SixDof {
        cam_bin: rel.join("pimax-cam"),
        slam_bin: rel.join("omarchyvr-slam"),
        basalt_dir: vr.session_dir.join("basalt"),
        sock: runtime.join("omarchyvr-cam-broker.sock"),
    };
    let set = |vr: &mut StartOptions, k: &str, v: String| {
        vr.extra_env.retain(|(n, _)| n != k);
        vr.extra_env.push((k.into(), v));
    };
    let or_env = |k: &str, d: &str| env(k).filter(|v| !v.is_empty()).unwrap_or_else(|| d.into());
    set(vr, "PIMAX_CAM_SOURCE", "broker".into());
    set(vr, "PIMAX_SLAM", "1".into());
    set(vr, "PIMAX_CAM_AE", or_env("PIMAX_CAM_AE", "1"));
    set(
        vr,
        "VIT_SYSTEM_LIBRARY_PATH",
        or_env(
            "VIT_SYSTEM_LIBRARY_PATH",
            &omarchyvr_config::paths::basalt_lib(home).to_string_lossy(),
        ),
    );
    set(
        vr,
        "SLAM_CONFIG",
        or_env("SLAM_CONFIG", &s.slam_config().to_string_lossy()),
    );
    if let Some(r) = env("PIMAX_SLAM_RECORD").filter(|v| !v.is_empty()) {
        set(vr, "PIMAX_SLAM_RECORD", r);
    }
    // head IMU + tracker pose times (first 10 min, and during a tracking take) so `Record tracking` takes can be
    // replayed through Basalt (`omarchyvr-slam take-euroc`); PIMAX_HEAD_IMU_LOG=0 turns it off
    match env("PIMAX_HEAD_IMU_LOG") {
        Some(v) if v == "0" => {}
        Some(v) if !v.is_empty() => set(vr, "PIMAX_HEAD_IMU_LOG", v),
        _ => {
            let p = vr.session_dir.join("head-imu.csv");
            set(vr, "PIMAX_HEAD_IMU_LOG", p.to_string_lossy().into_owned());
        }
    }
    Some(s)
}

/// True if the service binary contains the 6DoF code (the `PIMAX_SLAM` variable name; the plain `eye` build has none).
pub fn build_has_slam(service: &Path) -> Result<bool, String> {
    let data = std::fs::read(service).map_err(|e| format!("cannot read {}: {e}", service.display()))?;
    let needle = b"PIMAX_SLAM\0";
    Ok(data.windows(needle.len()).any(|w| w == needle))
}

/// Checks that need no side effects: the build has the SLAM code, the helper binaries and Basalt exist.
pub fn check(o: &StartOptions, s: &SixDof, env: &dyn Fn(&str) -> Option<String>) -> CliResult {
    let svc = o.service_path();
    match build_has_slam(&svc) {
        Ok(true) => {}
        Ok(false) => {
            return Err(CliError::new(format!(
                "--6dof needs a Monado build with the 6DoF code (branch desk or slam); {} has none (no PIMAX_SLAM in it). \
                 Use --monado-build with the desk build (~/.local/lib/omarchyvr/monado once installed) after merging, or drop --6dof",
                svc.display()
            )))
        }
        Err(e) => return Err(CliError::new(e)),
    }
    for (bin, hint) in [
        (&s.cam_bin, "cargo build --release -p pimax-cam"),
        (&s.slam_bin, "cargo build --release -p omarchyvr-slam"),
    ] {
        if !bin.exists() {
            return Err(CliError::new(format!(
                "{} does not exist (run ./install.sh again in your omarchyvr clone, or: cd ~/omarchyvr/rust && {hint})",
                bin.display()
            )));
        }
    }
    let vit = env("VIT_SYSTEM_LIBRARY_PATH")
        .filter(|v| !v.is_empty())
        .or_else(|| {
            o.extra_env
                .iter()
                .find(|(k, _)| k == "VIT_SYSTEM_LIBRARY_PATH")
                .map(|(_, v)| v.clone())
        })
        .unwrap_or_default();
    if !Path::new(&vit).exists() {
        return Err(CliError::new(format!(
            "--6dof needs Basalt: {vit} does not exist (build libbasalt, see analysis/research/6dof-head-tracking.md)"
        )));
    }
    Ok(())
}

/// Export the Basalt config, then start the broker and wait for its socket. Returns the broker pid.
pub fn start_broker(
    o: &StartOptions,
    s: &SixDof,
    d: &Deps,
    sp: &dyn Spawner,
    settle: Duration,
) -> Result<u32, CliError> {
    comp::broker::start(&s.broker_params(&o.session_dir), sp, d.procs, d.clock, settle, &mut broker_say).map_err(CliError::new)
}

/// The launcher's output for the broker's progress and problems.
fn broker_say(problem: bool, m: &str) {
    if problem {
        verr!("omarchyvr: {m}");
    } else {
        vout!("omarchyvr: {m}");
    }
}

/// SIGINT the broker by pid (from its pid file), if there is one.
pub fn stop_broker(session_dir: &Path, cam_bin: &Path, procs: &dyn Procs, clock: &dyn Clock) {
    comp::broker::stop(session_dir, cam_bin, procs, clock, &mut broker_say)
}

/// Pid file stem and log name of the controller tracker.
pub const CTRLTRACK: &str = comp::ctrltrack::NAME;

/// Default `pimax-ctrltrack` binary.
pub fn default_ctrltrack_bin(home: &Path) -> PathBuf {
    super::config::tools_dir(home).join("pimax-ctrltrack")
}

/// Parse `--ctrl-track` and `--ctrltrack-bin` (call after [`parse`]). Returns the tracker binary when it is enabled,
/// and pushes PIMAX_CTRL_TRACK / PIMAX_CTRL_IMU_LOG onto `vr.extra_env` (the caller's own PIMAX_CTRL_TRACK wins).
/// Errors without `--6dof`: the tracker reads the LED frames from the camera broker.
pub fn parse_ctrl_track(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
    home: &Path,
    sixdof: &Option<SixDof>,
) -> Result<Option<PathBuf>, CliError> {
    let on = a.flag("--ctrl-track");
    let bin = a.opt("--ctrltrack-bin")?.map(PathBuf::from).unwrap_or_else(|| default_ctrltrack_bin(home));
    if !on {
        return Ok(None);
    }
    if sixdof.is_none() {
        return Err(CliError::usage(
            "--ctrl-track needs --6dof (the controller tracker reads the LED frames from the camera broker)",
        ));
    }
    if env("PIMAX_CTRL_TRACK").is_none() {
        vr.extra_env.push(("PIMAX_CTRL_TRACK".into(), "1".into()));
    }
    // gyro + optical log (first 10 min) for `pimax-ctrltrack imufit`; the online fit needs no log
    if env("PIMAX_CTRL_IMU_LOG").is_none() {
        let prefix = vr.session_dir.join("ctrl-imu");
        vr.extra_env.push(("PIMAX_CTRL_IMU_LOG".into(), prefix.to_string_lossy().into_owned()));
    }
    Ok(Some(bin))
}

/// `--controllers when-held|always|off` (docs/CONTROLLERS-INHAND.md): when Monado shows the controllers. Pushes
/// PIMAX_CTRL_INHAND (auto | always | off) onto `vr.extra_env` unless the caller's environment sets it. `default`:
/// when-held for `desk start` (Work: a controller lying on the desk is hidden), always for `vr start` (games).
pub fn parse_controllers(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
    default: omarchyvr_config::ctrl_inhand::CtrlMode,
) -> Result<omarchyvr_config::ctrl_inhand::CtrlMode, CliError> {
    use omarchyvr_config::ctrl_inhand::CtrlMode;
    let mode = match a.opt("--controllers")? {
        Some(v) => CtrlMode::parse(&v)
            .ok_or_else(|| CliError::usage(format!("--controllers {v}: want when-held, always or off")))?,
        None => default,
    };
    if env("PIMAX_CTRL_INHAND").is_none() {
        vr.extra_env.push(("PIMAX_CTRL_INHAND".into(), mode.env_value().into()));
    }
    Ok(mode)
}

/// `--ctrl-angle DEG` (-20..70, default 30; docs/CONTROLLERS-INHAND.md): the controller angle. Pushes `PIMAX_CTRL_ANGLE`
/// onto `vr.extra_env` unless the caller's environment sets it. The argv is recorded in start-argv.txt, so a restart
/// keeps it; the running session can be changed through `omarchyvr_config::ctrl_angle::write_live`.
pub fn parse_ctrl_angle(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
    vr: &mut StartOptions,
) -> Result<i32, CliError> {
    use omarchyvr_config::ctrl_angle;
    let deg = match a.opt("--ctrl-angle")? {
        Some(v) => ctrl_angle::parse(&v).map_err(|e| CliError::usage(format!("--ctrl-angle: {e}")))?,
        // no flag: the saved setting (setup.toml of the active profile, the app's slider), else the default
        None => ctrl_angle::saved(&omarchyvr_config::Store::from_env(env)).unwrap_or(ctrl_angle::DEFAULT_DEG),
    };
    if env("PIMAX_CTRL_ANGLE").is_none() {
        vr.extra_env.push(("PIMAX_CTRL_ANGLE".into(), deg.to_string()));
    }
    Ok(deg)
}

/// The start value of the controller angle as Monado gets it (the launcher's `PIMAX_CTRL_ANGLE`, else the caller's).
pub fn start_ctrl_angle(vr: &StartOptions, env: &dyn Fn(&str) -> Option<String>) -> Option<i32> {
    let v = vr.extra_env.iter().find(|(k, _)| k == "PIMAX_CTRL_ANGLE").map(|(_, v)| v.clone()).or_else(|| env("PIMAX_CTRL_ANGLE"))?;
    omarchyvr_config::ctrl_angle::parse(&v).ok()
}

/// At a real start (never in a dry run): the live file `$XDG_RUNTIME_DIR/pimax-ctrl-angle` gets the start value. The
/// driver polls that file all session, so a file left by an earlier session would otherwise override this start's
/// `PIMAX_CTRL_ANGLE` within 100 ms.
pub fn sync_ctrl_angle_live(vr: &StartOptions, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let deg = start_ctrl_angle(vr, env)?;
    let rt = env("XDG_RUNTIME_DIR").filter(|v| !v.is_empty())?;
    omarchyvr_config::ctrl_angle::write_live(Path::new(&rt), deg).err()
}

/// A live change of the controller angle (the app's slider, `omarchyvr controllers angle DEG`): the driver's live file
/// and the recorded start (start-argv.txt, so a restart keeps it). Works without a session too (then only the files).
pub fn apply_ctrl_angle_live(runtime_dir: &Path, session_dir: &Path, deg: i32) -> Result<(), String> {
    use omarchyvr_config::ctrl_angle;
    ctrl_angle::write_live(runtime_dir, deg)?;
    if let Some((kind, args)) = super::supervise::read_argv(session_dir) {
        let new = ctrl_angle::argv_with_angle(&args, deg);
        if new != args {
            super::supervise::rewrite_argv(session_dir, &kind, &new).map_err(|e| format!("cannot update the recorded start: {e}"))?;
        }
    }
    Ok(())
}

/// The tracking kill switches of plan phase 1 (docs/TRACKING-STABILITY.md), all ON by default; the app passes the
/// flag of a switch that is off, and start-argv keeps it:
/// * `--old-exposure`: `PIMAX_CAM_AE_LAW=old`, the previous camera exposure law (1a).
/// * `--no-room-glide`: `PIMAX_ANCHOR_GLIDE=0`, room corrections snap again instead of gliding (1b).
/// * `--no-ctrl-steady`: `PIMAX_CTRL_TRACK_HYST=0`, the controller fuser's old 80 ms optical window (1d).
///
/// Without a flag the saved setting applies (setup.toml). The caller's own environment wins (a variable already set
/// is left alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrackingSwitches {
    pub new_exposure: bool,
    pub room_glide: bool,
    pub ctrl_steady: bool,
}

/// The three switches' flags, with their Monado environment variable and the value that turns the feature off.
pub const TRACKING_FLAGS: [(&str, &str, &str); 3] = [
    ("--old-exposure", "PIMAX_CAM_AE_LAW", "old"),
    ("--no-room-glide", "PIMAX_ANCHOR_GLIDE", "0"),
    ("--no-ctrl-steady", "PIMAX_CTRL_TRACK_HYST", "0"),
];

/// Parse [`TRACKING_FLAGS`] and push the kill-switch variables onto `vr.extra_env`. Without its flag a switch takes
/// the saved setting (setup.toml of the active profile, `omarchyvr_config::tracking`; the app's switches), else on.
pub fn parse_tracking_switches(a: &mut Args, env: &dyn Fn(&str) -> Option<String>, vr: &mut StartOptions) -> TrackingSwitches {
    let saved = omarchyvr_config::tracking::saved(&omarchyvr_config::Store::from_env(env));
    let mut off = [false; 3];
    for (i, (flag, var, val)) in TRACKING_FLAGS.iter().enumerate() {
        off[i] = a.flag(flag) || saved[i] == Some(false);
        if off[i] && env(var).is_none() {
            vr.extra_env.push((var.to_string(), val.to_string()));
        }
    }
    TrackingSwitches { new_exposure: !off[0], room_glide: !off[1], ctrl_steady: !off[2] }
}

/// The controller tracker as a process to spawn (not an XR client: reads the broker, writes the pose file Monado reads).
pub fn ctrltrack_spec(bin: &Path, s: &SixDof, session_dir: &Path) -> ProcSpec {
    comp::ctrltrack::spec(bin, &s.sock, session_dir)
}

/// `vr start --ctrl-track`, after the service is up: start the tracker. Failures only warn (the session itself does
/// not depend on it; Monado falls back to the rest pose).
pub fn vr_start_ctrltrack(bin: &Path, s: &SixDof, o: &StartOptions, d: &Deps, sp: &dyn Spawner) {
    let spec = ctrltrack_spec(bin, s, &o.session_dir);
    if !spec.program.exists() {
        verr!("omarchyvr vr: --ctrl-track: {} does not exist (run ./install.sh again in your omarchyvr clone (or: cd ~/omarchyvr/rust && cargo build --release -p pimax-ctrltrack))", spec.program.display());
        return;
    }
    match sp.spawn(&spec) {
        Ok(pid) => {
            let pf = o.session_dir.join("ctrltrack.pid");
            let _ = std::fs::write(&pf, format!("{pid}\n"));
            if stays_alive(pid, d.procs, d.clock, Duration::from_secs(2)) {
                vout!("omarchyvr vr: --ctrl-track: ctrltrack running, pid {pid}, log {}", spec.log.display());
            } else {
                let _ = std::fs::remove_file(&pf);
                verr!("omarchyvr vr: --ctrl-track: ctrltrack exited right after the start, see {}", spec.log.display());
            }
        }
        Err(e) => verr!("omarchyvr vr: --ctrl-track: cannot start {}: {e}", spec.program.display()),
    }
}

/// `vr stop`: SIGINT the tracker by pid (from its pid file), if there is one. Goes before the service.
pub fn stop_ctrltrack(session_dir: &Path, bin: &Path, procs: &dyn Procs, clock: &dyn Clock) {
    let pf = session_dir.join("ctrltrack.pid");
    if !pf.exists() {
        return;
    }
    match stop_client(CTRLTRACK, &pf, &comm_of(bin), procs, clock, Duration::from_secs(10), &mut |m| vout!("omarchyvr vr: {m}")) {
        ClientStop::StillAlive => verr!("omarchyvr vr: WARNING: ctrltrack is still running after SIGINT (pid file {})", pf.display()),
        ClientStop::CannotSignal(m) => verr!("omarchyvr vr: {m}"),
        ClientStop::NotRunning | ClientStop::Exited => {
            let _ = std::fs::remove_file(&pf);
        }
    }
}

/// `vr start --6dof`: checks, broker, then the service (stops the broker again if the service does not start).
/// With `ctrl_track` the tracker starts once the service is up (not with --wait, which stays in the foreground).
pub fn vr_start(o: &StartOptions, s: &SixDof, ctrl_track: Option<&Path>, d: &Deps, sp: &dyn Spawner) -> CliResult {
    check(o, s, d.env)?;
    if o.dry_run {
        vout!("omarchyvr vr: --6dof would first run: {} export --out {}, then {} broker (socket {})",
            s.slam_bin.display(), s.basalt_dir.display(), s.cam_bin.display(), s.sock.display());
        if let Some(b) = ctrl_track {
            let c = ctrltrack_spec(b, s, &o.session_dir);
            vout!("omarchyvr vr: --ctrl-track would also run, after the service: {} {}", c.program.display(), c.args.join(" "));
        }
        return session::start(o, d);
    }
    start_broker(o, s, d, sp, Duration::from_secs(10))?;
    let r = session::start(o, d);
    if r.is_err() {
        stop_broker(&o.session_dir, &s.cam_bin, d.procs, d.clock);
    } else if let Some(b) = ctrl_track {
        if o.wait {
            verr!("omarchyvr vr: --ctrl-track is ignored with --wait (the launcher stays in the foreground)");
        } else {
            vr_start_ctrltrack(b, s, o, d, sp);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::parse_options;

    fn go(args: &[&str], env: &[(&str, &str)]) -> (StartOptions, Option<SixDof>) {
        let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let lookup = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut a = Args(args.iter().map(|s| s.to_string()).collect());
        let mut vr = parse_options(&mut a, &lookup).unwrap();
        let s = parse(&mut a, &lookup, &mut vr, Path::new("/home/u"), false, Some("/home/u/.local/lib/omarchyvr/monado".into()));
        (vr, s)
    }

    fn get<'a>(vr: &'a StartOptions, k: &str) -> Option<&'a str> {
        vr.extra_env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn ctrl_angle_env() {
        let run = |args: &[&str], env: &[(&str, &str)]| {
            let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let lookup = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let mut a = Args(args.iter().map(|s| s.to_string()).collect());
            let mut vr = parse_options(&mut a, &lookup).unwrap();
            let r = parse_ctrl_angle(&mut a, &lookup, &mut vr);
            (vr, r)
        };
        let (vr, r) = run(&[], &[]);
        assert_eq!(r.unwrap(), 30);
        assert_eq!(get(&vr, "PIMAX_CTRL_ANGLE"), Some("30"));
        let (vr, _) = run(&["--ctrl-angle", "-20"], &[]);
        assert_eq!(get(&vr, "PIMAX_CTRL_ANGLE"), Some("-20"));
        let (vr, _) = run(&["--ctrl-angle=70"], &[]);
        assert_eq!(get(&vr, "PIMAX_CTRL_ANGLE"), Some("70"));
        let (vr, _) = run(&["--ctrl-angle", "10"], &[("PIMAX_CTRL_ANGLE", "5")]);
        assert_eq!(get(&vr, "PIMAX_CTRL_ANGLE"), None, "the caller's own wins");
        assert!(run(&["--ctrl-angle", "90"], &[]).1.is_err());
    }

    #[test]
    fn tracking_switches_default_on() {
        let run = |args: &[&str], env: &[(&str, &str)]| {
            let env: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
            let lookup = move |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
            let mut a = Args(args.iter().map(|s| s.to_string()).collect());
            let mut vr = parse_options(&mut a, &lookup).unwrap();
            let t = parse_tracking_switches(&mut a, &lookup, &mut vr);
            assert!(a.0.is_empty(), "every flag taken: {:?}", a.0);
            (vr, t)
        };
        let (vr, t) = run(&[], &[]);
        assert_eq!(t, TrackingSwitches { new_exposure: true, room_glide: true, ctrl_steady: true });
        for (_, var, _) in TRACKING_FLAGS {
            assert_eq!(get(&vr, var), None, "{var}: on by default, nothing pushed");
        }
        let (vr, t) = run(&["--old-exposure", "--no-room-glide", "--no-ctrl-steady"], &[]);
        assert_eq!(t, TrackingSwitches { new_exposure: false, room_glide: false, ctrl_steady: false });
        assert_eq!(get(&vr, "PIMAX_CAM_AE_LAW"), Some("old"));
        assert_eq!(get(&vr, "PIMAX_ANCHOR_GLIDE"), Some("0"));
        assert_eq!(get(&vr, "PIMAX_CTRL_TRACK_HYST"), Some("0"));
        let (vr, _) = run(&["--no-room-glide"], &[("PIMAX_ANCHOR_GLIDE", "1")]);
        assert_eq!(get(&vr, "PIMAX_ANCHOR_GLIDE"), None, "the caller's own wins");
    }

    #[test]
    fn ctrl_angle_live_change_keeps_the_restart() {
        let d = std::env::temp_dir().join(format!("ctrl-angle-live-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        super::super::supervise::record_argv(&d, "desk", &["--6dof".to_string(), "--ctrl-angle".into(), "30".into()]).unwrap();
        apply_ctrl_angle_live(&d, &d, 45).unwrap();
        assert_eq!(std::fs::read_to_string(d.join("pimax-ctrl-angle")).unwrap(), "45\n");
        let (kind, args) = super::super::supervise::read_argv(&d).unwrap();
        assert_eq!((kind.as_str(), args), ("desk", vec!["--6dof".to_string(), "--ctrl-angle".into(), "45".into()]));
        // a real start writes its own start value over a file left by an earlier session
        let rt = d.display().to_string();
        let lookup = move |k: &str| (k == "XDG_RUNTIME_DIR").then(|| rt.clone());
        let mut a = Args(vec!["--ctrl-angle".into(), "10".into()]);
        let mut vr = parse_options(&mut a, &lookup).unwrap();
        parse_ctrl_angle(&mut a, &lookup, &mut vr).unwrap();
        assert_eq!(sync_ctrl_angle_live(&vr, &lookup), None);
        assert_eq!(std::fs::read_to_string(d.join("pimax-ctrl-angle")).unwrap(), "10\n");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn off_without_the_flag() {
        let (vr, s) = go(&[], &[]);
        assert!(s.is_none());
        assert!(get(&vr, "PIMAX_SLAM").is_none());
    }

    #[test]
    fn env_matches_run_slam_live() {
        let (vr, s) = go(&["--6dof"], &[("XDG_RUNTIME_DIR", "/run/user/7")]);
        let s = s.unwrap();
        assert_eq!(get(&vr, "PIMAX_CAM_SOURCE"), Some("broker"));
        assert_eq!(get(&vr, "PIMAX_SLAM"), Some("1"));
        assert_eq!(get(&vr, "PIMAX_CAM_AE"), Some("1"));
        assert_eq!(get(&vr, "VIT_SYSTEM_LIBRARY_PATH"), Some("/home/u/.local/lib/libbasalt.so"));
        assert_eq!(get(&vr, "SLAM_CONFIG").unwrap(), s.slam_config().to_str().unwrap());
        assert!(get(&vr, "SLAM_CONFIG").unwrap().ends_with("/basalt/pimax-basalt.toml"));
        assert!(get(&vr, "PIMAX_SLAM_RECORD").is_none());
        assert!(get(&vr, "PIMAX_HEAD_IMU_LOG").unwrap().ends_with("/head-imu.csv"));
        assert_eq!(s.sock, PathBuf::from("/run/user/7/omarchyvr-cam-broker.sock"));
        assert_eq!(vr.monado_build, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
        assert_eq!(s.broker_spec(Path::new("/d")).args, vec!["broker".to_string()]);
    }

    #[test]
    fn caller_env_wins_and_record_is_optional() {
        let (vr, _) = go(&["--6dof"], &[("PIMAX_CAM_AE", "0"), ("PIMAX_SLAM_RECORD", "/rec"), ("VIT_SYSTEM_LIBRARY_PATH", "/x.so")]);
        assert_eq!(get(&vr, "PIMAX_CAM_AE"), Some("0"));
        assert_eq!(get(&vr, "PIMAX_SLAM_RECORD"), Some("/rec"));
        assert_eq!(get(&vr, "VIT_SYSTEM_LIBRARY_PATH"), Some("/x.so"));
    }

    #[test]
    fn build_without_slam_is_refused() {
        let dir = std::env::temp_dir().join(format!("sixdof-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("svc");
        std::fs::write(&f, b"abc PIMAX_EYE\0 PIMAX_SLAM_RECORD\0").unwrap();
        assert_eq!(build_has_slam(&f), Ok(false));
        std::fs::write(&f, b"abc\0PIMAX_SLAM\0def").unwrap();
        assert_eq!(build_has_slam(&f), Ok(true));
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn ct(args: &[&str], env: &[(&str, &str)]) -> (StartOptions, Result<Option<PathBuf>, CliError>) {
        let envv: Vec<(String, String)> = env.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect();
        let lookup = move |k: &str| envv.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let mut a = Args(args.iter().map(|s| s.to_string()).collect());
        let mut vr = parse_options(&mut a, &lookup).unwrap();
        let s = parse(&mut a, &lookup, &mut vr, Path::new("/home/u"), false, None);
        let r = parse_ctrl_track(&mut a, &lookup, &mut vr, Path::new("/home/u"), &s);
        (vr, r)
    }

    #[test]
    fn ctrl_track_needs_6dof_and_sets_the_env() {
        assert!(ct(&["--ctrl-track"], &[]).1.is_err());
        let (vr, r) = ct(&["--6dof", "--ctrl-track"], &[]);
        assert_eq!(r.unwrap(), Some(PathBuf::from("/home/u/.local/bin/pimax-ctrltrack")));
        assert_eq!(get(&vr, "PIMAX_CTRL_TRACK"), Some("1"));
        assert!(get(&vr, "PIMAX_CTRL_IMU_LOG").unwrap().ends_with("ctrl-imu"));
        let (vr, r) = ct(&["--6dof"], &[]);
        assert_eq!(r.unwrap(), None);
        assert!(get(&vr, "PIMAX_CTRL_TRACK").is_none());
        let (vr, r) = ct(&["--6dof", "--ctrl-track", "--ctrltrack-bin", "/x/ct"], &[("PIMAX_CTRL_TRACK", "0")]);
        assert_eq!(r.unwrap(), Some(PathBuf::from("/x/ct")));
        assert!(get(&vr, "PIMAX_CTRL_TRACK").is_none(), "the caller's own value wins");
    }

    #[test]
    fn ctrltrack_runs_on_the_broker_socket() {
        let (_, s) = go(&["--6dof"], &[("XDG_RUNTIME_DIR", "/run/user/7")]);
        let c = ctrltrack_spec(Path::new("/b/pimax-ctrltrack"), &s.unwrap(), Path::new("/d"));
        assert_eq!(c.name, "ctrltrack");
        assert_eq!(c.args, vec!["live", "--source", "broker:/run/user/7/omarchyvr-cam-broker.sock"]);
        assert_eq!(c.log, PathBuf::from("/d/ctrltrack.log"));
    }

    #[test]
    fn vr_stop_signals_the_tracker_by_pid_before_the_service_and_the_broker_last() {
        use crate::vr::host::Signal;
        use std::cell::{Cell, RefCell};
        struct Clk(Cell<Duration>);
        impl Clock for Clk {
            fn now(&self) -> Duration { self.0.get() }
            fn sleep(&self, d: Duration) { self.0.set(self.0.get() + d); }
        }
        struct P { dead: RefCell<Vec<u32>>, log: RefCell<Vec<(u32, Signal)>> }
        impl Procs for P {
            fn is_alive(&self, pid: u32) -> bool { pid != 0 && !self.dead.borrow().contains(&pid) }
            fn comm(&self, pid: u32) -> Option<String> { Some(if pid == 11 { "pimax-ctrltrack" } else { "other" }.into()) }
            fn signal(&self, pid: u32, sig: Signal) -> std::io::Result<()> {
                self.log.borrow_mut().push((pid, sig));
                self.dead.borrow_mut().push(pid);
                Ok(())
            }
            fn find_by_comm(&self, _: &str) -> Vec<u32> { vec![] }
            fn environ(&self, _: u32) -> Option<Vec<(String, String)>> { None }
            fn age_secs(&self, _: u32) -> Option<f64> { None }
        }
        let dir = crate::vr::test_dir("sixdof-ctrltrack-stop");
        std::fs::write(dir.join("ctrltrack.pid"), "11\n").unwrap();
        let (clk, p) = (Clk(Cell::new(Duration::ZERO)), P { dead: RefCell::new(vec![]), log: RefCell::new(vec![]) });
        stop_ctrltrack(&dir, Path::new("/b/pimax-ctrltrack"), &p, &clk);
        assert_eq!(*p.log.borrow(), vec![(11, Signal::Int)]);
        assert!(!dir.join("ctrltrack.pid").exists());
        // no pid file: nothing is signalled; a pid file of another process is left alone
        stop_ctrltrack(&dir, Path::new("/b/pimax-ctrltrack"), &p, &clk);
        std::fs::write(dir.join("ctrltrack.pid"), "12\n").unwrap();
        stop_ctrltrack(&dir, Path::new("/b/pimax-ctrltrack"), &p, &clk);
        assert_eq!(p.log.borrow().len(), 1);
        // order in `vr stop`: perf clients, tracker, service, broker
        let src = include_str!("mod.rs");
        let stop = &src[src.find("\"stop\" => {").unwrap()..];
        let pos = |n: &str| stop.find(n).unwrap();
        assert!(pos("perf::vr_stop_clients") < pos("sixdof::stop_ctrltrack"));
        assert!(pos("sixdof::stop_ctrltrack") < pos("session::stop("));
        assert!(pos("session::stop(") < pos("sixdof::stop_broker"));
    }
}
