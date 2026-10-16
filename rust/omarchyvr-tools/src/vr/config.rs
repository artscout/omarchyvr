//! Options of `omarchyvr vr start` and the environment of the monado-service it launches.
//!
//! The environment is the one of `analysis/tools/run_vr_session.sh` (Wayland DRM lease on DP-1, mode index 2 = 72 Hz,
//! 3 = 90 Hz) plus `PIMAX_FPS` (video-prepare rate, keepalive rate byte, frame interval of the HMD driver) and
//! `XRT_NO_STDIN` (a detached service has no usable stdin; the script feeds it from `sleep | monado-service` instead).
//! Everything here is pure so it can be unit-tested without hardware.
use crate::CliError;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where the log, the pid file and the session summary live.
pub const SESSION_DIR: &str = "/tmp/omarchyvr-session";
/// Process name (`/proc/PID/comm`) of the Monado service.
pub const SERVICE_COMM: &str = "monado-service";
/// Default connector leased to Monado.
pub const DEFAULT_CONNECTOR: &str = "DP-1";
/// Default seconds to wait for the gyro calibration line.
pub const DEFAULT_READY_TIMEOUT_SECS: f64 = 30.0;
/// Seconds between SIGINT and SIGKILL in `vr stop`.
pub const DEFAULT_STOP_GRACE_SECS: f64 = 5.0;

/// Refresh rate of the headset.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fps {
    /// 72 Hz (the rate that was used so far).
    Hz72,
    /// 90 Hz (Pimax Play's rate on Windows).
    Hz90,
}

impl Fps {
    /// Parse `72` or `90`.
    pub fn parse(s: &str) -> Result<Self, CliError> {
        match s.trim() {
            "72" => Ok(Fps::Hz72),
            "90" => Ok(Fps::Hz90),
            other => Err(CliError::usage(format!(
                "--fps must be 72 or 90, not {other}"
            ))),
        }
    }

    /// Hertz.
    pub fn hz(self) -> u32 {
        match self {
            Fps::Hz72 => 72,
            Fps::Hz90 => 90,
        }
    }

    /// Index into the list of display modes Monado enumerates for DP-1 (`XRT_COMPOSITOR_DESIRED_MODE`).
    /// The EDID override lists 7104x3840 @90, @72 and two fallbacks; 2 selects 72 Hz and 3 selects 90 Hz.
    pub fn desired_mode(self) -> u32 {
        match self {
            Fps::Hz72 => 2,
            Fps::Hz90 => 3,
        }
    }
}

/// Everything `vr start` needs to know.
#[derive(Debug, Clone)]
pub struct StartOptions {
    /// Refresh rate.
    pub fps: Fps,
    /// Overrides the mode index derived from `fps`.
    pub mode_override: Option<u32>,
    /// `XRT_COMPOSITOR_DUMP_DIR`: the compositor writes the final target image of selected frames here.
    pub dump_dir: Option<PathBuf>,
    /// `XRT_COMPOSITOR_DUMP_FRAMES` (comma separated frame numbers, Monado's default is `100,101,300,301`).
    pub dump_frames: Option<String>,
    /// Monado build directory (contains `openxr_monado-dev.json` and `src/xrt/targets/service/monado-service`).
    pub monado_build: PathBuf,
    /// Explicit service binary (tests, other builds); defaults to the one inside `monado_build`.
    pub service_bin: Option<PathBuf>,
    /// Connector to lease.
    pub connector: String,
    /// True when `--connector` was not given: [`super::resolve_connector`] may replace `connector` by the one whose
    /// EDID says Pimax Dream Air.
    pub connector_auto: bool,
    /// Extra environment for monado-service (for example `PIMAX_EYE=1`), appended by [`build_env`].
    pub extra_env: Vec<(String, String)>,
    /// Directory for the log, pid file and session summary.
    pub session_dir: PathBuf,
    /// Give up waiting for the gyro calibration after this long.
    pub ready_timeout: Duration,
    /// Start even if the preflight checks fail.
    pub force: bool,
    /// Only run the preflight and print the plan.
    pub dry_run: bool,
    /// Send desktop notifications.
    pub notify: bool,
    /// Stay in the foreground until the service exits (needed when a launcher kills the process group).
    pub wait: bool,
    /// Compositor and pacing debug logging (large log).
    pub debug: bool,
    /// `XRT_COMPOSITOR_OUTPUT_GAMMA` (needs the compositor patch `analysis/overnight/srgb-target.patch`).
    pub output_gamma: Option<f32>,
    /// `XRT_COMPOSITOR_OUTPUT_GAIN` (same patch).
    pub output_gain: Option<f32>,
    /// `XRT_COMPOSITOR_TARGET_FORMAT` (same patch).
    pub target_format: Option<String>,
    /// Point `~/.config/openxr/1/active_runtime.json` at this build's manifest on start (`--no-set-runtime` clears it).
    pub set_runtime: bool,
}

