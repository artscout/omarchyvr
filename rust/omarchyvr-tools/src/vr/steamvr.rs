//! "Games run with": Monado (monado-service + xrizer) or SteamVR (SteamVR's vrserver/vrcompositor with Monado's
//! SteamVR driver `driver_monado.so`, which carries the Pimax driver of the same Monado build). docs/STEAMVR.md.
//!
//! * The choice is saved in `~/.config/omarchyvr/games.toml` (`runtime = "monado"|"steamvr"`), shared by the app and the
//!   CLI. Without it, auto mode's `game_runtime` (auto.toml) counts, else [`DEFAULT`].
//! * `omarchyvr vr games monado|steamvr|status` saves the choice and hands `openvrpaths.vrpath` to that side: Monado =
//!   xrizer only, read-only (0444), no external driver; SteamVR = SteamVR first, writable (0644, SteamVR 2.17 exits
//!   on a read-only file), our driver in `external_drivers`. The OpenXR runtime (`active_runtime.json`) follows:
//!   SteamVR's `steamxr_linux64.json` in SteamVR mode (no monado-service runs, native OpenXR games would find no
//!   runtime), the Monado build's `openxr_monado-dev.json` in Monado mode ([`super::runtime_link`]).
//! * `omarchyvr vr start` with SteamVR chosen: refuses while a monado-service runs (both would open the headset), writes
//!   the same Pimax environment monado-service would get to `~/.config/omarchyvr/steamvr-driver.env` (the driver reads it
//!   in vrserver, marker `PIMAX_STEAMVR_ENV`), starts the camera broker for 6DoF, starts SteamVR ([`launch_spec`]) and
//!   records vrserver's pid in `<session dir>/steamvr.pid`. `omarchyvr vr stop` stops SteamVR by pid (vrmonitor first).
use super::config::StartOptions;
use super::desk::{ProcSpec, Spawner};
use super::session::Deps;
use super::sixdof::{self, SixDof};
use crate::CliError;
use omarchyvr_components::components::proc::Signal;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// What games run with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Games {
    /// monado-service + xrizer (today's path).
    Monado,
    /// SteamVR with Monado's driver.
    SteamVr,
}

/// The default until the user tested SteamVR in the headset (docs/STEAMVR.md). Flip to `Games::SteamVr` then.
pub const DEFAULT: Games = Games::Monado;

/// SteamVR's Steam app id.
pub const STEAMVR_APPID: &str = "250820";
/// Pid file stem in the session dir.
pub const PID_FILE: &str = "steamvr.pid";
/// Marker the driver logs when it read the environment file (also in driver_monado.so).
pub const DRIVER_MARKER: &str = "PIMAX_STEAMVR_ENV";

impl Games {
    /// `monado` / `steamvr`.
    pub fn name(self) -> &'static str {
        match self {
            Games::Monado => "monado",
            Games::SteamVr => "steamvr",
        }
    }

    /// From a word (`xrizer` counts as Monado).
    pub fn parse(s: &str) -> Option<Games> {
        match s.trim().trim_matches('"').to_ascii_lowercase().as_str() {
            "monado" | "xrizer" => Some(Games::Monado),
            "steamvr" => Some(Games::SteamVr),
            _ => None,
        }
    }
}

/// `~/.config/omarchyvr/games.toml`.
pub fn settings_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/games.toml")
}

/// `~/.config/omarchyvr/steamvr-driver.env`.
pub fn driver_env_path(home: &Path) -> PathBuf {
    home.join(".config/omarchyvr/steamvr-driver.env")
}

/// The driver directory of a Monado build (`steamvr-monado`, with `driver.vrdrivermanifest`).
pub fn driver_dir(build: &Path) -> PathBuf {
    build.join("steamvr-monado")
}

/// True when the build has the SteamVR driver.
pub fn has_driver(build: &Path) -> bool {
    let d = driver_dir(build);
    d.join("driver.vrdrivermanifest").exists() && d.join("bin/linux64/driver_monado.so").exists()
}

/// `runtime = "..."` of a games.toml text.
pub fn parse_settings(text: &str) -> Option<Games> {
    text.lines()
        .rev()
        .map(|l| l.split('#').next().unwrap_or("").trim())
        .filter_map(|l| l.split_once('='))
        .filter(|(k, _)| k.trim() == "runtime")
        .find_map(|(_, v)| Games::parse(v))
}

/// The saved choice (games.toml), else auto mode's `game_runtime`, else [`DEFAULT`].
pub fn games_runtime(env: &dyn Fn(&str) -> Option<String>) -> Games {
    let home = env("HOME").map(PathBuf::from);
    home.and_then(|h| std::fs::read_to_string(settings_path(&h)).ok())
        .and_then(|t| parse_settings(&t))
        .or_else(|| super::config::auto_game_runtime(env).and_then(|r| Games::parse(&r)))
        .unwrap_or(DEFAULT)
}

