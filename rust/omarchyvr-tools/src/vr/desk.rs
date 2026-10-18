//! `omarchyvr desk start|stop|status`: desk mode. Monado (as `omarchyvr vr start`), then `omarchyvr-passthrough run` (the room
//! camera view as the main OpenXR app), then WayVR as an `XR_EXTX_overlay` session on top (the desktop floating in
//! the passthrough view).
//!
//! Why this order works: WayVR creates its session with `XrSessionCreateInfoOverlayEXTX` (wayvr/src/backend/openxr/
//! helpers.rs `create_overlay_session`, and it refuses to start without `XR_EXTX_overlay`), so it is an overlay
//! session and the passthrough client stays the main session. The same combination (an overlay over a main app) was
//! measured with xr_room in `analysis/overnight/desk.md`. WayVR needs the main app to be there for its own frames to
//! be shown, so it is started last and stopped first.
//!
//! Stopping goes the other way round: SIGINT to WayVR (by pid, from the pid file; the process must still have the
//! right name), wait; SIGINT to passthrough, wait; then `vr stop` semantics for Monado (SIGINT, `--grace` 30 s by
//! default, SIGKILL only if it is still alive after that) and the kernel log check for Xid / "Idling display engine".
//! Processes are only ever signalled by pid.
use super::config::{comm_of, StartOptions};
use super::host::{Clock, Notifier, NotifySend, Procs, RealClock, RealHost};
use super::session::{self, Deps};
use super::perf::{self, PerfOptions};
use super::sixdof::{self, SixDof};
use super::wayvr_conf;
use super::{auto_connector, parse_options, real_env, reject_leftovers};
use crate::args::Args;
use crate::{CliError, CliResult};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// `PIMAX_CTRL_AIM` for Monado in desk mode unless the environment sets it (rays meet 1 m ahead of the view).
pub const DESK_CTRL_AIM: &str = "head";

