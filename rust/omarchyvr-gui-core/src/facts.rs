//! Collecting what the pages show: [`Live`] (session processes, the running service and its log; every second) and
//! [`Facts`] (doctor checks, kernel log, builds, device holders, installed files; every few seconds and on demand).
//! Read-only: files, `/proc`, `/sys`, and the read-only commands `journalctl -k`, `getcap`, `modinfo`, `omarchyvr desk help`.
use crate::builds::{self, BuildInfo, MarkerCache};
use crate::kernel::{self, KernelEvent};
use crate::launcher::{self, InstallState, LauncherCaps};
use crate::machine::Machine;
use crate::monlog::{self, LogFollower, MonadoLog};
use crate::paths::Paths;
use crate::procs::{self, Holder};
use omarchyvr_doctor::checks::usb_devices;
use omarchyvr_doctor::{Level, Opts, Report};
use std::path::{Path, PathBuf};

/// Processes of a launcher session (pid file stem, expected `comm`), in start order.
pub const SESSION_PROCS: [(&str, &str); 11] = [
    ("broker", "pimax-cam"),
    ("monado", "monado-service"),
    ("ctrltrack", "pimax-ctrltrack"),
    ("passthrough", "omarchyvr-passt"),
    ("wayvr", "wayvr"),
    ("perf", "omarchyvr-perf"),
    ("perfwayvr", "wayvr"),
    ("voice", "omarchyvr-voice"),
    ("voicecap", "omarchyvr-voice"),
    ("voiceact", "omarchyvr-voice"),
    ("voicehud", "omarchyvr-vhud"),
];

/// OpenXR clients that would attach to a new Monado (LESSONS row 4), by `comm`.
pub const XR_CLIENT_COMMS: [&str; 10] = [
    "pimax-lenstest",
    "pimax-eyecal",
    "omarchyvr-passt",
    "wayvr",
    "omarchyvr-vhud",
    "posecli",
    "xr_room",
    "xr_eyetest",
    "xr-handtest",
    "xr-gaze",
];

/// One process of the session.
#[derive(Debug, Clone, PartialEq)]
pub struct SessionProc {
    /// Pid file stem.
    pub name: &'static str,
    /// Pid from the pid file.
    pub pid: Option<u32>,
    /// Alive with the expected name.
    pub alive: bool,
    /// Age in seconds.
    pub age: Option<f64>,
}

/// The running service of the session.
#[derive(Debug, Clone, PartialEq)]
pub struct ServiceInfo {
    /// Pid.
    pub pid: u32,
    /// Age in seconds.
    pub age: Option<f64>,
    /// Build directory (from the executable path).
    pub build: Option<PathBuf>,
    /// Refresh rate (`PIMAX_FPS`).
    pub hz: Option<u32>,
    /// `PIMAX_EYE=1`.
    pub eye: bool,
    /// `PIMAX_SLAM=1`.
    pub sixdof: bool,
    /// `PIMAX_CTRL_TRACK=1`.
    pub ctrl_track: bool,
    /// Reads the camera broker.
    pub broker: bool,
    /// `OXR_DEBUG_IPD_MM`.
    pub ipd: Option<String>,
    /// `PIMAX_ANCHOR=1` (the room offset hook: AprilTag, room map).
    pub anchor: bool,
}

/// A Steam client that was started from inside SteamVR and still carries its variables.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TaintedSteam {
    /// Its pid.
    pub pid: u32,
    /// The SteamVR variables it carries.
    pub vars: Vec<String>,
    /// A game or a download seems to be running (plain words), when detectable.
    pub busy: Option<String>,
}

/// Plain words for "a game or a download seems to be running in Steam" (None = nothing seen). Steam starts every
/// game through a `reaper` process; running downloads leave files in `steamapps/downloading`.
pub fn steam_busy_hint(game_running: bool, download: bool) -> Option<String> {
    match (game_running, download) {
        (true, true) => Some("a game seems to be running and a download is in progress".into()),
        (true, false) => Some("a game seems to be running".into()),
        (false, true) => Some("a download seems to be in progress".into()),
        (false, false) => None,
    }
}

/// Fast-changing state.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Live {
    /// Session processes.
    pub session: Vec<SessionProc>,
    /// Our service (pid file, alive, right name).
    pub service: Option<ServiceInfo>,
    /// All monado-service pids.
    pub monado_pids: Vec<u32>,
    /// monado-service pids that are not the session's.
    pub foreign_monado: Vec<u32>,
    /// Parsed log of the session.
    pub log: MonadoLog,
    /// Last stats line of `pimax-ctrltrack live`.
    pub ctrltrack: Option<String>,
    /// The subtitles are switched off (`$XDG_RUNTIME_DIR/omarchyvr-voice-captions.off`, the voice commands' switch).
    pub captions_off: bool,
    /// The session watchdog's pid, when it runs (auto-reconnect; `watchdog.pid` in the session dir).
    pub watchdog: Option<u32>,
    /// The last line of its log (`watchdog.log`), for the last reconnect.
    pub watchdog_last: Option<String>,
    /// How the session was started (`start-argv.txt`: `desk` or `vr`), for Restart service.
    pub started_as: Option<String>,
    /// vrserver of the SteamVR that Play started (Games run with: SteamVR; `steamvr.pid` in the session dir).
    pub steamvr: Option<u32>,
    /// Plain words when Steam or SteamVR went wrong during a SteamVR session (Steam restarted or closed, a Steam that
    /// SteamVR started with its own variables, SteamVR quit): omarchyvr_tools::vr::steamvr::session_note_from.
    pub steam_note: Option<String>,
    /// A running Steam that carries SteamVR's variables (started from inside SteamVR): Play asks to restart it.
    pub steam_tainted: Option<TaintedSteam>,
    /// Render scale of the running Monado (`XRT_COMPOSITOR_SCALE_PERCENTAGE`; Monado's default 140 when unset).
    pub scale_pct: Option<u16>,
    /// Frame statistics of the running Monado (`omarchyvr-perf.stats`, only while it is fresh).
    pub perf: Option<crate::quality::PerfSnap>,
    /// What game mode paused for the session (Sunshine, local AI; `gamemode-paused.txt` in the session dir).
    pub gm_paused: Vec<String>,
    /// In hand / put down per controller, from the running Monado (`$XDG_RUNTIME_DIR/pimax-ctrl-inhand`,
    /// PIMAX_CTRL_INHAND; docs/CONTROLLERS-INHAND.md). None: no session, or a build without it.
    pub ctrl_inhand: Option<omarchyvr_config::ctrl_inhand::Status>,
    /// The Steam game that runs now (app id of Steam's `reaper SteamLaunch AppId=N` wrapper), if any.
    pub game: Option<String>,
}