/// Names the patched compositor accepts for `XRT_COMPOSITOR_TARGET_FORMAT`.
pub const TARGET_FORMATS: [&str; 7] = [
    "A2B10G10R10_UNORM",
    "B8G8R8A8_UNORM",
    "R8G8B8A8_UNORM",
    "A8B8G8R8_UNORM",
    "B8G8R8A8_SRGB",
    "R8G8B8A8_SRGB",
    "A8B8G8R8_SRGB",
];

/// Check `--target-format`.
pub fn check_target_format(s: &str) -> Result<String, CliError> {
    if TARGET_FORMATS.contains(&s) {
        Ok(s.to_string())
    } else {
        Err(CliError::usage(format!(
            "--target-format must be one of {}",
            TARGET_FORMATS.join(", ")
        )))
    }
}

/// Check a `--gamma` / `--gain` value against `lo..=hi`.
pub fn check_factor(name: &str, v: f32, lo: f32, hi: f32) -> Result<f32, CliError> {
    if v.is_finite() && v >= lo && v <= hi {
        Ok(v)
    } else {
        Err(CliError::usage(format!(
            "{name} must be a number between {lo} and {hi}"
        )))
    }
}

impl StartOptions {
    /// Path of the service binary.
    pub fn service_path(&self) -> PathBuf {
        self.service_bin.clone().unwrap_or_else(|| {
            self.monado_build
                .join("src/xrt/targets/service")
                .join(SERVICE_COMM)
        })
    }

    /// Path of the OpenXR runtime manifest of the same build.
    pub fn runtime_json(&self) -> PathBuf {
        self.monado_build.join("openxr_monado-dev.json")
    }

    /// `/proc/PID/comm` of the service: the file name of the binary, cut to the kernel's 15 characters.
    pub fn service_comm(&self) -> String {
        comm_of(&self.service_path())
    }

    /// Log file of the service.
    pub fn log_path(&self) -> PathBuf {
        self.session_dir.join("monado.log")
    }

    /// Pid file written by `vr start`.
    pub fn pid_path(&self) -> PathBuf {
        self.session_dir.join("monado.pid")
    }

    /// Summary of the last start (`key=value` lines).
    pub fn summary_path(&self) -> PathBuf {
        self.session_dir.join("session.txt")
    }
}

/// The kernel's `comm` of an executable: its file name, at most 15 bytes.
pub fn comm_of(path: &Path) -> String {
    omarchyvr_components::components::proc::comm_of(path)
}

/// Where the workspace's release tools are (pimax-cam, omarchyvr-passthrough, ...): next to the launcher when it runs
/// from a package (`/usr/bin/omarchyvr`) or from the user install (`~/.local/bin/omarchyvr`: an installed session uses only
/// installed binaries, never the development tree agents rebuild while working), else the development tree
/// `~/pimax-re/rust/target/release`.
pub fn tools_dir(home: &Path) -> PathBuf {
    tools_dir_for(std::env::current_exe().ok().as_deref(), home)
}

/// [`tools_dir`] for a given launcher path.
pub fn tools_dir_for(exe: Option<&Path>, home: &Path) -> PathBuf {
    match exe.and_then(|e| e.parent()) {
        Some(d) if d.starts_with("/usr/") => d.to_path_buf(),
        Some(d) if d == home.join(".local/bin") => d.to_path_buf(),
        _ => home.join("pimax-re/rust/target/release"),
    }
}

/// `$HOME/src/monado/build`, overridable with `PIMAX_MONADO_BUILD`.
pub fn default_monado_build(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    if let Some(p) = env("PIMAX_MONADO_BUILD").filter(|s| !s.is_empty()) {
        return PathBuf::from(p);
    }
    let home = PathBuf::from(env("HOME").unwrap_or_else(|| "/root".into()));
    // The desk build (eye + 6DoF + controllers) when present, else the plain one.
    let desk = omarchyvr_config::paths::src_dir(&home).join("monado-eye/build");
    if desk.join("openxr_monado-dev.json").exists() {
        return desk;
    }
    omarchyvr_config::paths::src_dir(&home).join("monado/build")
}