/// Save the choice.
pub fn save(home: &Path, g: Games) -> io::Result<()> {
    let p = settings_path(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    std::fs::write(p, format!("# What games run with (Pimax app: Games run with). monado | steamvr\nruntime = \"{}\"\n", g.name()))
}

/// The driver environment file: the variables monado-service would get, minus the ones of its own compositor and IPC.
pub fn driver_env_text(env: &[(String, String)]) -> String {
    let mut s = String::from("# Written by `omarchyvr vr start` (Games run with: SteamVR); read by driver_monado.so in vrserver.\n");
    for (k, v) in env {
        if (k.starts_with("XRT_COMPOSITOR_") && k != "XRT_COMPOSITOR_SCALE_PERCENTAGE") || matches!(k.as_str(), "XR_RUNTIME_JSON" | "IPC_IGNORE_VERSION" | "XRT_NO_STDIN" | "PIMAX_DEBUG_PTRACE") || k.starts_with("U_PACING_") {
            continue;
        }
        if v.contains('\n') {
            continue;
        }
        s.push_str(&format!("{k}={v}\n"));
    }
    s
}

/// Switch: save the choice and hand openvrpaths.vrpath to that side. Returns plain-words lines.
pub fn switch(home: &Path, g: Games, build: &Path, xrizer: &Path) -> Result<Vec<String>, String> {
    let mut out = Vec::new();
    match g {
        Games::SteamVr => {
            let sv = super::openvr_link::steamvr_dir(home);
            let drv = has_driver(build).then(|| driver_dir(build));
            let line = super::openvr_link::ensure_steamvr(home, &sv, drv.as_deref()).map_err(|e| format!("Could not switch games to SteamVR: {e}"))?;
            out.push(line);
            match super::runtime_link::ensure_steamvr(home) {
                Ok(Some(l)) => out.push(format!("OpenXR runtime set: {l}")),
                Ok(None) => {}
                Err(e) => out.push(format!("WARNING: could not point the OpenXR runtime at SteamVR ({e})")),
            }
            if drv.is_none() {
                out.push(format!("WARNING: {} has no SteamVR driver (steamvr-monado): SteamVR will not find the headset. Install the Monado build with the driver.", build.display()));
            }
        }
        Games::Monado => {
            if !super::openvr_link::is_runtime(xrizer) {
                return Err(format!("Could not switch games to Monado: xrizer is not built ({} has no bin/linux64/vrclient.so)", xrizer.display()));
            }
            match super::openvr_link::ensure_xrizer(home, xrizer) {
                Ok(Some(l)) => out.push(l),
                Ok(None) => out.push("openvrpaths.vrpath already lists only xrizer (read-only)".into()),
                Err(e) => return Err(format!("Could not switch games to Monado: {e}")),
            }
            match super::runtime_link::ensure(home, &build.join("openxr_monado-dev.json")) {
                Ok(Some(l)) => out.push(format!("OpenXR runtime set: {l}")),
                Ok(None) => {}
                Err(e) => out.push(format!("WARNING: could not point the OpenXR runtime back at Monado ({e})")),
            }
        }
    }
    save(home, g).map_err(|e| format!("Could not save the choice in {}: {e}", settings_path(home).display()))?;
    out.push(format!("Games run with: {}", if g == Games::SteamVr { "SteamVR" } else { "Monado" }));
    Ok(out)
}

/// The vrserver pid of the SteamVR this tool started, if it still runs.
pub fn running_pid(session_dir: &Path, d: &Deps) -> Option<u32> {
    let pid = super::session::read_pid(&session_dir.join(PID_FILE))?;
    (d.procs.is_alive(pid) && d.procs.comm(pid).as_deref() == Some("vrserver")).then_some(pid)
}

/// How SteamVR is started. Default (`direct`): SteamVR's own `bin/vrstartup.sh` with `VALVE_SKIP_RUNTIME_SAFETY=1`,
/// so vrserver runs on the host and not in Steam's "sniper" container: driver_monado.so of the one Monado build needs
/// host libraries (OpenCV, GStreamer, hidapi, Basalt) the container does not have. The Pimax settings also go into
/// its environment. `PIMAX_STEAMVR_LAUNCH=steam`: through Steam (`steam steam://rungameid/250820`), in the container.
pub fn launch_spec(env: &dyn Fn(&str) -> Option<String>, home: &Path, session_dir: &Path, pimax_env: &[(String, String)], build: &Path) -> ProcSpec {
    if env("PIMAX_STEAMVR_LAUNCH").as_deref() == Some("steam") {
        let steam = env("PIMAX_STEAM_BIN").filter(|v| !v.is_empty()).unwrap_or_else(|| "/usr/bin/steam".into());
        return ProcSpec {
            name: "steamvr",
            program: PathBuf::from(steam),
            args: vec![format!("steam://rungameid/{STEAMVR_APPID}")],
            env: Vec::new(),
            log: session_dir.join("steamvr-launch.log"),
        };
    }
    // QT_QPA_PLATFORM: Omarchy sets "wayland;xcb"; SteamVR's own Qt has only xcb, so vrmonitor aborted at start
    // ("could not find or load the Qt platform plugin") and took the session down with it (2026-10-15 test)
    let mut e: Vec<(String, String)> = vec![("VALVE_SKIP_RUNTIME_SAFETY".into(), "1".into()), ("QT_QPA_PLATFORM".into(), "xcb".into())];
    // vrserver sets RLIMIT_RTTIME to 0 and, on the host, the compositor is its child: with realtime threads (the
    // launcher's cap_sys_nice) the kernel SIGKILLs it right after "Startup Complete". The preload from the same
    // Monado build keeps the limit (2026-10-15 test: rttime 0 -> killed in 1 s, unlimited -> runs).
    let pre = rttime_preload(build);
    if pre.exists() {
        e.push(("LD_PRELOAD".into(), pre.display().to_string()));
    }
    e.extend(driver_env(pimax_env));
    // our shims for `xdg-open` and `steam` go first in PATH: whatever SteamVR opens (steam:// links when the user
    // picks a game in the dashboard, web pages) starts without SteamVR's variables ([`write_shims`], incident
    // 2026-10-16: Steam started by vrserver's `xdg-open steam://launch/...` inherited SteamAppId=250820 and froze)
    let path = env("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    e.push(("PATH".into(), format!("{}:{path}", shim_dir(session_dir).display())));
    ProcSpec {
        name: "steamvr",
        program: super::openvr_link::steamvr_dir(home).join("bin/vrstartup.sh"),
        args: Vec::new(),
        env: e,
        log: session_dir.join("steamvr-launch.log"),
    }
}

/// `~/.local/share/Steam/config/steamvr.vrsettings`.
pub fn vrsettings_path(home: &Path) -> PathBuf {
    home.join(".local/share/Steam/config/steamvr.vrsettings")
}

/// SteamVR picks the display mode by `steamvr.preferredRefreshRate` (default 90). The Pimax driver wakes the panel
/// at PIMAX_FPS (90 by default) and the firmware lights it only at that rate, so the setting must match. Only while
/// SteamVR is stopped (it rewrites the file). Returns a line when it changed something.
pub fn set_refresh_rate(home: &Path, hz: u32) -> io::Result<Option<String>> {
    let changed = set_steamvr_keys(home, &[("preferredRefreshRate", serde_json::json!(hz))])?;
    Ok(changed.then(|| format!("SteamVR refresh rate set to {hz} Hz (the rate the panel is woken at)")))
}

/// SteamVR's own resolution: Auto multiplies the driver's recommended size again (up to 150 % of the pixels on a fast
/// GPU: 4700x4348 per eye on an RTX 4090 at the Quality preset, 2026-10-15), so the app's Render quality preset
/// (XRT_COMPOSITOR_SCALE_PERCENTAGE, which the driver applies) would not be what games get. Manual 100 % makes the
/// preset the one setting. Only while SteamVR is stopped. Returns a line when it changed something.
pub fn set_render_scale(home: &Path) -> io::Result<Option<String>> {
    let changed = set_steamvr_keys(
        home,
        &[("supersampleManualOverride", serde_json::json!(true)), ("supersampleScale", serde_json::json!(1.0))],
    )?;
    Ok(changed.then(|| "SteamVR resolution set to 100 % (the Pimax app's Render quality decides it)".to_string()))
}

/// Brightness on the SteamVR path: vrcompositor (Direct mode) makes the final image and multiplies it by
/// `hmdDisplayColorGainR/G/B` (default 1.0); the app's image level maps to it. Only while SteamVR is stopped.
pub fn set_color_gain(home: &Path, image: &str) -> io::Result<Option<String>> {
    let g = super::display::steamvr_gain(image);
    let keys = ["hmdDisplayColorGainR", "hmdDisplayColorGainG", "hmdDisplayColorGainB"].map(|k| (k, serde_json::json!(g)));
    let changed = set_steamvr_keys(home, &keys)?;
    Ok(changed.then(|| format!("SteamVR colour gain set to {g} (Brightness: {image})")))
}

/// Set keys of the `steamvr` section of steamvr.vrsettings, keeping everything else; false when all already matched.
fn set_steamvr_keys(home: &Path, keys: &[(&str, serde_json::Value)]) -> io::Result<bool> {
    let p = vrsettings_path(home);
    let text = std::fs::read_to_string(&p).ok();
    let mut v: serde_json::Value = text.as_deref().and_then(|t| serde_json::from_str(t).ok()).filter(|v: &serde_json::Value| v.is_object()).unwrap_or_else(|| serde_json::json!({}));
    let sv = v.as_object_mut().unwrap().entry("steamvr").or_insert_with(|| serde_json::json!({}));
    if !sv.is_object() {
        *sv = serde_json::json!({});
    }
    let o = sv.as_object_mut().unwrap();
    let same = |a: Option<&serde_json::Value>, b: &serde_json::Value| match (a.and_then(|x| x.as_f64()), b.as_f64()) {
        (Some(x), Some(y)) => x == y,
        _ => a == Some(b),
    };
    if keys.iter().all(|(k, val)| same(o.get(*k), val)) {
        return Ok(false);
    }
    for (k, val) in keys {
        o.insert((*k).into(), val.clone());
    }
    std::fs::create_dir_all(p.parent().unwrap())?;
    let tmp = p.with_file_name(format!(".steamvr.vrsettings.pimax-{}", std::process::id()));
    std::fs::write(&tmp, format!("{}\n", serde_json::to_string_pretty(&v).unwrap()))?;
    std::fs::rename(&tmp, &p)?;
    Ok(true)
}

/// PIMAX_FPS of the session environment (90 when missing).
pub fn session_hz(env: &[(String, String)]) -> u32 {
    env.iter().find(|(k, _)| k == "PIMAX_FPS").and_then(|(_, v)| v.parse().ok()).unwrap_or(90)
}

/// `<build>/steamvr-monado/bin/linux64/pimax_rttime_preload.so` (Monado, marker PIMAX_RTTIME_PRELOAD).
pub fn rttime_preload(build: &Path) -> PathBuf {
    driver_dir(build).join("bin/linux64/pimax_rttime_preload.so")
}

/// The variables of [`driver_env_text`], as pairs.
pub fn driver_env(env: &[(String, String)]) -> Vec<(String, String)> {
    driver_env_text(env)
        .lines()
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| l.split_once('=').map(|(k, v)| (k.to_string(), v.to_string())))
        .collect()
}

/// `omarchyvr vr start` with SteamVR chosen. `env` is the Pimax environment monado-service would get.
pub fn start(
    o: &StartOptions,
    env: &[(String, String)],
    six: Option<&SixDof>,
    ctrl_track: Option<&Path>,
    d: &Deps,
    sp: &dyn Spawner,
) -> Result<(), CliError> {
    let home = PathBuf::from((d.env)("HOME").unwrap_or_default());
    let say = |m: &str| vout!("omarchyvr vr: {m}");
    if let Some(p) = running_pid(&o.session_dir, d) {
        say(&format!("SteamVR already runs (vrserver pid {p})"));
        return Ok(());
    }
    let monado = d.procs.find_by_comm(&o.service_comm());
    if !monado.is_empty() {
        return Err(CliError::new(format!(
            "Monado is running (pid {}): stop it first. With 'Games run with: SteamVR', SteamVR drives the headset itself and both cannot open it.",
            monado.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ")
        )));
    }
    let foreign = d.procs.find_by_comm("vrserver");
    if !foreign.is_empty() {
        return Err(CliError::new(format!(
            "SteamVR is already running (vrserver pid {}), started outside the Pimax app: close SteamVR first, then press Play again so it loads the Pimax driver.",
            foreign[0]
        )));
    }
    if !o.dry_run && has_driver(&o.monado_build) && !rttime_preload(&o.monado_build).exists() {
        verr!("omarchyvr vr: WARNING: {} is missing: SteamVR's compositor will be killed right after it starts (install a Monado build that has it)", rttime_preload(&o.monado_build).display());
    }
    if !has_driver(&o.monado_build) {
        return Err(CliError::new(format!(
            "this Monado build ({}) has no SteamVR driver (steamvr-monado): install the build that has it, or choose 'Games run with: Monado'",
            o.monado_build.display()
        )));
    }
    let text = driver_env_text(env);
    if o.dry_run {
        say(&format!("would write {}:\n{}", driver_env_path(&home).display(), text.trim_end()));
        say(&format!("would register {} and hand openvrpaths.vrpath to SteamVR (writable)", driver_dir(&o.monado_build).display()));
        if let Some(s) = six {
            say(&format!("would start the camera broker ({} broker)", s.cam_bin.display()));
        }
        let l = launch_spec(d.env, &home, &o.session_dir, env, &o.monado_build);
        let st = steam_launch_spec(&o.session_dir, &scrub_vars(env), d.env);
        say(&format!("would start Steam first unless it runs: {} {}", st.program.display(), st.args.join(" ")));
        say(&format!("would write the xdg-open/steam shims to {}", shim_dir(&o.session_dir).display()));
        say(&format!("would start SteamVR: {} {}", l.program.display(), l.args.join(" ")));
        return Ok(());
    }
    super::session::ensure_private_dir(&o.session_dir)?;
    // Steam first, started the normal way: SteamVR must never start Steam itself (it would hand Steam its own
    // variables; incident 2026-10-16). A Steam that already carries them is refused in plain words.
    let scrub = scrub_vars(env);
    if let Err(e) = write_shims(&o.session_dir, &scrub, d.env) {
        verr!("omarchyvr vr: WARNING: could not write the xdg-open/steam shims ({e}): links SteamVR opens get SteamVR's variables");
    }
    ensure_steam(o, &home, &scrub, d.env, d, sp)?;
    let ep = driver_env_path(&home);
    std::fs::create_dir_all(ep.parent().unwrap()).and_then(|()| std::fs::write(&ep, &text))
        .map_err(|e| CliError::new(format!("cannot write {}: {e}", ep.display())))?;
    say(&format!("Pimax settings for the SteamVR driver: {}", ep.display()));
    let line = super::openvr_link::ensure_steamvr(&home, &super::openvr_link::steamvr_dir(&home), Some(&driver_dir(&o.monado_build)))
        .map_err(|e| CliError::new(format!("Could not hand the OpenVR runtime to SteamVR: {e}")))?;
    say(&line);
    if o.set_runtime {
        match super::runtime_link::ensure_steamvr(&home) {
            Ok(Some(l)) => say(&format!("OpenXR runtime set: {l}")),
            Ok(None) => {}
            Err(e) => verr!("omarchyvr vr: WARNING: could not point the OpenXR runtime at SteamVR ({e}): native OpenXR games find no runtime"),
        }
    }
    match set_refresh_rate(&home, session_hz(env)) {
        Ok(Some(l)) => say(&l),
        Ok(None) => {}
        Err(e) => verr!("omarchyvr vr: WARNING: could not set SteamVR's refresh rate ({e}): at another rate the panel stays dark"),
    }
    match set_render_scale(&home) {
        Ok(Some(l)) => say(&l),
        Ok(None) => {}
        Err(e) => verr!("omarchyvr vr: WARNING: could not set SteamVR's resolution to 100 % ({e}): SteamVR may render above the Render quality preset"),
    }
    // the room setup for driver_monado's universe (play area, walls; docs/ROOM-SETUP.md), while SteamVR is stopped
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|t| t.as_secs() as i64).unwrap_or(0);
    if let Some(l) = super::room::write_chaperone(&home, d.env, now) {
        say(&l);
    }
    match set_color_gain(&home, &super::display::load(&home).image) {
        Ok(Some(l)) => say(&l),
        Ok(None) => {}
        Err(e) => verr!("omarchyvr vr: WARNING: could not set SteamVR's colour gain ({e}): the Brightness choice is not applied"),
    }
    if let Some(s) = six {
        sixdof::check(o, s, d.env)?;
        sixdof::start_broker(o, s, d, sp, Duration::from_secs(10))?;
    }
    let spec = launch_spec(d.env, &home, &o.session_dir, env, &o.monado_build);
    if let Err(e) = sp.spawn(&spec) {
        if let Some(s) = six {
            sixdof::stop_broker(&o.session_dir, &s.cam_bin, d.procs, d.clock);
        }
        return Err(CliError::new(format!("Could not start SteamVR through Steam ({}): {e}", spec.program.display())));
    }
    say(&format!("starting SteamVR ({})...", spec.program.display()));
    let t0 = d.clock.now();
    let wait = o.ready_timeout.max(Duration::from_secs(90));
    loop {
        if let Some(&pid) = d.procs.find_by_comm("vrserver").first() {
            let _ = std::fs::write(o.session_dir.join(PID_FILE), format!("{pid}\n"));
            say(&format!("SteamVR is up (vrserver pid {pid}); put the headset on"));
            if let (Some(s), Some(b)) = (six, ctrl_track) {
                sixdof::vr_start_ctrltrack(b, s, o, d, sp);
            }
            d.notifier.notify(false, "Pimax VR", "SteamVR started with the Pimax driver");
            return Ok(());
        }
        if (d.stop_requested)() || d.clock.now() - t0 > wait {
            break;
        }
        d.clock.sleep(Duration::from_millis(500));
    }
    if let Some(s) = six {
        sixdof::stop_broker(&o.session_dir, &s.cam_bin, d.procs, d.clock);
    }
    clean_session_files(&o.session_dir);
    let msg = format!("SteamVR did not start within {} s (is Steam logged in? see ~/.local/share/Steam/logs/vrserver.txt)", wait.as_secs());
    d.notifier.notify(true, "Pimax VR: SteamVR not started", &msg);
    Err(CliError::new(msg))
}