impl Live {
    /// A session process by name.
    pub fn proc(&self, name: &str) -> Option<&SessionProc> {
        self.session.iter().find(|p| p.name == name)
    }

    /// Is a session process alive?
    pub fn alive(&self, name: &str) -> bool {
        self.proc(name).is_some_and(|p| p.alive)
    }

    /// Any real desk client alive: WayVR, the passthrough or a voice process. The camera broker, the controller
    /// tracker and the wrist readout also run in a VR (Play) session, so they say nothing about the side.
    pub fn desk_clients_alive(&self) -> bool {
        self.session.iter().any(|p| matches!(p.name, "wayvr" | "passthrough" | "voice" | "voicecap" | "voiceact" | "voicehud") && p.alive)
    }

    /// Any helper of a session alive besides Monado and the wrist readout (broker, controller tracker, desk clients):
    /// what is left over when a session ended without Monado.
    pub fn session_helpers_alive(&self) -> bool {
        self.session.iter().any(|p| !matches!(p.name, "monado" | "perf" | "perfwayvr") && p.alive)
    }

    /// Does the running session belong to the desk (Work)? The start record (`started_as`) decides; without one
    /// (a session started outside the app) the desk clients do.
    pub fn desk_side(&self) -> bool {
        match self.started_as.as_deref() {
            Some(k) => k == "desk",
            None => self.desk_clients_alive(),
        }
    }

    /// Anything of the stack running (Monado or a client).
    pub fn anything_running(&self) -> bool {
        !self.monado_pids.is_empty() || self.session.iter().any(|p| p.alive)
    }
}

/// Read the session pid files and the process table.
pub fn collect_live(m: &dyn Machine, p: &Paths, follower: &mut LogFollower) -> Live {
    let mut live = Live::default();
    for (name, comm) in SESSION_PROCS {
        let pid = m
            .read_str(&p.session_dir.join(format!("{name}.pid")))
            .and_then(|s| s.trim().parse::<u32>().ok());
        let alive = pid.is_some_and(|pid| {
            procs::is_alive(m, pid) && procs::comm(m, pid).as_deref() == Some(comm)
        });
        live.session.push(SessionProc {
            name,
            pid,
            alive,
            age: if alive { pid.and_then(|x| procs::age_secs(m, x)) } else { None },
        });
    }
    live.monado_pids = procs::monado_pids(m);
    let ours = live.proc("monado").filter(|s| s.alive).and_then(|s| s.pid);
    live.foreign_monado = live.monado_pids.iter().copied().filter(|x| Some(*x) != ours).collect();
    if let Some(pid) = ours {
        let env = procs::environ(m, pid);
        let ev = |k: &str| env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        let on = |k: &str| ev(k).is_some_and(|v| v == "1");
        let build = procs::exe(m, pid).and_then(|e| {
            let s = e.to_string_lossy().into_owned();
            s.find("/src/xrt/targets/service/").map(|i| PathBuf::from(&s[..i]))
        });
        live.service = Some(ServiceInfo {
            pid,
            age: procs::age_secs(m, pid),
            build,
            hz: ev("PIMAX_FPS").and_then(|v| v.parse().ok()),
            eye: on("PIMAX_EYE"),
            sixdof: on("PIMAX_SLAM"),
            ctrl_track: on("PIMAX_CTRL_TRACK"),
            broker: ev("PIMAX_CAM_SOURCE").as_deref() == Some("broker"),
            ipd: ev("OXR_DEBUG_IPD_MM"),
            anchor: on("PIMAX_ANCHOR"),
        });
        live.scale_pct = Some(ev("XRT_COMPOSITOR_SCALE_PERCENTAGE").and_then(|v| v.trim().parse().ok()).unwrap_or(140));
        let f = ev("PIMAX_PERF_FILE").map(PathBuf::from).unwrap_or_else(|| p.runtime_dir.join("omarchyvr-perf.stats"));
        let fresh = m.meta(&f).is_some_and(|x| m.now_unix().saturating_sub(x.mtime) <= 5);
        if fresh {
            live.perf = m.read_str(&f).and_then(|t| crate::quality::PerfSnap::parse(&t));
        }
    }
    follower.poll(m);
    live.log = follower.log.clone();
    live.gm_paused = m.read_str(&p.session_dir.join("gamemode-paused.txt")).map(|t| omarchyvr_tools::vr::gamemode::paused_from(&t)).unwrap_or_default();
    live.captions_off = m.exists(&p.runtime_dir.join(crate::subtitles::OFF_FLAG));
    live.ctrl_inhand = m
        .read_str(&p.runtime_dir.join("pimax-ctrl-inhand"))
        .and_then(|t| omarchyvr_config::ctrl_inhand::parse_status(&t))
        // the writer: monado-service, or SteamVR's vrserver (driver_monado runs the same driver)
        .filter(|st| procs::is_alive(m, st.pid));
    live.watchdog = m
        .read_str(&p.session_dir.join(crate::reset::WATCHDOG_PID))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|pid| procs::is_alive(m, *pid));
    if live.watchdog.is_some() {
        let f = p.session_dir.join(crate::reset::WATCHDOG_LOG);
        let len = m.meta(&f).map(|x| x.len).unwrap_or(0);
        live.watchdog_last = m.read_range(&f, len.saturating_sub(4096), 4096).and_then(|b| {
            String::from_utf8_lossy(&b).lines().rev().map(str::trim).find(|l| !l.is_empty()).map(|l| l.chars().take(200).collect())
        });
    }
    live.steamvr = m
        .read_str(&p.session_dir.join(omarchyvr_tools::vr::steamvr::PID_FILE))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|pid| procs::is_alive(m, *pid) && procs::comm(m, *pid).as_deref() == Some("vrserver"));
    if m.exists(&p.session_dir.join(omarchyvr_tools::vr::steamvr::PID_FILE)) {
        let pidf = |f: &str| m.read_str(&p.session_dir.join(f)).and_then(|s| s.trim().parse::<u32>().ok());
        let at_play = pidf(omarchyvr_tools::vr::steamvr::STEAM_PID_FILE);
        let now = m
            .read_str(&p.home.join(".steam/steam.pid"))
            .and_then(|s| s.trim().parse::<u32>().ok())
            .filter(|pid| procs::is_alive(m, *pid) && procs::comm(m, *pid).as_deref() == Some("steam"))
            .or_else(|| procs::by_comm(m, "steam").first().copied())
            .map(|pid| (pid, procs::environ(m, pid)));
        live.steam_note = omarchyvr_tools::vr::steamvr::session_note_from(at_play, live.steamvr.is_some(), now);
    }
    live.steam_tainted = steam_taint(m, &p.home);
    live.game = crate::gameruntime::running_game(m);
    live.started_as = m.read_str(&p.session_dir.join(crate::reset::ARGV_FILE)).and_then(|s| s.lines().next().map(str::to_string)).filter(|k| k == "desk" || k == "vr");
    if live.alive("ctrltrack") {
        let f = p.session_dir.join("ctrltrack.log");
        let len = m.meta(&f).map(|x| x.len).unwrap_or(0);
        if let Some(b) = m.read_range(&f, len.saturating_sub(16 * 1024), 16 * 1024) {
            live.ctrltrack = monlog::ctrltrack_stats(&String::from_utf8_lossy(&b));
        }
    }
    live
}