/// Validate a list of frame numbers for `XRT_COMPOSITOR_DUMP_FRAMES`.
pub fn check_frame_list(s: &str) -> Result<String, CliError> {
    let parts: Vec<&str> = s.split(',').map(str::trim).collect();
    if parts.is_empty() || parts.iter().any(|p| p.parse::<u32>().is_err()) {
        return Err(CliError::usage(format!(
            "--dump-frames wants comma separated frame numbers, not {s:?}"
        )));
    }
    Ok(parts.join(","))
}

/// Environment variables set for monado-service, in a stable order. `base` reads the caller's environment: its
/// `XRT_COMPOSITOR_LOG`, `PIMAX_LOG` and `XRT_COMPOSITOR_SCALE_PERCENTAGE` (or `SCALE`, as in the shell script)
/// are kept; everything else listed here is forced.
pub fn build_env(o: &StartOptions, base: &dyn Fn(&str) -> Option<String>) -> Vec<(String, String)> {
    let get = |k: &str| base(k).filter(|v| !v.is_empty());
    let mut v: Vec<(String, String)> = Vec::new();
    let mut put = |k: &str, val: String| v.push((k.to_string(), val));
    put("XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT", "1".into());
    put("XRT_COMPOSITOR_WAYLAND_CONNECTOR", o.connector.clone());
    put("XRT_COMPOSITOR_FORCE_NVIDIA", "0".into());
    put(
        "XRT_COMPOSITOR_DESIRED_MODE",
        o.mode_override
            .unwrap_or_else(|| o.fps.desired_mode())
            .to_string(),
    );
    put("PIMAX_FPS", o.fps.hz().to_string());
    let comp_log = if o.debug {
        "debug".to_string()
    } else {
        get("XRT_COMPOSITOR_LOG").unwrap_or_else(|| "info".into())
    };
    put("XRT_COMPOSITOR_LOG", comp_log);
    // SCALE is the shell script's name for it; it is a generic word, so only a plausible percentage counts
    let scale_env =
        get("SCALE").filter(|v| v.parse::<f32>().is_ok_and(|p| (10.0..=400.0).contains(&p)));
    put(
        "XRT_COMPOSITOR_SCALE_PERCENTAGE",
        get("XRT_COMPOSITOR_SCALE_PERCENTAGE")
            .or(scale_env)
            .unwrap_or_else(|| "100".into()),
    );
    // "gyro bias done" is an INFO line of the headset driver: a quieter PIMAX_LOG would make every start time out
    put(
        "PIMAX_LOG",
        get("PIMAX_LOG")
            .filter(|v| matches!(v.to_ascii_lowercase().as_str(), "trace" | "debug" | "info"))
            .unwrap_or_else(|| "info".into()),
    );
    put(
        "XR_RUNTIME_JSON",
        o.runtime_json().to_string_lossy().into_owned(),
    );
    for (k, val) in &o.extra_env {
        put(k, val.clone());
    }
    // the IPD measured by `pimax-eye ipd --apply` (~/.config/omarchyvr/ipd.env); the caller's own value wins
    if get("OXR_DEBUG_IPD_MM").is_none() && !o.extra_env.iter().any(|(k, _)| k == "OXR_DEBUG_IPD_MM") {
        if let Some(mm) = get("HOME").and_then(|h| std::fs::read_to_string(Path::new(&h).join(IPD_ENV)).ok()).and_then(|t| ipd_from_env_file(&t)) {
            put("OXR_DEBUG_IPD_MM", mm);
        }
    }
    // lets `vr stop` attach eu-stack/gdb to a service that has to be SIGKILLed (yama ptrace_scope=1)
    put("PIMAX_DEBUG_PTRACE", "1".into());
    put("IPC_IGNORE_VERSION", "1".into());
    put("XRT_NO_STDIN", "1".into());
    // Compositor time budget (Home > Quality / Performance): Monado's pacer minimum compositor time in ms (default
    // 3.0); the caller's own value wins, auto = not set
    if get("U_PACING_COMP_MIN_TIME_MS").is_none() && !o.extra_env.iter().any(|(k, _)| k == "U_PACING_COMP_MIN_TIME_MS") {
        if let Some(ms) = get("HOME").and_then(|h| {
            let s = super::gamemode::load(Path::new(&h));
            super::gamemode::budget_env(&s.comp_budget)
        }) {
            put("U_PACING_COMP_MIN_TIME_MS", ms);
        }
    }
    if o.debug {
        put("U_PACING_COMPOSITOR_LOG", "debug".into());
        put("U_PACING_APP_LOG", "debug".into());
    }
    if let Some(d) = &o.dump_dir {
        put("XRT_COMPOSITOR_DUMP_DIR", d.to_string_lossy().into_owned());
        if let Some(f) = &o.dump_frames {
            put("XRT_COMPOSITOR_DUMP_FRAMES", f.clone());
        }
    }
    if let Some(g) = o.output_gamma {
        put("XRT_COMPOSITOR_OUTPUT_GAMMA", g.to_string());
    } else if get("XRT_COMPOSITOR_OUTPUT_GAMMA").is_none() {
        // Home > Quality / Performance > Brightness (display.toml); the caller's own value wins
        if let Some(g) = get("HOME").and_then(|h| super::display::gamma_env(&super::display::load(Path::new(&h)).image)) {
            put("XRT_COMPOSITOR_OUTPUT_GAMMA", g);
        }
    }
    if let Some(g) = o.output_gain {
        put("XRT_COMPOSITOR_OUTPUT_GAIN", g.to_string());
    }
    if let Some(f) = &o.target_format {
        put("XRT_COMPOSITOR_TARGET_FORMAT", f.clone());
    }
    v
}