// --- hermetic launch: SteamVR's variables stay in SteamVR (incident 2026-10-16, docs/STEAMVR.md) ---

/// Variables only SteamVR's own processes may have: ours (set by [`launch_spec`]) and the ones SteamVR's scripts
/// export (`vrenv.sh`, `vrstartup.sh`; `SteamAppId`/`SteamGameId` = 250820 make a Steam client that inherits them
/// count itself as SteamVR: "CClientGame::Init: passed own procID as game procID", SteamVR is then never restarted).
pub const STEAMVR_ONLY: &[&str] = &[
    "LD_PRELOAD",
    "VALVE_SKIP_RUNTIME_SAFETY",
    "QT_QPA_PLATFORM",
    "QT_PLUGIN_PATH",
    "LD_LIBRARY_PATH",
    "PATH",
    "SteamAppId",
    "SteamGameId",
    "SteamOverlayGameId",
    "STEAMVR_VRENV",
    "STEAMVR_TOOLSDIR",
    "VRCOMPOSITOR_LD_LIBRARY_PATH",
    "_STEAMVR_LOGGING_READY",
    "SRT_LAUNCHER_SERVICE_ALONGSIDE_STEAM",
    "STEAM_RUNTIME",
];
/// Of [`STEAMVR_ONLY`], these get the user's own value back (when the user has one) instead of being removed.
pub const RESTORED: &[&str] = &["LD_PRELOAD", "QT_QPA_PLATFORM", "QT_PLUGIN_PATH", "LD_LIBRARY_PATH", "PATH"];
/// Never removed: Steam must keep it (it comes from our steam.desktop / environment.d, steam_setup.rs).
pub const KEEP_ON_STEAM: &[&str] = &[super::steam_setup::VAR];
/// Steam's pid at Play (the clean Steam that SteamVR talks to), in the session dir.
pub const STEAM_PID_FILE: &str = "steam-client.pid";
/// Seconds Play waits for Steam to come up before it starts SteamVR.
pub const STEAM_WAIT: Duration = Duration::from_secs(60);

/// What SteamVR's PATH gets shims for: everything that can open a `steam://` link or start Steam (incident 2026-10-16).
pub const SHIMMED: [&str; 4] = ["xdg-open", "steam", "gio", "gtk-launch"];

/// `<session dir>/steamvr-shim`: `xdg-open` and `steam` for SteamVR's PATH.
pub fn shim_dir(session_dir: &Path) -> PathBuf {
    session_dir.join("steamvr-shim")
}

/// Every variable that must not leave SteamVR: [`STEAMVR_ONLY`] plus the Pimax ones `launch_spec` adds.
pub fn scrub_vars(pimax_env: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<String> = STEAMVR_ONLY.iter().map(|s| s.to_string()).collect();
    v.extend(driver_env(pimax_env).into_iter().map(|(k, _)| k));
    v.retain(|k| !KEEP_ON_STEAM.contains(&k.as_str()));
    v.sort();
    v.dedup();
    v
}

fn sh_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// `env` arguments that turn SteamVR's environment back into the user's: `-u` for each variable (all first: GNU env
/// takes no option after the first NAME=VALUE), then the user's own value of the [`RESTORED`] ones.
pub fn clean_env_args(scrub: &[String], user: &dyn Fn(&str) -> Option<String>) -> Vec<String> {
    let mut unset = Vec::new();
    let mut set = Vec::new();
    for k in scrub {
        match user(k).filter(|_| RESTORED.contains(&k.as_str())) {
            Some(v) => set.push(format!("{k}={v}")),
            None => {
                unset.push("-u".to_string());
                unset.push(k.clone());
            }
        }
    }
    unset.extend(set);
    unset
}

/// A shim script: `exec env <clean_env_args> <real> "$@"`.
pub fn shim_text(real: &Path, args: &[String]) -> String {
    let a: Vec<String> = args.iter().map(|x| if x.starts_with('-') { x.clone() } else { sh_quote(x) }).collect();
    format!(
        "#!/bin/sh\n# Written by `omarchyvr vr start` (Games run with: SteamVR). SteamVR runs with our variables; what it opens\n# (steam:// links, web pages) gets the user's own environment back (docs/STEAMVR.md, incident 2026-10-16).\nexec /usr/bin/env {} {} \"$@\"\n",
        a.join(" "),
        sh_quote(&real.display().to_string())
    )
}

/// `name` in `path` (a PATH value), skipping `skip`; `/usr/bin/<name>` when not found.
pub fn which_in(path: &str, name: &str, skip: &Path) -> PathBuf {
    path.split(':')
        .filter(|d| !d.is_empty() && Path::new(d) != skip)
        .map(|d| Path::new(d).join(name))
        .find(|p| p.is_file())
        .unwrap_or_else(|| Path::new("/usr/bin").join(name))
}