/// State of `~/.config/openvr/openvrpaths.vrpath` (LESSONS row 20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct OpenVr {
    /// The file exists.
    pub exists: bool,
    /// Points to xrizer.
    pub xrizer: bool,
    /// Read-only (chmod 444), so a SteamVR install cannot rewrite it.
    pub read_only: bool,
}

/// Slower-changing facts.
#[derive(Debug, Clone, Default)]
pub struct Facts {
    /// When collected (unix seconds).
    pub at: u64,
    /// Doctor report (None until the first run).
    pub report: Option<Report>,
    /// This boot's GPU kernel events.
    pub kernel: Vec<KernelEvent>,
    /// Why the kernel log could not be read.
    pub kernel_error: Option<String>,
    /// Monado builds.
    pub builds: Vec<BuildInfo>,
    /// What the installed launcher supports.
    pub caps: LauncherCaps,
    /// Installed launcher vs build.
    pub launcher: Option<InstallState>,
    /// Installed app vs build.
    pub app: Option<InstallState>,
    /// The desktop entry exists.
    pub desktop_entry: bool,
    /// `/dev/videoN` of the Pimax camera.
    pub camera_nodes: Vec<PathBuf>,
    /// Who holds them.
    pub camera_holders: Vec<Holder>,
    /// usbfs node of the eye chip.
    pub eye_node: Option<PathBuf>,
    /// Who holds it.
    pub eye_holders: Vec<Holder>,
    /// Who holds the headset lock.
    pub lock_holders: Vec<Holder>,
    /// OpenXR clients outside the session pid files (pid, comm).
    pub stray_clients: Vec<(u32, String)>,
    /// `/sys/module/nvidia_modeset/parameters/merged_head_sync`.
    pub merged_head_sync: Option<String>,
    /// `modinfo nvidia-modeset` lists `merged_head_sync` (the next boot loads a patched module).
    pub modinfo_mhs: Option<bool>,
    /// The two pacman hooks.
    pub hooks: [bool; 2],
    /// udev rule installed (Some(true) = same as the repository copy).
    pub udev_rule: Option<bool>,
    /// IPD from ipd.env.
    pub ipd_mm: Option<f32>,
    /// eye-calibration.toml exists.
    pub eye_calibration: bool,
    /// Its validated accuracy in degrees (the `# validated ...: accuracy mean X` line, else the mean of the eyes).
    pub eye_accuracy_deg: Option<f32>,
    /// Free bytes in `$HOME`.
    pub free_home: Option<u64>,
    /// Experiment variables in the app's environment (removed from the launcher's).
    pub stale_env: Vec<(String, String)>,
    /// openvrpaths.vrpath.
    pub openvr: OpenVr,
    /// Headset 34a4:0044 enumerated.
    pub headset: bool,
    /// `~/.config/openxr/1/active_runtime.json`: its link target (the file itself when it is not a link; None =
    /// missing).
    pub openxr_runtime: Option<PathBuf>,
    /// What games run with (`~/.config/omarchyvr/games.toml`, the same file the launcher reads).
    pub games_steamvr: bool,
    /// Sound in headset switch (`~/.config/omarchyvr/audio.toml`, default on).
    pub sound_headset: bool,
    /// Headset microphone switch (same file, `headset_mic`, default on).
    pub mic_headset: bool,
    /// The default input now (`pactl get-default-source`; None = unknown).
    pub mic_default: Option<String>,
    /// Game mode switch and compositor time budget (`~/.config/omarchyvr/gamemode.toml`; defaults AI on / auto).
    pub game_mode: bool,
    pub comp_budget: String,
    /// Light game mode (gamemode.toml `light_game`, default on): Play runs only what a game needs.
    pub light_game: bool,
    /// Eye tracking in games (gamemode.toml `eye_in_games`, default off): Play passes `--eye` / `--no-eye` from it.
    pub eye_games: bool,
    /// Games with a saved runtime (`game-runtime.json`; the built-in seed is added by the list function).
    pub game_runtimes: Vec<omarchyvr_tools::vr::gameruntime::Entry>,
    /// The game the next Play is for (`--game`).
    pub next_game: Option<String>,
    /// Brightness (`~/.config/omarchyvr/display.toml`; defaults bright / 100 %).
    pub image_brightness: String,
    pub backlight: u8,
    /// SteamVR is installed (Steam's SteamVR app).
    pub steamvr_installed: bool,
    /// The two Mercury hand models are installed (the Camera hands switch also needs the hands code in the build).
    pub hands_models: bool,
    /// `<session dir>/start-argv.txt` of the last start from any path, with its mtime.
    pub start_argv: Option<(String, u64)>,
    /// `<session dir>/screens.txt` (what the last Work start made of the VR screens), read here so the UI thread never does.
    pub screens_report: Option<String>,
    /// Eye chip 2104:0561 enumerated.
    pub tobii: bool,
    /// The paths the checks looked at that exist (binaries, models, libraries).
    pub present: Vec<PathBuf>,
    /// The shared NTFS drive (LESSONS row 21): None = not set or not on this machine, Some(mounted at /mnt/shared).
    pub shared_drive: Option<bool>,
    /// Its filesystem UUID (`shared_drive_uuid` in `~/.config/omarchyvr/paths.toml`), when set and the disk exists.
    pub shared_drive_uuid: Option<String>,
    /// uid and gid of the user (for the mount options).
    pub ids: (u32, u32),
    /// A polkit authentication agent runs (hyprpolkitagent, polkit-gnome, ...): pkexec can ask without a terminal.
    pub polkit_agent: bool,
    /// Serial number of the headset (profiles; redacted from problem reports).
    pub headset_serial: Option<String>,
    /// Serial number of the eye chip (redacted from problem reports).
    pub tobii_serial: Option<String>,
    /// `/proc/sys/kernel/osrelease`.
    pub kernel_release: Option<String>,
    /// The desktop (`XDG_CURRENT_DESKTOP`, or Hyprland).
    pub compositor: Option<String>,
    /// `systemd-run` on PATH (starts go into their own user scope).
    pub systemd_run: Option<PathBuf>,
    /// A systemd user manager runs (`$XDG_RUNTIME_DIR/systemd/private`).
    pub user_manager: bool,
    /// Every USB device (vendor, product): the headset model is matched against the pimax-device modules.
    pub usb: Vec<(u16, u16)>,
    /// The pimax-device checks plus every doctor check (`DoctorBridge`), run with the doctor (None until then).
    pub findings: Option<Vec<pimax_device::Finding>>,
    /// When the findings were collected (unix seconds).
    pub findings_at: u64,
    /// The manual steps the app checks (packages, units, EDID, Hyprland, calibration runs).
    pub manual: crate::manual::ManualFacts,
    /// The per-device data: the user's own or the shipped Dream Air reference, and what an import from the previous
    /// install would bring back (crate::previous).
    pub device: crate::previous::DeviceFacts,
}