/// The IPD file written by `pimax-eye ipd --apply`, relative to `$HOME`.
pub const IPD_ENV: &str = ".config/omarchyvr/ipd.env";

/// `OXR_DEBUG_IPD_MM=61.5` from the ipd.env text, if it is a plausible IPD (50-80 mm).
pub fn ipd_from_env_file(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .filter_map(|l| l.trim().strip_prefix("OXR_DEBUG_IPD_MM="))
        .map(|v| v.trim().trim_matches('"').to_string())
        .find(|v| v.parse::<f32>().is_ok_and(|mm| (50.0..=80.0).contains(&mm)))
}

/// `game_runtime` of auto mode (`$PIMAX_AUTO_CONFIG` or `~/.config/omarchyvr/auto.toml`): "monado" or "steamvr".
pub fn auto_game_runtime(env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    let path = env("PIMAX_AUTO_CONFIG")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| env("HOME").map(|h| Path::new(&h).join(".config/omarchyvr/auto.toml")))?;
    game_runtime_from_toml(&std::fs::read_to_string(path).ok()?)
}

/// The `game_runtime = "..."` value of an auto.toml text.
pub fn game_runtime_from_toml(text: &str) -> Option<String> {
    text.lines()
        .rev()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim() == "game_runtime")
        .map(|(_, v)| v.trim().trim_matches('"').to_string())
        .next()
}

/// Variables that change what the compositor or the headset driver does and that are NOT set by `build_env`; if the
/// caller's environment has them they reach monado-service anyway (that is how experiments work), so `vr start` lists
/// them: a stale `XRT_COMPOSITOR_DUMP_DIR` or `XRT_COMPOSITOR_OUTPUT_GAMMA` from an earlier experiment must not surprise anyone.
pub const INHERITED_WATCHLIST: [&str; 22] = [
    "XRT_COMPOSITOR_DUMP_DIR",
    "XRT_COMPOSITOR_DUMP_FRAMES",
    "XRT_COMPOSITOR_OUTPUT_GAMMA",
    "XRT_COMPOSITOR_OUTPUT_GAIN",
    "XRT_COMPOSITOR_TARGET_FORMAT",
    "XRT_COMPOSITOR_COMPUTE",
    "XRT_COMPOSITOR_PRESENT_MODE",
    "XRT_COMPOSITOR_FORCE_GPU_INDEX",
    "XRT_COMPOSITOR_PRINT_MODES",
    "XRT_COMPOSITOR_DEFAULT_FRAMERATE",
    "U_PACING_COMPOSITOR_LOG",
    "U_PACING_APP_LOG",
    "PIMAX_ROT_LEFT",
    "PIMAX_ROT_RIGHT",
    "PIMAX_MIRROR_LEFT",
    "PIMAX_MIRROR_RIGHT",
    "PIMAX_AXIS_MODE",
    "PIMAX_BIAS_MS",
    "PIMAX_NO_DEVICE",
    "PIMAX_LENS_CONF",
    "PIMAX_DEBUG_DIV",
    "PIMAX_PANEL_ROTATION",
];