/// Usage of `omarchyvr desk`.
pub const USAGE: &str = "usage: omarchyvr desk <command> [options]
  start [--no-passthrough] [--eye|--no-eye] [--6dof [--ctrl-track] [--hands [--hands-roles]]] [--controllers when-held|always|off] [--ctrl-angle DEG] [--old-exposure] [--no-room-glide] [--no-ctrl-steady] [--no-drift-hold] [--voice [--voice-hud]] [--no-perf] [--preset work-4|focus-3|work-5|work-6|cinema|cinema-huge|none] [--screens 1-6] [--anchor] [--no-auto-recover] [--settle SECS] [--passthrough-bin P] [--wayvr-bin P] [--wayvr-home DIR]
        [--voice-bin P] [--voice-hud-bin P] [--wayvrctl-bin P] [--perf-bin P] [--pimax-bin P]
        [+ the options of `omarchyvr vr start`: --fps, --monado-build, --connector, --dry-run, --force, --no-notify, ...]
      monado-service (like `omarchyvr vr start`), then omarchyvr-passthrough run (the main OpenXR app), then WayVR as an
      overlay (CPU screencopy config). --no-passthrough: Monado + WayVR only. Eye tracking (a dim dot; never input unless gaze_mouse=on in ~/.config/omarchyvr/eye.conf) is on when the
      eye build (~/.local/lib/omarchyvr/monado) and ~/.config/omarchyvr/eye-calibration.toml exist: PIMAX_EYE=1 and
      conf.d/55-eye.yaml (written every start, the only file with WayVR's gaze input keys; docs/EYE.md); --no-eye
      turns it off. --eye: PIMAX_EYE=1 and the build in ~/.local/lib/omarchyvr/monado (unless --monado-build or
      PIMAX_MONADO_BUILD says otherwise), still only the dot. --6dof: live 6DoF head
      tracking: starts `pimax-cam broker` first (stopped last by `desk stop`), PIMAX_SLAM=1 PIMAX_CAM_AE=1
      PIMAX_CAM_SOURCE=broker + the Basalt config for Monado, passthrough reads the broker; needs the merged Monado
      build (eye + slam, branch desk of the Monado fork) and refuses a build without the 6DoF code.
      --ctrl-track (with --6dof; also `omarchyvr vr start --ctrl-track`): optical 6DoF controllers: `pimax-ctrltrack live --source broker` (LED frames ->
      controller poses in the head frame, $XDG_RUNTIME_DIR/pimax-ctrltrack.pose) and PIMAX_CTRL_TRACK=1 for Monado,
      which puts the controllers there. Out of view: gyro orientation (fitted from the optical poses,
      ~/.config/omarchyvr/ctrl-imu.json, else built in; PIMAX_CTRL_TRACK_IMU=0 turns it off), the controller swings
      around a held elbow that follows the head position and half its yaw, rest pose after PIMAX_CTRL_TRACK_LOST_S
      (4 s); back in view it slides at most 0.5 m/s. Logs gyro + optical fixes to <session dir>/ctrl-imu-{left,right}.csv
      (pimax-ctrltrack imufit / occlusion) and, with --6dof, the head IMU to head-imu.csv (Record tracking replays).
      See analysis/controller-6dof.md, analysis/linux-recordings-20261016.md.
      --controllers (default when-held; also `omarchyvr vr start`, default always there): when-held = a controller is
      shown (pose, model, ray, buttons) only while it is in the hand: moved, tilted or a button touched picks it up,
      5 s still or no reports for 1.5 s puts it down (PIMAX_CTRL_INHAND=auto for Monado, docs/CONTROLLERS-INHAND.md);
      always = as before; off = never. Change it live with `omarchyvr controllers use when-held|always|off`.
      --ctrl-angle DEG (-20..70; default the saved setting, ctrl_angle in setup.toml, else 30): how far the
      controller pointer is tilted up (PIMAX_CTRL_ANGLE for Monado; live: `omarchyvr controllers angle DEG`).
      Tracking switches (all on by default, docs/TRACKING-STABILITY.md): --old-exposure = the previous camera exposure
      law (PIMAX_CAM_AE_LAW=old); --no-room-glide = room corrections snap instead of gliding (PIMAX_ANCHOR_GLIDE=0);
      --no-ctrl-steady = the controller fuser's old 80 ms optical window (PIMAX_CTRL_TRACK_HYST=0).
      --no-drift-hold = the head position follows the tracker even when the accelerometer says it is still (PIMAX_SLAM_DRIFT_HOLD=0).
      Controller rays aim where you look (PIMAX_CTRL_AIM=head for Monado; set PIMAX_CTRL_AIM=level to keep the old
      straight-ahead rays); hand placement: PIMAX_CTRL_REST (desk default, legacy), see analysis/hands-desk.md.
      --hands (with --6dof): camera hand tracking (Mercury) on the one Monado build, when it has the hands code and the models
      exist, else a warning and no hands. --hands-roles: the hands take the controller roles. See `omarchyvr vr help`, docs/HANDS.md.
      --voice: voice captions and commands: `omarchyvr-voice listen` (headset mic -> local speech-to-text, socket
      $XDG_RUNTIME_DIR/omarchyvr-voice.sock), `omarchyvr-voice captions` (the words on a WayVR panel under the main screen,
      driven with wayvrctl; nothing is typed) and `omarchyvr-voice act` (\"Pimax ...\" voice commands, docs/VOICE.md). --voice-hud adds the old head-locked `omarchyvr-voice-hud` overlay too.
      Performance readout on the RIGHT wrist (WayVR panel `perf`, like the watch but not grabbable), by default:
      HMD fps vs 72/90, main app fps, frames missed in the last 5 s, app CPU/GPU ms; green, yellow under 95 %,
      red under 85 %. Monado writes $XDG_RUNTIME_DIR/omarchyvr-perf.stats (PIMAX_PERF_FILE), `omarchyvr-perf run` shows it.
      --no-perf turns it off.
      Camera (passthrough / --6dof) missing while the headset HID is present (USB3 side dropped): the start first restarts
      the headset ONCE (`reset-headset` sys mode, the keepalive back as soon as it leaves USB) and waits up to 60 s for the
      camera (progress lines), then continues; no second reset. If the camera stays away the start stops and points to the
      app's 'Get the headset ready' guide. It refuses while monado-service runs. --no-auto-recover skips this.
      --preset (default work-4): the locked, fixed screen layout (docs/SCREENS.md \"Locked layouts\"; the screens never
      move by themselves or by hand, `desk recenter` moves the whole layout): work-4 (alias work: 1 main, 2 left, 3 right,
      4 up; 1.6 m wide at 2 m), focus-3 (alias focus: 1, 2, 3 close, 1.2 m at 1.3 m), work-5 (1, 2, 3, 5 up-left,
      6 up-right), work-6 (all six), cinema (one big curved screen, passthrough dimmed: --gain 0.35), cinema-huge
      (aliases imax, huge-cinema, theater: one 11 m curved screen at 8 m = 68 degrees of view, the room black, backdrop
      off; docs/CINEMA.md), none. Old names: work-3 = focus-3, work-1 = cinema, assistant = work-6 (+ --voice captions).
      Written to conf.d/60-preset.yaml every start (with desk_locked: true; removed with none); --screens is raised to
      the outputs the layout needs; the layout is applied over wayvrctl once WayVR runs.
      --screens N (1-6, default 6): VR screens = Hyprland outputs. The main desktop output is the centre screen; up to
      five headless outputs PIMAX-LEFT, PIMAX-RIGHT, PIMAX-UP, PIMAX-UPLEFT, PIMAX-UPRIGHT (the main one's size,
      left/right/above it and above left/right) are created
      BEFORE Monado (hyprctl eval 'hl.monitor(...)' + hyprctl output create headless; ~/.config/hypr is not edited)
      and removed by `desk stop` once Monado is gone. Idempotent; stale outputs of a crashed session are removed or
      reused; if creating fails the session starts with one screen and <session dir>/screens.txt says why.
      `omarchyvr desk screens watch` (started after WayVR) shows a VR screen only while it has an app or the focus
      (wayvrctl desk-screens); docs/SCREENS.md.
      Desk mode always hides the WayVR keyboard (conf.d/35-desk.yaml: enable_keyboard: false).
      Logs per process in the session dir: monado.log, passthrough.log, wayvr.log, wayvr.stdout (+ voice.log, voicecap.log, voicehud.log).
  preset NAME [--dry-run]
      switch the running desk session to a layout live (no restart): rewrites conf.d/60-preset.yaml, sets the
      passthrough (dim/normal/off, control socket), `wayvrctl desk-screens --place` with the layout's numbers (WayVR
      builds it around the stored layout anchor, never around where the head looks; --replace is accepted and changes
      nothing). Used by voice commands (omarchyvr-voice act) and the app's Layout picker.
  recenter [--dry-run]
      the whole screen layout in front of you (yaw only, same shape and height relation): `wayvrctl desk-recenter`;
      WayVR keeps the anchor for the next start. Prints one plain line; exit 1 when no Work session runs.
  captions on|off|status
      the live subtitles of the running desk session: off and on flip the flag file omarchyvr-voice reads
      ($XDG_RUNTIME_DIR/omarchyvr-voice-captions.off, like the voice commands \"captions on/off\"); on also starts
      `omarchyvr-voice listen` and `omarchyvr-voice captions` when they are missing. Needs a session started with voice
      (--voice or the assistant preset): only those have the captions panel in WayVR.
  screens status|sync|watch|create [--screens N]|remove [--dry-run]
      the VR screens (docs/SCREENS.md): status = the outputs and what VR shows; sync = send the visibility to WayVR
      once; watch = keep it in sync (the session's leaf process); create/remove = the outputs, refused while a
      monado-service runs (desk start/stop do this themselves).
  screens show N | more K | add SLOT | hide N[,N..]|others [--dry-run]
      (voice) change which VR screens show in the running session; prints ONE plain line for the headset toast and
      exits 0, or a plain error and exits 1. show N (1-6): the main screen + the first N-1 of left, right, up,
      up-left, up-right are pinned and shown (above 6: 6, \"That's the most: 6 screens\"). more K: the screens
      shown now + K. add SLOT (left|right|up|up-left|up-right): that one as well. Outputs are only made by
      `desk start` (WayVR learns them at its start), so a screen the session lacks is not made live: the line says
      \"6 screens need Screens = 6 in the Pimax app and a Work restart\", the screens that exist are shown and the
      recorded Screens setting is raised so a restart has them. hide N[,N..] (screen numbers 2-6 or slot names):
      those screens no longer show, even with apps on them; hide others: only the main screen (\"close
      everything\"). Never closes, kills or moves an app.
  restart [--session-dir DIR]
      stop with grace 30, then start again with the SAME arguments as the last `desk start` (recorded in
      <session dir>/start-argv.txt on every start). The camera is recovered automatically if it went missing.
      start ... [--no-watchdog]: a detached watchdog (`omarchyvr desk watchdog`, log <session dir>/watchdog.log) restarts the
      session when monado-service dies (stop without SIGKILL -> start, at most 3 times per 10 minutes). An unplugged
      headset is waited for without spending tries and the session is restored once it is back; a camera that does not
      come back within 20 s gets one automatic headset restart, then you are asked. No automatic restart after a GPU
      error that needs a reboot. `desk stop` is never fought. Not started with --wait or --dry-run.
  stop [--grace SECS] [--no-kill] [--keep-ai] [--dry-run] [--no-daemon]
      the one full stop of Work (the app's Stop, Exit Work in the headset, a switch to Play): SIGINT to the voice
      processes and the leaves (if started), WayVR (waits until it has exited: 15 s, then a notice and 15 s
      more), then passthrough (by pid, 10 s each), the local AI (every omarchyvr-ai slot: the model, Lara, the speech
      server, the agent model, the VRAM watch; by their pid files), then Monado: SIGINT, SIGKILL only after --grace
      (default 30 s); then the kernel log check (Xid / Idling display engine), the camera broker, the VR screens'
      outputs, and a check that nothing of Work still runs (named as a problem otherwise). --no-kill (automatic
      callers: the watchdog, auto mode): never SIGKILL Monado; a hung one is left alive and the GPU gate says reboot.
      --keep-ai (only a supervisor restart of a running Work session: `desk restart`, the watchdog): the local AI stays loaded. Every other stop, omarchyvrd's own (headset unplugged, automatic switch) included, unloads it.
      --dry-run: print what would be signalled, in order, and touch nothing.
  exit [--now]
      Exit Work from the headset (the Work menu's and the watch's \"Exit Work\" button): the first press asks for a
      second one within 6 s (a toast in the headset); the second press starts `omarchyvr desk stop --grace 30`
      detached from WayVR (log <session dir>/exit.log), so the stop outlives WayVR's own exit. --now: no question.
      Refused when no Work session runs.
  status
      pids of the desk processes and `omarchyvr vr status` (plus a omarchyvrd line when it runs).
  omarchyvrd: with an executing omarchyvrd, `start` asks it to start the session and follows it (see `omarchyvr vr help`);
      start/stop --no-daemon (or PIMAX_NO_DAEMON=1), --dry-run, --wait and --force take the direct path.
environment: PIMAX_MONADO_BUILD (default ~/.local/lib/omarchyvr/monado if built, else ~/src/monado/build), PIMAX_VR_SESSION_DIR.";

/// Seconds a client gets to exit after SIGINT.
const CLIENT_GRACE: Duration = Duration::from_secs(10);
/// Default grace of Monado in `desk stop`.
const MONADO_GRACE_SECS: f64 = 30.0;

/// The wayvr CPU capture config written when `conf.d/10-am.yaml` does not exist yet.
const WAYVR_CONF: &str =
    "capture_method: screencopy\nblock_game_input: true\nnotifications_sound_enabled: false\n";

/// Desk-mode screen handling (`conf.d/30-worldlock.yaml`, REWRITTEN on every start so stale or duplicate keys cannot
/// break WayVR): grip holds a screen rigidly, stick = push/pull (size follows), both grips = resize/rotate, release =
/// world-locked (Static) and saved. A/X pin is off unless `world_lock_pin: true`.
const WAYVR_WORLDLOCK_CONF: &str = "world_lock_grab: true
allow_sliding: true
world_lock_pin: false
grab_keep_apparent_size: true
grab_dist_rate: 0.8
grab_size_min: 0.3
grab_size_max: 6.0
grab_dist_min: 0.4
grab_dist_max: 8.0
";

/// `conf.d/35-desk.yaml`: no keyboard overlay in desk mode (nothing is typed; voice shows captions instead), and the
/// left controller's menu button opens the Work menu panel (`menu_panel`; WayVR's `menu` action).
const WAYVR_DESK_CONF: &str = "enable_keyboard: false\nmenu_panel: work_menu\n";

/// The Work menu (custom panel `work_menu`): either controller's menu button opens it in front of you; it is the VR
/// home's panel in Work (`home::panel_xml`, docs/VR-HOME.md): VR home, Games with SteamVR, the games, Room setup,
/// Close menu, Recenter, WayVR settings and "Exit Work", which asks for a second press, then stops everything
/// (`omarchyvr desk exit`, docs/WAYVR-CONTROLS.md). Hands: the watch on the left wrist has the same Exit Work button.
pub const WORK_MENU_NAME: &str = "work_menu";

/// The caption panel: two single-line labels in a translucent box; `omarchyvr-voice captions` sets cap_l1/cap_l2 text and
/// shows/hides cap_box over wayvrctl (the outer div keeps the panel size when the box is hidden).
const CAPTIONS_XML: &str = r##"<layout>
  <elements>
    <div id="cap_outer" width="1200" height="150">
      <rectangle id="cap_box" width="100%" height="100%" padding="14" flex_direction="column" justify_content="center" gap="4" color="#000000c8" round="16">
        <label id="cap_l1" size="40" weight="bold" color="#ffffff" />
        <label id="cap_l2" size="40" weight="bold" color="#ffffff" />
      </rectangle>
    </div>
  </elements>
</layout>
"##;

/// Everything `desk start` needs.
#[derive(Debug, Clone)]
pub struct DeskOptions {
    /// The options of the Monado part (same as `vr start`).
    pub vr: StartOptions,
    /// Start `omarchyvr-passthrough run`.
    pub passthrough: bool,
    /// `--eye` was given.
    pub eye: bool,
    /// Eye tracking: silent (default when possible), pointer (`--eye`) or off (`--no-eye`); see `eye_desk`.
    pub eye_mode: super::eye_desk::EyeMode,
    /// Why eye tracking is off, if it is off without `--no-eye`.
    pub eye_note: Option<String>,
    /// `--hands`: camera hands (Mercury) and the note for the start output.
    pub hands: super::hands::Hands,
    /// `--6dof`: camera broker + live SLAM.
    pub sixdof: Option<SixDof>,
    /// `omarchyvr-passthrough` binary.
    pub passthrough_bin: PathBuf,
    /// `wayvr` binary.
    pub wayvr_bin: PathBuf,
    /// Directory with `config/`, `data/` and `state/` for WayVR (`XDG_*_HOME`).
    pub wayvr_home: PathBuf,
    /// Directory with `libopenvr_api.so` (the WayVR build links it dynamically), if it exists.
    pub openvr_lib: Option<PathBuf>,
    /// Seconds each client must stay alive before the next one is started.
    pub settle: Duration,
    /// `--voice`: start `omarchyvr-voice listen` and `omarchyvr-voice-hud` after WayVR.
    pub voice: bool,
    /// `--voice-hud`: also the head-locked XR overlay HUD.
    pub voice_hud: bool,
    /// `wayvrctl` (drives the caption panel).
    pub wayvrctl_bin: PathBuf,
    /// `omarchyvr-voice` binary.
    pub voice_bin: PathBuf,
    /// `omarchyvr-voice-hud` binary.
    pub voice_hud_bin: PathBuf,
    /// `--ctrl-track`: optical controller tracking (`pimax-ctrltrack live` on the broker + PIMAX_CTRL_TRACK=1).
    pub ctrl_track: bool,
    /// `pimax-ctrltrack` binary.
    pub ctrltrack_bin: PathBuf,
    /// The wrist performance readout (None with `--no-perf`).
    pub perf: Option<PerfOptions>,
    /// `--preset`: work-4 (default) / focus-3 / work-5 / work-6 / cinema / cinema-huge, None = `--preset none` (see `preset`).
    pub preset: Option<super::preset::Preset>,
    /// `desk preset NAME --replace`: accepted for old callers; every switch places the layout now (locked layouts).
    pub replace: bool,
    /// `--screens N`: VR screens (Hyprland outputs), 1 to 4 (see `screens`).
    pub screens: u8,
    /// `omarchyvr` itself (runs the screens watcher).
    pub pimax_bin: PathBuf,
}

impl DeskOptions {
    pub(crate) fn pid_file(&self, name: &str) -> PathBuf {
        self.vr.session_dir.join(format!("{name}.pid"))
    }
}

/// Parse `desk start|stop|status` options.
pub fn parse_desk(
    a: &mut Args,
    env: &dyn Fn(&str) -> Option<String>,
) -> Result<DeskOptions, CliError> {
    let home = PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into()));
    let no_passthrough = a.flag("--no-passthrough");
    let eye = a.flag("--eye");
    let no_eye = a.flag("--no-eye");
    let voice = a.flag("--voice");
    let voice_hud = a.flag("--voice-hud");
    let no_perf = a.flag("--no-perf");
    let (preset, preset_captions) = super::preset::parse_opt_captions(a)?;
    let screens = match a.opt("--screens")? {
        Some(v) => super::screens::parse_count(&v).map_err(CliError::usage)?,
        None => super::screens::DEFAULT,
    };
    // the outputs the layout needs (work-5 shows up-right = screen 6); never lowered
    let screens = screens.max(preset.map(|p| p.outputs_needed()).unwrap_or(1));
    let pimax_bin = a
        .opt("--pimax-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config::tools_dir(&home).join("omarchyvr"));
    let voice_bin = a
        .opt("--voice-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config::tools_dir(&home).join("omarchyvr-voice"));
    let voice_hud_bin = a
        .opt("--voice-hud-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config::tools_dir(&home).join("omarchyvr-voice-hud"));
    let passthrough_bin = a
        .opt("--passthrough-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| super::config::tools_dir(&home).join("omarchyvr-passthrough"));
    let wayvr_bin = a
        .opt("--wayvr-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| omarchyvr_config::paths::wayvr_bin(&home));
    let wayvrctl_bin = a
        .opt("--wayvrctl-bin")?
        .map(PathBuf::from)
        .unwrap_or_else(|| wayvr_bin.with_file_name("wayvrctl"));
    let wayvr_home = a
        .opt("--wayvr-home")?
        .map(PathBuf::from)
        .unwrap_or_else(|| omarchyvr_config::paths::wayvr_home(&home));
    let settle: f64 = a.opt_parse("--settle")?.unwrap_or(6.0);
    if !settle.is_finite() || !(0.0..=120.0).contains(&settle) {
        return Err(CliError::usage("--settle must be 0..120 seconds"));
    }
    let build_given =
        a.0.iter()
            .any(|x| x == "--monado-build" || x.starts_with("--monado-build="))
            || env("PIMAX_MONADO_BUILD").is_some_and(|v| !v.is_empty());
    let mut vr = parse_options(a, env)?;
    let eye_build = omarchyvr_config::paths::monado_build(&home);
    let sixdof = sixdof::parse(a, env, &mut vr, &home, build_given, Some(eye_build.clone()));
    let ctrl_track_bin = sixdof::parse_ctrl_track(a, env, &mut vr, &home, &sixdof)?;
    let ctrl_track = ctrl_track_bin.is_some();
    // Work: a controller is shown only while it is in the hand (PIMAX_CTRL_INHAND=auto) unless --controllers says
    let _controllers = sixdof::parse_controllers(a, env, &mut vr, omarchyvr_config::ctrl_inhand::CtrlMode::WhenHeld)?;
    sixdof::parse_ctrl_angle(a, env, &mut vr)?;
    // the phase 1 tracking switches (exposure law, room glide, steadier controllers): on unless their flag says off
    sixdof::parse_tracking_switches(a, env, &mut vr);
    let ctrltrack_bin = ctrl_track_bin.unwrap_or_else(|| sixdof::default_ctrltrack_bin(&home));
    let (eye_mode, eye_note) = super::eye_desk::parse(eye, no_eye, &mut vr, &eye_build, build_given, env);
    // --hands after eye and 6DoF: it may move the build to the hands build (a superset of the desk build)
    let hands = super::hands::parse(a, env, &mut vr, &home, build_given, sixdof.is_some())?;
    // The controllers are not tracked: in desk mode their rays aim where the head looks (rust/pimax-hands rest.rs),
    // so pointing at a panel is "look at it, pull the trigger". The caller's own PIMAX_CTRL_AIM wins.
    if env("PIMAX_CTRL_AIM").is_none() {
        vr.extra_env
            .push(("PIMAX_CTRL_AIM".into(), DESK_CTRL_AIM.into()));
    }
    let perf = if no_perf {
        None
    } else {
        Some(perf::parse(a, env, &mut vr, &home, &wayvr_bin, &wayvrctl_bin)?)
    };
    // libopenvr_api.so for WayVR: next to the installed wayvr, else a developer clone (paths::openvr_lib_dir)
    let openvr_lib = omarchyvr_config::paths::openvr_lib_dir(&home);
    Ok(DeskOptions {
        vr,
        passthrough: !no_passthrough,
        eye,
        eye_mode,
        eye_note,
        hands,
        sixdof,
        passthrough_bin,
        wayvr_bin,
        openvr_lib,
        wayvr_home,
        settle: Duration::from_secs_f64(settle),
        voice: voice || preset_captions,
        voice_hud,
        wayvrctl_bin,
        voice_bin,
        voice_hud_bin,
        ctrl_track,
        ctrltrack_bin,
        perf,
        preset,
        replace: a.flag("--replace"),
        screens,
        pimax_bin,
    })
}

// The process mechanics (spawn spec, spawner, polite stop by pid, settle wait) and the per-component spawn specs
// live in `omarchyvr-components` (resilience plan P3); the launcher composes them.
pub use omarchyvr_components::components::proc::{log_tail, stays_alive, stop_client, ClientStop, ProcSpec, RealSpawner, Spawner};
pub use omarchyvr_components::components::wayvr::{end_hung_wayvr, stop_wayvr, HungEnd, WAYVR_EXTRA, WAYVR_GRACE};
use omarchyvr_components::components as comp;

/// The clients to start, in order.
pub fn plan(o: &DeskOptions) -> Vec<ProcSpec> {
    let dir = &o.vr.session_dir;
    let runtime = o.vr.runtime_json();
    let mut v = Vec::new();
    if let (true, Some(sd)) = (o.ctrl_track, &o.sixdof) {
        v.push(sixdof::ctrltrack_spec(&o.ctrltrack_bin, sd, dir));
    }
    if o.passthrough {
        let preset: &[&str] = o.preset.map(|p| p.passthrough_args()).unwrap_or(&[]);
        v.push(comp::passthrough::spec(&o.passthrough_bin, preset, &runtime, o.sixdof.is_some(), dir));
    }
    v.push(comp::wayvr::desk_spec(&comp::wayvr::Params {
        bin: &o.wayvr_bin,
        home: &o.wayvr_home,
        openvr_lib: o.openvr_lib.as_deref(),
        runtime_json: &runtime,
        session_dir: dir,
        // the watch's and the Work menu's buttons run `omarchyvr ...` (Exit Work) and `omarchyvr-voice ...`
        tools_dir: o.pimax_bin.parent(),
    }));
    if o.screens > 1 {
        // not an XR client: Hyprland events -> which VR screens WayVR shows (wayvrctl desk-screens)
        v.push(screens_spec(o));
    }
    if let Some(p) = &o.perf {
        // not an XR client: stats file of Monado -> panel `perf` of the WayVR above, through wayvrctl
        v.push(perf::perf_spec(p, dir));
    }
    if o.voice {
        // not XR clients: the mic + recogniser + socket, the captions panel, the voice commands
        v.push(comp::voice::listen_spec(&o.voice_bin, dir));
        v.push(comp::voice::captions_spec(&o.voice_bin, &o.wayvrctl_bin, dir));
        v.push(comp::voice::act_spec(&o.voice_bin, &o.wayvrctl_bin, dir));
        if o.voice_hud {
            // an XR_EXTX_overlay client (head-locked quad at the bottom of the view)
            v.push(comp::voicehud::spec(&o.voice_hud_bin, &runtime, dir));
        }
    }
    v
}

/// `omarchyvr desk screens watch`: the on-demand VR screens (leaf; see `screens`).
pub fn screens_spec(o: &DeskOptions) -> ProcSpec {
    ProcSpec {
        name: super::screens::PROC_NAME,
        program: o.pimax_bin.clone(),
        args: vec!["desk".into(), "screens".into(), "watch".into(), "--wayvrctl-bin".into(), o.wayvrctl_bin.to_string_lossy().into_owned()],
        env: Vec::new(),
        log: o.vr.session_dir.join("screens.log"),
    }
}

/// The clients `desk stop` signals, in order (the voice ones only when their pid file exists).
pub(crate) fn stop_order(o: &DeskOptions) -> Vec<(&'static str, &PathBuf)> {
    let mut v = Vec::new();
    for (name, bin) in [
        ("voicehud", &o.voice_hud_bin),
        ("voiceact", &o.voice_bin),
        ("voicecap", &o.voice_bin),
        ("voice", &o.voice_bin),
        (super::screens::PROC_NAME, &o.pimax_bin),
    ] {
        if o.pid_file(name).exists() {
            v.push((name, bin));
        }
    }
    if let (Some(p), true) = (&o.perf, o.pid_file("perf").exists()) {
        v.push(("perf", &p.perf_bin));
    }
    v.push(("wayvr", &o.wayvr_bin));
    v.push(("passthrough", &o.passthrough_bin));
    if o.pid_file("ctrltrack").exists() {
        v.push(("ctrltrack", &o.ctrltrack_bin));
    }
    v
}

/// Create WayVR's isolated XDG directories and the CPU capture config if missing. Returns a warning if an existing
/// config does not select `capture_method: screencopy` (the dmabuf capture of WayVR 26.8.0 can Xid at exit).
pub fn ensure_wayvr_home(home: &Path, voice: bool, perf: bool) -> io::Result<Option<String>> {
    ensure_wayvr_home_with(home, voice, perf, &super::home::panel_xml(super::home::Ctx::Work, &[], ""))
}

/// [`ensure_wayvr_home`] with the Work menu's layout (`home::panel_xml` in Work: the VR home's entries plus Exit Work,
/// Recenter and WayVR settings; `desk start` passes the installed games and the last transition's words).
pub fn ensure_wayvr_home_with(home: &Path, voice: bool, perf: bool, work_menu_xml: &str) -> io::Result<Option<String>> {
    let conf_d = home.join("config/wayvr/conf.d");
    std::fs::create_dir_all(&conf_d)?;
    std::fs::create_dir_all(home.join("data"))?;
    std::fs::create_dir_all(home.join("state"))?;
    // desk mode never shows the WayVR keyboard (rewritten every start; the user's own conf.d files are not touched)
    std::fs::write(conf_d.join("35-desk.yaml"), WAYVR_DESK_CONF)?;
    // custom panels (gui/<name>.xml in the theme folder): captions only in voice sessions, perf (right wrist) unless
    // --no-perf; one list because a later conf.d file replaces the list of an earlier one
    let _ = std::fs::remove_file(conf_d.join("40-captions.yaml"));
    let panels_conf = conf_d.join("40-panels.yaml");
    let mut panels: Vec<&str> = Vec::new();
    let gui = home.join("config/wayvr/theme/gui");
    // the Work menu (the controller's menu button; Exit Work), in every desk session
    std::fs::create_dir_all(&gui)?;
    std::fs::write(gui.join(format!("{WORK_MENU_NAME}.xml")), work_menu_xml)?;
    panels.push(WORK_MENU_NAME);
    if voice {
        std::fs::create_dir_all(&gui)?;
        std::fs::write(gui.join("captions.xml"), CAPTIONS_XML)?;
        panels.push("captions");
    }
    if perf {
        perf::write_panel(home)?;
        panels.push("perf");
    }
    if panels.is_empty() {
        let _ = std::fs::remove_file(&panels_conf);
    } else {
        let list: String = panels.iter().map(|p| format!("  - {p}\n")).collect();
        std::fs::write(&panels_conf, format!("custom_panels:\n{list}"))?;
    }
    let wl = conf_d.join("30-worldlock.yaml");
    std::fs::write(&wl, WAYVR_WORLDLOCK_CONF)?;
    let f = conf_d.join("10-am.yaml");
    if !f.exists() {
        std::fs::write(&f, WAYVR_CONF)?;
        return Ok(None);
    }
    let text = std::fs::read_to_string(&f)?;
    let ok = text
        .lines()
        .any(|l| l.trim().replace(' ', "") == "capture_method:screencopy");
    Ok((!ok).then(|| {
        format!(
            "{} has no `capture_method: screencopy`: WayVR would use the GPU (dmabuf) capture, which can trigger an NVIDIA Xid at exit",
            f.display()
        )
    }))
}

/// The parts a Work session runs without, in plain words, one per line (`<session dir>/degraded.txt`, next to the logs);
/// a full start removes it.
pub const DEGRADED_FILE: &str = "degraded.txt";

/// The one client Work cannot do without: WayVR (the desktop in the headset). Every other client is optional.
pub fn required_client(name: &str) -> bool {
    name == "wayvr"
}

/// What a session without this client means, in plain words, with the reason.
pub fn degraded_note(name: &str, reason: &str) -> String {
    let what = match name {
        "ctrltrack" => "Optical controller tracking is off this session: the controllers still work (buttons and pointing), without their position from the cameras",
        "passthrough" => "The room view (passthrough) is off this session: the screens show on a dark background",
        "perf" => "The performance readout on the wrist is off this session",
        "voice" | "voicecap" | "voiceact" => "Voice is off this session (captions and voice commands)",
        "voicehud" => "The voice HUD is off this session",
        n if n == super::screens::PROC_NAME => "The VR screens follow the preset only this session (they do not follow your apps)",
        _ => "A part of Work is off this session",
    };
    format!("{what} ({name}: {reason}).")
}

/// A start that cannot go on: stop what THIS start began so nothing is left half-running. `monado_spawned`: this
/// start launched a monado-service (its pid is new in the pid file): then everything goes (clients, Monado by pid, the
/// broker, the VR screens' outputs) through the one stop path. Otherwise (the preflight refused, another session runs)
/// only this start's broker and screen outputs go; a running session is never touched. The local AI is not touched
/// (Work starts it only after a good start).
fn abort_start(o: &DeskOptions, d: &Deps, why: &str, monado_spawned: bool) {
    verr!("omarchyvr desk: the start did not complete ({why}): stopping what this start began");
    if monado_spawned {
        let kill = !super::supervise::in_watchdog();
        if let Err(e) = stop(o, Duration::from_secs_f64(super::config::DEFAULT_STOP_GRACE_SECS * 6.0), kill, true, d) {
            verr!("omarchyvr desk: WARNING: {}", e.msg);
        }
        return;
    }
    if let Some(sd) = &o.sixdof {
        sixdof::stop_broker(&o.vr.session_dir, &sd.cam_bin, d.procs, d.clock);
    }
    // the outputs go only once no monado-service runs (remove_screens checks it)
    remove_screens(o, d, &mut super::screens::RealHypr);
}

/// `desk start`.
pub fn start(o: &DeskOptions, d: &Deps, sp: &dyn Spawner) -> CliResult {
    let mut specs = plan(o);
    // the WayVR settings of the Pimax app (~/.config/omarchyvr/wayvr-desk.conf): conf.d/50-pimax-app.yaml below, and the
    // screen the captions sit under (WayVR reads PIMAX_CAPTIONS_SCREEN from its own environment)
    let (app_wayvr, notes) = wayvr_conf::load(&wayvr_conf::settings_path(d.env));
    for n in notes {
        verr!("omarchyvr desk: {}: {n}", wayvr_conf::settings_path(d.env).display());
    }
    if let Some(screen) = app_wayvr.as_ref().and_then(|w| w.captions_screen.clone()).filter(|_| app_wayvr.as_ref().is_some_and(|w| w.validate().is_empty())) {
        for s in specs.iter_mut().filter(|s| s.name == "wayvr") {
            s.env.push(("PIMAX_CAPTIONS_SCREEN".into(), screen.clone()));
        }
    }
    // Only WayVR (the desk itself) is required. Every other client is optional: a missing one is left out with a plain
    // note and Work starts without it (2026-10-17: a missing controller model made the whole Work start fail).
    let mut without: Vec<String> = Vec::new();
    let mut i = 0;
    while i < specs.len() {
        let s = &specs[i];
        if s.program.exists() {
            i += 1;
            continue;
        }
        if required_client(s.name) {
            return Err(CliError::new(format!(
                "{} does not exist (run ./install.sh again in your omarchyvr clone)",
                s.program.display()
            )));
        }
        let note = degraded_note(s.name, &format!("{} is not installed; ./install.sh puts it back", s.program.display()));
        verr!("omarchyvr desk: {note}");
        without.push(note);
        specs.remove(i);
    }
    if let Some(sd) = &o.sixdof {
        sixdof::check(&o.vr, sd, d.env)?;
    }
    // where the headset data comes from: the user's own or the shipped reference (the job log says it)
    vout!("omarchyvr desk: {}", crate::import_cmd::start_line(&home_of(d), d.env));
    if o.passthrough && o.sixdof.is_none() {
        for k in ["PIMAX_CAM", "PIMAX_HANDS"] {
            if (d.env)(k).is_some_and(|v| !v.is_empty()) {
                verr!("omarchyvr desk: WARNING: {k} is set: Monado would open the camera and passthrough could not (unset it for desk mode)");
            }
        }
    }
    if o.vr.dry_run {
        vout!("omarchyvr desk: would start, in this order:");
        if let Some(sd) = &o.sixdof {
            vout!("  0. {} export --out {}, then {} broker (socket {})",
                sd.slam_bin.display(), sd.basalt_dir.display(), sd.cam_bin.display(), sd.sock.display());
        }
        if o.screens > 1 {
            vout!(
                "  1a. VR screens: {} (the main desktop output + {} as headless outputs of its size, created before Monado; hyprctl eval 'hl.monitor(...)', hyprctl output create headless)",
                o.screens,
                super::screens::extra_slots(o.screens).iter().filter_map(|s| s.output()).collect::<Vec<_>>().join(", ")
            );
        }
        vout!("  1. monado-service ({})", o.vr.service_path().display());
        for (i, s) in specs.iter().enumerate() {
            vout!(
                "  {}. {} {}  (log {})",
                i + 2,
                s.program.display(),
                s.args.join(" "),
                s.log.display()
            );
            for (k, v) in &s.env {
                vout!("       {k}={v}");
            }
        }
        return session::start(&o.vr, d);
    }
    let menu = super::home::panel_xml(super::home::Ctx::Work, &super::home::games(&home_of(d), d.env), &super::home::start_status(&o.vr.session_dir));
    match ensure_wayvr_home_with(&o.wayvr_home, o.voice, o.perf.is_some(), &menu) {
        Ok(Some(w)) => verr!("omarchyvr desk: WARNING: {w}"),
        Ok(None) => {}
        Err(e) => {
            return Err(CliError::new(format!(
                "cannot prepare {}: {e}",
                o.wayvr_home.display()
            )))
        }
    }
    if let Err(e) = super::preset::write_conf(&o.wayvr_home, o.preset) {
        verr!("omarchyvr desk: WARNING: preset not applied: {e}");
    }
    match wayvr_conf::apply(&o.wayvr_home, app_wayvr.as_ref()) {
        Ok(Some(n)) => vout!("omarchyvr desk: {n}"),
        Ok(None) => {}
        Err(e) => verr!("omarchyvr desk: WARNING: the app's WayVR settings were not applied: {e}"),
    }
    match super::eye_desk::apply_conf(&o.wayvr_home, o.eye_mode, &omarchyvr_config::eye::EyeSettings::load(&omarchyvr_config::eye::default_path()),
        &super::eye_desk::calibration_key(&super::eye_desk::calibration_path(&|k| std::env::var(k).ok()))) {
        Ok(n) => {
            if let Some(n) = n.or_else(|| o.eye_note.clone()) {
                vout!("omarchyvr desk: {n}");
            }
        }
        Err(e) => verr!("omarchyvr desk: WARNING: the eye tracking config was not written: {e}"),
    }
    let mut vr = o.vr.clone();
    vr.wait = false;
    if let Some(sd) = &o.sixdof {
        sixdof::start_broker(&vr, sd, d, sp, Duration::from_secs(10))?;
    }
    prepare_screens(o, d, app_wayvr.as_ref().is_some_and(|w| w.auto_arrange));
    let pid_before = session::read_pid(&vr.pid_path());
    if let Err(e) = session::start(&vr, d) {
        // nothing half-started stays behind: a Monado that is still up (no gyro line in time), the broker, the VR
        // screens' outputs (2026-10-17 parity audit)
        let pid_after = session::read_pid(&vr.pid_path());
        abort_start(o, d, &e.msg, pid_after.is_some() && pid_after != pid_before);
        return Err(e);
    }
    for s in &specs {
        vout!("omarchyvr desk: starting {} ({})", s.name, s.program.display());
        let pid = match comp::proc::start_client(s, &o.vr.session_dir, sp, d.procs, d.clock, o.settle) {
            Ok(p) => p,
            Err(err) => {
                let reason = match err {
                    comp::proc::StartError::Spawn(e) => format!("cannot start {}: {e}", s.program.display()),
                    comp::proc::StartError::DiedEarly { tail, .. } => {
                        verr!("omarchyvr desk: {} exited right after the start. Last log lines:", s.name);
                        for l in &tail {
                            verr!("  {l}");
                        }
                        // the client's own last words are the plain reason when it gives one (pimax-ctrltrack does)
                        let last = tail.iter().rev().find(|l| !l.trim().is_empty()).map(|l| l.trim().to_string());
                        format!("it ended during start-up{} (log {})", last.map(|l| format!(": {l}")).unwrap_or_default(), s.log.display())
                    }
                };
                if required_client(s.name) {
                    let msg = format!("{}: {reason}", s.name);
                    abort_start(o, d, &msg, true);
                    d.notifier.notify(true, "Pimax desk: not started", &msg);
                    return Err(CliError::new(format!("{msg}; everything that was started is stopped again")));
                }
                let note = degraded_note(s.name, &reason);
                verr!("omarchyvr desk: {note}");
                without.push(note);
                continue;
            }
        };
        vout!(
            "omarchyvr desk: {} running, pid {pid}, log {}",
            s.name,
            s.log.display()
        );
    }
    // the parts this session runs without, in plain words, next to the logs (a full start removes the file)
    let degraded = o.vr.session_dir.join(DEGRADED_FILE);
    if without.is_empty() {
        let _ = std::fs::remove_file(&degraded);
    } else {
        let _ = std::fs::write(&degraded, without.join("\n") + "\n");
        d.notifier.notify(true, "Pimax desk ready, with parts off", &without.join(" "));
    }
    if let Some(p) = o.preset {
        // the layout (and, with one output, the app panels) once WayVR runs; best effort
        let panels = super::preset::parse_panels(
            std::fs::read_to_string(super::preset::panels_path(&home_of(d))).ok().as_deref(),
        );
        let vis = super::screens::preset_visibility(p, &mut super::screens::RealHypr);
        let _ = super::preset::apply_live(
            p,
            &o.wayvrctl_bin,
            &super::preset::passthrough_socket(d.env),
            &panels,
            vis.as_deref(),
            false,
            &mut |l| vout!("omarchyvr desk: preset {}: {l}", p.name()),
        );
    }
    // the VR background chosen in the app (Backgrounds page, `omarchyvr-backgrounds`, "Max, set background ..."): shown
    // once WayVR runs, over the preset's passthrough; best effort
    let bg = omarchyvr_backgrounds::Dirs::for_home(&home_of(d));
    // (not under cinema-huge: its dark room wins; the choice stays saved for the next mode)
    if !o.vr.dry_run && !o.preset.is_some_and(|p| p.skybox_off()) && omarchyvr_backgrounds::Setting::load(&bg).enabled {
        match omarchyvr_backgrounds::apply_to(&bg, &o.wayvrctl_bin, &super::preset::passthrough_socket(d.env)) {
            Ok(m) => vout!("omarchyvr desk: {m}"),
            Err(e) => verr!("omarchyvr desk: WARNING: background not applied: {e}"),
        }
    }
    d.notifier
        .notify(false, "Pimax desk ready", "Stop with: omarchyvr desk stop");
    vout!("omarchyvr desk: ready. Stop with: omarchyvr desk stop");
    Ok(())
}

/// The VR screens' outputs, before Monado starts (never while one runs): see `screens`. Never fails the start: the
/// report (and any error) goes to `<session dir>/screens.txt` and the output; the show state follows the preset.
fn prepare_screens(o: &DeskOptions, d: &Deps, auto_arrange: bool) {
    let running = !d.procs.find_by_comm(&o.vr.service_comm()).is_empty();
    let r = super::screens::prepare(o.screens, &mut super::screens::RealHypr, false, running);
    for l in r.lines() {
        if l.starts_with("error: ") {
            verr!("omarchyvr desk: WARNING: VR {l}");
        } else {
            vout!("omarchyvr desk: VR {l}");
        }
    }
    let _ = std::fs::create_dir_all(&o.vr.session_dir);
    let _ = std::fs::write(o.vr.session_dir.join(super::screens::REPORT_FILE), r.lines().join("\n") + "\n");
    // `sort` (apps hop to their own screen) follows the app's "Auto-arrange apps" switch, default off
    let st = super::screens::ShowState { sort: auto_arrange, ..o.preset.map(super::screens::ShowState::for_preset).unwrap_or_default() };
    if let Err(e) = super::screens::write_state(&super::screens::state_path(d.env), &st) {
        verr!("omarchyvr desk: WARNING: screens state: {e}");
    }
}

/// After the session: remove the VR screens' outputs, only once no monado-service runs (and not between the
/// watchdog's stop and start: the next start keeps them).
fn remove_screens(o: &DeskOptions, d: &Deps, hypr: &mut dyn super::screens::Hypr) {
    if super::supervise::in_watchdog() {
        return;
    }
    let running = !d.procs.find_by_comm(&o.vr.service_comm()).is_empty();
    let r = super::screens::remove_all(hypr, false, running);
    if !r.removed.is_empty() {
        vout!("omarchyvr desk: VR screens removed: {}", r.removed.join(", "));
    }
    if let Some(e) = r.error {
        verr!("omarchyvr desk: WARNING: VR screens: {e}");
    }
}

/// `desk screens status|sync|watch|create|remove`.
pub fn screens_cmd(o: &DeskOptions, what: &str, d: &Deps) -> CliResult {
    use super::screens::{self as sc, Hypr};
    let running = !d.procs.find_by_comm(&o.vr.service_comm()).is_empty();
    let mut h = sc::RealHypr;
    match what {
        "status" => {
            let report = std::fs::read_to_string(o.vr.session_dir.join(sc::REPORT_FILE)).unwrap_or_default();
            for l in report.lines() {
                vout!("last start: {l}");
            }
            let mons = sc::parse_monitors(&h.run(&["-j", "monitors", "all"]).map_err(CliError::new)?).map_err(CliError::new)?;
            let apps = sc::parse_client_counts(&h.run(&["-j", "clients"]).map_err(CliError::new)?).map_err(CliError::new)?;
            let st = sc::read_state(&sc::state_path(d.env));
            vout!("main output: {}", sc::main_output(&mons).map(|m| m.name.as_str()).unwrap_or("none"));
            vout!("state: limit {}, pinned {}", st.limit, st.pinned.iter().map(|s| s.key()).collect::<Vec<_>>().join(","));
            for (slot, name, on) in sc::visibility(&mons, &apps, &st) {
                vout!("{}: {name}: {}", slot.key(), if on { "shown" } else { "hidden" });
            }
            Ok(())
        }
        "sync" => {
            let st = sc::read_state(&sc::state_path(d.env));
            sc::sync_once(&mut h, &o.wayvrctl_bin, &st, None, true, o.vr.dry_run, &mut |m| vout!("omarchyvr desk: screens: {m}"))
                .map(|_| ())
                .map_err(CliError::new)
        }
        "watch" => {
            sc::watch(&o.wayvrctl_bin, &o.pid_file("wayvr"), d.env, d.stop_requested);
            Ok(())
        }
        "create" | "remove" => {
            let r = if what == "create" { sc::prepare(o.screens, &mut h, o.vr.dry_run, running) } else { sc::remove_all(&mut h, o.vr.dry_run, running) };
            for l in r.steps.iter().chain(r.lines().iter()) {
                vout!("{l}");
            }
            match r.error {
                Some(e) => Err(CliError::new(e)),
                None => Ok(()),
            }
        }
        other => Err(CliError::usage(format!("desk screens: status, sync, watch, create or remove, not {other}"))),
    }
}

/// `desk screens show N | more K | add SLOT | hide N,N|others` (voice): see `screens::run_show`. One plain line on stdout.
pub fn screens_show(o: &DeskOptions, sub: &str, arg: Option<&str>, d: &Deps) -> CliResult {
    use super::screens as sc;
    let cmd = sc::parse_show(sub, arg).map_err(CliError::usage)?;
    let running = !d.procs.find_by_comm(&o.vr.service_comm()).is_empty();
    let state_file = sc::state_path(d.env);
    let env = sc::ShowEnv { state_file: &state_file, session_dir: &o.vr.session_dir, wayvrctl: &o.wayvrctl_bin, running, dry_run: o.vr.dry_run };
    let line = sc::run_show(&cmd, &mut sc::RealHypr, &env).map_err(CliError::new)?;
    vout!("{line}");
    Ok(())
}

fn home_of(d: &Deps) -> PathBuf {
    PathBuf::from((d.env)("HOME").unwrap_or_else(|| "/root".into()))
}

/// `desk recenter`: the whole locked layout in front of the user (WayVR `desk-recenter`: the layout anchor from the
/// head now, yaw only; kept for the next start). One plain line for the headset toast and the app.
pub fn recenter(o: &DeskOptions, d: &Deps) -> CliResult {
    let running = !d.procs.find_by_comm(&o.vr.service_comm()).is_empty();
    vout!("{}", recenter_with(o, running, &mut |args| run_wayvrctl(&o.wayvrctl_bin, args)).map_err(CliError::new)?);
    Ok(())
}

/// [`recenter`] with the session state and wayvrctl given (tests).
pub(crate) fn recenter_with(o: &DeskOptions, running: bool, ctl: &mut dyn FnMut(&[&str]) -> Result<(), String>) -> Result<String, String> {
    if o.vr.dry_run {
        return Ok(format!("would run {} desk-recenter", o.wayvrctl_bin.display()));
    }
    if !running {
        return Err("No Work session is running".into());
    }
    ctl(&["desk-recenter"]).map_err(|e| format!("The screens could not be recentered: {e}"))?;
    Ok("Screens recentered in front of you".into())
}

fn run_wayvrctl(bin: &Path, args: &[&str]) -> Result<(), String> {
    use std::process::{Command, Stdio};
    match Command::new(bin).args(args).stdout(Stdio::null()).stderr(Stdio::null()).status() {
        Ok(st) if st.success() => Ok(()),
        Ok(st) => Err(format!("WayVR did not answer ({st}); an older WayVR has no desk-recenter")),
        Err(e) => Err(format!("{}: {e}", bin.display())),
    }
}

/// `desk preset NAME`: the live switch (see `preset`).
pub fn preset_live(o: &DeskOptions, name: &str, d: &Deps) -> CliResult {
    let Some(p) = super::preset::Preset::parse(name)? else {
        return Err(CliError::usage("desk preset: none is only for `desk start`"));
    };
    let panels = super::preset::parse_panels(
        std::fs::read_to_string(super::preset::panels_path(&home_of(d))).ok().as_deref(),
    );
    preset_switch(
        o,
        p,
        &mut super::screens::RealHypr,
        &super::screens::state_path(d.env),
        &super::preset::passthrough_socket(d.env),
        &panels,
        &mut |l| vout!("omarchyvr desk: {l}"),
    )
}

/// The live switch with its Hyprland and files given (the tests use fakes). A mode only changes how many screens
/// show and where they are: it reads Hyprland (`hyprctl -j`, never a dispatcher), writes the show state (keeping
/// the "sort" setting) and tells WayVR; it never closes, kills or moves an app. Screens it hides keep their apps,
/// and those apps are named in the output ("still open on hidden screens: left (google-chrome)") for the toast.
pub(crate) fn preset_switch(
    o: &DeskOptions,
    p: super::preset::Preset,
    h: &mut dyn super::screens::Hypr,
    state_file: &std::path::Path,
    pt_sock: &std::path::Path,
    panels: &[super::preset::Panel],
    out: &mut dyn FnMut(&str),
) -> CliResult {
    use super::screens as sc;
    // every switch places the layout with its own numbers (WayVR, locked: around the layout anchor)
    if o.vr.dry_run {
        out(&format!("would write {} ({})", o.wayvr_home.join("config/wayvr/conf.d").join(super::preset::CONF_NAME).display(), p.name()));
    } else if let Err(e) = super::preset::write_conf(&o.wayvr_home, Some(p)) {
        out(&format!("WARNING: {e}"));
    }
    // the preset's show state for the screens watcher (a live switch; desk start writes it before Monado)
    if !o.vr.dry_run {
        let _ = sc::write_state(state_file, &sc::read_state(state_file).switch_to(p));
    }
    let vis = if o.vr.dry_run { None } else { sc::preset_visibility(p, h) };
    if let Some(v) = &vis {
        let mons = h.run(&["-j", "monitors", "all"]).ok().and_then(|j| sc::parse_monitors(&j).ok()).unwrap_or_default();
        let wins = h.run(&["-j", "clients"]).ok().and_then(|j| sc::parse_windows(&j).ok()).unwrap_or_default();
        let hidden = sc::hidden_with_apps(v, &mons, &wins);
        if !hidden.is_empty() {
            out(&format!("still open on hidden screens: {}", hidden.join(", ")));
        }
    }
    super::preset::apply_live(p, &o.wayvrctl_bin, pt_sock, panels, vis.as_deref(), o.vr.dry_run, out).map_err(CliError::new)?;
    out(&format!("preset {}", p.name()));
    Ok(())
}

/// `desk stop`, the one full stop of Work (Exit Work in the headset, the app's Stop, a switch to Play): voice, the
/// leaves, WayVR, passthrough, then Monado with `vr stop` semantics, the camera broker, the VR screens' outputs, and
/// the local AI (every `omarchyvr-ai` slot: the model, Lara, the speech server, the agent model, the VRAM watch) unless
/// `keep_ai` (a restart of the session: `desk restart`, the watchdog). Then what of ours still runs is named.
/// `kill`: Monado may be SIGKILLed after `grace` (explicit user stops only; `--no-kill` and the watchdog pass false).
/// Everything is signalled by pid (pid files, names checked), never by name.
pub fn stop(o: &DeskOptions, grace: Duration, kill: bool, keep_ai: bool, d: &Deps) -> CliResult {
    stop_with(o, grace, kill, keep_ai, d, &omarchyvr_ai::Dirs::from_env(d.env), &mut super::screens::RealHypr)
}

/// [`stop`] with the AI's directories and Hyprland given (tests use temporary ones and a fake `hyprctl`).
pub fn stop_with(o: &DeskOptions, grace: Duration, kill: bool, keep_ai: bool, d: &Deps, dirs: &omarchyvr_ai::Dirs, hypr: &mut dyn super::screens::Hypr) -> CliResult {
    // only an explicit --keep-ai (a supervisor restart of a running Work session) keeps the AI; every other stop, omarchyvrd's own
    // (headset unplugged, automatic switch) included, unloads it like Exit Work
    if o.vr.dry_run {
        vout!("omarchyvr desk: would stop, in this order (SIGINT by pid, never by name):");
        let mut n = 1;
        if o.vr.session_dir.join("record.pid").exists() {
            vout!("  {n}. record: {} (omarchyvr-record), wait up to 25 s for the mp4 to be finished", pid_note(&o.vr.session_dir.join("record.pid")));
            n += 1;
        }
        for (name, bin) in stop_order(o) {
            let wait = if name == "wayvr" {
                format!(
                    "{:.0} s, then a notice and {:.0} s more; still alive once Monado is gone: SIGTERM, then SIGKILL by pid",
                    WAYVR_GRACE.as_secs_f64(),
                    WAYVR_EXTRA.as_secs_f64()
                )
            } else {
                format!("{:.0} s", CLIENT_GRACE.as_secs_f64())
            };
            vout!("  {n}. {name}: {} ({}), wait {wait}", pid_note(&o.pid_file(name)), comm_of(bin));
            n += 1;
        }
        vout!("  {n}. monado-service: {}", monado_stop_note(&o.vr.pid_path(), grace, kill));
        let cam_bin = o.sixdof.as_ref().map(|s| s.cam_bin.clone()).unwrap_or_else(|| {
            super::config::tools_dir(&PathBuf::from(std::env::var("HOME").unwrap_or_default())).join("pimax-cam")
        });
        vout!("  {}. broker: {} ({}), wait 10 s", n + 1, pid_note(&o.vr.session_dir.join("broker.pid")), comm_of(&cam_bin));
        vout!("  then the kernel log check (Xid / Idling display engine)");
        vout!("  then the VR screens' outputs (PIMAX-*) are removed once no monado-service runs");
        if keep_ai {
            vout!("  the local AI is kept (a restart of the session)");
        } else {
            vout!("  then the local AI: {} (by their pid files, SIGTERM, SIGKILL after 5 s)", super::work::ai_stop_note(dirs));
        }
        vout!("  then a check that nothing of Work is left running");
        return Ok(());
    }
    // the recorder first: it finishes its mp4 while Monado still runs (it only reads shared memory, never blocks)
    if let comp::proc::ClientStop::StillAlive =
        comp::record::stop_first(&o.vr.session_dir, d.procs, d.clock, &mut |m| vout!("omarchyvr desk: {m}"))
    {
        verr!("omarchyvr desk: the recorder is still finishing its file; going on (it does not hold the session)");
    }
    let (mut problems, hung) = stop_clients(o, d);
    // the local AI belongs to Work: it goes with it (its VRAM is free before Monado's teardown even starts)
    if !keep_ai {
        for l in super::work::stop_ai(dirs) {
            vout!("omarchyvr desk: {l}");
        }
    }
    let monado_pids = session::stop_targets(&o.vr, d.procs).ours;
    let mon = session::stop(&o.vr, Some(grace), false, kill, d);
    end_hung_wayvrs(&hung, &monado_pids, &o.vr.service_comm(), "omarchyvr desk", d);
    // The broker feeds Monado and passthrough: it goes last (pid file only exists if --6dof started it).
    let cam_bin = o.sixdof.as_ref().map(|s| s.cam_bin.clone()).unwrap_or_else(|| {
        super::config::tools_dir(&PathBuf::from(std::env::var("HOME").unwrap_or_default())).join("pimax-cam")
    });
    sixdof::stop_broker(&o.vr.session_dir, &cam_bin, d.procs, d.clock);
    remove_screens(o, d, hypr);
    // nothing of Work may be left behind: name whatever still runs (Monado's own verdict comes from its stop)
    let left: Vec<String> = super::work::survivors(o, d, dirs, keep_ai).into_iter().filter(|l| !l.starts_with("monado")).collect();
    for l in &left {
        verr!("omarchyvr desk: WARNING: still running after the stop: {l}");
    }
    if left.is_empty() && !keep_ai {
        vout!("omarchyvr desk: Work stopped: nothing of it is left running");
    }
    mon?;
    problems.extend(left.into_iter().map(|l| format!("still running after the stop: {l}")));
    match problems.first() {
        Some(p) => Err(CliError::new(p.clone())),
        None => Ok(()),
    }
}

/// The clients of a desk session (voice, the leaves, WayVR, passthrough, the controller tracker), in [`stop_order`]:
/// SIGINT by pid, a wait, never SIGKILL here (a hung WayVR is ended after Monado by [`end_hung_wayvrs`]). Returns the
/// problems in plain words and the hung WayVRs. Also used before Play starts, for a crashed Work's leftovers.
pub(crate) fn stop_clients(o: &DeskOptions, d: &Deps) -> (Vec<String>, Vec<HungWayvr>) {
    let mut problems: Vec<String> = Vec::new();
    let mut hung: Vec<HungWayvr> = Vec::new();
    for (name, bin) in stop_order(o) {
        let mut info = |m: &str| vout!("omarchyvr desk: {m}");
        let r = if name == "wayvr" {
            stop_wayvr(name, &o.pid_file(name), &comm_of(bin), d.procs, d.clock, &mut info, &mut |m| {
                verr!("omarchyvr desk: {m}");
                d.notifier.notify(false, "WayVR is slow to exit", m);
            })
        } else {
            stop_client(name, &o.pid_file(name), &comm_of(bin), d.procs, d.clock, CLIENT_GRACE, &mut info)
        };
        match r {
            ClientStop::StillAlive if name == "wayvr" => {
                let m = format!(
                    "{name} is still running {:.0} s after SIGINT (hung at exit); it gets SIGTERM, then SIGKILL by pid, once Monado is gone",
                    (WAYVR_GRACE + WAYVR_EXTRA).as_secs_f64()
                );
                verr!("omarchyvr desk: WARNING: {m}");
                problems.push(m);
                if let Some(pid) = session::read_pid(&o.pid_file(name)) {
                    hung.push(HungWayvr { name: name.to_string(), pid, comm: comm_of(bin), pid_file: o.pid_file(name) });
                }
            }
            ClientStop::StillAlive => {
                let m = format!("{name} is still running {:.0} s after SIGINT; leaving it to Monado's teardown (no SIGKILL by this tool)", CLIENT_GRACE.as_secs_f64());
                verr!("omarchyvr desk: WARNING: {m}");
                problems.push(m);
            }
            ClientStop::CannotSignal(m) => {
                verr!("omarchyvr desk: {m}");
                problems.push(m);
            }
            ClientStop::NotRunning | ClientStop::Exited => {
                let _ = std::fs::remove_file(o.pid_file(name));
            }
        }
        d.clock.sleep(Duration::from_millis(500));
    }
    (problems, hung)
}

/// A WayVR still alive after [`stop_wayvr`]'s whole wait.
pub struct HungWayvr {
    pub name: String,
    pub pid: u32,
    pub comm: String,
    pub pid_file: PathBuf,
}

/// After Monado's stop: every WayVR in `hung` gets [`end_hung_wayvr`] (SIGTERM, then SIGKILL, by pid) if the
/// Monado pids `monado_pids` (taken before the stop) are all gone; otherwise it is left alone and said so (Monado
/// hung too: the GPU gate says reboot). Every step goes to stderr and the end to a notification.
pub fn end_hung_wayvrs(hung: &[HungWayvr], monado_pids: &[u32], service_comm: &str, prefix: &str, d: &Deps) {
    end_hung_wayvrs_with(hung, monado_pids, service_comm, prefix, d.procs, d.clock, d.notifier)
}

fn end_hung_wayvrs_with(
    hung: &[HungWayvr],
    monado_pids: &[u32],
    service_comm: &str,
    prefix: &str,
    procs: &dyn Procs,
    clock: &dyn Clock,
    notifier: &dyn Notifier,
) {
    if hung.is_empty() {
        return;
    }
    let monado_alive = monado_pids
        .iter()
        .any(|p| procs.is_alive(*p) && procs.comm(*p).as_deref() == Some(service_comm));
    for h in hung {
        if monado_alive {
            let m = format!(
                "{} (pid {}) is hung at exit, but monado-service did not stop either: both left running (no SIGKILL \
                 while Monado is up); reboot before the next session",
                h.name, h.pid
            );
            verr!("{prefix}: WARNING: {m}");
            notifier.notify(true, "WayVR hung at exit", &m);
            continue;
        }
        let r = end_hung_wayvr(&h.name, h.pid, &h.comm, procs, clock, &mut |m| verr!("{prefix}: WARNING: {m}"));
        let summary = match &r {
            HungEnd::Gone => format!("{} (pid {}) exited on its own after Monado stopped", h.name, h.pid),
            HungEnd::Terminated => format!("{} (pid {}) was hung at exit; it exited on SIGTERM once Monado was gone", h.name, h.pid),
            HungEnd::Killed => format!("{} (pid {}) was hung at exit and ignored SIGTERM: SIGKILLed after Monado stopped", h.name, h.pid),
            HungEnd::Stuck => format!("{} (pid {}) is hung at exit and survives SIGKILL (stuck in the kernel): reboot", h.name, h.pid),
            HungEnd::CannotSignal(e) => format!("{} (pid {}) is hung at exit and cannot be signalled: {e}", h.name, h.pid),
        };
        verr!("{prefix}: {summary}");
        match r {
            HungEnd::Gone | HungEnd::Terminated | HungEnd::Killed => {
                let _ = std::fs::remove_file(&h.pid_file);
            }
            HungEnd::Stuck | HungEnd::CannotSignal(_) => {}
        }
        if r != HungEnd::Gone {
            notifier.notify(matches!(r, HungEnd::Stuck | HungEnd::CannotSignal(_)), "WayVR hung at exit", &summary);
        }
    }
}

/// `pid N` from a pid file, or why there is none (dry runs).
pub(crate) fn pid_note(pid_file: &Path) -> String {
    match session::read_pid(pid_file) {
        Some(p) => format!("pid {p} from {}", pid_file.display()),
        None => format!("no pid file {} (skipped)", pid_file.display()),
    }
}

/// What a stop does to Monado (dry runs).
pub(crate) fn monado_stop_note(pid_file: &Path, grace: Duration, kill: bool) -> String {
    let then = if kill { "then SIGKILL" } else { "never SIGKILL (a hung service is left alive, the GPU gate says reboot)" };
    format!("{}, SIGINT, wait {:.0} s, {then}", pid_note(pid_file), grace.as_secs_f64())
}

/// `desk status` lines.
pub fn status_lines(o: &DeskOptions, d: &Deps) -> Vec<String> {
    let mut out = Vec::new();
    let mut clients = vec![("passthrough", &o.passthrough_bin), ("wayvr", &o.wayvr_bin)];
    clients.extend(
        stop_order(o)
            .into_iter()
            .filter(|(n, _)| n.starts_with("voice") || *n == "perf"),
    );
    for (name, bin) in clients {
        let comm = comm_of(bin);
        let st = match session::read_pid(&o.pid_file(name)) {
            Some(p) if d.procs.is_alive(p) && d.procs.comm(p).as_deref() == Some(comm.as_str()) => {
                format!("running, pid {p}")
            }
            Some(p) => format!("not running (stale pid file, pid {p})"),
            None => "not running".into(),
        };
        out.push(format!("{name}: {st}"));
    }
    out.extend(session::status_lines(&o.vr, d, None).0);
    out
}

/// Entry point of `omarchyvr desk ...`.
pub fn run(mut a: Args) -> CliResult {
    let Some(first) = a.0.first().cloned() else {
        return Err(CliError::usage(USAGE));
    };
    if matches!(first.as_str(), "-h" | "--help" | "help") {
        vout!("{USAGE}");
        return Ok(());
    }
    a.0.remove(0);
    // SAFETY: signal(2) with SIG_DFL.
    unsafe {
        libc::signal(libc::SIGCHLD, libc::SIG_DFL);
    }
    match first.as_str() {
        "restart" => return super::supervise::restart_cli("desk", a),
        "watchdog" => return super::supervise::watchdog_cli("desk", a),
        _ => {}
    }
    let argv0 = a.0.clone();
    match first.as_str() {
        "start" | "stop" | "status" | "preset" | "recenter" | "captions" | "screens" | "exit" => {}
        other => {
            return Err(CliError::usage(format!(
                "unknown desk command {other}\n{USAGE}"
            )))
        }
    }
    let grace = match a.opt_parse::<f64>("--grace")? {
        Some(g) if g.is_finite() && g >= 0.0 => g.min(600.0),
        Some(_) => return Err(CliError::usage("--grace must be a number of seconds")),
        None => MONADO_GRACE_SECS,
    };
    let mut screens_val: Option<String> = None;
    let screens_arg = if first == "screens" {
        match a.0.iter().position(|x| !x.starts_with("--")) {
            Some(i) => {
                let sub = a.0.remove(i);
                if matches!(sub.as_str(), "show" | "more" | "add" | "hide") {
                    if let Some(j) = a.0.iter().position(|x| !x.starts_with("--")) {
                        screens_val = Some(a.0.remove(j));
                    }
                }
                Some(sub)
            }
            None => return Err(CliError::usage("usage: omarchyvr desk screens status|sync|watch|create|remove [--screens N] [--dry-run] | show N | more K | add SLOT")),
        }
    } else {
        None
    };
    let captions_arg = if first == "captions" {
        match a.0.iter().position(|x| !x.starts_with("--")) {
            Some(i) => Some(a.0.remove(i)),
            None => return Err(CliError::usage("usage: omarchyvr desk captions on|off|status")),
        }
    } else {
        None
    };
    let preset_name = if first == "preset" {
        match a.0.iter().position(|x| !x.starts_with("--")) {
            Some(i) => Some(a.0.remove(i)),
            None => return Err(CliError::usage("usage: omarchyvr desk preset work-4|focus-3|work-5|work-6|cinema|cinema-huge [--dry-run]")),
        }
    } else {
        None
    };
    let no_auto_recover = a.flag("--no-auto-recover");
    let no_watchdog = a.flag("--no-watchdog");
    let no_kill = first == "stop" && a.flag("--no-kill");
    // a restart (`desk restart`) keeps the local AI loaded; a real stop takes it down with Work
    let keep_ai = first == "stop" && a.flag("--keep-ai");
    // Exit Work from the headset: the second press stops everything; --now skips the confirmation (tests, voice)
    let exit_now = first == "exit" && a.flag("--now");
    let no_daemon = matches!(first.as_str(), "start" | "stop") && a.flag("--no-daemon");
    let mut o = parse_desk(&mut a, &real_env)?;
    reject_leftovers(&a)?;
    if first == "start" {
        super::hands::print_note(&o.hands, "omarchyvr desk");
    }
    auto_connector(&mut o.vr);
    supervise_before(&first, &o, &super::daemon::argv_for_record(&argv0));
    // omarchyvrd (P6): an executing omarchyvrd starts the session itself; otherwise the direct path below, unchanged
    let (link, daemon) = match first.as_str() {
        "start" | "stop" | "status" => super::daemon::contact(no_daemon, o.vr.dry_run),
        _ => (super::daemon::SockLink::from_env(), super::daemon::Daemon::Absent),
    };
    if first == "start" && super::daemon::start_through(&daemon, o.vr.wait, o.vr.force) {
        let r = super::daemon::start_via(&link, "desk", &RealClock::new(), &mut |m| vout!("omarchyvr desk: {m}")).map_err(CliError::new);
        if r.is_ok() && !o.vr.dry_run {
            room_on_start(&o);
        }
        return r;
    }
    // camera needed (passthrough or 6DoF) but missing while the headset is there: restart the headset, wait for it
    if first == "start" && !no_auto_recover && !o.vr.dry_run && (o.passthrough || o.sixdof.is_some()) {
        super::reset::real::auto_recover_real(&o.vr.session_dir, super::reset::AUTO_WAIT).map_err(CliError::new)?;
    }
    let mut host = RealHost::from_env();
    host.service_comm = o.vr.service_comm();
    host.session_dir = o.vr.session_dir.clone();
    let notifier = NotifySend::new(o.vr.notify);
    let clock = RealClock::new();
    let flag = pimax::sys::install_term_flag();
    let stop_req = || flag.load(std::sync::atomic::Ordering::Relaxed);
    let deps = Deps {
        host: &host,
        procs: &host,
        notifier: &notifier,
        clock: &clock,
        env: &real_env,
        stop_requested: &stop_req,
    };
    match first.as_str() {
        "start" => {
            let r = start(&o, &deps, &RealSpawner);
            // an executing omarchyvrd owns the restarts (it adopts a --wait / --force start): no second watchdog
            supervise_after_start(&r, &o, no_watchdog || daemon == super::daemon::Daemon::Executing);
            if r.is_ok() && !o.vr.dry_run {
                super::daemon::tell_direct(&link, &daemon, true, "desk", &mut |m| vout!("omarchyvr desk: {m}"));
                room_on_start(&o);
            }
            r
        }
        "stop" => {
            if !o.vr.dry_run {
                super::daemon::tell_direct(&link, &daemon, false, "desk", &mut |m| vout!("omarchyvr desk: {m}"));
                super::room::on_stop(&o.vr.session_dir);
            }
            let r = stop(&o, Duration::from_secs_f64(grace), !no_kill && !super::supervise::in_watchdog(), keep_ai, &deps);
            if !o.vr.dry_run {
                if let Some(l) = super::audio::on_stop(&o.vr.session_dir) {
                    vout!("omarchyvr desk: {l}");
                }
            }
            r
        }
        "exit" => {
            let l = super::work::exit(&o, exit_now, &deps, &mut super::work::spawn_detached_stop).map_err(CliError::new)?;
            vout!("omarchyvr desk: {l}");
            Ok(())
        }
        "preset" => preset_live(&o, preset_name.as_deref().unwrap_or_default(), &deps),
        "recenter" => recenter(&o, &deps),
        "screens" => match screens_arg.as_deref() {
            Some(sub @ ("show" | "more" | "add" | "hide")) => screens_show(&o, sub, screens_val.as_deref(), &deps),
            what => screens_cmd(&o, what.unwrap_or("status"), &deps),
        },
        "captions" => super::captions::run(&o, captions_arg.as_deref().unwrap_or("status"), &deps, &RealSpawner),
        _ => {
            for l in status_lines(&o, &deps) {
                vout!("{l}");
            }
            if let Some(l) = super::daemon::status_line(&link).filter(|_| daemon != super::daemon::Daemon::Absent) {
                vout!("{l}");
            }
            Ok(())
        }
    }
}

/// The room at every start (docs/ROOM-SETUP.md): `omarchyvr-roommap auto` after a desk start (Work); and the headset
/// speaker and microphone become the default output/input (docs/AUDIO.md).
fn room_on_start(o: &DeskOptions) {
    let home = PathBuf::from(real_env("HOME").unwrap_or_default());
    if let Some(l) = super::audio::on_start(&home, &o.vr.session_dir) {
        vout!("omarchyvr desk: {l}");
    }
    if let Some(l) = super::room::on_start(&home, &o.vr.session_dir) {
        vout!("omarchyvr desk: {l}");
    }
    // voice: the local AI's missing slots (the GPU speech server) come up with Work when Local AI is on
    if o.voice {
        if let Ok(cfg) = omarchyvr_config::ai::AiConfig::load() {
            if let Some(l) = super::gamemode::local_ai_up(&super::gamemode::Tools::real(&home), &cfg) {
                vout!("omarchyvr desk: {l}");
            }
        }
    }
}

/// Restart/watchdog bookkeeping around `desk start|stop` (see `supervise`): record the start argv, mark user stops.
fn supervise_before(first: &str, o: &DeskOptions, argv: &[String]) {
    match first {
        "start" if !o.vr.dry_run => {
            if let Err(e) = super::supervise::record_argv(&o.vr.session_dir, "desk", argv) {
                verr!("omarchyvr desk: WARNING: cannot record the start arguments for restart: {e}");
            }
            if let Some(e) = sixdof::sync_ctrl_angle_live(&o.vr, &real_env) {
                verr!("omarchyvr desk: WARNING: controller angle: {e}");
            }
        }
        "stop" if !o.vr.dry_run => super::supervise::mark_user_stop(&o.vr.session_dir),
        _ => {}
    }
}

/// After a successful `desk start`: the session watchdog (unless --no-watchdog / --dry-run / --wait).
fn supervise_after_start(r: &CliResult, o: &DeskOptions, no_watchdog: bool) {
    if r.is_ok() && !o.vr.dry_run && !o.vr.wait && !no_watchdog {
        super::supervise::spawn_watchdog("desk", &o.vr.session_dir, o.passthrough || o.sixdof.is_some(), o.vr.notify);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::host::{Clock, Procs, Signal};
    use std::cell::{Cell, RefCell};

    fn parse(args: &[&str], env: &[(&str, &str)]) -> Result<DeskOptions, CliError> {
        let env: Vec<(String, String)> = env
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        let lookup = move |k: &str| {
            env.iter()
                .find(|(n, _)| n == k)
                .map(|(_, v)| v.clone())
                .or_else(|| (k == "HOME").then(|| "/home/u".to_string()))
        };
        let mut a = Args(args.iter().map(|s| s.to_string()).collect());
        let o = parse_desk(&mut a, &lookup)?;
        reject_leftovers(&a)?;
        Ok(o)
    }

    #[test]
    fn defaults_are_the_documented_paths_with_passthrough_on() {
        let o = parse(&[], &[]).unwrap();
        assert!(o.passthrough && !o.eye);
        assert_eq!(o.vr.monado_build, PathBuf::from("/home/u/src/monado/build"));
        assert_eq!(
            o.passthrough_bin,
            PathBuf::from("/home/u/.local/bin/omarchyvr-passthrough")
        );
        assert_eq!(
            o.wayvr_bin,
            PathBuf::from("/home/u/.local/lib/omarchyvr/wayvr/wayvr")
        );
        assert_eq!(
            o.wayvr_home,
            PathBuf::from("/home/u/.local/share/omarchyvr/wayvr-home")
        );
        assert_eq!(
            o.vr.extra_env,
            vec![
                ("PIMAX_CTRL_INHAND".to_string(), "auto".to_string()),
                ("PIMAX_CTRL_ANGLE".to_string(), "30".to_string()),
                ("PIMAX_CTRL_AIM".to_string(), DESK_CTRL_AIM.to_string()),
                ("PIMAX_PERF_FILE".to_string(), "/tmp/omarchyvr-perf.stats".to_string()),
            ]
        );
    }

    #[test]
    fn controllers_switch_reaches_monado() {
        let env_of = |o: &DeskOptions| {
            o.vr.extra_env.iter().find(|(k, _)| k == "PIMAX_CTRL_INHAND").map(|(_, v)| v.clone())
        };
        assert_eq!(env_of(&parse(&[], &[]).unwrap()).as_deref(), Some("auto"), "Work: when picked up");
        assert_eq!(env_of(&parse(&["--controllers", "always"], &[]).unwrap()).as_deref(), Some("always"));
        assert_eq!(env_of(&parse(&["--controllers", "off"], &[]).unwrap()).as_deref(), Some("off"));
        assert_eq!(env_of(&parse(&[], &[("PIMAX_CTRL_INHAND", "always")]).unwrap()), None, "the caller's own wins");
        assert!(parse(&["--controllers", "sometimes"], &[]).is_err());
    }

    #[test]
    fn the_callers_ctrl_aim_wins() {
        let o = parse(&[], &[("PIMAX_CTRL_AIM", "level")]).unwrap();
        assert!(!o.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_CTRL_AIM"));
    }

    #[test]
    fn monado_build_env_is_honoured() {
        let o = parse(&[], &[("PIMAX_MONADO_BUILD", "/b")]).unwrap();
        assert_eq!(o.vr.monado_build, PathBuf::from("/b"));
    }

    #[test]
    fn eye_sets_the_variable_and_the_eye_build_unless_overridden() {
        let o = parse(&["--eye"], &[]).unwrap();
        assert!(o.eye);
        assert_eq!(
            o.vr.monado_build,
            PathBuf::from("/home/u/.local/lib/omarchyvr/monado")
        );
        assert!(o.vr.extra_env.contains(&("PIMAX_EYE".into(), "1".into())));
        let env = crate::vr::config::build_env(&o.vr, &|_| None);
        assert!(env.contains(&("PIMAX_EYE".into(), "1".into())));
        let o = parse(&["--eye", "--monado-build", "/x"], &[]).unwrap();
        assert_eq!(o.vr.monado_build, PathBuf::from("/x"));
        let o = parse(&["--eye"], &[("PIMAX_MONADO_BUILD", "/y")]).unwrap();
        assert_eq!(o.vr.monado_build, PathBuf::from("/y"));
        assert!(o.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE"));
    }

    #[test]
    fn sixdof_uses_the_merged_build_the_broker_env_and_a_broker_passthrough() {
        let o = parse(&["--6dof"], &[]).unwrap();
        assert_eq!(o.vr.monado_build, PathBuf::from("/home/u/.local/lib/omarchyvr/monado"));
        assert!(o.sixdof.is_some());
        assert!(o.vr.extra_env.contains(&("PIMAX_SLAM".into(), "1".into())));
        assert!(!o.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE"));
        let p = &plan(&o)[0];
        assert_eq!(p.args, vec!["run", "--source", "broker"]); // work-4 (the default) keeps the room normal
        assert!(p.env.contains(&("PIMAX_CAM_SOURCE".into(), "broker".into())));
        let e = parse(&["--eye", "--6dof"], &[]).unwrap();
        assert!(e.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_EYE"));
        let n = parse(&[], &[]).unwrap();
        assert_eq!(plan(&n)[0].args, vec!["run"]);
    }

    #[test]
    fn ctrl_track_needs_6dof_and_starts_the_tracker_first_on_the_broker() {
        assert!(parse(&["--ctrl-track"], &[]).is_err());
        let o = parse(&["--6dof", "--ctrl-track"], &[]).unwrap();
        assert!(o.vr.extra_env.contains(&("PIMAX_CTRL_TRACK".into(), "1".into())));
        assert!(o.vr.extra_env.iter().any(|(k, v)| k == "PIMAX_CTRL_IMU_LOG" && v.ends_with("ctrl-imu")));
        let p = plan(&o);
        assert_eq!(p[0].name, "ctrltrack");
        assert_eq!(p[0].program, PathBuf::from("/home/u/.local/bin/pimax-ctrltrack"));
        assert_eq!(p[0].args[0..2], ["live".to_string(), "--source".to_string()]);
        assert!(p[0].args[2].starts_with("broker:"));
        assert_eq!(p[1].name, "passthrough");
        let n = parse(&["--6dof"], &[]).unwrap();
        assert!(!n.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_CTRL_TRACK"));
        assert!(plan(&n).iter().all(|s| s.name != "ctrltrack"));
        let own = parse(&["--6dof", "--ctrl-track"], &[("PIMAX_CTRL_TRACK", "0")]).unwrap();
        assert!(!own.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_CTRL_TRACK"));
    }

    #[test]
    fn plan_runs_passthrough_before_wayvr_and_wayvr_is_an_xr_overlay_client() {
        let o = parse(&["--no-perf", "--screens", "1", "--preset", "none"], &[]).unwrap();
        let p = plan(&o);
        assert_eq!(
            p.iter().map(|s| s.name).collect::<Vec<_>>(),
            ["passthrough", "wayvr"]
        );
        assert_eq!(p[0].args, ["run"]); // no preset: the room normal
        let w = &p[1];
        assert_eq!(&w.args[..3], ["--openxr", "--show", "--no-autostart"]);
        let get = |k: &str| w.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str());
        assert_eq!(
            get("XDG_CONFIG_HOME"),
            Some("/home/u/.local/share/omarchyvr/wayvr-home/config")
        );
        assert_eq!(
            get("XR_RUNTIME_JSON"),
            Some("/home/u/src/monado/build/openxr_monado-dev.json")
        );
        // logs per process in the session dir
        assert_eq!(
            p[0].log,
            PathBuf::from("/tmp/omarchyvr-session/passthrough.log")
        );
        assert_eq!(w.log, PathBuf::from("/tmp/omarchyvr-session/wayvr.stdout"));
        assert!(w
            .args
            .contains(&"/tmp/omarchyvr-session/wayvr.log".to_string()));
    }

    #[test]
    fn voice_adds_the_listener_and_the_overlay_hud_after_wayvr() {
        let o = parse(&["--no-perf", "--screens", "1", "--preset", "none"], &[]).unwrap();
        assert!(!o.voice);
        assert_eq!(plan(&o).len(), 2);
        let o = parse(&["--no-perf", "--screens", "1", "--preset", "none", "--voice", "--voice-hud-bin", "/b/hud"], &[]).unwrap();
        assert!(o.voice);
        assert_eq!(
            o.voice_bin,
            PathBuf::from("/home/u/.local/bin/omarchyvr-voice")
        );
        // captions are the default: no head-locked HUD unless asked
        assert_eq!(
            plan(&o).iter().map(|s| s.name).collect::<Vec<_>>(),
            ["passthrough", "wayvr", "voice", "voicecap", "voiceact"]
        );
        let o = parse(&["--no-perf", "--screens", "1", "--preset", "none", "--voice", "--voice-hud", "--voice-hud-bin", "/b/hud"], &[]).unwrap();
        let p = plan(&o);
        assert_eq!(
            p.iter().map(|s| s.name).collect::<Vec<_>>(),
            ["passthrough", "wayvr", "voice", "voicecap", "voiceact", "voicehud"]
        );
        assert_eq!(p[2].args, ["listen"]);
        assert_eq!(p[2].log, PathBuf::from("/tmp/omarchyvr-session/voice.log"));
        assert_eq!(p[3].args[0], "captions");
        assert_eq!(p[3].args[2], "/home/u/.local/lib/omarchyvr/wayvr/wayvrctl");
        assert_eq!(p[4].args[0], "act");
        assert_eq!(p[5].program, PathBuf::from("/b/hud"));
        assert!(p[5]
            .env
            .contains(&("IPC_IGNORE_VERSION".to_string(), "1".to_string())));
        assert!(p[5]
            .env
            .iter()
            .any(|(k, v)| k == "XR_RUNTIME_JSON" && v.ends_with("openxr_monado-dev.json")));
        // the binary names fit in a 15 character comm (desk stop compares them)
        assert_eq!(comm_of(&o.voice_hud_bin), "hud");
        let d = parse(&["--voice"], &[]).unwrap();
        assert_eq!(comm_of(&d.voice_hud_bin), "omarchyvr-vhud");
        assert_eq!(comm_of(&d.voice_bin), "omarchyvr-voice");
    }

    #[test]
    fn stop_includes_the_voice_clients_only_when_they_were_started() {
        let d = crate::vr::test_dir("desk-voice-stop");
        let dir = d.to_string_lossy().into_owned();
        let o = parse(&[], &[("PIMAX_VR_SESSION_DIR", dir.as_str())]).unwrap();
        let names = |o: &DeskOptions| stop_order(o).iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(names(&o), ["wayvr", "passthrough"]);
        std::fs::write(o.pid_file("voice"), "123\n").unwrap();
        std::fs::write(o.pid_file("voicecap"), "125\n").unwrap();
        std::fs::write(o.pid_file("voicehud"), "124\n").unwrap();
        assert_eq!(
            names(&o),
            ["voicehud", "voicecap", "voice", "wayvr", "passthrough"]
        );
        std::fs::write(o.pid_file("voiceact"), "126\n").unwrap();
        assert_eq!(names(&o)[..3], ["voicehud", "voiceact", "voicecap"]);
    }

    #[test]
    fn vr_screens_default_to_six_with_the_watcher_after_wayvr() {
        let o = parse(&[], &[]).unwrap();
        assert_eq!(o.screens, 6);
        let p = plan(&o);
        assert_eq!(p.iter().map(|s| s.name).collect::<Vec<_>>(), ["passthrough", "wayvr", "screens", "perf"]);
        assert_eq!(p[2].program, PathBuf::from("/home/u/.local/bin/omarchyvr"));
        assert_eq!(p[2].args, ["desk", "screens", "watch", "--wayvrctl-bin", "/home/u/.local/lib/omarchyvr/wayvr/wayvrctl"]);
        assert_eq!(p[2].log, PathBuf::from("/tmp/omarchyvr-session/screens.log"));
        assert!(plan(&parse(&["--screens", "1", "--preset", "none"], &[]).unwrap()).iter().all(|s| s.name != "screens"));
        assert_eq!(parse(&["--screens", "2"], &[]).unwrap().screens, 4, "work-4 (the default) needs 4 outputs");
        assert_eq!(parse(&["--screens", "7"], &[]).unwrap_err().code, 2);
        assert_eq!(parse(&["--screens", "6"], &[]).unwrap().screens, 6);
    }

    #[test]
    fn no_passthrough_leaves_only_wayvr() {
        let o = parse(&["--no-passthrough", "--no-perf", "--screens", "1", "--preset", "none"], &[]).unwrap();
        assert_eq!(
            plan(&o).iter().map(|s| s.name).collect::<Vec<_>>(),
            ["wayvr"]
        );
    }

    #[test]
    fn perf_readout_is_on_by_default_after_wayvr_and_off_with_no_perf() {
        let o = parse(&["--screens", "1", "--preset", "none"], &[("XDG_RUNTIME_DIR", "/run/user/7")]).unwrap();
        let p = plan(&o);
        assert_eq!(p.iter().map(|s| s.name).collect::<Vec<_>>(), ["passthrough", "wayvr", "perf"]);
        assert_eq!(p[2].program, PathBuf::from("/home/u/.local/bin/omarchyvr-perf"));
        assert_eq!(
            p[2].args,
            ["run", "--file", "/run/user/7/omarchyvr-perf.stats", "--wayvrctl", "/home/u/.local/lib/omarchyvr/wayvr/wayvrctl"]
        );
        assert!(o.vr.extra_env.contains(&("PIMAX_PERF_FILE".into(), "/run/user/7/omarchyvr-perf.stats".into())));
        let n = parse(&["--no-perf"], &[]).unwrap();
        assert!(n.perf.is_none());
        assert!(!n.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_PERF_FILE"));
        assert!(plan(&n).iter().all(|s| s.name != "perf"));
        // the caller's own file wins
        let own = parse(&[], &[("PIMAX_PERF_FILE", "/x/s")]).unwrap();
        assert_eq!(own.perf.unwrap().stats, PathBuf::from("/x/s"));
    }

    #[test]
    fn stop_includes_perf_only_when_it_was_started() {
        let d = crate::vr::test_dir("desk-perf-stop");
        let dir = d.to_string_lossy().into_owned();
        let o = parse(&[], &[("PIMAX_VR_SESSION_DIR", dir.as_str())]).unwrap();
        let names = |o: &DeskOptions| stop_order(o).iter().map(|(n, _)| *n).collect::<Vec<_>>();
        assert_eq!(names(&o), ["wayvr", "passthrough"]);
        std::fs::write(o.pid_file("perf"), "9\n").unwrap();
        assert_eq!(names(&o), ["perf", "wayvr", "passthrough"]);
    }

    #[test]
    fn bad_settle_is_a_usage_error() {
        assert_eq!(parse(&["--settle", "-1"], &[]).unwrap_err().code, 2);
        assert_eq!(parse(&["--settle", "x"], &[]).unwrap_err().code, 2);
        assert_eq!(parse(&["--bogus"], &[]).unwrap_err().code, 2);
    }

    #[test]
    fn wayvr_home_is_created_with_the_cpu_capture_config() {
        let d = crate::vr::test_dir("desk-home");
        let home = d.join("h");
        assert_eq!(ensure_wayvr_home(&home, false, false).unwrap(), None);
        let f = home.join("config/wayvr/conf.d/10-am.yaml");
        assert!(std::fs::read_to_string(&f)
            .unwrap()
            .contains("capture_method: screencopy"));
        assert!(home.join("data").is_dir() && home.join("state").is_dir());
        // an existing config is not overwritten, and a GPU one is flagged
        std::fs::write(&f, "capture_method: pipewire\n").unwrap();
        assert!(ensure_wayvr_home(&home, false, false)
            .unwrap()
            .unwrap()
            .contains("screencopy"));
        assert!(std::fs::read_to_string(&f).unwrap().contains("pipewire"));
        std::fs::write(&f, "capture_method:   screencopy\n").unwrap();
        assert_eq!(ensure_wayvr_home(&home, false, false).unwrap(), None);
        // keyboard off always, the menu button opens the Work menu; the caption panel only with voice
        let cd = home.join("config/wayvr/conf.d");
        assert_eq!(
            std::fs::read_to_string(cd.join("35-desk.yaml")).unwrap(),
            "enable_keyboard: false\nmenu_panel: work_menu\n"
        );
        // the Work menu is in every desk session: Exit Work runs the full stop (two presses)
        assert_eq!(std::fs::read_to_string(cd.join("40-panels.yaml")).unwrap(), "custom_panels:\n  - work_menu\n");
        let menu = std::fs::read_to_string(home.join("config/wayvr/theme/gui/work_menu.xml")).unwrap();
        assert!(menu.contains("_press=\"::ShellExec omarchyvr desk exit\"") && menu.contains("_press=\"::OverlayToggle work_menu\""), "{menu}");
        assert_eq!(ensure_wayvr_home(&home, true, false).unwrap(), None);
        assert!(std::fs::read_to_string(cd.join("40-panels.yaml"))
            .unwrap()
            .contains("- captions"));
        assert!(std::fs::read_to_string(home.join("config/wayvr/theme/gui/captions.xml"))
            .unwrap()
            .contains("id=\"cap_l2\""));
        assert_eq!(ensure_wayvr_home(&home, false, false).unwrap(), None);
        assert_eq!(std::fs::read_to_string(cd.join("40-panels.yaml")).unwrap(), "custom_panels:\n  - work_menu\n");
        // the wrist panel alone, and together with the captions in one list
        assert_eq!(ensure_wayvr_home(&home, false, true).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(cd.join("40-panels.yaml")).unwrap(),
            "custom_panels:\n  - work_menu\n  - perf\n"
        );
        assert!(std::fs::read_to_string(home.join("config/wayvr/theme/gui/perf.xml"))
            .unwrap()
            .contains("id=\"perf_l4\""));
        assert_eq!(ensure_wayvr_home(&home, true, true).unwrap(), None);
        assert_eq!(
            std::fs::read_to_string(cd.join("40-panels.yaml")).unwrap(),
            "custom_panels:\n  - work_menu\n  - captions\n  - perf\n"
        );
    }

    struct FakeClock(Cell<Duration>);
    impl Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    /// pid, comm, seconds until it dies after SIGINT (None = ignores SIGINT), time of the SIGINT
    type FakeProc = (u32, String, Option<Duration>, Option<Duration>);

    struct FakeProcs<'a> {
        clock: &'a FakeClock,
        procs: RefCell<Vec<FakeProc>>,
        log: RefCell<Vec<(u32, Signal)>>,
    }
    impl FakeProcs<'_> {
        fn add(&self, pid: u32, comm: &str, dies_after: Option<u64>) {
            self.procs.borrow_mut().push((
                pid,
                comm.into(),
                dies_after.map(Duration::from_secs),
                None,
            ));
        }
    }
    impl Procs for FakeProcs<'_> {
        fn is_alive(&self, pid: u32) -> bool {
            self.procs.borrow().iter().any(|(p, _, die, at)| {
                *p == pid
                    && match (die, at) {
                        (Some(d), Some(t)) => self.clock.now() < *t + *d,
                        _ => true,
                    }
            })
        }
        fn comm(&self, pid: u32) -> Option<String> {
            self.procs
                .borrow()
                .iter()
                .find(|x| x.0 == pid)
                .map(|x| x.1.clone())
        }
        fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
            self.log.borrow_mut().push((pid, sig));
            for x in self.procs.borrow_mut().iter_mut().filter(|x| x.0 == pid) {
                match sig {
                    // the hung WayVR of the 2026-10-15 incident: deaf to SIGTERM, ended by SIGKILL
                    Signal::Term => {}
                    Signal::Kill => {
                        x.2 = Some(Duration::ZERO);
                        x.3 = Some(self.clock.now());
                    }
                    Signal::Int => x.3 = Some(self.clock.now()),
                }
            }
            Ok(())
        }
        fn find_by_comm(&self, _: &str) -> Vec<u32> {
            vec![]
        }
        fn environ(&self, _: u32) -> Option<Vec<(String, String)>> {
            None
        }
        fn age_secs(&self, _: u32) -> Option<f64> {
            None
        }
    }

    #[test]
    fn stop_signals_by_pid_only_with_sigint_and_waits() {
        let d = crate::vr::test_dir("desk-stop");
        let clock = FakeClock(Cell::new(Duration::ZERO));
        let procs = FakeProcs {
            clock: &clock,
            procs: RefCell::new(vec![]),
            log: RefCell::new(vec![]),
        };
        procs.add(100, "wayvr", Some(3));
        procs.add(200, "omarchyvr-passt", Some(1));
        procs.add(300, "wayvr", Some(1)); // same name, not in a pid file: must not be touched
        let w = d.join("wayvr.pid");
        std::fs::write(&w, "100\n").unwrap();
        let mut msgs = vec![];
        let r = stop_client(
            "wayvr",
            &w,
            "wayvr",
            &procs,
            &clock,
            CLIENT_GRACE,
            &mut |m| msgs.push(m.to_string()),
        );
        assert_eq!(r, ClientStop::Exited);
        assert_eq!(*procs.log.borrow(), vec![(100, Signal::Int)]);
        assert!(clock.now() >= Duration::from_secs(3) && clock.now() < Duration::from_secs(5));
        // the pid file points at a process with another name (pid reuse): left alone
        let p = d.join("passthrough.pid");
        std::fs::write(&p, "200\n").unwrap();
        let r = stop_client(
            "passthrough",
            &p,
            "other",
            &procs,
            &clock,
            CLIENT_GRACE,
            &mut |_| {},
        );
        assert_eq!(r, ClientStop::NotRunning);
        assert_eq!(procs.log.borrow().len(), 1);
        // no pid file
        let r = stop_client(
            "x",
            &d.join("none.pid"),
            "x",
            &procs,
            &clock,
            CLIENT_GRACE,
            &mut |_| {},
        );
        assert_eq!(r, ClientStop::NotRunning);
        assert!(procs.is_alive(300));
    }

    #[test]
    fn a_client_that_ignores_sigint_is_reported_not_killed() {
        let d = crate::vr::test_dir("desk-stuck");
        let clock = FakeClock(Cell::new(Duration::ZERO));
        let procs = FakeProcs {
            clock: &clock,
            procs: RefCell::new(vec![]),
            log: RefCell::new(vec![]),
        };
        procs.add(100, "wayvr", None);
        let w = d.join("wayvr.pid");
        std::fs::write(&w, "100\n").unwrap();
        let r = stop_client(
            "wayvr",
            &w,
            "wayvr",
            &procs,
            &clock,
            CLIENT_GRACE,
            &mut |_| {},
        );
        assert_eq!(r, ClientStop::StillAlive);
        assert!(clock.now() >= CLIENT_GRACE);
        assert_eq!(
            *procs.log.borrow(),
            vec![(100, Signal::Int)],
            "never SIGKILL"
        );
    }

    #[derive(Default)]
    struct Notes(RefCell<Vec<(bool, String)>>);
    impl Notifier for Notes {
        fn notify(&self, critical: bool, _summary: &str, body: &str) {
            self.0.borrow_mut().push((critical, body.into()));
        }
    }

    #[test]
    fn a_wayvr_hung_at_exit_is_killed_by_pid_once_monado_is_gone() {
        let d = crate::vr::test_dir("desk-hung-wayvr");
        let clock = FakeClock(Cell::new(Duration::ZERO));
        let procs = FakeProcs { clock: &clock, procs: RefCell::new(vec![]), log: RefCell::new(vec![]) };
        procs.add(100, "wayvr", None); // ignores SIGINT and SIGTERM (100 % CPU in its teardown)
        procs.add(300, "wayvr", None); // same name, not ours: never touched
        let w = d.join("wayvr.pid");
        std::fs::write(&w, "100\n").unwrap();
        // the whole stop wait: still alive, and only SIGINT so far
        let r = stop_wayvr("wayvr", &w, "wayvr", &procs, &clock, &mut |_| {}, &mut |_| {});
        assert_eq!(r, ClientStop::StillAlive);
        let hung = [HungWayvr { name: "wayvr".into(), pid: 100, comm: "wayvr".into(), pid_file: w.clone() }];
        let notes = Notes::default();
        // Monado (pid 500) is gone: SIGTERM, then SIGKILL, by pid
        end_hung_wayvrs_with(&hung, &[500], "monado-service", "omarchyvr desk", &procs, &clock, &notes);
        assert_eq!(*procs.log.borrow(), vec![(100, Signal::Int), (100, Signal::Term), (100, Signal::Kill)]);
        assert!(!procs.is_alive(100) && procs.is_alive(300));
        assert!(!w.exists(), "the pid file goes with it");
        let n = notes.0.borrow();
        assert_eq!(n.len(), 1);
        assert!(n[0].1.contains("SIGKILLed after Monado stopped"), "{n:?}");
    }

    #[test]
    fn a_hung_wayvr_is_not_killed_while_monado_is_up() {
        let d = crate::vr::test_dir("desk-hung-wayvr-monado-up");
        let clock = FakeClock(Cell::new(Duration::ZERO));
        let procs = FakeProcs { clock: &clock, procs: RefCell::new(vec![]), log: RefCell::new(vec![]) };
        procs.add(100, "wayvr", None);
        procs.add(500, "monado-service", None); // Monado hung too
        let w = d.join("wayvr.pid");
        std::fs::write(&w, "100\n").unwrap();
        let hung = [HungWayvr { name: "wayvr".into(), pid: 100, comm: "wayvr".into(), pid_file: w.clone() }];
        let notes = Notes::default();
        end_hung_wayvrs_with(&hung, &[500], "monado-service", "omarchyvr desk", &procs, &clock, &notes);
        assert!(procs.log.borrow().is_empty(), "no signal while Monado is up");
        assert!(w.exists());
        assert!(notes.0.borrow()[0].0, "a critical notification");
        // and a WayVR that exited on its own in the meantime gets nothing either
        let procs = FakeProcs { clock: &clock, procs: RefCell::new(vec![]), log: RefCell::new(vec![]) };
        end_hung_wayvrs_with(&hung, &[500], "monado-service", "omarchyvr desk", &procs, &clock, &notes);
        assert!(procs.log.borrow().is_empty());
    }

    #[test]
    fn the_hand_coded_stop_order_agrees_with_the_component_graph() {
        // every pid file present: the full desk stop order, then Monado and the broker (see `stop`)
        let mut o = parse(&["--6dof", "--ctrl-track", "--voice", "--voice-hud"], &[]).unwrap();
        o.vr.session_dir = crate::vr::test_dir("desk-graph-order");
        for n in ["voicehud", "voiceact", "voicecap", "voice", "perf", "ctrltrack"] {
            std::fs::write(o.pid_file(n), "1\n").unwrap();
        }
        let id = |n: &str| match n {
            "voiceact" => "voice.act",
            "voicecap" => "voice.captions",
            "voice" => "voice.listen",
            x => x,
        }
        .to_string();
        let mut order: Vec<String> = stop_order(&o).iter().map(|(n, _)| id(n)).collect();
        order.extend(["monado".to_string(), "broker".to_string()]);
        let d = omarchyvr_components::model::Desired { voice: true, voicehud: true, ..Default::default() };
        for (i, c) in order.iter().enumerate() {
            // nothing is stopped after one of the components it depends on
            for dep in omarchyvr_components::graph::hard_deps(c, &d) {
                if let Some(j) = order.iter().position(|x| x == dep) {
                    assert!(j > i, "{c} is stopped after its dependency {dep}: {order:?}");
                }
            }
        }
    }

    #[test]
    fn start_waits_for_the_settle_time_and_notices_early_exits() {
        let clock = FakeClock(Cell::new(Duration::ZERO));
        let procs = FakeProcs {
            clock: &clock,
            procs: RefCell::new(vec![]),
            log: RefCell::new(vec![]),
        };
        procs.add(1, "a", None);
        assert!(stays_alive(1, &procs, &clock, Duration::from_secs(6)));
        assert!(clock.now() >= Duration::from_secs(6));
        assert!(!stays_alive(99, &procs, &clock, Duration::from_secs(6)));
    }

    #[test]
    fn anchor_sets_the_monado_hook() {
        let o = parse(&["--6dof", "--anchor"], &[]).unwrap();
        assert!(o.vr.extra_env.contains(&("PIMAX_ANCHOR".into(), "1".into())));
        let n = parse(&["--6dof"], &[]).unwrap();
        assert!(!n.vr.extra_env.iter().any(|(k, _)| k == "PIMAX_ANCHOR"));
    }

    #[test]
    fn recenter_needs_a_session_and_sends_desk_recenter_only() {
        let o = parse(&[], &[]).unwrap();
        let sent = std::cell::RefCell::new(Vec::<String>::new());
        let mut ctl = |a: &[&str]| -> Result<(), String> {
            sent.borrow_mut().push(a.join(" "));
            Ok(())
        };
        assert_eq!(recenter_with(&o, false, &mut ctl).unwrap_err(), "No Work session is running");
        assert!(sent.borrow().is_empty());
        assert_eq!(recenter_with(&o, true, &mut ctl).unwrap(), "Screens recentered in front of you");
        assert_eq!(*sent.borrow(), ["desk-recenter"]);
        let mut fail = |_: &[&str]| -> Result<(), String> { Err("WayVR did not answer".into()) };
        assert!(recenter_with(&o, true, &mut fail).unwrap_err().starts_with("The screens could not be recentered"));
        let dry = parse(&["--dry-run"], &[]).unwrap();
        assert!(recenter_with(&dry, false, &mut ctl).unwrap().starts_with("would run "));
    }

    /// Regression (user, 2026-10-15): "Max, put everything in cinema mode" made the apps vanish. A mode switch must
    /// never close, kill or move an app: Hyprland is only read (`hyprctl -j`), WayVR only gets `desk-screens`
    /// (show/hide/place screens, never process-stop or a close), the apps on the screens it hides are named, the
    /// "sort" setting is kept, and the next mode shows them again.
    #[test]
    fn switching_modes_never_closes_or_moves_apps() {
        use crate::vr::preset::Preset;
        use crate::vr::screens::{self as sc, Hypr};
        struct FakeHypr(Vec<String>);
        impl Hypr for FakeHypr {
            fn run(&mut self, args: &[&str]) -> Result<String, String> {
                self.0.push(args.join(" "));
                let m = |name: &str, x: i64, y: i64, ws: i64, focused: bool| {
                    serde_json::json!({"name": name, "width": 1920, "height": 1080, "x": x, "y": y, "focused": focused,
                        "disabled": false, "mirrorOf": "none", "activeWorkspace": {"id": ws, "name": ws.to_string()}})
                };
                let c = |a: &str, class: &str, ws: i64, focus: i64| {
                    serde_json::json!({"address": a, "class": class, "workspace": {"id": ws, "name": ws.to_string()},
                        "floating": false, "fullscreen": 0, "pinned": false, "focusHistoryID": focus})
                };
                match args {
                    // after "put everything in its own window": terminal on main (2), Chrome left (1), Pimax right (3)
                    ["-j", "monitors", "all"] => Ok(serde_json::json!([m("SUNSHINE", 0, 0, 2, true), m("PIMAX-LEFT", -1920, 0, 1, false),
                        m("PIMAX-RIGHT", 1920, 0, 3, false), m("PIMAX-UP", 0, -1080, 4, false)])
                    .to_string()),
                    ["-j", "clients"] => Ok(serde_json::json!([c("0xf00", "foot", 2, 0), c("0xc0", "google-chrome", 1, 1), c("0xa0", "pimax", 3, 2)]).to_string()),
                    _ => Err(format!("a mode switch must not run hyprctl {args:?}")),
                }
            }
            fn sleep(&mut self, _d: Duration) {}
        }
        let dir = crate::vr::test_dir("desk-preset-keeps-apps");
        let log = dir.join("wayvrctl.log");
        let ctl = dir.join("wayvrctl");
        std::fs::write(&ctl, format!("#!/bin/sh\necho \"$*\" >> '{}'\n", log.display())).unwrap();
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&ctl, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let state = dir.join("omarchyvr-screens.state");
        // "put everything back together" had turned the sorting off: a mode keeps that
        sc::write_state(&state, &sc::ShowState { sort: false, ..sc::ShowState::default() }).unwrap();
        let home = dir.join("wayvr-home");
        let o = parse(&["--wayvrctl-bin", ctl.to_str().unwrap(), "--wayvr-home", home.to_str().unwrap()], &[]).unwrap();
        let panels = crate::vr::preset::parse_panels(None);
        for p in [Preset::Cinema, Preset::CinemaHuge, Preset::Work4, Preset::Focus3, Preset::Work5, Preset::Work6, Preset::Cinema] {
            let _ = std::fs::remove_file(&log);
            let mut h = FakeHypr(Vec::new());
            let mut lines = Vec::new();
            preset_switch(&o, p, &mut h, &state, &dir.join("no-passthrough.sock"), &panels, &mut |l| lines.push(l.to_string())).unwrap();
            assert!(h.0.iter().all(|c| c.starts_with("-j ")), "{p:?}: only reads, no dispatch: {:?}", h.0);
            let calls = std::fs::read_to_string(&log).unwrap();
            // cinema-huge also turns the backdrop off (a second wayvrctl call); every layout is placed with its numbers
            let want = if p == Preset::CinemaHuge { 2 } else { 1 };
            assert_eq!(calls.lines().count(), want, "{p:?}: {calls}");
            assert!(calls.contains("desk-screens --place ") && calls.contains("main=SUNSHINE"), "{p:?}: {calls}");
            let l = p.layout();
            assert!(calls.contains(&format!("--width {} --distance {} --curvature {}", l.width, l.distance, l.curvature)), "{p:?}: {calls}");
            if p == Preset::CinemaHuge {
                assert!(calls.contains("skybox off") && calls.contains("--place --width 11 --distance 8"), "{calls}");
            }
            for bad in ["process-stop", "process-launch", "kill", "close", "terminate", "desk-layout"] {
                assert!(!calls.contains(bad), "{p:?} ran {bad}: {calls}");
            }
            assert!(!sc::read_state(&state).sort, "{p:?}: sort kept off");
            let hidden = lines.iter().find(|l| l.starts_with("still open on hidden screens: "));
            match p {
                Preset::Cinema | Preset::CinemaHuge => {
                    assert!(calls.contains("left=PIMAX-LEFT:hide") && calls.contains("right=PIMAX-RIGHT:hide"), "{calls}");
                    assert_eq!(hidden.map(String::as_str), Some("still open on hidden screens: left (google-chrome), right (pimax)"));
                }
                _ => {
                    assert!(calls.contains("left=PIMAX-LEFT ") && calls.contains("right=PIMAX-RIGHT "), "{p:?}: shown again: {calls}");
                    assert!(hidden.is_none(), "{p:?}: {hidden:?}");
                }
            }
        }
    }
}