impl Facts {
    /// "headset" / "other" for the default input (None: unknown).
    pub fn mic_kind(&self) -> Option<&'static str> {
        self.mic_default.as_deref().map(omarchyvr_tools::vr::audio::mic_kind)
    }

    /// Was this path present at collection time?
    pub fn has(&self, p: &Path) -> bool {
        self.present.iter().any(|x| x == p)
    }

    /// Doctor result by id.
    pub fn doctor(&self, id: &str) -> Option<&omarchyvr_doctor::Check> {
        self.report.as_ref().and_then(|r| r.get(id))
    }

    /// Doctor level by id.
    pub fn doctor_level(&self, id: &str) -> Option<Level> {
        self.doctor(id).map(|c| c.level)
    }

    /// Build info of a build directory.
    pub fn build(&self, dir: &Path) -> Option<&BuildInfo> {
        self.builds.iter().find(|b| b.build.dir == dir)
    }

    /// Kernel verdict (an unreadable log counts as clean here; the preflight warns about it separately).
    pub fn kernel_verdict(&self) -> kernel::Verdict {
        kernel::verdict(&self.kernel)
    }
}

/// The root-owned copy of the NVIDIA patch script (installed with the pacman hooks): root fixes prefer it over the
/// user-writable copy in the repository.
pub const ROOT_REAPPLY: &str = "/usr/local/lib/omarchyvr/nvidia-patch-reapply.sh";
/// Before the rename the root copy lived here; machines keep it until they reinstall the hooks.
pub const ROOT_REAPPLY_LEGACY: &str = "/usr/local/lib/pimax/nvidia-patch-reapply.sh";  // names:keep

/// The release binaries the app and the launcher use (name, cargo package).
pub const RELEASE_BINS: [(&str, &str); 11] = [
    ("omarchyvr-passthrough", "omarchyvr-passthrough"),
    ("omarchyvr-perf", "omarchyvr-perf"),
    ("pimax-cam", "pimax-cam"),
    ("omarchyvr-slam", "omarchyvr-slam"),
    ("pimax-ctrltrack", "pimax-ctrltrack"),
    ("pimax-eye", "pimax-eye"),
    ("pimax-eyecal", "pimax-eyecal"),
    ("pimax-lenstest", "pimax-lenstest"),
    ("omarchyvr-voice", "omarchyvr-voice"),
    ("omarchyvr-voice-hud", "omarchyvr-voicehud"),
    ("xr-handtest", "omarchyvr-xrtest"),
];

/// Release binaries of optional tools (Room row): looked for, not required by the checklist.
pub const OPTIONAL_BINS: [(&str, &str); 2] = [("omarchyvr-roommap", "omarchyvr-roommap"), ("omarchyvr-anchor", "omarchyvr-anchor")];

/// Video nodes of the Pimax camera (by the V4L2 name); all video nodes if none is named (LESSONS row 2 says
/// `fuser /dev/video*`).
pub fn camera_nodes(m: &dyn Machine) -> Vec<PathBuf> {
    let base = Path::new("/sys/class/video4linux");
    let all: Vec<String> = m.list(base);
    let named: Vec<&String> = all
        .iter()
        .filter(|n| {
            m.read_str(&base.join(n.as_str()).join("name"))
                .is_some_and(|s| s.to_ascii_lowercase().contains("pimax"))
        })
        .collect();
    let pick: Vec<&String> = if named.is_empty() { all.iter().collect() } else { named };
    pick.into_iter().map(|n| Path::new("/dev").join(n)).collect()
}