/// Write the `xdg-open` and `steam` shims (0755) into [`shim_dir`].
pub fn write_shims(session_dir: &Path, scrub: &[String], user: &dyn Fn(&str) -> Option<String>) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let dir = shim_dir(session_dir);
    std::fs::create_dir_all(&dir)?;
    let path = user("PATH").unwrap_or_else(|| "/usr/local/bin:/usr/bin:/bin".into());
    let args = clean_env_args(scrub, user);
    for name in SHIMMED {
        let f = dir.join(name);
        std::fs::write(&f, shim_text(&which_in(&path, name, &dir), &args))?;
        std::fs::set_permissions(&f, std::fs::Permissions::from_mode(0o755))?;
    }
    Ok(())
}

/// How Play starts Steam when it is not running: its desktop entry (`gtk-launch steam`, our steam.desktop with the
/// pressure-vessel variable), through `env` without SteamVR's or our variables.
pub fn steam_launch_spec(session_dir: &Path, scrub: &[String], user: &dyn Fn(&str) -> Option<String>) -> ProcSpec {
    let mut args = clean_env_args(scrub, user);
    args.extend(["gtk-launch".to_string(), "steam".to_string()]);
    ProcSpec { name: "steam", program: PathBuf::from("/usr/bin/env"), args, env: Vec::new(), log: session_dir.join("steam-launch.log") }
}

/// The running Steam client: `~/.steam/steam.pid` when that process is `steam`, else the first `steam` process.
pub fn steam_pid(home: &Path, procs: &dyn super::host::Procs) -> Option<u32> {
    super::session::read_pid(&home.join(".steam/steam.pid"))
        .filter(|p| procs.is_alive(*p) && procs.comm(*p).as_deref() == Some("steam"))
        .or_else(|| procs.find_by_comm("steam").first().copied())
}

/// Variables in a Steam client's environment that only a Steam started from inside SteamVR has.
pub fn leaked_vars(env: &[(String, String)]) -> Vec<String> {
    let mut v: Vec<String> = env
        .iter()
        .filter(|(k, val)| {
            k.starts_with("PIMAX_")
                || matches!(k.as_str(), "VALVE_SKIP_RUNTIME_SAFETY" | "STEAMVR_VRENV" | "STEAMVR_TOOLSDIR" | "VRCOMPOSITOR_LD_LIBRARY_PATH")
                || (matches!(k.as_str(), "SteamAppId" | "SteamGameId") && val == STEAMVR_APPID)
                || (k == "LD_PRELOAD" && val.contains("pimax_rttime_preload"))
        })
        .map(|(k, _)| k.clone())
        .collect();
    v.sort();
    v
}

/// Plain words for a Steam that carries SteamVR's variables.
pub fn leaked_message(pid: u32, vars: &[String]) -> String {
    format!(
        "Steam (pid {pid}) was started from inside SteamVR and still carries SteamVR's settings ({}), so games can freeze. Quit Steam (Steam menu > Exit), then press Play again: Play starts Steam the normal way first.",
        vars.join(", ")
    )
}

/// Seconds the restart waits for a tainted Steam to exit after `steam -shutdown` (Steam saves cloud data first).
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(90);

/// `steam -shutdown` through `env` without SteamVR's or our variables (the clean environment). The real `steam`
/// script finds the running client through `~/.steam/steam.pipe`.
pub fn steam_shutdown_spec(session_dir: &Path, scrub: &[String], user: &dyn Fn(&str) -> Option<String>) -> ProcSpec {
    let mut args = clean_env_args(scrub, user);
    args.extend(["/usr/bin/steam".to_string(), "-shutdown".to_string()]);
    ProcSpec { name: "steam-shutdown", program: PathBuf::from("/usr/bin/env"), args, env: Vec::new(), log: session_dir.join("steam-shutdown.log") }
}

/// What [`restart_steam`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Restart {
    /// Steam is not running or runs clean: nothing was closed (a clean Steam is never shut down here).
    NothingToDo,
    /// The tainted Steam with this pid asked to quit and is gone.
    Closed(u32),
}

/// The tainted Steam: its pid and the SteamVR variables it carries (None = no Steam, or a clean one).
pub fn tainted_steam(home: &Path, procs: &dyn super::host::Procs) -> Option<(u32, Vec<String>)> {
    let pid = steam_pid(home, procs)?;
    let leaked = leaked_vars(&procs.environ(pid).unwrap_or_default());
    (!leaked.is_empty()).then_some((pid, leaked))
}

/// Close a Steam that carries SteamVR's variables, politely: `steam -shutdown` in a clean environment, then wait for
/// that pid to exit. Never signals Steam: when it does not exit within `wait` (a game or a download may be
/// finishing), the Err says so in plain words and nothing else happens. A clean or absent Steam is left alone.
#[allow(clippy::too_many_arguments)]
pub fn restart_steam(
    home: &Path,
    session_dir: &Path,
    scrub: &[String],
    user: &dyn Fn(&str) -> Option<String>,
    procs: &dyn super::host::Procs,
    sp: &dyn Spawner,
    clock: &dyn super::host::Clock,
    wait: Duration,
) -> Result<Restart, String> {
    let Some((pid, _)) = tainted_steam(home, procs) else { return Ok(Restart::NothingToDo) };
    let spec = steam_shutdown_spec(session_dir, scrub, user);
    sp.spawn(&spec).map_err(|e| format!("Could not ask Steam to close ({e}). Close it yourself (Steam menu > Exit), then press Play again."))?;
    let t0 = clock.now();
    loop {
        if !procs.is_alive(pid) || procs.comm(pid).as_deref() != Some("steam") {
            return Ok(Restart::Closed(pid));
        }
        if clock.now() - t0 > wait {
            return Err(format!(
                "Steam (pid {pid}) did not close within {} s. A game or a download may still be finishing. Close Steam yourself when it is safe (Steam menu > Exit), then press Play again. Nothing was forced.",
                wait.as_secs()
            ));
        }
        clock.sleep(Duration::from_millis(500));
    }
}

/// `omarchyvr vr steam-restart [--wait SECS]` for the app (the confirmed restart step of Play).
pub fn restart_cli(wait: Duration) -> Result<(), CliError> {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let session_dir = PathBuf::from(std::env::var("PIMAX_VR_SESSION_DIR").ok().filter(|s| !s.is_empty()).unwrap_or_else(|| super::config::SESSION_DIR.to_string()));
    super::session::ensure_private_dir(&session_dir)?;
    let pimax: Vec<(String, String)> = std::env::vars().filter(|(k, _)| k.starts_with("PIMAX_")).collect();
    let scrub = scrub_vars(&pimax);
    let user = |k: &str| std::env::var(k).ok();
    let procs = super::host::RealHost::from_env();
    let clock = super::host::RealClock::new();
    match restart_steam(&home, &session_dir, &scrub, &user, &procs, &super::desk::RealSpawner, &clock, wait) {
        Ok(Restart::NothingToDo) => {
            vout!("omarchyvr vr: Steam is not running with SteamVR's settings: nothing to close");
            Ok(())
        }
        Ok(Restart::Closed(pid)) => {
            vout!("omarchyvr vr: Steam (pid {pid}) closed");
            Ok(())
        }
        Err(e) => Err(CliError::new(e)),
    }
}

/// Make sure a clean Steam runs before SteamVR starts, so SteamVR never starts Steam itself. Records its pid in the
/// session dir. Err (plain words) when the running Steam came from inside SteamVR; Ok(None) when Steam did not come up
/// (SteamVR starts anyway).
pub fn ensure_steam(o: &StartOptions, home: &Path, scrub: &[String], user: &dyn Fn(&str) -> Option<String>, d: &Deps, sp: &dyn Spawner) -> Result<Option<u32>, CliError> {
    let say = |m: &str| vout!("omarchyvr vr: {m}");
    let record = |pid: u32| {
        let _ = std::fs::write(o.session_dir.join(STEAM_PID_FILE), format!("{pid}\n"));
    };
    if let Some(pid) = steam_pid(home, d.procs) {
        let leaked = leaked_vars(&d.procs.environ(pid).unwrap_or_default());
        if !leaked.is_empty() {
            let msg = leaked_message(pid, &leaked);
            d.notifier.notify(true, "Pimax VR: restart Steam", &msg);
            return Err(CliError::new(msg));
        }
        say(&format!("Steam runs (pid {pid})"));
        record(pid);
        return Ok(Some(pid));
    }
    let spec = steam_launch_spec(&o.session_dir, scrub, user);
    if let Err(e) = sp.spawn(&spec) {
        verr!("omarchyvr vr: WARNING: could not start Steam ({e}): SteamVR starts without it; games picked inside VR start only once Steam runs");
        return Ok(None);
    }
    say("starting Steam first (its own desktop entry, your normal environment), so SteamVR does not start it...");
    let t0 = d.clock.now();
    loop {
        if let Some(pid) = steam_pid(home, d.procs) {
            if !d.procs.find_by_comm("steamwebhelper").is_empty() {
                say(&format!("Steam is up (pid {pid})"));
                record(pid);
                return Ok(Some(pid));
            }
        }
        if (d.stop_requested)() || d.clock.now() - t0 > STEAM_WAIT {
            break;
        }
        d.clock.sleep(Duration::from_millis(500));
    }
    if let Some(pid) = steam_pid(home, d.procs) {
        record(pid);
        verr!("omarchyvr vr: WARNING: Steam (pid {pid}) is still starting after {} s: SteamVR starts now", STEAM_WAIT.as_secs());
        return Ok(Some(pid));
    }
    verr!("omarchyvr vr: WARNING: Steam did not start within {} s: SteamVR starts without it; games picked inside VR start only once Steam runs", STEAM_WAIT.as_secs());
    Ok(None)
}

