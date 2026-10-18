//! `omarchyvr vr start|stop|status|steam-options`: one-click daily VR.
//!
//! * `start` runs the preflight checks (headset on USB, `omarchyvr-keepalived` active, DP-1 connected, no Xid in this
//!   boot's kernel log, no monado-service running), starts monado-service detached with the environment of
//!   `analysis/tools/run_vr_session.sh` and its log in `/tmp/omarchyvr-session`, and waits for the
//!   `gyro bias done` line. Progress and failures go to the terminal and to `notify-send`.
//! * `stop` sends SIGINT, escalates to SIGKILL after 5 s (with a warning) and then checks the kernel log.
//! * `status` shows the service, the headset/display state and a summary of the log.
//! * `steam-options` prints the Steam launch options line.
//!
//! The logic is split so that it can be tested without hardware: [`config`] (environment), [`logs`] (log parsing),
//! [`preflight`] (checks against a mock [`preflight::Host`]), [`session`] (flows with a fake clock) and [`host`]
//! (the real machine).
/// `println!` that ignores a closed or broken stdout: a launcher started from a desktop entry must not panic on it.
macro_rules! vout {
    ($($a:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stdout(), $($a)*);
    }};
}

/// `eprintln!` that ignores a broken stderr.
macro_rules! verr {
    ($($a:tt)*) => {{
        use std::io::Write as _;
        let _ = writeln!(std::io::stderr(), $($a)*);
    }};
}

pub mod audio;
pub mod display;
pub mod gamemode;
pub mod gameruntime;
pub mod captions;
pub mod config;
pub mod daemon;
pub mod desk;
pub mod dump;
pub mod edid;
pub mod eye_desk;
pub mod game_check;
pub mod gate;
pub mod hand_models;
pub mod hands;
pub mod hang;
pub mod home;
pub mod host;
pub mod logs;
pub mod openvr_link;
pub mod perf;
pub mod preset;
pub mod preflight;
pub mod reset;
pub mod room;
pub mod runtime_link;
pub mod screens;
pub mod session;
pub mod steam_setup;
pub mod steamvr;
pub mod supervise;
pub mod sixdof;
pub mod wayvr_conf;
pub mod work;

use crate::args::Args;
use crate::{CliError, CliResult};
use config::{
    check_factor, check_frame_list, check_target_format, default_monado_build, steam_options, Fps,
    StartOptions, DEFAULT_CONNECTOR, DEFAULT_READY_TIMEOUT_SECS, SESSION_DIR,
};
use host::{NotifySend, RealClock, RealHost};
use session::Deps;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::time::Duration;