/// This boot's kernel log through `journalctl -k -b` (else `dmesg`), classified.
pub fn kernel_events(m: &dyn Machine) -> Result<Vec<KernelEvent>, String> {
    let out = m
        .run("journalctl", &["-k", "-b", "--no-pager", "-q"])
        .filter(|o| o.code == Some(0))
        .or_else(|| m.run("dmesg", &[]).filter(|o| o.code == Some(0)));
    match out {
        // a real boot always has kernel lines: journalctl -q prints nothing and exits 0 when the journal is not readable
        Some(o) if o.stdout.trim().is_empty() => {
            Err("the kernel log came back empty (is your user in the systemd-journal or adm group?)".into())
        }
        Some(o) => Ok(kernel::scan(&o.stdout)),
        None => Err("cannot read the kernel log (journalctl -k / dmesg): add yourself to the systemd-journal group".into()),
    }
}

/// Collect everything. `doctor` = also run the doctor checks (a few hundred ms: they run `patch --dry-run` etc.).
pub fn collect_facts(
    m: &dyn Machine,
    p: &Paths,
    opts: &Opts,
    cache: &MarkerCache,
    previous: Option<&Facts>,
    doctor: bool,
    caps: bool,
) -> Facts {
    let mut f = Facts {
        at: m.now_unix(),
        ..Default::default()
    };
    match kernel_events(m) {
        Ok(v) => f.kernel = v,
        Err(e) => f.kernel_error = Some(e),
    }
    let devs = usb_devices(m.host());
    f.usb = devs.iter().map(|d| (d.vid, d.pid)).collect();
    f.manual = crate::manual::collect(m, p);
    if doctor {
        // the device checks run the doctor once more (DoctorBridge): its commands are answered from this round's
        // results instead of being started twice
        let memo = crate::findings::Memo::new(m.host());
        f.report = Some(omarchyvr_doctor::run_checks(&memo, opts));
        let mut found = crate::findings::run(&memo, opts, &f.usb);
        crate::findings::adjust(&mut found, &kernel::verdict(&f.kernel));
        f.findings = Some(found);
        f.findings_at = f.at;
    } else {
        f.report = previous.and_then(|x| x.report.clone());
        f.findings = previous.and_then(|x| x.findings.clone());
        f.findings_at = previous.map(|x| x.findings_at).unwrap_or(0);
    }
    f.builds = builds::inspect_all(m, &p.builds, Some(cache));
    let launcher_path = p.launcher(&|x| m.exists(x));
    let old_caps = previous.map(|x| x.caps.clone()).unwrap_or_default();
    f.caps = if caps || previous.is_none() {
        // a failed `desk help` (binary being replaced, timeout) keeps what was known
        m.run(&launcher_path.display().to_string(), &["desk", "help"])
            .filter(|o| o.code == Some(0))
            .map(|o| launcher::parse_caps(&format!("{}{}", o.stdout, o.stderr)))
            .filter(|c| c.desk)
            .unwrap_or(old_caps)
    } else {
        old_caps
    };
    let newest = launcher::newest_source(m, &launcher::launcher_crates(p));
    f.launcher = Some(launcher::install_state(m, &p.launcher_installed, &p.launcher_built, newest));
    let app_src = launcher::newest_source(m, &[p.rust.join("omarchyvr-app"), p.rust.join("omarchyvr-gui-core")]);
    f.app = Some(launcher::install_state(m, &p.app_installed, &p.app_built, app_src));
    f.desktop_entry = m.exists(&p.desktop_entry);
    f.camera_nodes = camera_nodes(m);
    let headset = devs.iter().find(|d| d.vid == omarchyvr_doctor::PIMAX_VID && d.pid == omarchyvr_doctor::DREAM_AIR_PID);
    f.headset = headset.is_some();
    f.headset_serial = headset.and_then(|d| d.serial.clone()).filter(|s| !s.is_empty());
    let tobii = devs.iter().find(|d| d.vid == omarchyvr_doctor::TOBII_VID && d.pid == omarchyvr_doctor::TOBII_PID);
    f.tobii = tobii.is_some();
    f.tobii_serial = tobii.and_then(|d| d.serial.clone()).filter(|s| !s.is_empty());
    f.eye_node = tobii.and_then(|d| d.node.clone());
    let mut watched: Vec<PathBuf> = f.camera_nodes.clone();
    watched.extend(f.eye_node.clone());
    watched.push(p.headset_lock.clone());
    let all = procs::holders(m, &watched);
    f.camera_holders = all.iter().filter(|h| f.camera_nodes.contains(&h.path)).cloned().collect();
    f.eye_holders = all.iter().filter(|h| Some(&h.path) == f.eye_node.as_ref()).cloned().collect();
    f.lock_holders = all.iter().filter(|h| h.path == p.headset_lock).cloned().collect();
    let session_pids: Vec<u32> = SESSION_PROCS
        .iter()
        .filter_map(|(n, _)| m.read_str(&p.session_dir.join(format!("{n}.pid"))))
        .filter_map(|s| s.trim().parse().ok())
        .collect();
    for c in XR_CLIENT_COMMS {
        for pid in procs::by_comm(m, c) {
            if !session_pids.contains(&pid) {
                f.stray_clients.push((pid, c.to_string()));
            }
        }
    }
    f.merged_head_sync = m.read_str(Path::new("/sys/module/nvidia_modeset/parameters/merged_head_sync"));
    f.modinfo_mhs = m
        .run("modinfo", &["-F", "parm", "nvidia-modeset"])
        .filter(|o| o.code == Some(0))
        .map(|o| o.stdout.contains("merged_head_sync"));
    f.hooks = [
        m.exists(Path::new("/etc/pacman.d/hooks/65-nvidia-pimax-patch.hook")),
        m.exists(Path::new("/etc/pacman.d/hooks/75-nvidia-pimax-verify.hook")),
    ];
    // a package installs the rule in /usr/lib/udev/rules.d, a manual install in /etc/udev/rules.d
    let rule = m.read(Path::new("/etc/udev/rules.d/70-pimax.rules")).or_else(|| m.read(Path::new("/usr/lib/udev/rules.d/70-pimax.rules")));
    f.udev_rule = rule.map(|r| Some(r) == m.read(&p.repo.join("linux/70-pimax.rules")));
    f.ipd_mm = m.read_str(&p.ipd_env).and_then(|t| launcher::parse_ipd_env(&t));
    f.eye_calibration = m.exists(&p.eye_calibration);
    f.eye_accuracy_deg = m.read_str(&p.eye_calibration).and_then(|t| eye_accuracy(&t));
    f.device = crate::previous::collect(&p.home, &|k| m.env(k));
    f.free_home = m.free_bytes(&p.home);
    f.stale_env = launcher::stale_experiment_env(&|k| m.env(k));
    if let Some(meta) = m.meta(&p.openvr_paths) {
        let text = m.read(&p.openvr_paths).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        f.openvr = OpenVr {
            exists: true,
            xrizer: text.contains("xrizer"),
            read_only: meta.mode & 0o222 == 0,
        };
    }
    f.games_steamvr = m
        .read_str(&omarchyvr_tools::vr::steamvr::settings_path(&p.home))
        .and_then(|t| omarchyvr_tools::vr::steamvr::parse_settings(&t))
        .unwrap_or(omarchyvr_tools::vr::steamvr::DEFAULT)
        == omarchyvr_tools::vr::steamvr::Games::SteamVr;
    let audio_toml = m.read_str(&omarchyvr_tools::vr::audio::settings_path(&p.home));
    f.sound_headset = audio_toml.as_deref().and_then(omarchyvr_tools::vr::audio::parse_settings).unwrap_or(true);
    f.mic_headset = audio_toml.as_deref().and_then(omarchyvr_tools::vr::audio::parse_mic).unwrap_or(true);
    f.mic_default = m
        .run("pactl", &["get-default-source"])
        .filter(|o| o.code == Some(0))
        .map(|o| o.stdout.trim().to_string())
        .filter(|s| !s.is_empty());
    let gm = m.read_str(&omarchyvr_tools::vr::gamemode::settings_path(&p.home)).map(|t| omarchyvr_tools::vr::gamemode::parse_settings(&t)).unwrap_or_default();
    f.game_mode = gm.pause_ai;
    f.comp_budget = gm.comp_budget;
    f.light_game = gm.light;
    f.eye_games = gm.eye_games;
    let grs = m.read_str(&omarchyvr_tools::vr::gameruntime::store_path(&p.home)).map(|t| omarchyvr_tools::vr::gameruntime::Store::parse(&t)).unwrap_or_default();
    f.game_runtimes = grs.saved.into_values().collect();
    f.next_game = grs.next;
    let ds = m.read_str(&omarchyvr_tools::vr::display::settings_path(&p.home)).map(|t| omarchyvr_tools::vr::display::parse_settings(&t)).unwrap_or_default();
    f.image_brightness = ds.image;
    f.backlight = ds.backlight;
    f.steamvr_installed = m.exists(&omarchyvr_tools::vr::openvr_link::steamvr_dir(&p.home).join("bin/linux64/vrclient.so"));
    let xr = omarchyvr_tools::vr::runtime_link::link_path(&p.home);
    f.openxr_runtime = m.read_link(&xr).or_else(|| m.exists(&xr).then(|| xr.clone()));
    f.hands_models = omarchyvr_tools::vr::hands::MODELS.iter().all(|n| {
            [p.home.join(".local/share"), PathBuf::from("/usr/local/share"), PathBuf::from("/usr/share")]
                .iter()
                .any(|d| {
                    let f = d.join("monado/hand-tracking-models").join(n);
                    // a git-lfs pointer (an unfinished download) is a few hundred bytes, not a model
                    m.exists(&f) && m.meta(&f).map_or(true, |x| x.len >= 1024)
                })
        });
    let argv = p.session_dir.join(omarchyvr_tools::vr::supervise::ARGV_FILE);
    f.screens_report = m.read_str(&p.session_dir.join(omarchyvr_tools::vr::screens::REPORT_FILE));
    f.start_argv = m.read_str(&argv).and_then(|t| m.meta(&argv).map(|x| (t, x.mtime)));
    let mut check: Vec<PathBuf> = RELEASE_BINS.iter().chain(OPTIONAL_BINS.iter()).map(|(b, _)| p.bin(b)).collect();
    // the installed set (~/.local/bin) is what an installed launcher runs
    check.extend(RELEASE_BINS.iter().chain(OPTIONAL_BINS.iter()).map(|(b, _)| p.installed_bin(b)));
    check.extend([
        p.wayvr_bin.clone(),
        p.wayvr_src.clone(),
        p.basalt_lib.clone(),
        p.voice_model.clone(),
        p.vad_model.clone(),
        p.sherpa_lib.clone(),
        p.launcher_built.clone(),
        p.launcher_installed.clone(),
        p.icon.clone(),
        PathBuf::from(ROOT_REAPPLY),
        PathBuf::from(ROOT_REAPPLY_LEGACY),
    ]);
    check.extend(crate::autosetup::EDID_FILES.iter().map(PathBuf::from));
    {
        // the installed root job (its EDID-only mode is the connector check's fix)
        let i = crate::install::Install::of_home(&p.home);
        check.extend([i.root_script(), i.plan_env()]);
    }
    for b in &p.builds {
        check.push(b.dir.join("CMakeCache.txt"));
    }
    f.present = check.into_iter().filter(|x| m.exists(x)).collect();
    use omarchyvr_tools::mount_shared::{mounted_at, SHARED_TARGET};
    // the shared drive is a personal setting (paths.toml shared_drive_uuid, no default): no UUID, no check
    let uuid = std::env::var("PIMAX_SHARED_UUID").ok().filter(|u| omarchyvr_config::paths::valid_uuid(u)).or_else(|| {
        m.read_str(&p.home.join(omarchyvr_config::paths::PATHS_FILE))
            .and_then(|t| omarchyvr_config::paths::value(&t, "shared_drive_uuid"))
            .filter(|u| omarchyvr_config::paths::valid_uuid(u))
    });
    if let Some(uuid) = uuid.filter(|u| m.exists(&Path::new("/dev/disk/by-uuid").join(u))) {
        let mounts = m.read(Path::new("/proc/mounts")).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default();
        f.shared_drive = Some(mounted_at(&mounts, SHARED_TARGET).is_some());
        f.shared_drive_uuid = Some(uuid);
    }
    f.ids = (m.uid(), m.gid());
    f.kernel_release = m.read_str(Path::new("/proc/sys/kernel/osrelease"));
    f.compositor = m.env("XDG_CURRENT_DESKTOP").filter(|s| !s.is_empty()).or_else(|| m.env("HYPRLAND_INSTANCE_SIGNATURE").map(|_| "Hyprland".into()));
    let path = m.env("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    f.systemd_run = crate::term::which("systemd-run", &path, &|x| m.exists(x));
    f.user_manager = m.exists(&p.runtime_dir.join("systemd/private"));
    f.polkit_agent = procs::pids(m).into_iter().any(|p| polkit_agent_proc(m, p));
    f
}

/// A polkit authentication agent: a dedicated one (hyprpolkitagent, polkit-gnome, ...), or the Omarchy shell
/// (quickshell running /usr/share/omarchy/shell), which shows the password dialog itself.
fn polkit_agent_proc(m: &dyn Machine, pid: u32) -> bool {
    match procs::comm(m, pid) {
        Some(c) if c.contains("polkit") => c != "polkitd",
        Some(c) if c == "quickshell" => procs::cmdline(m, pid).iter().any(|a| a.contains("/omarchy/shell")),
        _ => false,
    }
}

/// The accuracy of an eye calibration file: the validation summary line, else the mean of `accuracy_deg`.
pub fn eye_accuracy(text: &str) -> Option<f32> {
    let validated = text.lines().find_map(|l| {
        let rest = l.strip_prefix('#')?.split_once("accuracy mean ")?.1;
        rest.split_whitespace().next()?.parse::<f32>().ok()
    });
    if validated.is_some() {
        return validated;
    }
    let eyes: Vec<f32> = text
        .lines()
        .filter_map(|l| l.trim().strip_prefix("accuracy_deg")?.trim().strip_prefix('=')?.trim().parse::<f32>().ok())
        .collect();
    (!eyes.is_empty()).then(|| eyes.iter().sum::<f32>() / eyes.len() as f32)
}

/// Free space in GiB, one decimal.
pub fn gib(bytes: u64) -> String {
    format!("{:.1} GiB", bytes as f64 / (1u64 << 30) as f64)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    #[test]
    fn live_session_from_pid_files_and_proc() {
        let (m, p) = demo::running_machine();
        let mut f = LogFollower::new(&p.session_dir.join("monado.log"));
        let live = collect_live(&m, &p, &mut f);
        let s = live.service.as_ref().unwrap();
        assert_eq!(s.hz, Some(72));
        assert!(s.eye && s.sixdof && s.broker && !s.ctrl_track);
        assert_eq!(s.build.as_deref(), Some(Path::new("/home/u/.local/lib/omarchyvr/monado")));
        assert!(live.alive("broker") && live.alive("passthrough") && live.alive("wayvr"));
        assert!(!live.alive("voice"));
        assert!(live.foreign_monado.is_empty());
        assert!(live.desk_clients_alive() && live.anything_running());
        assert_eq!(live.log.sixdof.state, crate::monlog::TrackState::Tracking);
        assert!(live.proc("monado").unwrap().age.unwrap() > 0.0);
    }

    #[test]
    fn the_omarchy_shell_counts_as_a_polkit_agent() {
        let (mut m, _p) = demo::running_machine();
        m.file("/proc/71001/comm", "polkitd\n");
        assert!(!polkit_agent_proc(&m, 71001), "the polkit daemon is not an agent");
        m.file("/proc/71002/comm", "quickshell\n");
        m.file("/proc/71002/cmdline", "quickshell\0-n\0-p\0/usr/share/omarchy/shell\0");
        assert!(polkit_agent_proc(&m, 71002), "Omarchy's shell shows the password dialog (2026-10-17 install)");
        m.file("/proc/71003/comm", "quickshell\n");
        m.file("/proc/71003/cmdline", "quickshell\0-p\0/home/u/my-bar\0");
        assert!(!polkit_agent_proc(&m, 71003), "another quickshell config is not known to have an agent");
        m.file("/proc/71004/comm", "hyprpolkitagent\n");
        assert!(polkit_agent_proc(&m, 71004));
    }

    #[test]
    fn steamvr_session_problems_are_plain_words() {
        let (mut m, p) = demo::running_machine();
        let mut f = LogFollower::new(&p.session_dir.join("monado.log"));
        assert_eq!(collect_live(&m, &p, &mut f).steam_note, None, "no SteamVR session: nothing to say");
        // our vrserver is gone (incident 2026-10-16: vrmonitor quit all)
        m.file(&p.session_dir.join("steamvr.pid").display().to_string(), "99991\n");
        let n = collect_live(&m, &p, &mut f).steam_note.unwrap();
        assert!(n.contains("SteamVR has quit"), "{n}");
    }

    #[test]
    fn a_steam_started_from_inside_steamvr_is_detected_with_the_busy_hint() {
        let (mut m, p) = demo::idle_machine();
        let home = p.home.display().to_string();
        assert_eq!(steam_taint(&m, &p.home), None, "no Steam");
        m.process(7001, "steam", "/home/u/.local/share/Steam/ubuntu12_32/steam steam://launch/658920/VR", &[("HOME", "/home/u"), ("PIMAX_SLAM", "1"), ("SteamAppId", "250820")], 5, &[]);
        let t = steam_taint(&m, &p.home).unwrap();
        assert_eq!((t.pid, t.vars.clone(), t.busy.clone()), (7001, vec!["PIMAX_SLAM".to_string(), "SteamAppId".to_string()], None));
        m.process(7002, "reaper", "reaper SteamLaunch AppId=730", &[], 6, &[]);
        assert_eq!(steam_taint(&m, &p.home).unwrap().busy.as_deref(), Some("a game seems to be running"));
        m.process(7001, "steam", "/home/u/.local/share/Steam/ubuntu12_32/steam", &[("HOME", "/home/u"), ("SteamAppId", "730")], 5, &[]);
        assert_eq!(steam_taint(&m, &p.home), None, "a clean Steam is not asked about ({home})");
        assert_eq!(steam_busy_hint(false, true).as_deref(), Some("a download seems to be in progress"));
        assert!(steam_busy_hint(true, true).unwrap().contains("download"));
        assert_eq!(steam_busy_hint(false, false), None);
    }

    #[test]
    fn stale_pid_file_with_reused_pid_is_not_alive() {
        let (mut m, p) = demo::running_machine();
        // pid of the wayvr pid file now belongs to another program
        m.file("/proc/378947/comm", "bash\n");
        let mut f = LogFollower::new(&p.session_dir.join("monado.log"));
        let live = collect_live(&m, &p, &mut f);
        assert!(!live.alive("wayvr"));
        assert_eq!(live.proc("wayvr").unwrap().pid, Some(378_947));
    }

    #[test]
    fn facts_of_the_demo_machine() {
        let (m, p) = demo::idle_machine();
        let f = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, true, true);
        assert!(f.headset && f.tobii);
        assert_eq!(f.camera_nodes, vec![PathBuf::from("/dev/video0"), PathBuf::from("/dev/video1")]);
        assert!(f.camera_holders.is_empty());
        assert_eq!(f.eye_node, Some(PathBuf::from("/dev/bus/usb/001/009")));
        assert!(f.caps.sixdof && f.caps.ctrl_track);
        assert_eq!(f.merged_head_sync.as_deref(), Some("1"));
        assert_eq!(f.hooks, [true, true]);
        assert_eq!(f.ipd_mm, Some(65.9));
        assert!(matches!(f.kernel_verdict(), crate::kernel::Verdict::WayvrOnly(_)));
        assert!(f.report.as_ref().unwrap().get("headset").is_some());
        let desk = f.build(&p.desk_build().dir).unwrap();
        assert_eq!(desk.pimax_cam(), Some(true));
        assert_eq!(desk.sys_nice, Some(true));
        let slam = f.build(&p.build("slam").unwrap().dir).unwrap();
        assert_eq!(slam.sys_nice, Some(false), "the slam build lost its capability");
        assert!(f.has(&p.bin("pimax-eye")));
    }

    #[test]
    fn empty_journal_is_unreadable_not_clean_and_caps_survive_a_failed_help() {
        let (mut m, p) = demo::idle_machine();
        m.cmd("journalctl -k -b --no-pager -q", 0, "");
        let f = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, false, true);
        assert!(f.kernel_error.as_deref().is_some_and(|e| e.contains("systemd-journal")), "{:?}", f.kernel_error);
        let (mut m, p) = demo::idle_machine();
        let first = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, false, true);
        assert!(first.caps.sixdof);
        m.cmd("/home/u/.local/bin/omarchyvr desk help", 1, "");
        let again = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), Some(&first), false, true);
        assert_eq!(again.caps, first.caps);
    }

    #[test]
    fn eye_calibration_accuracy() {
        let t = "# validated 20261014-103752: accuracy mean 3.08 median 2.93 p95 5.85 deg on 10 targets\n[left]\naccuracy_deg = 3.1005\n";
        assert_eq!(eye_accuracy(t), Some(3.08));
        assert_eq!(eye_accuracy("[left]\naccuracy_deg = 3.0\n[right]\naccuracy_deg = 5.0\n"), Some(4.0));
        assert_eq!(eye_accuracy("version = 1\n"), None);
    }

    #[test]
    fn holders_of_camera_eye_and_lock() {
        let (mut m, p) = demo::idle_machine();
        m.process(4242, "pimax-cam", "/home/u/omarchyvr/rust/target/release/pimax-cam broker", &[], 10, &["/dev/video0"]);
        m.process(4243, "pimax-eye", "pimax-eye watch", &[], 10, &["/dev/bus/usb/001/009", "/tmp/omarchyvr-headset.lock"]);
        m.process(4244, "pimax-lenstest", "pimax-lenstest", &[], 10, &[]);
        let f = collect_facts(&m, &p, &demo::opts(&p), &MarkerCache::default(), None, false, false);
        assert_eq!(f.camera_holders.len(), 1);
        assert_eq!(f.camera_holders[0].pid, 4242);
        assert_eq!(f.eye_holders[0].comm, "pimax-eye");
        assert_eq!(f.lock_holders[0].pid, 4243);
        assert_eq!(f.stray_clients, vec![(4244, "pimax-lenstest".to_string())]);
    }
}

/// The running Steam when it carries SteamVR's variables (reads one `/proc/PID/environ`).
pub fn steam_taint(m: &dyn Machine, home: &Path) -> Option<TaintedSteam> {
    let pid = m
        .read_str(&home.join(".steam/steam.pid"))
        .and_then(|s| s.trim().parse::<u32>().ok())
        .filter(|pid| procs::is_alive(m, *pid) && procs::comm(m, *pid).as_deref() == Some("steam"))
        .or_else(|| procs::by_comm(m, "steam").first().copied())?;
    let vars = omarchyvr_tools::vr::steamvr::leaked_vars(&procs::environ(m, pid));
    if vars.is_empty() {
        return None;
    }
    let download = m.list(&home.join(".local/share/Steam/steamapps/downloading")).iter().any(|n| !n.starts_with('.'));
    Some(TaintedSteam { pid, vars, busy: steam_busy_hint(!procs::by_comm(m, "reaper").is_empty(), download) })
}