/// What is wrong with Steam or SteamVR during a SteamVR session, in plain words (None = all fine).
/// `steam_at_play`: Steam's pid when Play started; `vrserver_alive`: our vrserver still runs; `steam_now`: the Steam
/// client now and its environment.
pub fn session_note_from(steam_at_play: Option<u32>, vrserver_alive: bool, steam_now: Option<(u32, Vec<(String, String)>)>) -> Option<String> {
    if let Some((pid, env)) = &steam_now {
        let leaked = leaked_vars(env);
        if !leaked.is_empty() {
            return Some(leaked_message(*pid, &leaked));
        }
    }
    if !vrserver_alive {
        return Some("SteamVR has quit on its own. Press Stop Play to clean up, then Play to start again.".into());
    }
    match (steam_at_play, steam_now) {
        (Some(a), Some((b, _))) if a != b => Some(format!(
            "Steam restarted during the VR session (pid {a} -> {b}). If a game does not start from inside VR, Stop Play and press Play again."
        )),
        (Some(_), None) => Some("Steam closed during the VR session: games cannot be started from inside VR. Stop Play, then press Play again (Play starts Steam first).".into()),
        _ => None,
    }
}

/// [`session_note_from`] for the session in `session_dir` (None when no SteamVR session of ours is recorded).
pub fn session_note(session_dir: &Path, home: &Path, procs: &dyn super::host::Procs) -> Option<String> {
    let server = super::session::read_pid(&session_dir.join(PID_FILE))?;
    let alive = procs.is_alive(server) && procs.comm(server).as_deref() == Some("vrserver");
    let at_play = super::session::read_pid(&session_dir.join(STEAM_PID_FILE));
    let now = steam_pid(home, procs).map(|p| (p, procs.environ(p).unwrap_or_default()));
    session_note_from(at_play, alive, now)
}

/// The SteamVR files of a session (pid files, shims) after it ended.
fn clean_session_files(session_dir: &Path) {
    let _ = std::fs::remove_file(session_dir.join(PID_FILE));
    let _ = std::fs::remove_file(session_dir.join(STEAM_PID_FILE));
    let _ = std::fs::remove_dir_all(shim_dir(session_dir));
}

/// `omarchyvr vr stop`: stop the SteamVR this tool started, by pid: SIGTERM to vrmonitor (SteamVR's own quit), then to
/// vrserver and vrcompositor, SIGKILL to what is left after `grace`. Nothing happens without our pid file.
pub fn stop(session_dir: &Path, grace: Duration, d: &Deps) -> Result<(), CliError> {
    let pf = session_dir.join(PID_FILE);
    let Some(server) = super::session::read_pid(&pf) else {
        clean_session_files(session_dir);
        return Ok(());
    };
    let say = |m: &str| vout!("omarchyvr vr: {m}");
    if !(d.procs.is_alive(server) && d.procs.comm(server).as_deref() == Some("vrserver")) {
        // SteamVR quit on its own (incident 2026-10-16: vrmonitor "quit all"): close what it left, by pid
        for p in d.procs.find_by_comm("vrmonitor").into_iter().chain(d.procs.find_by_comm("vrcompositor")) {
            say(&format!("SteamVR's vrserver is gone: SIGTERM leftover {} pid {p}", d.procs.comm(p).unwrap_or_default()));
            let _ = d.procs.signal(p, Signal::Term);
        }
        clean_session_files(session_dir);
        say("SteamVR is not running");
        return Ok(());
    }
    // SteamVR's processes of this user (comm checked again before every signal)
    let others = |name: &str| d.procs.find_by_comm(name);
    for p in others("vrmonitor") {
        say(&format!("closing SteamVR: SIGTERM vrmonitor pid {p}"));
        let _ = d.procs.signal(p, Signal::Term);
    }
    let gone = |pids: &[u32]| pids.iter().all(|p| !d.procs.is_alive(*p));
    let wait_gone = |pids: &[u32], limit: Duration| {
        let t0 = d.clock.now();
        while !gone(pids) && d.clock.now() - t0 < limit {
            d.clock.sleep(Duration::from_millis(250));
        }
        gone(pids)
    };
    if !wait_gone(&[server], Duration::from_secs(8)) {
        let mut v = vec![server];
        v.extend(others("vrcompositor"));
        for p in &v {
            if d.procs.is_alive(*p) {
                say(&format!("SIGTERM {} pid {p}", d.procs.comm(*p).unwrap_or_default()));
                let _ = d.procs.signal(*p, Signal::Term);
            }
        }
        if !wait_gone(&v, grace) {
            for p in v {
                if d.procs.is_alive(p) && matches!(d.procs.comm(p).as_deref(), Some("vrserver" | "vrcompositor")) {
                    verr!("omarchyvr vr: WARNING: {} pid {p} ignored SIGTERM: SIGKILL", d.procs.comm(p).unwrap_or_default());
                    let _ = d.procs.signal(p, Signal::Kill);
                }
            }
        }
    }
    clean_session_files(session_dir);
    say("SteamVR stopped");
    Ok(())
}