/// The watch-listed variables that the caller has set and that `env` (the result of [`build_env`]) does not override.
pub fn inherited_overrides(
    env: &[(String, String)],
    base: &dyn Fn(&str) -> Option<String>,
) -> Vec<(String, String)> {
    INHERITED_WATCHLIST
        .iter()
        .filter(|k| !env.iter().any(|(n, _)| n == *k))
        .filter_map(|k| {
            base(k)
                .filter(|v| !v.is_empty())
                .map(|v| (k.to_string(), v))
        })
        .collect()
}

/// The launch option every Steam game needs for Monado: since pressure-vessel 0.20260925 (Steam Linux Runtime 3/4 of
/// 2026-09-28) the container imports the host's OpenXR runtime and shares `$XDG_RUNTIME_DIR/monado_comp_ipc` only
/// when it is set; without it the game's OpenXR loader (xrizer or wineopenxr) gets `XR_ERROR_RUNTIME_UNAVAILABLE`
/// (LESSONS row 27).
pub const STEAM_PV_IMPORT: &str = "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1";

/// The Steam launch options line, `runtime_json` given explicitly (works whatever `active_runtime.json` points at).
pub fn steam_options(runtime_json: &Path) -> String {
    format!("{STEAM_PV_IMPORT} XR_RUNTIME_JSON={} %command%", runtime_json.display())
}