/// Usage text of the `vr` subcommands.
pub const USAGE: &str = "usage: omarchyvr vr <command> [options]
  hand-models status|download --yes   fetch the Mercury hand models from Monado's public server (no licence, personal use; docs/HANDS.md)
  start [--fps 72|90] [--6dof [--ctrl-track] [--hands [--hands-roles]]] [--controllers always|when-held|off] [--ctrl-angle DEG] [--old-exposure] [--no-room-glide] [--no-ctrl-steady] [--no-drift-hold] [--eye|--no-eye] [--perf] [--anchor] [--light|--no-light] [--game APPID] [--home] [--dump DIR [--dump-frames 100,300]] [--wait] [--dry-run] [--force] [--debug] [--no-set-runtime]
        [--no-notify] [--monado-build DIR] [--connector NAME] [--mode N] [--timeout SECS]
      the connector is found by the headset's EDID (PVR 0xD242 / product 0x1019) unless --connector is given; the
      preflight also warns when monado-service lacks cap_sys_nice and prints the setcap line for its path.
      --mode is the Vulkan display-mode index (default 3 = 90 Hz, 2 = 72 Hz): sysfs does not list the refresh
      rate or Monado's order, so it is not derived from the EDID.
      preflight checks, then monado-service detached (log /tmp/omarchyvr-session/monado.log); returns when
      the gyro calibration is done. --wait stays in the foreground until the service exits (desktop entry).
      Play keeps the graphics card for the game: before anything starts, a crashed Work's leftovers (voice, the desk's
      clients, the VR screens' outputs) and the local AI (every omarchyvr-ai slot) are stopped by pid; nothing of a
      running session is touched (docs/PLAY-WORK-AUDIT.md).
      --light (default: the saved switch, on): light game mode, only what a game needs: no wrist readout (--perf), room
      anchor or room search; Monado, head 6DoF, controllers and their optical tracking (and camera hands if chosen)
      stay. --no-light keeps them. --eye / --no-eye (default: the saved switch \"Eye tracking in games\", off): the eye
      tracker in this game session. The launcher records both choices in start-argv. docs/GAME-MODE.md.
      --game APPID: the Steam game this session is for; a game that crashes with xrizer (Half-Life: Alyx and the games the
      app found crashing, `omarchyvr vr game-runtime list`) starts the session with SteamVR for this start only.
      --home: this session is the VR home (docs/VR-HOME.md, `omarchyvr home help`): a minimal WayVR (no screens) shows
      the home panel (Work, Games with SteamVR, the installed VR games, Room setup Quick/Full, Exit VR) in front of you,
      over the passthrough (or WayVR's sky: ~/.config/omarchyvr/home.toml); the room anchor and room search run, no wrist
      readout. Without --home the same panel is opened by the controllers' menu button in every Monado game session
      (pi_menu = true, the default; hidden until pressed).
      --anchor: PIMAX_ANCHOR=1 for Monado: its head-pose ring and the world offset file that `omarchyvr-anchor apply`
      (AprilTag) and `omarchyvr-roommap relocalize --apply` (room map) write; needs the `desk` Monado build with the hook.
      --ctrl-track (with --6dof): optical 6DoF controllers, same as `desk start --ctrl-track`: `pimax-ctrltrack live
      --source broker` (started after the service, stopped by pid by `vr stop` before the service) and PIMAX_CTRL_TRACK=1
      for Monado; [--ctrltrack-bin P]. Ignored with --wait. See `omarchyvr desk` help for the details.
      --controllers (default always for games): when-held = controllers shown only while in the hand
      (PIMAX_CTRL_INHAND=auto, docs/CONTROLLERS-INHAND.md), always, off.
      --ctrl-angle DEG, --old-exposure, --no-room-glide, --no-ctrl-steady, --no-drift-hold: as for `desk start` (see `omarchyvr desk help`).
      --hands (with --6dof; also `desk start --hands`): camera hand tracking (Monado's Mercury, XR_EXT_hand_tracking +
      XR_EXT_hand_interaction), off by default. Sets PIMAX_HANDS=mercury on the one Monado build (another build only
      with --hands-build DIR / PIMAX_HANDS_BUILD; an explicit --monado-build is used as given).
      If the build lacks the hands code or the models are missing: a warning and the session starts without hands. --hands-roles: the
      hands take the left/right controller roles (PIMAX_HT_ROLES=hands). docs/HANDS.md.
      --dump writes the final compositor image of selected frames (about 109 MB each) for checking colours.
      [--gamma G] [--gain K] [--target-format B8G8R8A8_UNORM|...]: colour experiments, only effective with the compositor
      patch analysis/overnight/srgb-target.patch (G < 1 brightens the mid tones; K scales the linear light).
  stop [--grace SECS] [--all] [--no-kill] [--dry-run] [--no-daemon]
      SIGINT to the service this tool started, SIGKILL after 5 s (15 s when a client is still connected) with a warning,
      then check the kernel log for Xid / display engine errors. --all also stops monado-service processes of yours
      that were not started by `omarchyvr vr start`. --no-kill (automatic callers: the watchdog, auto mode): never SIGKILL;
      a hung service is left alive, its hang is captured and the GPU gate ($XDG_RUNTIME_DIR/omarchyvrd/gpu-gate) says reboot.
      --dry-run: print what would be signalled, in order, and touch nothing.
  status [--log FILE] [--notify]
      service, headset, display, kernel state and a summary of the log (exit code 3 if not running).
  (start with --6dof also takes --no-auto-recover: if the camera 34a4:0014 is missing but the headset HID is present, start
      first restarts the headset ONCE (its own restart command, the keepalive back as soon as it leaves USB) and waits up
      to 60 s for the camera; no second reset. If the camera stays away the start stops and points to the app's
      'Get the headset ready' guide (a power cycle, the power button first). Refuses while monado-service runs.)
  reset-headset [--mode sys|exit|dp] [--once] [--wait SECS] [--session-dir DIR]     (GUI: 'Restart headset')
      --once (sys only; the setup's step for missing cameras): exactly one restart command, up to 60 s for the camera,
      no slow fallback and no dp retry; the verdict then says the headset needs a power cycle (the app's guide).
      sys (default): the headset's own restart command, 58 04 28 00 00 (eSpiCmdSysReset, captured from Pimax Play):
      stops omarchyvr-keepalived, sends it once, starts the keepalive again as soon as the headset leaves USB (without a host
      heartbeat it re-enumerates every ~6 s and the camera does not return), watches USB (HID back after ~8 s, camera
      ~17 s) and waits until 22 s after the command for the picture; plain progress lines starting with restart:. If nothing leaves USB within 15 s the headset
      ignored it and the exit sequence below follows. Then the dp retry if the camera is still missing.
      exit: make the headset power-cycle itself like after Pimax Play exits: stops omarchyvr-keepalived, sends the
      captured exit frames (0x1a/5 [04], 0x39 rate 0 twice), watches USB for --wait seconds (default 150; HID 34a4:0044,
      camera 34a4:0014). Also works when the camera is missing at the start (USB3 side dropped): it waits for the camera
      and checks /sys/class/video4linux (measured: back ~90 s after the reset). No unplugging needed.
      dp: power-cycle the display chain only: stops omarchyvr-keepalived until the bridge logs dp_power_off (heartbeat
      timeout, ~5 s, HPD drop), restarts it and watches for dp_power_on / Panel ON (~20 s). Does NOT clear a stuck
      NVIDIA display (vkAcquireDrmDisplayEXT failing): that needs a reboot.
      Both always restart omarchyvr-keepalived and refuse while monado-service runs.
  restart
      stop with grace 30, then start again with the SAME arguments as the last `vr start` (recorded in
      <session dir>/start-argv.txt on every start). Auto camera recovery applies as in start.
  start ... [--no-watchdog]
      a detached watchdog (`omarchyvr vr watchdog`, log <session dir>/watchdog.log) restarts the session when monado-service
      dies (stop without SIGKILL -> start, at most 3 times per 10 minutes, then a critical notification). An unplugged
      headset or a dropped camera (--6dof) only notifies: a running game is never stopped. No automatic restart after a
      GPU error that needs a reboot. A user `vr stop` (marker file user-stop, stop.lock) is never fought. Not started
      with --wait or --dry-run.
  omarchyvrd (docs/PIMAXD.md): when omarchyvrd runs and executes (not observe-only, not in safe mode), `start` records its
      arguments, asks omarchyvrd to start the session (set session=on) and follows it until it is up (120 s at most; no
      watchdog: omarchyvrd owns restarts). `stop` tells omarchyvrd session=off, then stops directly. --no-daemon (or
      PIMAX_NO_DAEMON=1), --dry-run, --wait and --force always take the direct path. `status` adds a omarchyvrd line.
  games [monado|steamvr|status] [--monado-build DIR]      (GUI: Games run with)
      saves what games run with (~/.config/omarchyvr/games.toml) and hands ~/.config/openvr/openvrpaths.vrpath to it:
      monado = xrizer only, read-only, no SteamVR driver; steamvr = SteamVR first, writable, Monado's SteamVR driver
      (<build>/steamvr-monado) registered. Never changes the OpenXR runtime. With steamvr, `start` runs SteamVR
      (SteamVR/bin/vrstartup.sh on the host, VALVE_SKIP_RUNTIME_SAFETY=1; PIMAX_STEAMVR_LAUNCH=steam: through Steam)
      instead of monado-service, with the Pimax settings in ~/.config/omarchyvr/steamvr-driver.env, and refuses while a
      monado-service runs; `stop` closes it by pid (vrmonitor first). `start --runtime monado|steamvr` overrides the choice once. docs/STEAMVR.md.
  room [status|on|off|steamvr on|off|name NAME]      (GUI: Room > Room at every start, Tell SteamVR)
      on (default, with a saved room): every start gets the room anchor hook and runs `omarchyvr-roommap auto` after it
      (recognise the room, put floor + centre + forward in place; state in $XDG_RUNTIME_DIR/omarchyvr-room.status);
      steamvr on: before SteamVR starts, its room setup (chaperone_info.vrchap, universe 2 of driver_monado) gets the
      play area and walls of the room setup. docs/ROOM-SETUP.md.
  game-runtime [list|set APPID steamvr|xrizer|default [NAME]|next APPID|none|archive|check APPID SINCE_UNIX]   (GUI: Play > Games that need SteamVR)
      per-game OpenVR runtime and xrizer crash evidence (xrizer's log is kept per game run); docs/GAME-MODE.md.
  gamemode [status|light on|off|eye on|off|budget auto|6|8|restore]   (GUI: Light game mode, Eye tracking in games, Compositor time budget)
      Play always stops the local AI and Work's voice before it starts (no switch: the graphics card is for the game);
      nothing of Work starts again at Stop (it starts with the next Work); Sunshine is never stopped or paused.
      light: Light game mode; eye: Eye tracking in games (default off). budget: U_PACING_COMP_MIN_TIME_MS for the next
      start (auto = Monado's 3 ms). restore: crash recovery of an old state file (Sunshine entries only).
  display [status|image standard|bright|brighter|backlight 20..100|apply]   (GUI: Brightness)
  audio [status|on|off|mic-on|mic-off|mic-test|speaker-test]      (GUI: Sound in headset, Headset microphone, Test microphone, Test headset sound)
      saves ~/.config/omarchyvr/audio.toml. While VR runs (Play, Work, SteamVR) the headset speaker (DisplayPort audio:
      the NVIDIA HDA card's Pimax port; its card profile is selected) is the default output and the headset microphone
      the default input; Stop restores the old profile, output and input. Sunshine and other devices are left alone.
      mic-test records 3 s from the headset microphone, prints the level and plays it back on the current output.
      speaker-test plays a short chime on the headset speaker only (refuses with the reason when it is not there).
  steam-setup [status|on|off]      (GUI: Ready check > Steam for VR games)
  steam-restart [--wait SECS]      close a Steam that carries SteamVR's settings (steam -shutdown, clean env, by pid; the app asks first)
      PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 for Steam itself, so every game reaches Monado without launch options:
      ~/.local/share/applications/steam.desktop (from the next Steam start) and ~/.config/environment.d (from the
      next log in). off removes only our files. Exit code 3 from status when not set up.
  game-check [--appid N] [--json] [--no-container]
      VR games check (docs/GAMES-CHECK.md): OpenXR/OpenVR runtime files, monado-service, a tiny OpenXR client inside Steam's
      runtime (SteamLinuxRuntime_4 and sniper) without and with PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES, Steam env and
      Launch Options, last game log. PASS/FAIL lines with the one fix each; exit 1 on any FAIL. GUI: 'Check VR games'.
  steam-options
      prints the Steam launch options: always PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 (Steam's container shares
      Monado's socket only with it, LESSONS row 27), plus XR_RUNTIME_JSON while ~/.config/openxr/1/active_runtime.json
      does not point at the Monado build (start sets it, unless --no-set-runtime; stop leaves it).
  dump-stats FILE.raw... [--step N] [--probe X,Y]... [--ppm OUT.ppm]
      mean colours, dominant colours, black percentage and probe pixels of compositor dumps (start --dump);
      --ppm writes a preview.
  dump-diff A.raw B.raw
      compare two dumps: differing pixels, size of the differences, and how many are in flat areas (not on an edge).
environment: PIMAX_MONADO_BUILD (default ~/.local/lib/omarchyvr/monado if built, else ~/src/monado/build), PIMAX_VR_SESSION_DIR, PIMAX_NO_NOTIFY=1,
  XRT_COMPOSITOR_LOG, PIMAX_LOG, SCALE / XRT_COMPOSITOR_SCALE_PERCENTAGE are passed on to monado-service.";

/// Fresh scratch directory for unit tests (`$TMPDIR/omarchyvr-vr-test-<pid>-<name>`).
#[cfg(test)]
pub(crate) fn test_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("omarchyvr-vr-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Upper bounds for `--timeout` and `--grace` (Duration::from_secs_f64 panics on absurd values).
const MAX_TIMEOUT_SECS: f64 = 3600.0;
const MAX_GRACE_SECS: f64 = 600.0;

pub(crate) fn real_env(k: &str) -> Option<String> {
    std::env::var(k).ok()
}

/// Parse the options shared by all commands (and the start-only ones, which are simply unused elsewhere).
pub fn parse_options(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<StartOptions, CliError> {
    let fps = match a.opt("--fps")? {
        Some(v) => Fps::parse(&v)?,
        None => Fps::Hz90,
    };
    let dump_dir = a.opt("--dump")?.map(PathBuf::from);
    let dump_frames = match a.opt("--dump-frames")? {
        Some(f) => {
            if dump_dir.is_none() {
                return Err(CliError::usage("--dump-frames needs --dump DIR"));
            }
            Some(check_frame_list(&f)?)
        }
        None => None,
    };
    let monado_build = a
        .opt("--monado-build")?
        .map(PathBuf::from)
        .unwrap_or_else(|| default_monado_build(env));
    let service_bin = a.opt("--service-bin")?.map(PathBuf::from);
    let connector_opt = a.opt("--connector")?;
    let connector_auto = connector_opt.is_none();
    let connector = connector_opt.unwrap_or_else(|| DEFAULT_CONNECTOR.to_string());
    let mode_override = a.opt_parse::<u32>("--mode")?;
    let timeout: f64 = a
        .opt_parse("--timeout")?
        .unwrap_or(DEFAULT_READY_TIMEOUT_SECS);
    if !timeout.is_finite() || timeout <= 0.0 {
        return Err(CliError::usage(
            "--timeout must be a positive number of seconds",
        ));
    }
    let session_dir = a
        .opt("--session-dir")?
        .or_else(|| env("PIMAX_VR_SESSION_DIR").filter(|s| !s.is_empty()))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(SESSION_DIR));
    let output_gamma = match a.opt_parse::<f32>("--gamma")? {
        Some(v) => Some(check_factor("--gamma", v, 0.2, 5.0)?),
        None => None,
    };
    let output_gain = match a.opt_parse::<f32>("--gain")? {
        Some(v) => Some(check_factor("--gain", v, 0.1, 10.0)?),
        None => None,
    };
    let target_format = match a.opt("--target-format")? {
        Some(v) => Some(check_target_format(&v)?),
        None => None,
    };
    let no_notify =
        a.flag("--no-notify") || env("PIMAX_NO_NOTIFY").is_some_and(|v| !v.is_empty() && v != "0");
    // --anchor: Monado keeps its head-pose ring and applies the room offset (omarchyvr-anchor, omarchyvr-roommap)
    // the room switch (~/.config/omarchyvr/room.toml, a saved room) also turns the hook on (docs/ROOM-SETUP.md)
    let anchor = a.flag("--anchor") || room::auto_anchor(env);
    Ok(StartOptions {
        fps,
        mode_override,
        dump_dir,
        dump_frames,
        monado_build,
        service_bin,
        connector,
        connector_auto,
        extra_env: if anchor { vec![("PIMAX_ANCHOR".into(), "1".into())] } else { Vec::new() },
        session_dir,
        ready_timeout: Duration::from_secs_f64(timeout.min(MAX_TIMEOUT_SECS)),
        force: a.flag("--force"),
        dry_run: a.flag("--dry-run"),
        notify: !no_notify,
        wait: a.flag("--wait"),
        debug: a.flag("--debug"),
        output_gamma,
        output_gain,
        target_format,
        set_runtime: !a.flag("--no-set-runtime"),
    })
}

/// [`resolve_connector`] against the real machine, with a note on stderr.
pub(crate) fn auto_connector(opts: &mut StartOptions) {
    if let Some(note) = resolve_connector(opts, &RealHost::from_env()) {
        verr!("omarchyvr vr: note: {note}");
    }
}

pub(crate) fn reject_leftovers(a: &Args) -> CliResult {
    a.no_unknown_flags()?;
    match a.0.first() {
        Some(x) => Err(CliError::usage(format!("unexpected argument {x}\n{USAGE}"))),
        None => Ok(()),
    }
}

pub(crate) fn with_deps<T>(
    opts: &StartOptions,
    stop_requested: &dyn Fn() -> bool,
    f: impl FnOnce(&Deps) -> T,
) -> T {
    let mut host = RealHost::from_env();
    host.service_comm = opts.service_comm();
    host.session_dir = opts.session_dir.clone();
    let notifier = NotifySend::new(opts.notify);
    let clock = RealClock::new();
    let deps = Deps {
        host: &host,
        procs: &host,
        notifier: &notifier,
        clock: &clock,
        env: &real_env,
        stop_requested,
    };
    f(&deps)
}

/// Without `--connector`: use the connector whose EDID says Pimax Dream Air instead of the hard-coded `DP-1`.
/// Returns a note for the user when the connector changed.
pub fn resolve_connector(opts: &mut StartOptions, host: &dyn preflight::Host) -> Option<String> {
    if !opts.connector_auto {
        return None;
    }
    let found = host.headset_connector()?;
    if found == opts.connector {
        return None;
    }
    let note = format!(
        "headset found on connector {found} by its EDID (PVR 0xD242 / 0x1019), not {}",
        opts.connector
    );
    opts.connector = found;
    Some(note)
}

/// Entry point of `omarchyvr vr ...` (arguments after `vr`).
pub fn run(mut a: Args) -> CliResult {
    let Some(first) = a.0.first().cloned() else {
        return Err(CliError::usage(USAGE));
    };
    if matches!(first.as_str(), "-h" | "--help" | "help") {
        vout!("{USAGE}");
        return Ok(());
    }
    if first.starts_with('-') {
        return Err(CliError::usage(format!(
            "the command comes first: omarchyvr vr start {first} ...\n{USAGE}"
        )));
    }
    a.0.remove(0);
    // A parent that ignores SIGCHLD would make waiting for our child fail with ECHILD.
    // SAFETY: signal(2) with SIG_DFL, no handler involved.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
    }
    match first.as_str() {
        "start" => {
            let mut argv0 = a.0.clone();
            let home = PathBuf::from(real_env("HOME").unwrap_or_else(|| "/root".into()));
            // light game mode: --light / --no-light, else the saved switch (default on); the resolved choice is recorded in
            // start-argv so every restart keeps it
            let gm = gamemode::load(&home);
            // --home: this session is the VR home (docs/VR-HOME.md): the home panel shows at start, the room anchor and
            // the room search run (room setup from the panel), no wrist readout; recorded in start-argv like any flag
            let home_flag = a.flag(home::HOME_FLAG);
            let (light_flag, no_light_flag) = (a.flag("--light"), a.flag("--no-light"));
            let light = gamemode::resolve_light(light_flag, no_light_flag, &gm).map_err(CliError::usage)? && !home_flag;
            if !home_flag && !light_flag && !no_light_flag {
                argv0.push(if light { "--light" } else { "--no-light" }.into());
            }
            // eye tracking in games: --eye / --no-eye, else the saved switch (default off); recorded like light
            let (eye_flag, no_eye_flag) = (a.flag("--eye"), a.flag("--no-eye"));
            let eye_games = gamemode::resolve_eye(eye_flag, no_eye_flag, &gm).map_err(CliError::usage)?;
            if !eye_flag && !no_eye_flag {
                argv0.push(if eye_games { "--eye" } else { "--no-eye" }.into());
            }
            let game = a.opt("--game")?;
            if let Some(g) = &game {
                if !g.bytes().all(|b| b.is_ascii_digit()) || g.is_empty() {
                    return Err(CliError::usage("--game wants a Steam app id (digits)"));
                }
            }
            let mut opts = parse_options(&mut a, &real_env)?;
            if light {
                // the room anchor hook is the one thing parse_options turned on by itself
                opts.extra_env.retain(|(k, _)| k != "PIMAX_ANCHOR");
            }
            // argv0: parse_options has already taken --monado-build out of `a`
            let build_given = argv0.iter().any(|x| x == "--monado-build" || x.starts_with("--monado-build="))
                || real_env("PIMAX_MONADO_BUILD").is_some_and(|v| !v.is_empty());
            // --6dof: camera broker + SLAM; needs the merged build (default ~/.local/lib/omarchyvr/monado)
            let sixdof = sixdof::parse(&mut a, &real_env, &mut opts, &home, build_given, Some(omarchyvr_config::paths::monado_build(&home)));
            // --ctrl-track: optical controllers (needs --6dof); shared with `desk start`
            let ctrl_track = sixdof::parse_ctrl_track(&mut a, &real_env, &mut opts, &home, &sixdof)?;
            // --controllers: games keep the controllers always shown unless the user switched it (Play's own switch)
            let _controllers = sixdof::parse_controllers(&mut a, &real_env, &mut opts, omarchyvr_config::ctrl_inhand::CtrlMode::Always)?;
            sixdof::parse_ctrl_angle(&mut a, &real_env, &mut opts)?;
            sixdof::parse_tracking_switches(&mut a, &real_env, &mut opts);
            // eye tracking in games, visual only (the "Eye tracking in games" switch, default off): PIMAX_EYE=1 when on and
            // the eye build and a calibration exist
            let eye_build = omarchyvr_config::paths::monado_build(&home);
            let (eye_mode, eye_note) = eye_desk::parse(false, !eye_games, &mut opts, &eye_build, build_given, &real_env);
            // --hands: camera hands (Mercury); may move the Monado build to the hands build (docs/HANDS.md)
            let hands_opt = hands::parse(&mut a, &real_env, &mut opts, &home, build_given, sixdof.is_some())?;
            // --perf: wrist readout (minimal WayVR overlay without screens + omarchyvr-perf); Monado gets PIMAX_PERF_FILE
            let perf_wanted = a.flag("--perf") && !home_flag;
            let perf_opts = if perf_wanted && !light {
                let wayvr = a.opt("--wayvr-bin")?.map(PathBuf::from).unwrap_or_else(|| omarchyvr_config::paths::wayvr_bin(&home));
                let ctl = a.opt("--wayvrctl-bin")?.map(PathBuf::from).unwrap_or_else(|| wayvr.with_file_name("wayvrctl"));
                Some(perf::parse(&mut a, &real_env, &mut opts, &home, &wayvr, &ctl)?)
            } else {
                None
            };
            let no_auto_recover = a.flag("--no-auto-recover");
            let no_watchdog = a.flag("--no-watchdog");
            let no_daemon = a.flag("--no-daemon");
            // Games run with (the app's switch, ~/.config/omarchyvr/games.toml); --runtime overrides it for this start
            let games = match a.opt("--runtime")? {
                Some(r) => steamvr::Games::parse(&r).ok_or_else(|| CliError::usage("--runtime must be monado or steamvr"))?,
                None => {
                    // a game that crashes with xrizer starts this session with SteamVR (per start; docs/GAME-MODE.md)
                    let global = steamvr::games_runtime(&real_env);
                    let r = gameruntime::resolve(&gameruntime::Store::load(&home), game.as_deref(), global);
                    if r != global {
                        verr!("omarchyvr vr: game {} runs with {} (game-runtime list), for this start only", game.as_deref().unwrap_or("?"), r.name());
                    }
                    r
                }
            };
            reject_leftovers(&a)?;
            hands::print_note(&hands_opt, "omarchyvr vr");
            if eye_mode == eye_desk::EyeMode::Off {
                // only the reason that matters to the user (the eye build is in use but not calibrated)
                if let Some(n) = eye_note.filter(|n| n.contains("calibration")) {
                    verr!("omarchyvr vr: {n}");
                }
            }
            auto_connector(&mut opts);
            // where the headset data comes from: the user's own or the shipped reference (the job log says it)
            vout!("omarchyvr vr: {}", crate::import_cmd::start_line(&home, &real_env));
            if !opts.dry_run {
                // Play keeps the graphics card for the game: a crashed Work's leftovers and the local AI go first, by
                // pid (nothing of a running session is touched; docs/PLAY-WORK-AUDIT.md)
                let dirs = omarchyvr_ai::Dirs::from_env(&real_env);
                let cleared = with_deps(&opts, &|| false, |d| work::clear_for_play(&opts, d, &dirs, &mut screens::RealHypr));
                for l in &cleared.lines {
                    vout!("omarchyvr vr: Play: {l}");
                }
                gamemode::record_stopped(&opts.session_dir, &cleared.entries);
                // keep the last game's xrizer log before the next game truncates it
                if let Ok(Some(p)) = gameruntime::archive_log(&home, "pre-start", gameruntime::now_unix()) {
                    vout!("omarchyvr vr: xrizer log of the last game kept: {}", p.display());
                }
                if let Err(e) = supervise::record_argv(&opts.session_dir, "vr", &daemon::argv_for_record(&argv0)) {
                    verr!("omarchyvr vr: WARNING: cannot record the start arguments for restart: {e}");
                }
                if let Some(e) = sixdof::sync_ctrl_angle_live(&opts, &real_env) {
                    verr!("omarchyvr vr: WARNING: controller angle: {e}");
                }
            }
            // SteamVR: SteamVR's vrserver runs the Pimax driver (driver_monado.so); no monado-service, no watchdog
            if games == steamvr::Games::SteamVr {
                let env = config::build_env(&opts, &real_env);
                let flag = pimax::sys::install_term_flag();
                return with_deps(&opts, &|| flag.load(Ordering::Relaxed), |d| {
                    let r = steamvr::start(&opts, &env, sixdof.as_ref(), ctrl_track.as_deref(), d, &desk::RealSpawner);
                    if r.is_ok() && !opts.dry_run {
                        if let Some(l) = audio::on_start(&home, &opts.session_dir) {
                            vout!("omarchyvr vr: {l}");
                        }
                        if let Some(l) = display::on_start_steamvr(&home) {
                            vout!("omarchyvr vr: {l}");
                        }
                        if let Some(l) = (!light).then(|| room::on_start(&home, &opts.session_dir)).flatten() {
                            vout!("omarchyvr vr: {l}");
                        }
                    }
                    r
                });
            }
            // omarchyvrd (P6): an executing omarchyvrd starts the session itself; otherwise the direct path below, unchanged
            let (link, dmn) = daemon::contact(no_daemon, opts.dry_run);
            if daemon::start_through(&dmn, opts.wait, opts.force) {
                let r = daemon::start_via(&link, "vr", &RealClock::new(), &mut |m| vout!("omarchyvr vr: {m}")).map_err(CliError::new);
                if r.is_ok() && !opts.dry_run {
                        if let Some(l) = audio::on_start(&home, &opts.session_dir) {
                        vout!("omarchyvr vr: {l}");
                    }
                        if let Some(l) = display::on_start(&home) {
                            vout!("omarchyvr vr: {l}");
                        }
                        if let Some(l) = (!light).then(|| room::on_start(&home, &opts.session_dir)).flatten() {
                            vout!("omarchyvr vr: {l}");
                        }
                }
                return r;
            }
            // camera missing but headset present: restart the headset and wait for the camera (needs --6dof)
            if sixdof.is_some() && !opts.dry_run && !no_auto_recover {
                reset::real::auto_recover_real(&opts.session_dir, reset::AUTO_WAIT).map_err(CliError::new)?;
            }
            // Ctrl-C / SIGTERM while the service starts (or, with --wait, runs) must stop it, not leave it behind
            let flag = pimax::sys::install_term_flag();
            let started = with_deps(&opts, &|| flag.load(Ordering::Relaxed), |d| {
                let go = || match &sixdof {
                    Some(s) => sixdof::vr_start(&opts, s, ctrl_track.as_deref(), d, &desk::RealSpawner),
                    None => session::start(&opts, d),
                };
                // the home overlay (the VR home panel; the menu button opens it over games too) and the wrist readout
                // share one minimal WayVR (docs/VR-HOME.md, docs/WAYVR-CONTROLS.md)
                let overlay = home::overlay_options(&home, home_flag, perf_opts.as_ref(), &opts);
                if opts.dry_run {
                    if let Some(p) = &perf_opts {
                        vout!("omarchyvr vr: --perf would also run, after the service: {} (minimal WayVR, no screens), {}", p.wayvr_bin.display(), p.perf_bin.display());
                        vout!("       PIMAX_PERF_FILE={} for monado-service", p.stats.display());
                    }
                    if let Some((_, open)) = overlay.as_ref().and_then(|ov| ov.panel.as_ref()).filter(|_| !opts.wait) {
                        vout!(
                            "omarchyvr vr: the VR home panel ({}) on {} (minimal WayVR, no screens){}",
                            if *open { "shown at start" } else { "the menu button opens it" },
                            overlay.as_ref().map(|ov| ov.wayvr_bin.display().to_string()).unwrap_or_default(),
                            if overlay.as_ref().is_some_and(|ov| ov.passthrough.is_some()) { ", the home's passthrough before it" } else { "" }
                        );
                    }
                    return go();
                }
                let r = go();
                if r.is_ok() {
                    if opts.wait {
                        if overlay.is_some() {
                            verr!("omarchyvr vr: the VR home and --perf are ignored with --wait (the launcher stays in the foreground)");
                        }
                    } else if let Some(ov) = &overlay {
                        perf::overlay_start(perf_opts.as_ref(), ov, &opts, d, &desk::RealSpawner);
                    }
                }
                r
            });
            if started.is_ok() && !opts.dry_run && !opts.wait && !no_watchdog && dmn != daemon::Daemon::Executing {
                supervise::spawn_watchdog("vr", &opts.session_dir, sixdof.is_some(), opts.notify);
            }
            if started.is_ok() && !opts.dry_run {
                daemon::tell_direct(&link, &dmn, true, "vr", &mut |m| vout!("omarchyvr vr: {m}"));
                        if let Some(l) = audio::on_start(&home, &opts.session_dir) {
                    vout!("omarchyvr vr: {l}");
                }
                        if let Some(l) = display::on_start(&home) {
                            vout!("omarchyvr vr: {l}");
                        }
                        if let Some(l) = (!light).then(|| room::on_start(&home, &opts.session_dir)).flatten() {
                            vout!("omarchyvr vr: {l}");
                        }
            }
            started
        }
        "restart" => supervise::restart_cli("vr", a),
        "watchdog" => supervise::watchdog_cli("vr", a),
        "stop" => {
            let grace = match a.opt_parse::<f64>("--grace")? {
                Some(g) if g.is_finite() && g >= 0.0 => {
                    Some(Duration::from_secs_f64(g.min(MAX_GRACE_SECS)))
                }
                Some(_) => return Err(CliError::usage("--grace must be a number of seconds")),
                None => None,
            };
            let all = a.flag("--all");
            let kill = !a.flag("--no-kill") && !supervise::in_watchdog();
            let no_daemon = a.flag("--no-daemon");
            let mut opts = parse_options(&mut a, &real_env)?;
            reject_leftovers(&a)?;
            auto_connector(&mut opts);
            if opts.dry_run {
                let home = PathBuf::from(real_env("HOME").unwrap_or_default());
                let wayvr = omarchyvr_config::paths::wayvr_bin(&home);
                vout!("omarchyvr vr: would stop, in this order (SIGINT by pid, never by name):");
                let mut n = 1;
                if let Ok(p) = perf::parse(&mut Args(Vec::new()), &real_env, &mut opts.clone(), &home, &wayvr, &wayvr) {
                    for (name, bin, wait) in [("perf", &p.perf_bin, "10 s"), ("perfwayvr", &p.wayvr_bin, "15 s, then 15 s more; still alive once Monado is gone: SIGTERM, then SIGKILL by pid")] {
                        let pf = opts.session_dir.join(format!("{name}.pid"));
                        if pf.exists() {
                            vout!("  {n}. {name}: {} ({}), wait {wait}", desk::pid_note(&pf), config::comm_of(bin));
                            n += 1;
                        }
                    }
                }
                let ct = opts.session_dir.join("ctrltrack.pid");
                if ct.exists() {
                    vout!("  {n}. ctrltrack: {} ({}), wait 10 s", desk::pid_note(&ct), config::comm_of(&sixdof::default_ctrltrack_bin(&home)));
                    n += 1;
                }
                let (g, _) = match grace {
                    Some(g) => (g, false),
                    None => session::default_grace(&opts),
                };
                vout!("  {n}. monado-service{}: {}", if all { " (and every other monado-service of yours: --all)" } else { "" }, desk::monado_stop_note(&opts.pid_path(), g, kill));
                let cam = config::tools_dir(&home).join("pimax-cam");
                vout!("  {}. broker: {} ({}), wait 10 s", n + 1, desk::pid_note(&opts.session_dir.join("broker.pid")), config::comm_of(&cam));
                vout!("  then the kernel log check (Xid / Idling display engine)");
                return Ok(());
            }
            supervise::mark_user_stop(&opts.session_dir);
            let (link, dmn) = daemon::contact(no_daemon, false);
            daemon::tell_direct(&link, &dmn, false, "vr", &mut |m| vout!("omarchyvr vr: {m}"));
            with_deps(&opts, &|| false, |d| {
                // the wrist readout (only if `vr start --perf` started it) goes before the service
                let home = PathBuf::from(real_env("HOME").unwrap_or_default());
                let wayvr = omarchyvr_config::paths::wayvr_bin(&home);
                let mut hung = Vec::new();
                if let Ok(p) = perf::parse(&mut Args(Vec::new()), &real_env, &mut opts.clone(), &home, &wayvr, &wayvr) {
                    hung = perf::vr_stop_clients(&p, &opts.session_dir, d.procs, d.clock);
                }
                // the VR home's passthrough (only if `vr start --home` started it), after the overlay, before the service
                home::stop_home_passthrough(&opts.session_dir, &home, d);
                // the controller tracker (only if `vr start --ctrl-track` started it) goes before the service
                sixdof::stop_ctrltrack(&opts.session_dir, &sixdof::default_ctrltrack_bin(&home), d.procs, d.clock);
                // SteamVR started by `vr start` (Games run with: SteamVR), by pid, before the camera broker
                steamvr::stop(&opts.session_dir, Duration::from_secs(10), d)?;
                let monado_pids = session::stop_targets(&opts, d.procs).ours;
                let r = session::stop(&opts, grace, all, kill, d);
                // a minimal WayVR hung at exit: SIGTERM, then SIGKILL by pid, now that Monado is gone
                desk::end_hung_wayvrs(&hung, &monado_pids, &opts.service_comm(), "omarchyvr vr", d);
                let cam = config::tools_dir(&PathBuf::from(real_env("HOME").unwrap_or_default())).join("pimax-cam");
                sixdof::stop_broker(&opts.session_dir, &cam, d.procs, d.clock);
                room::on_stop(&opts.session_dir);
                if let Some(l) = audio::on_stop(&opts.session_dir) {
                    vout!("omarchyvr vr: {l}");
                }
                if let Some(l) = gamemode::on_stop(&gamemode::Tools::real(&PathBuf::from(real_env("HOME").unwrap_or_default())), &opts.session_dir) {
                    vout!("omarchyvr vr: {l}");
                }
                r
            })
        }
        "status" => {
            let log = a.opt("--log")?.map(PathBuf::from);
            let notify = a.flag("--notify");
            let mut opts = parse_options(&mut a, &real_env)?;
            reject_leftovers(&a)?;
            auto_connector(&mut opts);
            with_deps(&opts, &|| false, |d| {
                let (lines, running) = session::status_lines(&opts, d, log.as_deref());
                for l in &lines {
                    vout!("{l}");
                }
                // Games run with: SteamVR: Steam restarted / closed, or a Steam started from inside SteamVR
                if let Some(n) = steamvr::session_note(&opts.session_dir, &PathBuf::from(real_env("HOME").unwrap_or_default()), d.procs) {
                    vout!("steamvr: {n}");
                }
                if !daemon::bypass(false, false) {
                    if let Some(l) = daemon::status_line(&daemon::SockLink::from_env()) {
                        vout!("{l}");
                    }
                }
                if notify {
                    let body = lines.iter().take(3).cloned().collect::<Vec<_>>().join("\n");
                    d.notifier.notify(false, "Pimax VR status", &body);
                }
                if running {
                    Ok(())
                } else {
                    Err(CliError {
                        msg: String::new(),
                        code: 3,
                    })
                }
            })
        }
        "reset-headset" => {
            let dir = a
                .opt("--session-dir")?
                .or_else(|| real_env("PIMAX_VR_SESSION_DIR").filter(|s| !s.is_empty()))
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(SESSION_DIR));
            let mode = match a.opt("--mode")? {
                Some(m) => reset::Mode::parse(&m).map_err(CliError::usage)?,
                None => reset::Mode::Sys,
            };
            // --once: one restart command and no fallback (the setup's step for missing cameras)
            let once = a.flag("--once");
            if once && mode != reset::Mode::Sys {
                return Err(CliError::usage("--once goes with --mode sys (the default)"));
            }
            let wait = match a.opt_parse::<f64>("--wait")? {
                Some(w) if w.is_finite() && w >= 1.0 => Duration::from_secs_f64(w.min(900.0)),
                Some(_) => return Err(CliError::usage("--wait must be a number of seconds (1..900)")),
                None if once => reset::AUTO_WAIT,
                None => reset::WATCH,
            };
            reject_leftovers(&a)?;
            let flag = pimax::sys::install_term_flag();
            let mut host = reset::real::RealReset {
                host: RealHost::from_env(),
                pid_file: dir.join("monado.pid"),
                if0: None,
                flag,
                bridge: None,
            };
            let clock = RealClock::new();
            let verdict = match mode {
                reset::Mode::Sys if once => reset::reset_once(&mut host, &clock, wait).map_err(CliError::new)?.0,
                reset::Mode::Sys => reset::sys_then_exit_then_dp(&mut host, &clock, wait, true)
                    .map_err(CliError::new)?
                    .0,
                reset::Mode::Exit => reset::sys_then_exit_then_dp(&mut host, &clock, wait, false)
                    .map_err(CliError::new)?
                    .0,
                reset::Mode::Dp => reset::reset_dp(&mut host, &clock)
                    .map_err(CliError::new)?
                    .verdict(),
            };
            vout!("verdict: {verdict}");
            Ok(())
        }
        "games" => {
            let what = if a.0.first().is_some_and(|x| !x.starts_with('-')) { a.0.remove(0) } else { "status".into() };
            let opts = parse_options(&mut a, &real_env)?;
            reject_leftovers(&a)?;
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            steamvr::games_cli(&what, &home, &opts.monado_build, &real_env).map_err(CliError::new)
        }
        "room" => {
            let what = std::mem::take(&mut a.0);
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            let rt = PathBuf::from(real_env("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()));
            room::cli(&what, &home, &rt).map_err(CliError::new)
        }
        "game-runtime" => {
            let what = std::mem::take(&mut a.0);
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            gameruntime::cli(&what, &home).map_err(CliError::new)
        }
        "gamemode" => {
            let mut opts = parse_options(&mut a, &real_env)?;
            auto_connector(&mut opts);
            let what = std::mem::take(&mut a.0);
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            gamemode::cli(&what, &home, &opts.session_dir).map_err(CliError::new)
        }
        "display" => {
            let what = std::mem::take(&mut a.0);
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            display::cli(&what, &home).map_err(CliError::new)
        }
        "audio" => {
            let what = if a.0.first().is_some_and(|x| !x.starts_with('-')) { a.0.remove(0) } else { "status".into() };
            let arg = if a.0.first().is_some_and(|x| !x.starts_with('-')) { Some(a.0.remove(0)) } else { None };
            reject_leftovers(&a)?;
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            audio::cli(&what, arg.as_deref(), &home).map_err(CliError::new)
        }
        "steam-restart" => {
            let wait = match a.opt_parse::<u64>("--wait")? {
                Some(w) if (1..=900).contains(&w) => Duration::from_secs(w),
                Some(_) => return Err(CliError::usage("--wait must be a number of seconds (1..900)")),
                None => steamvr::SHUTDOWN_WAIT,
            };
            reject_leftovers(&a)?;
            steamvr::restart_cli(wait)
        }
        "hand-models" => {
            let what = if a.0.first().is_some_and(|x| !x.starts_with('-')) { a.0.remove(0) } else { "status".into() };
            let yes = a.flag("--yes");
            reject_leftovers(&a)?;
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            hand_models::run(&what, yes, &home).map_err(CliError::new)
        }
        "steam-setup" => {
            let what = if a.0.first().is_some_and(|x| !x.starts_with('-')) { a.0.remove(0) } else { "status".into() };
            reject_leftovers(&a)?;
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            steam_setup::run(&what, &home).map_err(|m| CliError { code: if m.is_empty() { 3 } else { 1 }, msg: m })
        }
        "dump-stats" => dump::run(&mut a),
        "dump-diff" => dump::run_diff(&mut a),
        "game-check" => {
            let line = config::steam_options_linked();
            game_check::run(&mut a, line)
        }
        "steam-options" => {
            let opts = parse_options(&mut a, &real_env)?;
            reject_leftovers(&a)?;
            // auto mode's game runtime (~/.config/omarchyvr/auto.toml): SteamVR games need no Monado/xrizer options
            if steamvr::games_runtime(&real_env) == steamvr::Games::SteamVr {
                vout!("{}", config::steam_options_linked());
                verr!("# Games run with: SteamVR (~/.config/omarchyvr/games.toml): active_runtime.json points at SteamVR's OpenXR runtime; Play starts SteamVR.");
                verr!("# The import variable lets Steam's container see that runtime for OpenXR games (OpenVR games ignore it).");
                verr!("# `omarchyvr vr games monado` (app: Games run with: Monado) for Monado + xrizer.");
                return Ok(());
            }
            let home = PathBuf::from(real_env("HOME").unwrap_or_default());
            if steam_setup::inspect(&home, None).configured() {
                verr!("# Steam for VR games is set up (omarchyvr vr steam-setup): no launch options are needed once Steam was restarted.");
                verr!("# The line below is only a fallback for a Steam started some other way.");
            }
            if runtime_link::inspect(&home, &opts.runtime_json()) == runtime_link::State::Ok {
                vout!("{}", config::steam_options_linked());
                verr!("# {} already points at this Monado build.", runtime_link::link_path(&home).display());
                verr!("# Steam > game > Properties > General > Launch Options: paste the line above. Steam's container shares");
                verr!("# Monado's socket only with PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 (LESSONS row 27).");
                return Ok(());
            }
            vout!("{}", steam_options(&opts.runtime_json()));
            verr!("# active_runtime.json does not point at this build yet: `omarchyvr vr start` / `desk start` sets it.");
            verr!("# Until then, Steam > game > Properties > General > Launch Options: paste the line above.");
            verr!("# If the game ignores ~/.config/openvr/openvrpaths.vrpath add: VR_OVERRIDE=$HOME/src/xrizer/target/release");
            verr!("# Start the service first: omarchyvr vr start   (Steam's 'needs a VR headset / SteamVR' popup is harmless)");
            Ok(())
        }
        other => Err(CliError::usage(format!(
            "unknown vr command {other}\n{USAGE}"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(args: &[&str]) -> Result<StartOptions, CliError> {
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        let mut a = Args(args.iter().map(|s| s.to_string()).collect());
        let o = parse_options(&mut a, &env)?;
        reject_leftovers(&a)?;
        Ok(o)
    }

    #[test]
    fn defaults() {
        let o = parse(&[]).unwrap();
        assert_eq!(o.fps, Fps::Hz90);
        assert_eq!(o.connector, "DP-1");
        assert_eq!(o.monado_build, PathBuf::from("/home/u/src/monado/build"));
        assert_eq!(o.session_dir, PathBuf::from("/tmp/omarchyvr-session"));
        assert_eq!(o.ready_timeout, Duration::from_secs(30));
        assert!(o.notify && !o.force && !o.dry_run && !o.wait && !o.debug);
        assert!(o.dump_dir.is_none() && o.mode_override.is_none() && o.service_bin.is_none());
    }

    #[test]
    fn all_options() {
        let o = parse(&[
            "--fps",
            "90",
            "--dump=/tmp/d",
            "--dump-frames",
            "100, 300",
            "--monado-build",
            "/b",
            "--connector",
            "DP-2",
            "--mode",
            "5",
            "--timeout",
            "12.5",
            "--session-dir",
            "/s",
            "--service-bin",
            "/x/fake",
            "--force",
            "--dry-run",
            "--wait",
            "--debug",
            "--no-notify",
            "--gamma",
            "0.9",
            "--gain=1.1",
            "--target-format",
            "B8G8R8A8_UNORM",
        ])
        .unwrap();
        assert_eq!(o.output_gamma, Some(0.9));
        assert_eq!(o.output_gain, Some(1.1));
        assert_eq!(o.target_format.as_deref(), Some("B8G8R8A8_UNORM"));
        assert_eq!(o.fps, Fps::Hz90);
        assert_eq!(o.dump_dir, Some(PathBuf::from("/tmp/d")));
        assert_eq!(o.dump_frames.as_deref(), Some("100,300"));
        assert_eq!(o.monado_build, PathBuf::from("/b"));
        assert_eq!(o.connector, "DP-2");
        assert_eq!(o.mode_override, Some(5));
        assert_eq!(o.ready_timeout, Duration::from_millis(12_500));
        assert_eq!(o.session_dir, PathBuf::from("/s"));
        assert_eq!(o.service_bin, Some(PathBuf::from("/x/fake")));
        assert!(o.force && o.dry_run && o.wait && o.debug && !o.notify);
    }

    #[test]
    fn bad_input_is_a_usage_error() {
        for args in [
            &["--fps", "60"][..],
            &["--fps"],
            &["--dump-frames", "100"],
            &["--dump", "/d", "--dump-frames", "x"],
            &["--timeout", "0"],
            &["--timeout", "abc"],
            &["--mode", "-1"],
            &["--bogus"],
            &["--gamma", "0"],
            &["--gamma", "x"],
            &["--gain", "100"],
            &["--target-format", "nope"],
            &["stray"],
        ] {
            let e = parse(args).unwrap_err();
            assert_eq!(e.code, 2, "{args:?}: {}", e.msg);
        }
    }

    #[test]
    fn session_dir_and_notify_from_the_environment() {
        let env = |k: &str| match k {
            "PIMAX_VR_SESSION_DIR" => Some("/run/x".to_string()),
            "PIMAX_NO_NOTIFY" => Some("1".to_string()),
            _ => None,
        };
        let mut a = Args(vec![]);
        let o = parse_options(&mut a, &env).unwrap();
        assert_eq!(o.session_dir, PathBuf::from("/run/x"));
        assert!(!o.notify);
        let env0 = |k: &str| (k == "PIMAX_NO_NOTIFY").then(|| "0".to_string());
        assert!(parse_options(&mut Args(vec![]), &env0).unwrap().notify);
    }

    #[test]
    fn connector_follows_the_edid_unless_given() {
        struct H(Option<&'static str>);
        impl preflight::Host for H {
            fn usb_headset_present(&self) -> bool {
                true
            }
            fn hidraw_interfaces(&self) -> usize {
                4
            }
            fn keepalive_state(&self) -> String {
                "active".into()
            }
            fn connector_status(&self, _: &str) -> Option<String> {
                None
            }
            fn kernel_errors(&self) -> Result<Vec<String>, String> {
                Ok(vec![])
            }
            fn service_pids(&self) -> Vec<u32> {
                vec![]
            }
            fn env(&self, _: &str) -> Option<String> {
                None
            }
            fn headset_connector(&self) -> Option<String> {
                self.0.map(str::to_string)
            }
        }
        let mut o = parse(&[]).unwrap();
        assert!(o.connector_auto);
        assert!(resolve_connector(&mut o, &H(Some("DP-3")))
            .unwrap()
            .contains("DP-3"));
        assert_eq!(o.connector, "DP-3");
        // same connector as the default: no note; nothing found: default stays
        let mut o = parse(&[]).unwrap();
        assert!(resolve_connector(&mut o, &H(Some("DP-1"))).is_none());
        assert!(resolve_connector(&mut o, &H(None)).is_none());
        assert_eq!(o.connector, "DP-1");
        // --connector wins even when the EDID says otherwise
        let mut o = parse(&["--connector", "DP-2"]).unwrap();
        assert!(!o.connector_auto);
        assert!(resolve_connector(&mut o, &H(Some("DP-3"))).is_none());
        assert_eq!(o.connector, "DP-2");
    }

    #[test]
    fn command_must_come_first() {
        let e = run(Args(vec!["--fps".into(), "90".into(), "start".into()])).unwrap_err();
        assert_eq!(e.code, 2);
        assert!(e.msg.contains("the command comes first"));
        assert_eq!(run(Args(vec![])).unwrap_err().code, 2);
        assert_eq!(run(Args(vec!["frobnicate".into()])).unwrap_err().code, 2);
        assert!(run(Args(vec!["help".into()])).is_ok());
    }
}