/// `omarchyvr vr games [monado|steamvr|status] [--monado-build DIR]`.
pub fn games_cli(what: &str, home: &Path, build: &Path, env: &dyn Fn(&str) -> Option<String>) -> Result<(), String> {
    let xrizer = super::openvr_link::xrizer_dir(env, home);
    match what {
        "status" => {
            let g = games_runtime(env);
            vout!("Games run with: {}{}", if g == Games::SteamVr { "SteamVR" } else { "Monado" }, if settings_path(home).exists() { "" } else { " (default)" });
            let text = std::fs::read_to_string(super::openvr_link::vrpath(home)).unwrap_or_default();
            vout!("OpenVR runtime: {}", super::openvr_link::first_runtime(&text).unwrap_or_else(|| "none".into()));
            let drv = super::openvr_link::external_drivers(&text);
            vout!("SteamVR driver registered: {}", if drv.is_empty() { "no".to_string() } else { drv.join(", ") });
            vout!("Monado build has the SteamVR driver: {}", if has_driver(build) { "yes" } else { "no" });
            Ok(())
        }
        w => {
            let g = Games::parse(w).ok_or_else(|| format!("unknown choice {w} (monado|steamvr|status)"))?;
            for l in switch(home, g, build, &xrizer)? {
                vout!("omarchyvr vr: {l}");
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vr::host::{Notifier, Procs};
    use std::cell::RefCell;
    use std::collections::HashMap;

    #[test]
    fn settings_parse_and_fall_back() {
        assert_eq!(parse_settings("runtime = \"steamvr\"\n"), Some(Games::SteamVr));
        assert_eq!(parse_settings("# x\nruntime=monado # c\n"), Some(Games::Monado));
        assert_eq!(parse_settings("runtime = \"bogus\""), None);
        let h = crate::vr::test_dir("games-settings");
        let hs = h.display().to_string();
        let env = |k: &str| (k == "HOME").then(|| hs.clone());
        assert_eq!(games_runtime(&env), DEFAULT);
        save(&h, Games::SteamVr).unwrap();
        assert_eq!(games_runtime(&env), Games::SteamVr);
        save(&h, Games::Monado).unwrap();
        assert_eq!(games_runtime(&env), Games::Monado);
    }

    #[test]
    fn launch_is_direct_on_the_host_unless_steam_is_asked_for() {
        let env = vec![("PIMAX_SLAM".to_string(), "1".to_string()), ("XRT_COMPOSITOR_LOG".into(), "info".into())];
        let b = crate::vr::test_dir("steamvr-launch-build");
        let l = launch_spec(&|_| None, Path::new("/h"), Path::new("/s"), &env, &b);
        assert!(!l.env.iter().any(|(k, _)| k == "LD_PRELOAD"), "no preload without the file");
        std::fs::create_dir_all(rttime_preload(&b).parent().unwrap()).unwrap();
        std::fs::write(rttime_preload(&b), "").unwrap();
        let lp = launch_spec(&|_| None, Path::new("/h"), Path::new("/s"), &env, &b);
        assert!(lp.env.contains(&("LD_PRELOAD".into(), rttime_preload(&b).display().to_string())), "{:?}", lp.env);
        assert_eq!(l.program, PathBuf::from("/h/.local/share/Steam/steamapps/common/SteamVR/bin/vrstartup.sh"));
        assert!(l.env.contains(&("VALVE_SKIP_RUNTIME_SAFETY".into(), "1".into())) && l.env.contains(&("PIMAX_SLAM".into(), "1".into())));
        assert!(l.env.contains(&("QT_QPA_PLATFORM".into(), "xcb".into())), "SteamVR's Qt has only xcb");
        assert!(!l.env.iter().any(|(k, _)| k == "XRT_COMPOSITOR_LOG"));
        let s = launch_spec(&|k| (k == "PIMAX_STEAMVR_LAUNCH").then(|| "steam".to_string()), Path::new("/h"), Path::new("/s"), &env, &b);
        assert_eq!(s.program, PathBuf::from("/usr/bin/steam"));
        assert_eq!(s.args, vec!["steam://rungameid/250820".to_string()]);
    }

    #[test]
    fn refresh_rate_follows_pimax_fps_and_keeps_other_settings() {
        let h = crate::vr::test_dir("steamvr-rate");
        assert!(set_refresh_rate(&h, 72).unwrap().is_some(), "created");
        std::fs::write(vrsettings_path(&h), "{\"GpuSpeed\": {\"x\": 1}, \"steamvr\": {\"installID\": \"7\", \"preferredRefreshRate\": 90}}").unwrap();
        assert!(set_refresh_rate(&h, 72).unwrap().unwrap().contains("72 Hz"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vrsettings_path(&h)).unwrap()).unwrap();
        assert_eq!(v["steamvr"]["preferredRefreshRate"], 72);
        assert_eq!(v["steamvr"]["installID"], "7");
        assert_eq!(v["GpuSpeed"]["x"], 1);
        assert!(set_refresh_rate(&h, 72).unwrap().is_none(), "unchanged");
        assert!(set_render_scale(&h).unwrap().unwrap().contains("100 %"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vrsettings_path(&h)).unwrap()).unwrap();
        assert_eq!((v["steamvr"]["supersampleManualOverride"].clone(), v["steamvr"]["supersampleScale"].as_f64()), (serde_json::json!(true), Some(1.0)));
        assert_eq!(v["steamvr"]["preferredRefreshRate"], 72);
        assert!(set_render_scale(&h).unwrap().is_none(), "unchanged");
        assert!(set_refresh_rate(&h, 72).unwrap().is_none(), "still unchanged");
        assert!(set_color_gain(&h, "bright").unwrap().unwrap().contains("1.12"));
        let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vrsettings_path(&h)).unwrap()).unwrap();
        assert_eq!((v["steamvr"]["hmdDisplayColorGainR"].as_f64(), v["steamvr"]["hmdDisplayColorGainB"].as_f64()), (Some(1.12), Some(1.12)));
        assert_eq!((v["steamvr"]["installID"].clone(), v["steamvr"]["preferredRefreshRate"].clone()), (serde_json::json!("7"), serde_json::json!(72)));
        assert!(set_color_gain(&h, "bright").unwrap().is_none());
        assert!(set_color_gain(&h, "standard").unwrap().is_some());
        assert_eq!(session_hz(&[("PIMAX_FPS".into(), "90".into())]), 90);
        assert_eq!(session_hz(&[]), 90);
    }

    #[test]
    fn driver_env_keeps_the_pimax_variables_only() {
        let env: Vec<(String, String)> = [
            ("XRT_COMPOSITOR_FORCE_WAYLAND_DIRECT", "1"),
            ("PIMAX_FPS", "90"),
            ("PIMAX_SLAM", "1"),
            ("XR_RUNTIME_JSON", "/x.json"),
            ("OXR_DEBUG_IPD_MM", "63.0"),
            ("IPC_IGNORE_VERSION", "1"),
        ]
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect();
        let t = driver_env_text(&env);
        assert!(t.contains("PIMAX_FPS=90\n") && t.contains("PIMAX_SLAM=1\n") && t.contains("OXR_DEBUG_IPD_MM=63.0\n"));
        assert!(!t.contains("XRT_COMPOSITOR") && !t.contains("XR_RUNTIME_JSON") && !t.contains("IPC_"));
    }

    fn fake_build(root: &Path) -> PathBuf {
        let b = root.join("build");
        std::fs::create_dir_all(b.join("steamvr-monado/bin/linux64")).unwrap();
        std::fs::write(b.join("steamvr-monado/driver.vrdrivermanifest"), "{}").unwrap();
        std::fs::write(b.join("steamvr-monado/bin/linux64/driver_monado.so"), "").unwrap();
        b
    }

    fn runtime(dir: &Path) -> PathBuf {
        std::fs::create_dir_all(dir.join("bin/linux64")).unwrap();
        std::fs::write(dir.join("bin/linux64/vrclient.so"), "").unwrap();
        dir.to_path_buf()
    }

    #[test]
    fn switch_steamvr_without_backup_and_back() {
        let h = crate::vr::test_dir("games-switch");
        let b = fake_build(&h);
        let x = runtime(&h.join("xrizer"));
        runtime(&super::super::openvr_link::steamvr_dir(&h));
        std::fs::write(super::super::runtime_link::steamvr_manifest(&h), "{}").unwrap();
        std::fs::write(b.join("openxr_monado-dev.json"), "{}").unwrap();
        // a read-only xrizer file and no backup at all (the case that failed in the app)
        super::super::openvr_link::ensure_xrizer(&h, &x).unwrap();
        let out = switch(&h, Games::SteamVr, &b, &x).unwrap();
        assert!(out.iter().any(|l| l.contains("registered")), "{out:?}");
        let text = std::fs::read_to_string(super::super::openvr_link::vrpath(&h)).unwrap();
        assert_eq!(super::super::openvr_link::external_drivers(&text), vec![driver_dir(&b).display().to_string()]);
        assert_eq!(parse_settings(&std::fs::read_to_string(settings_path(&h)).unwrap()), Some(Games::SteamVr));
        // OpenXR follows: SteamVR's runtime in SteamVR mode, the Monado build's in Monado mode
        let link = super::super::runtime_link::link_path(&h);
        assert_eq!(std::fs::read_link(&link).unwrap(), super::super::runtime_link::steamvr_manifest(&h));
        let out = switch(&h, Games::Monado, &b, &x).unwrap();
        assert_eq!(std::fs::read_link(&link).unwrap(), b.join("openxr_monado-dev.json"));
        assert!(out.last().unwrap().contains("Monado"));
        let text = std::fs::read_to_string(super::super::openvr_link::vrpath(&h)).unwrap();
        assert!(super::super::openvr_link::external_drivers(&text).is_empty(), "driver unregistered");
    }

    #[test]
    fn switch_errors_are_plain_words() {
        let h = crate::vr::test_dir("games-switch-err");
        let e = switch(&h, Games::SteamVr, &h.join("b"), &h.join("x")).unwrap_err();
        assert!(e.starts_with("Could not switch games to SteamVR") && e.contains("install SteamVR"), "{e}");
        let e = switch(&h, Games::Monado, &h.join("b"), &h.join("x")).unwrap_err();
        assert!(e.contains("xrizer is not built"), "{e}");
        assert!(!settings_path(&h).exists(), "a failed switch saves nothing");
    }

    // --- start/stop with fakes ---

    #[derive(Default)]
    struct FakeProcs {
        procs: RefCell<HashMap<u32, String>>,
        signals: RefCell<Vec<(u32, Signal)>>,
        /// vrserver appears after the launcher was spawned
        spawn_vrserver: bool,
        /// processes that ignore SIGTERM
        stubborn: Vec<u32>,
        /// /proc/PID/environ
        environs: RefCell<HashMap<u32, Vec<(String, String)>>>,
    }
    impl Procs for FakeProcs {
        fn is_alive(&self, pid: u32) -> bool {
            self.procs.borrow().contains_key(&pid)
        }
        fn comm(&self, pid: u32) -> Option<String> {
            self.procs.borrow().get(&pid).cloned()
        }
        fn signal(&self, pid: u32, sig: Signal) -> io::Result<()> {
            self.signals.borrow_mut().push((pid, sig));
            let name = self.procs.borrow().get(&pid).cloned();
            if sig == Signal::Kill || !self.stubborn.contains(&pid) {
                self.procs.borrow_mut().remove(&pid);
                // vrmonitor's quit takes vrserver and vrcompositor with it
                if name.as_deref() == Some("vrmonitor") {
                    self.procs.borrow_mut().retain(|p, n| !(matches!(n.as_str(), "vrserver" | "vrcompositor") && !self.stubborn.contains(p)));
                }
            }
            Ok(())
        }
        fn find_by_comm(&self, comm: &str) -> Vec<u32> {
            let mut v: Vec<u32> = self.procs.borrow().iter().filter(|(_, n)| *n == comm).map(|(p, _)| *p).collect();
            v.sort();
            v
        }
        fn environ(&self, pid: u32) -> Option<Vec<(String, String)>> {
            self.environs.borrow().get(&pid).cloned()
        }
        fn age_secs(&self, _: u32) -> Option<f64> {
            None
        }
    }
    struct FakeSpawner<'a>(&'a FakeProcs, RefCell<Vec<String>>);
    impl Spawner for FakeSpawner<'_> {
        fn spawn(&self, s: &ProcSpec) -> io::Result<u32> {
            self.1.borrow_mut().push(format!("{} {}", s.program.display(), s.args.join(" ")));
            if s.args.iter().any(|a| a == "gtk-launch") {
                self.0.procs.borrow_mut().insert(3000, "steam".into());
                self.0.procs.borrow_mut().insert(3001, "steamwebhelper".into());
                return Ok(98);
            }
            if self.0.spawn_vrserver {
                self.0.procs.borrow_mut().insert(4242, "vrserver".into());
                self.0.procs.borrow_mut().insert(4241, "vrmonitor".into());
                self.0.procs.borrow_mut().insert(4243, "vrcompositor".into());
            }
            Ok(99)
        }
    }
    struct Quiet;
    impl Notifier for Quiet {
        fn notify(&self, _: bool, _: &str, _: &str) {}
    }
    struct FakeClock(std::cell::Cell<Duration>);
    impl crate::vr::host::Clock for FakeClock {
        fn now(&self) -> Duration {
            self.0.get()
        }
        fn sleep(&self, d: Duration) {
            self.0.set(self.0.get() + d);
        }
    }

    fn opts(h: &Path) -> StartOptions {
        let mut a = crate::args::Args(vec![]);
        let hs = h.display().to_string();
        let mut o = crate::vr::parse_options(&mut a, &|k| (k == "HOME").then(|| hs.clone())).unwrap();
        o.session_dir = h.join("session");
        o.monado_build = fake_build(h);
        o
    }

    fn with<T>(h: &Path, procs: &FakeProcs, f: impl FnOnce(&Deps) -> T) -> T {
        let hs = h.display().to_string();
        let env = move |k: &str| match k {
            "HOME" => Some(hs.clone()),
            "PIMAX_STEAM_BIN" => Some("/fake/steam".into()),
            _ => None,
        };
        let clock = FakeClock(std::cell::Cell::new(Duration::ZERO));
        let deps = Deps { host: &NoHost, procs, notifier: &Quiet, clock: &clock, env: &env, stop_requested: &|| false };
        f(&deps)
    }

    struct NoHost;
    impl crate::vr::preflight::Host for NoHost {
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
    }

    #[test]
    fn start_registers_the_driver_writes_the_env_and_records_vrserver_then_stop_closes_it_by_pid() {
        let h = crate::vr::test_dir("steamvr-start");
        runtime(&super::super::openvr_link::steamvr_dir(&h));
        let o = opts(&h);
        let procs = FakeProcs { spawn_vrserver: true, ..Default::default() };
        let sp = FakeSpawner(&procs, RefCell::new(vec![]));
        let env = vec![("PIMAX_FPS".to_string(), "90".to_string()), ("XRT_COMPOSITOR_LOG".into(), "info".into())];
        with(&h, &procs, |d| start(&o, &env, None, None, d, &sp)).unwrap();
        {
            let spawned = sp.1.borrow();
            assert_eq!(spawned.len(), 2, "{spawned:?}");
            assert!(spawned[0].starts_with("/usr/bin/env -u ") && spawned[0].ends_with(" gtk-launch steam"), "Steam first, the normal way: {}", spawned[0]);
            assert!(spawned[0].contains("-u PIMAX_FPS") && spawned[0].contains("-u SteamAppId") && !spawned[0].contains("PRESSURE_VESSEL"), "{}", spawned[0]);
            assert_eq!(spawned[1], format!("{} ", super::super::openvr_link::steamvr_dir(&h).join("bin/vrstartup.sh").display()));
        }
        assert_eq!(std::fs::read_to_string(o.session_dir.join(STEAM_PID_FILE)).unwrap().trim(), "3000");
        assert!(shim_dir(&o.session_dir).join("xdg-open").exists() && shim_dir(&o.session_dir).join("steam").exists());
        assert_eq!(std::fs::read_to_string(o.session_dir.join(PID_FILE)).unwrap().trim(), "4242");
        assert_eq!(with(&h, &procs, |d| session_note(&o.session_dir, &h, d.procs)), None, "all fine");
        let vs: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(vrsettings_path(&h)).unwrap()).unwrap();
        assert_eq!(vs["steamvr"]["preferredRefreshRate"], 90, "SteamVR's rate follows PIMAX_FPS");
        let e = std::fs::read_to_string(driver_env_path(&h)).unwrap();
        assert!(e.contains("PIMAX_FPS=90") && !e.contains("XRT_COMPOSITOR"));
        let text = std::fs::read_to_string(super::super::openvr_link::vrpath(&h)).unwrap();
        assert_eq!(super::super::openvr_link::external_drivers(&text), vec![driver_dir(&o.monado_build).display().to_string()]);
        // a second Play while it runs changes nothing
        with(&h, &procs, |d| start(&o, &env, None, None, d, &sp)).unwrap();
        assert_eq!(sp.1.borrow().len(), 2);
        // Steam restarted during the session: said in plain words
        procs.procs.borrow_mut().remove(&3000);
        procs.procs.borrow_mut().insert(3100, "steam".into());
        let n = with(&h, &procs, |d| session_note(&o.session_dir, &h, d.procs)).unwrap();
        assert!(n.contains("Steam restarted during the VR session"), "{n}");
        // stop: vrmonitor first, by pid; the pid file goes
        with(&h, &procs, |d| stop(&o.session_dir, Duration::from_secs(5), d)).unwrap();
        assert_eq!(procs.signals.borrow().as_slice(), [(4241, Signal::Term)]);
        assert!(procs.procs.borrow().keys().all(|p| *p >= 3000), "only Steam is left: {:?}", procs.procs.borrow());
        assert!(!o.session_dir.join(PID_FILE).exists() && !o.session_dir.join(STEAM_PID_FILE).exists() && !shim_dir(&o.session_dir).exists());
    }

    /// Spawner for the restart tests: `steam -shutdown` makes the Steam leave unless it is stubborn.
    struct ShutdownSpawner<'a>(&'a FakeProcs, RefCell<Vec<String>>, bool);
    impl Spawner for ShutdownSpawner<'_> {
        fn spawn(&self, s: &ProcSpec) -> io::Result<u32> {
            self.1.borrow_mut().push(format!("{} {}", s.program.display(), s.args.join(" ")));
            if self.2 {
                self.0.procs.borrow_mut().retain(|_, n| n != "steam");
            }
            Ok(97)
        }
    }

    fn tainted_world() -> (std::path::PathBuf, FakeProcs) {
        let h = crate::vr::test_dir("steam-restart");
        let procs = FakeProcs::default();
        procs.procs.borrow_mut().insert(500, "steam".into());
        procs.environs.borrow_mut().insert(500, vec![("SteamAppId".into(), STEAMVR_APPID.into()), ("PIMAX_SLAM".into(), "1".into())]);
        (h, procs)
    }

    #[test]
    fn a_tainted_steam_is_found_a_clean_or_absent_one_is_not() {
        let (h, procs) = tainted_world();
        let (pid, vars) = tainted_steam(&h, &procs).unwrap();
        assert_eq!((pid, vars), (500, vec!["PIMAX_SLAM".to_string(), "SteamAppId".to_string()]));
        procs.environs.borrow_mut().insert(500, vec![("HOME".into(), "/h".into()), ("SteamAppId".into(), "730".into())]);
        assert_eq!(tainted_steam(&h, &procs), None, "a game's own app id is not SteamVR's");
        procs.procs.borrow_mut().clear();
        assert_eq!(tainted_steam(&h, &procs), None);
    }

    #[test]
    fn restart_closes_a_tainted_steam_with_a_clean_shutdown_and_waits_for_the_pid() {
        let (h, procs) = tainted_world();
        let sp = ShutdownSpawner(&procs, RefCell::new(vec![]), true);
        let clock = FakeClock(std::cell::Cell::new(Duration::ZERO));
        let scrub = scrub_vars(&[("PIMAX_FPS".into(), "90".into())]);
        let r = restart_steam(&h, &h.join("s"), &scrub, &|_| None, &procs, &sp, &clock, Duration::from_secs(5)).unwrap();
        assert_eq!(r, Restart::Closed(500));
        let spawned = sp.1.borrow();
        assert_eq!(spawned.len(), 1);
        assert!(spawned[0].starts_with("/usr/bin/env -u ") && spawned[0].ends_with(" /usr/bin/steam -shutdown"), "{}", spawned[0]);
        assert!(spawned[0].contains("-u SteamAppId") && spawned[0].contains("-u LD_PRELOAD"), "clean environment: {}", spawned[0]);
        assert!(procs.signals.borrow().is_empty(), "never a signal to Steam");
    }

    #[test]
    fn restart_that_times_out_says_so_and_never_signals_steam() {
        let (h, procs) = tainted_world();
        let sp = ShutdownSpawner(&procs, RefCell::new(vec![]), false);
        let clock = FakeClock(std::cell::Cell::new(Duration::ZERO));
        let e = restart_steam(&h, &h.join("s"), &[], &|_| None, &procs, &sp, &clock, Duration::from_secs(5)).unwrap_err();
        assert!(e.contains("did not close within 5 s") && e.contains("Nothing was forced"), "{e}");
        assert!(procs.signals.borrow().is_empty() && procs.is_alive(500));
    }

    #[test]
    fn restart_leaves_a_clean_or_absent_steam_alone() {
        let (h, procs) = tainted_world();
        procs.environs.borrow_mut().insert(500, vec![("HOME".into(), "/h".into())]);
        let sp = ShutdownSpawner(&procs, RefCell::new(vec![]), true);
        let clock = FakeClock(std::cell::Cell::new(Duration::ZERO));
        assert_eq!(restart_steam(&h, &h.join("s"), &[], &|_| None, &procs, &sp, &clock, Duration::from_secs(5)), Ok(Restart::NothingToDo));
        assert!(sp.1.borrow().is_empty() && procs.is_alive(500), "a clean Steam is never shut down");
    }

    #[test]
    fn start_refuses_while_monado_runs_or_a_foreign_steamvr_runs() {
        let h = crate::vr::test_dir("steamvr-refuse");
        runtime(&super::super::openvr_link::steamvr_dir(&h));
        let o = opts(&h);
        let procs = FakeProcs::default();
        procs.procs.borrow_mut().insert(10, "monado-service".into());
        let sp = FakeSpawner(&procs, RefCell::new(vec![]));
        let e = with(&h, &procs, |d| start(&o, &[], None, None, d, &sp)).unwrap_err();
        assert!(e.msg.contains("Monado is running"), "{}", e.msg);
        procs.procs.borrow_mut().clear();
        procs.procs.borrow_mut().insert(11, "vrserver".into());
        let e = with(&h, &procs, |d| start(&o, &[], None, None, d, &sp)).unwrap_err();
        assert!(e.msg.contains("close SteamVR first"), "{}", e.msg);
        assert!(sp.1.borrow().is_empty());
    }

    #[test]
    fn start_without_the_driver_or_a_vrserver_fails_in_plain_words() {
        let h = crate::vr::test_dir("steamvr-nodrv");
        runtime(&super::super::openvr_link::steamvr_dir(&h));
        let mut o = opts(&h);
        o.monado_build = h.join("nodriver");
        let procs = FakeProcs::default();
        let sp = FakeSpawner(&procs, RefCell::new(vec![]));
        let e = with(&h, &procs, |d| start(&o, &[], None, None, d, &sp)).unwrap_err();
        assert!(e.msg.contains("has no SteamVR driver"), "{}", e.msg);
        let o = opts(&h);
        let e = with(&h, &procs, |d| start(&o, &[], None, None, d, &sp)).unwrap_err();
        assert!(e.msg.contains("did not start within"), "{}", e.msg);
        assert!(!o.session_dir.join(PID_FILE).exists());
    }

    #[test]
    fn stop_escalates_by_pid_only_for_steamvr_processes() {
        let h = crate::vr::test_dir("steamvr-stop");
        let dir = h.join("s");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(PID_FILE), "7\n").unwrap();
        let procs = FakeProcs { stubborn: vec![7], ..Default::default() };
        procs.procs.borrow_mut().insert(7, "vrserver".into());
        procs.procs.borrow_mut().insert(8, "vrcompositor".into());
        procs.procs.borrow_mut().insert(9, "monado-service".into());
        with(&h, &procs, |d| stop(&dir, Duration::from_secs(2), d)).unwrap();
        let s = procs.signals.borrow().clone();
        assert!(s.contains(&(7, Signal::Term)) && s.contains(&(8, Signal::Term)) && s.contains(&(7, Signal::Kill)), "{s:?}");
        assert!(!s.iter().any(|(p, _)| *p == 9), "never touches other processes");
        // vrserver gone on its own: its leftovers are closed by pid, the files go
        std::fs::write(dir.join(PID_FILE), "7\n").unwrap();
        let procs = FakeProcs::default();
        procs.procs.borrow_mut().insert(21, "vrmonitor".into());
        procs.procs.borrow_mut().insert(9, "monado-service".into());
        with(&h, &procs, |d| stop(&dir, Duration::from_secs(2), d)).unwrap();
        assert_eq!(procs.signals.borrow().as_slice(), [(21, Signal::Term)]);
        assert!(!dir.join(PID_FILE).exists());
        // no pid file: nothing at all
        let procs = FakeProcs::default();
        procs.procs.borrow_mut().insert(7, "vrserver".into());
        with(&h, &procs, |d| stop(&dir, Duration::from_secs(2), d)).unwrap();
        assert!(procs.signals.borrow().is_empty());
    }

    #[test]
    fn env_builder_keeps_steamvr_variables_in_steamvr() {
        // which variable goes where: vrstartup.sh gets ours, Steam and the shims get them removed or the user's back
        let pimax = vec![("PIMAX_SLAM".to_string(), "1".to_string()), ("OXR_DEBUG_IPD_MM".into(), "61.5".into()), ("XR_RUNTIME_JSON".into(), "/x".into())];
        let b = crate::vr::test_dir("steamvr-envb");
        std::fs::create_dir_all(rttime_preload(&b).parent().unwrap()).unwrap();
        std::fs::write(rttime_preload(&b), "").unwrap();
        let user = |k: &str| match k {
            "PATH" => Some("/usr/bin:/bin".to_string()),
            "QT_QPA_PLATFORM" => Some("wayland;xcb".to_string()),
            _ => None,
        };
        let l = launch_spec(&user, Path::new("/h"), Path::new("/s"), &pimax, &b);
        let get = |k: &str| l.env.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        assert_eq!(get("PATH").as_deref(), Some("/s/steamvr-shim:/usr/bin:/bin"), "shims first for SteamVR");
        assert_eq!(get("QT_QPA_PLATFORM").as_deref(), Some("xcb"));
        assert!(get("LD_PRELOAD").is_some() && get("PIMAX_SLAM").is_some() && get("VALVE_SKIP_RUNTIME_SAFETY").is_some());
        let scrub = scrub_vars(&pimax);
        for k in ["LD_PRELOAD", "QT_QPA_PLATFORM", "VALVE_SKIP_RUNTIME_SAFETY", "PIMAX_SLAM", "OXR_DEBUG_IPD_MM", "SteamAppId", "SteamGameId", "STEAMVR_VRENV", "PATH"] {
            assert!(scrub.contains(&k.to_string()), "{k} must not reach Steam: {scrub:?}");
        }
        assert!(!scrub.iter().any(|k| k == super::super::steam_setup::VAR || k == "XR_RUNTIME_JSON"), "{scrub:?}");
        let a = clean_env_args(&scrub, &user);
        let first_set = a.iter().position(|x| x.contains('=')).unwrap();
        assert!(a[..first_set].chunks(2).all(|c| c[0] == "-u"), "all -u before the first NAME=VALUE: {a:?}");
        assert!(a.contains(&"QT_QPA_PLATFORM=wayland;xcb".to_string()) && a.contains(&"PATH=/usr/bin:/bin".to_string()), "the user's own value back: {a:?}");
        assert!(a.windows(2).any(|w| w == ["-u", "LD_PRELOAD"]) && a.windows(2).any(|w| w == ["-u", "SteamAppId"]));
        let st = steam_launch_spec(Path::new("/s"), &scrub, &user);
        assert_eq!((st.program.as_path(), st.env.len()), (Path::new("/usr/bin/env"), 0));
        assert_eq!(&st.args[st.args.len() - 2..], ["gtk-launch", "steam"]);
    }

    #[test]
    fn shim_runs_the_real_program_without_steamvr_variables() {
        let d = crate::vr::test_dir("steamvr-shim");
        let real_dir = d.join("realbin");
        std::fs::create_dir_all(&real_dir).unwrap();
        // the "real" xdg-open prints its environment
        let real = real_dir.join("xdg-open");
        std::fs::write(&real, "#!/bin/sh\n/usr/bin/env\necho ARGS=\"$*\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o755)).unwrap();
        let path = format!("{}:/usr/bin:/bin", real_dir.display());
        let p2 = path.clone();
        let user = move |k: &str| match k {
            "PATH" => Some(p2.clone()),
            "QT_QPA_PLATFORM" => Some("wayland;xcb".to_string()),
            _ => None,
        };
        let scrub = scrub_vars(&[("PIMAX_SLAM".to_string(), "1".to_string())]);
        write_shims(&d, &scrub, &user).unwrap();
        let out = std::process::Command::new(shim_dir(&d).join("xdg-open"))
            .arg("steam://launch/658920/VR")
            .env("PATH", format!("{}:{path}", shim_dir(&d).display()))
            .env("PIMAX_SLAM", "1")
            .env("SteamAppId", "250820")
            .env("SteamGameId", "250820")
            .env("VALVE_SKIP_RUNTIME_SAFETY", "1")
            .env("QT_QPA_PLATFORM", "xcb")
            .env("LD_LIBRARY_PATH", "/steamvr/lib")
            .env(super::super::steam_setup::VAR, "1")
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        assert!(text.contains("ARGS=steam://launch/658920/VR"), "{text}");
        for gone in ["PIMAX_SLAM=", "SteamAppId=", "SteamGameId=", "VALVE_SKIP_RUNTIME_SAFETY=", "LD_LIBRARY_PATH="] {
            assert!(!text.lines().any(|l| l.starts_with(gone)), "{gone} leaked: {text}");
        }
        assert!(text.lines().any(|l| l == "QT_QPA_PLATFORM=wayland;xcb"), "{text}");
        assert!(text.lines().any(|l| l == format!("PATH={path}")), "shim dir out of PATH: {text}");
        assert!(text.lines().any(|l| l == format!("{}=1", super::super::steam_setup::VAR)), "Steam keeps the pressure-vessel variable");
    }

    #[test]
    fn a_steam_started_from_inside_steamvr_is_refused_and_reported() {
        let leaked: Vec<(String, String)> = [("SteamAppId", "250820"), ("LD_PRELOAD", "/b/pimax_rttime_preload.so"), ("PIMAX_FPS", "72"), ("HOME", "/h")]
            .iter()
            .map(|(k, v)| (k.to_string(), v.to_string()))
            .collect();
        assert_eq!(leaked_vars(&leaked), vec!["LD_PRELOAD", "PIMAX_FPS", "SteamAppId"]);
        assert!(leaked_vars(&[("SteamAppId".into(), "620".into()), ("LD_PRELOAD".into(), "/usr/lib/libgamemodeauto.so".into())]).is_empty());
        let n = session_note_from(Some(1), true, Some((2, leaked.clone()))).unwrap();
        assert!(n.contains("started from inside SteamVR") && n.contains("Quit Steam"), "{n}");
        assert!(session_note_from(Some(1), false, Some((1, vec![]))).unwrap().contains("SteamVR has quit"));
        assert!(session_note_from(Some(1), true, None).unwrap().contains("Steam closed"));
        assert_eq!(session_note_from(Some(1), true, Some((1, vec![]))), None);
        // Play refuses while such a Steam runs (it would freeze games), without touching it
        let h = crate::vr::test_dir("steamvr-leaked");
        runtime(&super::super::openvr_link::steamvr_dir(&h));
        let o = opts(&h);
        let procs = FakeProcs { spawn_vrserver: true, ..Default::default() };
        procs.procs.borrow_mut().insert(16972, "steam".into());
        procs.environs.borrow_mut().insert(16972, leaked);
        let sp = FakeSpawner(&procs, RefCell::new(vec![]));
        let e = with(&h, &procs, |d| start(&o, &[], None, None, d, &sp)).unwrap_err();
        assert!(e.msg.contains("Steam (pid 16972) was started from inside SteamVR"), "{}", e.msg);
        assert!(sp.1.borrow().is_empty() && procs.signals.borrow().is_empty());
    }
}