/// The Steam launch options line while `~/.config/openxr/1/active_runtime.json` points at the build (see
/// [`super::runtime_link`]).
pub fn steam_options_linked() -> String {
    format!("{STEAM_PV_IMPORT} %command%")
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn opts() -> StartOptions {
        StartOptions {
            fps: Fps::Hz72,
            mode_override: None,
            dump_dir: None,
            dump_frames: None,
            monado_build: PathBuf::from("/home/u/src/monado/build"),
            service_bin: None,
            connector: "DP-1".into(),
            connector_auto: false,
            extra_env: vec![],
            session_dir: PathBuf::from(SESSION_DIR),
            ready_timeout: Duration::from_secs(30),
            force: false,
            dry_run: false,
            notify: true,
            wait: false,
            debug: false,
            output_gamma: None,
            output_gain: None,
            target_format: None,
            set_runtime: false,
        }
    }

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn lookup<'a>(env: &'a [(String, String)], k: &str) -> Option<&'a str> {
        env.iter().find(|(n, _)| n == k).map(|(_, v)| v.as_str())
    }

    #[test]
    fn fps_parses_and_maps_to_modes() {
        assert_eq!(Fps::parse("72").unwrap(), Fps::Hz72);
        assert_eq!(Fps::parse(" 90 ").unwrap(), Fps::Hz90);
        for bad in ["60", "", "90hz", "72.0"] {
            let e = Fps::parse(bad).unwrap_err();
            assert_eq!(e.code, 2, "{bad}");
        }
        assert_eq!((Fps::Hz72.hz(), Fps::Hz72.desired_mode()), (72, 2));
        assert_eq!((Fps::Hz90.hz(), Fps::Hz90.desired_mode()), (90, 3));
    }

    #[test]
    fn env_72_matches_the_shell_script() {
        let e = build_env(&opts(), &no_env);
        let want = [
            ("XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT", "1"),
            ("XRT_COMPOSITOR_WAYLAND_CONNECTOR", "DP-1"),
            ("XRT_COMPOSITOR_FORCE_NVIDIA", "0"),
            ("XRT_COMPOSITOR_DESIRED_MODE", "2"),
            ("PIMAX_FPS", "72"),
            ("XRT_COMPOSITOR_LOG", "info"),
            ("XRT_COMPOSITOR_SCALE_PERCENTAGE", "100"),
            ("PIMAX_LOG", "info"),
            (
                "XR_RUNTIME_JSON",
                "/home/u/src/monado/build/openxr_monado-dev.json",
            ),
            ("PIMAX_DEBUG_PTRACE", "1"),
            ("IPC_IGNORE_VERSION", "1"),
            ("XRT_NO_STDIN", "1"),
        ];
        let got: Vec<(&str, &str)> = e.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        assert_eq!(got, want);
    }

    #[test]
    fn env_90_changes_mode_and_rate_only() {
        let mut o = opts();
        o.fps = Fps::Hz90;
        let e = build_env(&o, &no_env);
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_DESIRED_MODE"), Some("3"));
        assert_eq!(lookup(&e, "PIMAX_FPS"), Some("90"));
        assert_eq!(e.len(), 12);
    }

    #[test]
    fn env_dump_debug_and_overrides() {
        let mut o = opts();
        o.dump_dir = Some(PathBuf::from("/tmp/d"));
        o.dump_frames = Some("100,300".into());
        o.debug = true;
        o.mode_override = Some(7);
        let e = build_env(&o, &no_env);
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_DUMP_DIR"), Some("/tmp/d"));
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_DUMP_FRAMES"), Some("100,300"));
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_LOG"), Some("debug"));
        assert_eq!(lookup(&e, "U_PACING_APP_LOG"), Some("debug"));
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_DESIRED_MODE"), Some("7"));
        // no frames list without --dump-frames, no dump variables without --dump
        o.dump_frames = None;
        assert!(lookup(&build_env(&o, &no_env), "XRT_COMPOSITOR_DUMP_FRAMES").is_none());
        o.dump_dir = None;
        assert!(lookup(&build_env(&o, &no_env), "XRT_COMPOSITOR_DUMP_DIR").is_none());
    }

    #[test]
    fn colour_experiment_options_become_compositor_variables() {
        let mut o = opts();
        assert!(lookup(&build_env(&o, &no_env), "XRT_COMPOSITOR_OUTPUT_GAMMA").is_none());
        o.output_gamma = Some(0.85);
        o.output_gain = Some(1.25);
        o.target_format = Some("B8G8R8A8_UNORM".into());
        let e = build_env(&o, &no_env);
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_OUTPUT_GAMMA"), Some("0.85"));
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_OUTPUT_GAIN"), Some("1.25"));
        assert_eq!(
            lookup(&e, "XRT_COMPOSITOR_TARGET_FORMAT"),
            Some("B8G8R8A8_UNORM")
        );
        assert_eq!(
            check_target_format("B8G8R8A8_UNORM").unwrap(),
            "B8G8R8A8_UNORM"
        );
        for bad in [
            "",
            "b8g8r8a8_unorm",
            "VK_FORMAT_B8G8R8A8_UNORM",
            "A2R10G10B10",
        ] {
            assert!(check_target_format(bad).is_err(), "{bad}");
        }
        assert_eq!(check_factor("--gamma", 0.9, 0.2, 5.0).unwrap(), 0.9);
        for bad in [0.0, 0.1, 6.0, f32::NAN, f32::INFINITY, -1.0] {
            assert!(check_factor("--gamma", bad, 0.2, 5.0).is_err(), "{bad}");
        }
    }

    #[test]
    fn a_quiet_pimax_log_or_a_silly_scale_cannot_break_the_start() {
        let base = |k: &str| match k {
            "PIMAX_LOG" => Some("warn".to_string()),
            "SCALE" => Some("2".to_string()), // 2 would mean "render at 2 %"
            _ => None,
        };
        let e = build_env(&opts(), &base);
        assert_eq!(
            lookup(&e, "PIMAX_LOG"),
            Some("info"),
            "warn would hide 'gyro bias done'"
        );
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_SCALE_PERCENTAGE"), Some("100"));
        for (level, keep) in [
            ("debug", true),
            ("DEBUG", true),
            ("trace", true),
            ("error", false),
            ("off", false),
        ] {
            let b = |k: &str| (k == "PIMAX_LOG").then(|| level.to_string());
            let e = build_env(&opts(), &b);
            let want = if keep { level } else { "info" };
            assert_eq!(lookup(&e, "PIMAX_LOG"), Some(want), "{level}");
        }
        let scale = |k: &str| (k == "SCALE").then(|| "400".to_string());
        assert_eq!(
            lookup(
                &build_env(&opts(), &scale),
                "XRT_COMPOSITOR_SCALE_PERCENTAGE"
            ),
            Some("400")
        );
        let scale = |k: &str| (k == "SCALE").then(|| "401".to_string());
        assert_eq!(
            lookup(
                &build_env(&opts(), &scale),
                "XRT_COMPOSITOR_SCALE_PERCENTAGE"
            ),
            Some("100")
        );
    }

    #[test]
    fn stale_experiment_variables_in_the_environment_are_listed() {
        let base = |k: &str| match k {
            "XRT_COMPOSITOR_OUTPUT_GAMMA" => Some("0.8".to_string()),
            "XRT_COMPOSITOR_DUMP_DIR" => Some("/tmp/old".to_string()),
            "PIMAX_ROT_LEFT" => Some(String::new()), // empty counts as unset
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        let mut o = opts();
        let env = build_env(&o, &base);
        let inh = inherited_overrides(&env, &base);
        assert_eq!(
            inh,
            vec![
                (
                    "XRT_COMPOSITOR_DUMP_DIR".to_string(),
                    "/tmp/old".to_string()
                ),
                ("XRT_COMPOSITOR_OUTPUT_GAMMA".to_string(), "0.8".to_string()),
            ]
        );
        // an explicit option overrides the inherited value and is therefore not listed as inherited
        o.output_gamma = Some(0.9);
        let env = build_env(&o, &base);
        let inh = inherited_overrides(&env, &base);
        assert_eq!(inh.len(), 1);
        assert_eq!(inh[0].0, "XRT_COMPOSITOR_DUMP_DIR");
        assert!(inherited_overrides(&env, &|_: &str| None).is_empty());
    }

    #[test]
    fn env_keeps_caller_log_levels_and_scale() {
        let base = |k: &str| match k {
            "XRT_COMPOSITOR_LOG" => Some("warn".to_string()),
            "PIMAX_LOG" => Some("debug".to_string()),
            "SCALE" => Some("80".to_string()),
            _ => None,
        };
        let e = build_env(&opts(), &base);
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_LOG"), Some("warn"));
        assert_eq!(lookup(&e, "PIMAX_LOG"), Some("debug"));
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_SCALE_PERCENTAGE"), Some("80"));
        // the explicit variable wins over the script's SCALE, empty values count as unset
        let base2 = |k: &str| match k {
            "XRT_COMPOSITOR_SCALE_PERCENTAGE" => Some("60".to_string()),
            "SCALE" => Some("80".to_string()),
            "PIMAX_LOG" => Some(String::new()),
            _ => None,
        };
        let e = build_env(&opts(), &base2);
        assert_eq!(lookup(&e, "XRT_COMPOSITOR_SCALE_PERCENTAGE"), Some("60"));
        assert_eq!(lookup(&e, "PIMAX_LOG"), Some("info"));
        // --debug beats the caller's log level
        let mut o = opts();
        o.debug = true;
        assert_eq!(
            lookup(&build_env(&o, &base), "XRT_COMPOSITOR_LOG"),
            Some("debug")
        );
    }

    #[test]
    fn paths_and_comm() {
        let o = opts();
        assert_eq!(
            o.service_path(),
            PathBuf::from("/home/u/src/monado/build/src/xrt/targets/service/monado-service")
        );
        assert_eq!(o.service_comm(), "monado-service");
        assert_eq!(
            o.log_path(),
            PathBuf::from("/tmp/omarchyvr-session/monado.log")
        );
        assert_eq!(
            o.pid_path(),
            PathBuf::from("/tmp/omarchyvr-session/monado.pid")
        );
        let mut o = opts();
        o.service_bin = Some(PathBuf::from("/x/a-very-long-fake-service-name"));
        assert_eq!(o.service_comm(), "a-very-long-fak"); // 15 characters, like the kernel
        assert_eq!(comm_of(Path::new("/")), "");
    }

    #[test]
    fn default_build_dir() {
        let home = |k: &str| (k == "HOME").then(|| "/home/zed".to_string());
        assert_eq!(
            default_monado_build(&home),
            PathBuf::from("/home/zed/src/monado/build")
        );
        let over = |k: &str| match k {
            "PIMAX_MONADO_BUILD" => Some("/b".to_string()),
            _ => None,
        };
        assert_eq!(default_monado_build(&over), PathBuf::from("/b"));
    }

    #[test]
    fn frame_list_validation() {
        assert_eq!(check_frame_list("100, 300,900").unwrap(), "100,300,900");
        assert_eq!(check_frame_list("7").unwrap(), "7");
        for bad in ["", "a,b", "1,,2", "-1", "1.5"] {
            assert!(check_frame_list(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn ipd_env_file_reaches_monado_unless_the_caller_sets_it() {
        assert_eq!(ipd_from_env_file("# x\nOXR_DEBUG_IPD_MM=61.5\n").as_deref(), Some("61.5"));
        assert_eq!(ipd_from_env_file("OXR_DEBUG_IPD_MM=\"63\""), Some("63".into()));
        assert_eq!(ipd_from_env_file("OXR_DEBUG_IPD_MM=640"), None, "mm*10 typo");
        assert_eq!(ipd_from_env_file("OXR_DEBUG_IPD_MM=abc"), None);
        assert_eq!(ipd_from_env_file(""), None);
        let d = crate::vr::test_dir("ipd-env");
        std::fs::create_dir_all(d.join(".config/omarchyvr")).unwrap();
        std::fs::write(d.join(IPD_ENV), "OXR_DEBUG_IPD_MM=61.5\n").unwrap();
        let home = d.to_string_lossy().into_owned();
        let env = build_env(&opts(), &|k| (k == "HOME").then(|| home.clone()));
        assert_eq!(lookup(&env, "OXR_DEBUG_IPD_MM"), Some("61.5"));
        let env = build_env(&opts(), &|k| match k {
            "HOME" => Some(home.clone()),
            "OXR_DEBUG_IPD_MM" => Some("64".into()),
            _ => None,
        });
        assert_eq!(lookup(&env, "OXR_DEBUG_IPD_MM"), None, "inherited from the caller instead");
        assert_eq!(lookup(&build_env(&opts(), &no_env), "OXR_DEBUG_IPD_MM"), None);
    }

    #[test]
    fn auto_toml_game_runtime() {
        assert_eq!(game_runtime_from_toml("mode = \"desk\"\ngame_runtime = \"steamvr\" # x\n").as_deref(), Some("steamvr"));
        assert_eq!(game_runtime_from_toml("# game_runtime = steamvr\n"), None);
        let d = crate::vr::test_dir("auto-toml");
        std::fs::write(d.join("auto.toml"), "game_runtime = monado\n").unwrap();
        let p = d.join("auto.toml").to_string_lossy().into_owned();
        assert_eq!(auto_game_runtime(&|k| (k == "PIMAX_AUTO_CONFIG").then(|| p.clone())).as_deref(), Some("monado"));
        assert_eq!(auto_game_runtime(&|_| None), None);
    }

    #[test]
    fn steam_line_is_exactly_what_the_script_prints() {
        let l = steam_options(Path::new("/home/u/src/monado/build/openxr_monado-dev.json"));
        assert_eq!(
            l,
            "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 XR_RUNTIME_JSON=/home/u/src/monado/build/openxr_monado-dev.json %command%"
        );
        assert_eq!(steam_options_linked(), "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%");
    }

    #[test]
    fn tools_next_to_a_packaged_launcher() {
        let h = Path::new("/home/u");
        assert_eq!(tools_dir_for(Some(Path::new("/usr/bin/omarchyvr")), h), PathBuf::from("/usr/bin"));
        assert_eq!(tools_dir_for(Some(Path::new("/home/u/.local/bin/omarchyvr")), h), PathBuf::from("/home/u/.local/bin"), "an installed launcher never uses the dev tree");
        assert_eq!(tools_dir_for(Some(Path::new("/home/u/pimax-re/rust/target/release/omarchyvr")), h), PathBuf::from("/home/u/pimax-re/rust/target/release"));
        assert_eq!(tools_dir_for(None, h), PathBuf::from("/home/u/pimax-re/rust/target/release"));
    }

    #[test]
    fn comp_budget_from_the_saved_setting() {
        let home = crate::vr::test_dir("budget-env").to_string_lossy().into_owned();
        let h = home.clone();
        let base = move |k: &str| (k == "HOME").then(|| h.clone());
        assert_eq!(lookup(&build_env(&opts(), &base), "U_PACING_COMP_MIN_TIME_MS"), None); // auto
        let s = super::super::gamemode::Settings { comp_budget: "8".into(), ..Default::default() };
        super::super::gamemode::save(Path::new(&home), &s).unwrap();
        assert_eq!(lookup(&build_env(&opts(), &base), "U_PACING_COMP_MIN_TIME_MS"), Some("8.0"));
        let h2 = home.clone();
        let own = move |k: &str| match k {
            "HOME" => Some(h2.clone()),
            "U_PACING_COMP_MIN_TIME_MS" => Some("5".to_string()),
            _ => None,
        };
        assert_eq!(lookup(&build_env(&opts(), &own), "U_PACING_COMP_MIN_TIME_MS"), None); // inherited, not duplicated
    }
}
