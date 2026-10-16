//! `omarchyvr vr game-check [--appid N] [--json] [--no-container]`: "do VR games reach the headset?" in plain PASS/FAIL lines.
//!
//! Written after a day of hand-found causes of "games show nothing in VR" (LESSONS rows 20, 23, 27):
//!
//! 1. `openvrpaths.vrpath` listed SteamVR first (SteamVR rewrites it when it starts), so OpenVR games used SteamVR;
//! 2. Steam's pressure-vessel shares Monado's socket into the game container only with
//!    `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1`;
//! 3. the game's Launch Options were empty;
//! 4. SteamVR exits when `openvrpaths.vrpath` is read-only.
//!
//! The checks are read-only (the container test starts a tiny OpenXR client, `xrCreateInstance` + `xrGetSystem`, no
//! session, so a running Monado only sees a short-lived client). The client is C ([`PROBE_C`]) built once with `cc`
//! into `~/.local/share/omarchyvr/bin/xr-probe` (it loads the loader with `dlopen`, so it runs inside any runtime).
//! Everything that parses is a pure function over text, tested with fixtures.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// The env var Steam's container needs (LESSONS row 27).
pub const PV_VAR: &str = "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES";

/// Result of one check.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Fine.
    Pass,
    /// Wrong; the fix line says what to do.
    Fail,
    /// Worth a look, not blocking.
    Warn,
    /// Not run.
    Skip,
}

impl Status {
    /// "PASS", "FAIL", ...
    pub fn word(self) -> &'static str {
        match self {
            Status::Pass => "PASS",
            Status::Fail => "FAIL",
            Status::Warn => "WARN",
            Status::Skip => "SKIP",
        }
    }
}

/// One line of the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Check {
    /// Stable id (`openxr`, `openvr`, `service`, `container`, `steam-env`, `last-run`, ...).
    pub id: &'static str,
    /// Result.
    pub status: Status,
    /// What was checked, with the result in plain words.
    pub title: String,
    /// The one fix when it is not a PASS (empty otherwise).
    pub fix: String,
    /// Text to copy for the fix (launch options line, ...); empty when none.
    pub copy: String,
}

fn check(id: &'static str, status: Status, title: impl Into<String>) -> Check {
    Check { id, status, title: title.into(), fix: String::new(), copy: String::new() }
}

impl Check {
    fn fix(mut self, fix: impl Into<String>) -> Self {
        self.fix = fix.into();
        self
    }
    fn copy(mut self, copy: impl Into<String>) -> Self {
        self.copy = copy.into();
        self
    }
}

/// The whole report as JSON (`{"ok":bool,"checks":[...]}`).
pub fn to_json(checks: &[Check]) -> String {
    let arr: Vec<_> = checks
        .iter()
        .map(|c| serde_json::json!({"id": c.id, "status": c.status.word(), "title": c.title, "fix": c.fix, "copy": c.copy}))
        .collect();
    let ok = !checks.iter().any(|c| c.status == Status::Fail);
    serde_json::to_string_pretty(&serde_json::json!({"ok": ok, "checks": arr})).unwrap_or_default()
}

/// Parse the JSON of [`to_json`] back (the app shows it).
pub fn from_json(text: &str) -> Option<Vec<Check>> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mut out = vec![];
    for c in v.get("checks")?.as_array()? {
        let id = match c.get("id")?.as_str()? {
            "openxr" => "openxr",
            "openvr" => "openvr",
            "service" => "service",
            "container" => "container",
            "container-openvr" => "container-openvr",
            "steam-env" => "steam-env",
            "last-run" => "last-run",
            _ => "other",
        };
        let status = match c.get("status")?.as_str()? {
            "PASS" => Status::Pass,
            "FAIL" => Status::Fail,
            "WARN" => Status::Warn,
            _ => Status::Skip,
        };
        out.push(Check {
            id,
            status,
            title: c.get("title")?.as_str()?.to_string(),
            fix: c.get("fix").and_then(|x| x.as_str()).unwrap_or("").to_string(),
            copy: c.get("copy").and_then(|x| x.as_str()).unwrap_or("").to_string(),
        });
    }
    Some(out)
}

/// Plain-text report.
pub fn to_text(checks: &[Check]) -> String {
    let mut s = String::new();
    for c in checks {
        s.push_str(&format!("{}  {}\n", c.status.word(), c.title));
        if !c.fix.is_empty() {
            s.push_str(&format!("      fix: {}\n", c.fix));
        }
        if !c.copy.is_empty() {
            s.push_str(&format!("      paste: {}\n", c.copy));
        }
    }
    s
}

// ---------------------------------------------------------------------------------------------------------------
// pure parsers

/// First `runtime` entry of an openvrpaths.vrpath text.
pub fn vrpath_first(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("runtime")?.as_array()?.first()?.as_str().map(|s| s.trim_end_matches('/').to_string())
}

/// What the first OpenVR runtime entry is.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenVr {
    /// xrizer (Monado path).
    Xrizer,
    /// SteamVR (named by its path).
    SteamVr,
    /// Something else.
    Other(String),
}

/// Classify a runtime dir (xrizer is whatever contains "xrizer"; SteamVR lives in `steamapps/common/SteamVR`).
pub fn classify_openvr(first: &str) -> OpenVr {
    let l = first.to_lowercase();
    if l.contains("xrizer") {
        OpenVr::Xrizer
    } else if l.contains("steamvr") {
        OpenVr::SteamVr
    } else {
        OpenVr::Other(first.to_string())
    }
}

/// The OpenVR runtime check from the file text (`None` = no file), whether it is read-only and whether SteamVR runs.
pub fn openvr_check(text: Option<&str>, read_only: bool, steamvr_running: bool, xrizer: &str) -> Check {
    let fixtxt = "Press 'Use xrizer + Monado' (Games run with / VR mode), or start VR: it puts xrizer back first.";
    let Some(text) = text else {
        return check("openvr", Status::Fail, "OpenVR runtime: no ~/.config/openvr/openvrpaths.vrpath, OpenVR games find no runtime").fix(fixtxt);
    };
    match vrpath_first(text).map(|f| (classify_openvr(&f), f)) {
        Some((OpenVr::Xrizer, _)) => {
            let mut c = check("openvr", Status::Pass, "OpenVR runtime: xrizer is first (OpenVR games go to Monado)");
            if !read_only {
                c.title.push_str(" (the file is writable, so SteamVR can rewrite it when it starts; this check will then say so)");
            }
            c
        }
        Some((OpenVr::SteamVr, f)) => check(
            "openvr",
            Status::Fail,
            format!(
                "OpenVR runtime: SteamVR is first ({f}), not xrizer. SteamVR rewrites this file every time it starts{}; OpenVR games then use SteamVR and fail with InterfaceNotFound while Monado runs",
                if steamvr_running { " (SteamVR is running right now)" } else { "" }
            ),
        )
        .fix(fixtxt),
        Some((OpenVr::Other(f), _)) => check("openvr", Status::Fail, format!("OpenVR runtime: first entry is {f} (expected xrizer at {xrizer})")).fix(fixtxt),
        None => check("openvr", Status::Fail, "OpenVR runtime: openvrpaths.vrpath cannot be read (broken JSON or no runtime entry)").fix(fixtxt),
    }
}

/// SteamVR mode: the OpenVR runtime must be SteamVR (first) and our `driver_monado` must be in `external_drivers`.
pub fn openvr_check_steamvr(text: Option<&str>, driver_dir: &str) -> Check {
    let fix = "Press 'Use SteamVR' (Games run with / VR mode), or start VR: it hands openvrpaths.vrpath to SteamVR and registers the Pimax driver.";
    let Some(text) = text else {
        return check("openvr", Status::Fail, "OpenVR runtime: no ~/.config/openvr/openvrpaths.vrpath, SteamVR will not know the Pimax driver").fix(fix);
    };
    let drivers = super::openvr_link::external_drivers(text);
    let has = drivers.iter().any(|d| d.trim_end_matches('/').ends_with("steamvr-monado") || d.trim_end_matches('/') == driver_dir.trim_end_matches('/'));
    match vrpath_first(text).map(|f| (classify_openvr(&f), f)) {
        Some((OpenVr::SteamVr, _)) if has => check("openvr", Status::Pass, "OpenVR runtime: SteamVR is first and the Pimax driver (driver_monado) is registered in external_drivers"),
        Some((OpenVr::SteamVr, f)) => check("openvr", Status::Fail, format!("OpenVR runtime: SteamVR is first ({f}) but the Pimax driver (driver_monado) is not in external_drivers: SteamVR will not find the headset")).fix(fix),
        Some((OpenVr::Xrizer, _)) => check("openvr", Status::Fail, "OpenVR runtime: xrizer is first, but 'Games run with: SteamVR' is chosen: OpenVR games would look for Monado, which is not running").fix(fix),
        Some((OpenVr::Other(f), _)) => check("openvr", Status::Fail, format!("OpenVR runtime: first entry is {f} (expected SteamVR)")).fix(fix),
        None => check("openvr", Status::Fail, "OpenVR runtime: openvrpaths.vrpath cannot be read (broken JSON or no runtime entry)").fix(fix),
    }
}

/// SteamVR mode: is SteamVR up? (`vrserver` and `vrcompositor` pids).
pub fn steamvr_service_check(vrserver: Option<u32>, vrcompositor: Option<u32>) -> Check {
    let fix = "Press Play / Start VR in the app (it starts SteamVR with the Pimax driver).";
    match (vrserver, vrcompositor) {
        (Some(s), Some(c)) => check("service", Status::Pass, format!("SteamVR is running (vrserver pid {s}, vrcompositor pid {c})")),
        (Some(s), None) => check("service", Status::Fail, format!("vrserver runs (pid {s}) but vrcompositor does not (still starting, or the headset was not found)")).fix("Wait a few seconds and check again; if it stays, restart VR from the app."),
        (None, _) => check("service", Status::Fail, "SteamVR is not running (no vrserver): games find no VR runtime").fix(fix),
    }
}

/// SteamVR mode while Work (the desk: Monado + WayVR) runs and SteamVR does not. Work leaves the global runtimes at
/// the user's games choice (WayVR and the session's other apps get `XR_RUNTIME_JSON` for Monado directly), so games
/// go to SteamVR, which starts from Play. Not a failure of the game setup: a WARN that says what to do.
pub fn work_mode_check(monado: Option<u32>, vrserver: Option<u32>) -> Option<Check> {
    match (monado, vrserver) {
        (Some(p), None) => Some(
            check(
                "service",
                Status::Warn,
                format!("Work is running (Monado pid {p}, the desk). Games run with SteamVR, which is not running: games are played from Play, not inside Work"),
            )
            .fix("To play: stop Work and press Play (it starts SteamVR with the Pimax driver). To play inside Work instead, set 'Games run with: Monado'."),
        ),
        _ => None,
    }
}

/// SteamVR mode: the OpenXR runtime must be SteamVR's own (`steamxr_linux64.json`, `bin/linux64/vrclient.so`).
pub fn openxr_check_steamvr(manifest: Option<&str>, manifest_text: Option<&str>, lib: Option<&Path>, lib_exists: bool) -> Check {
    let fix = "Start VR from the app: in SteamVR mode it points ~/.config/openxr/1/active_runtime.json at SteamVR's OpenXR runtime.";
    let Some(manifest) = manifest else {
        return check("openxr", Status::Fail, "OpenXR runtime: ~/.config/openxr/1/active_runtime.json is missing or unreadable").fix(fix);
    };
    let Some(lib) = lib else {
        return check("openxr", Status::Fail, format!("OpenXR runtime: {manifest} has no library_path")).fix(fix);
    };
    let steam = lib.to_string_lossy().to_lowercase().contains("steamvr") || manifest_text.is_some_and(|t| t.contains("VALVE_runtime_is_steamvr"));
    if !steam {
        return check("openxr", Status::Fail, format!("OpenXR runtime: points at {} (Monado), but games run with SteamVR and no Monado is running: OpenXR games find no runtime", lib.display())).fix(fix);
    }
    if !lib_exists {
        return check("openxr", Status::Fail, format!("OpenXR runtime: {manifest} points at {} which does not exist (SteamVR not installed?)", lib.display())).fix(fix);
    }
    check("openxr", Status::Pass, format!("OpenXR runtime: SteamVR's own ({})", lib.display()))
}

/// `library_path` of an OpenXR runtime manifest.
pub fn manifest_library(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("runtime")?.get("library_path")?.as_str().map(String::from)
}

/// The Monado build root of a `monado-service` path (`.../build/src/xrt/targets/service/monado-service` -> `.../build`).
pub fn build_root_of_service(exe: &Path) -> Option<PathBuf> {
    let s = exe.to_str()?;
    s.strip_suffix("/src/xrt/targets/service/monado-service").map(PathBuf::from)
}

/// Decide the OpenXR check. `lib` is the manifest's real library path, `service_root` the real build root of the
/// running service (`None` = no service), `link` what the manifest link is called in words.
pub fn openxr_check(manifest: Option<&str>, lib: Option<&Path>, lib_exists: bool, service_root: Option<&Path>) -> Check {
    let fix = "Start VR from the app (it points ~/.config/openxr/1/active_runtime.json at the running build).";
    let Some(manifest) = manifest else {
        return check("openxr", Status::Fail, "OpenXR runtime: ~/.config/openxr/1/active_runtime.json is missing or unreadable").fix(fix);
    };
    let Some(lib) = lib else {
        return check("openxr", Status::Fail, format!("OpenXR runtime: {manifest} has no library_path")).fix(fix);
    };
    if !lib_exists {
        return check("openxr", Status::Fail, format!("OpenXR runtime: {manifest} points at {} which does not exist", lib.display())).fix(fix);
    }
    match service_root {
        Some(root) if lib.starts_with(root) => check("openxr", Status::Pass, format!("OpenXR runtime: points at the running Monado build ({})", root.display())),
        Some(root) => check(
            "openxr",
            Status::Fail,
            format!("OpenXR runtime: points at {}, but the running Monado is {}", lib.display(), root.display()),
        )
        .fix(fix),
        None => check("openxr", Status::Pass, format!("OpenXR runtime: points at {} (no Monado running to compare)", lib.display())),
    }
}

/// One `key`/`value` or `key`/`{ ... }` tree of a Valve VDF file.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Vdf {
    /// Child nodes in file order (lower-cased key, node).
    pub kids: Vec<(String, Vdf)>,
    /// The string value of a leaf.
    pub val: Option<String>,
}

impl Vdf {
    /// Child by case-insensitive key.
    pub fn get(&self, key: &str) -> Option<&Vdf> {
        let k = key.to_lowercase();
        self.kids.iter().find(|(n, _)| *n == k).map(|(_, v)| v)
    }
    /// Walk a path of keys.
    pub fn path(&self, keys: &[&str]) -> Option<&Vdf> {
        keys.iter().try_fold(self, |n, k| n.get(k))
    }
}

/// Parse VDF text (quoted strings with `\"` `\\` `\n` `\t` escapes, `{`/`}`, `//` comments).
pub fn parse_vdf(text: &str) -> Option<Vdf> {
    #[derive(PartialEq)]
    enum T {
        S(String),
        Open,
        Close,
    }
    let mut toks = vec![];
    let mut it = text.chars().peekable();
    while let Some(c) = it.next() {
        match c {
            '"' => {
                let mut s = String::new();
                loop {
                    match it.next()? {
                        '"' => break,
                        '\\' => match it.next()? {
                            'n' => s.push('\n'),
                            't' => s.push('\t'),
                            o => s.push(o),
                        },
                        o => s.push(o),
                    }
                }
                toks.push(T::S(s));
            }
            '{' => toks.push(T::Open),
            '}' => toks.push(T::Close),
            '/' if it.peek() == Some(&'/') => {
                for o in it.by_ref() {
                    if o == '\n' {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    fn block(toks: &[T], i: &mut usize) -> Option<Vdf> {
        let mut node = Vdf::default();
        while *i < toks.len() {
            match &toks[*i] {
                T::Close => {
                    *i += 1;
                    return Some(node);
                }
                T::S(k) => {
                    *i += 1;
                    match toks.get(*i)? {
                        T::S(v) => {
                            node.kids.push((k.to_lowercase(), Vdf { kids: vec![], val: Some(v.clone()) }));
                            *i += 1;
                        }
                        T::Open => {
                            *i += 1;
                            node.kids.push((k.to_lowercase(), block(toks, i)?));
                        }
                        T::Close => return None,
                    }
                }
                T::Open => return None,
            }
        }
        Some(node)
    }
    let mut i = 0;
    block(&toks, &mut i)
}

/// `Software/Valve/Steam/apps` of a localconfig.vdf.
pub fn local_apps(vdf: &Vdf) -> Option<&Vdf> {
    vdf.get("UserLocalConfigStore")?.path(&["Software", "Valve", "Steam", "apps"])
}

/// LaunchOptions of an app (`None` = app not in the file).
pub fn launch_options(vdf: &Vdf, appid: &str) -> Option<String> {
    let app = local_apps(vdf)?.get(appid)?;
    Some(app.get("LaunchOptions").and_then(|v| v.val.clone()).unwrap_or_default())
}

/// The most recently played app id (largest `LastPlayed`) that is in `candidates` (empty = any).
pub fn last_played(vdf: &Vdf, candidates: &[String]) -> Option<String> {
    let apps = local_apps(vdf)?;
    apps.kids
        .iter()
        .filter(|(id, _)| candidates.is_empty() || candidates.contains(id))
        .filter_map(|(id, a)| Some((a.get("LastPlayed")?.val.as_ref()?.parse::<u64>().ok()?, id.clone())))
        .max()
        .map(|(_, id)| id)
}

/// Does an environment (`environ` file bytes) set the variable to 1?
pub fn environ_has_pv(environ: &[u8]) -> bool {
    environ.split(|b| *b == 0).any(|kv| kv == format!("{PV_VAR}=1").as_bytes())
}

/// The Steam env check. `steam_env`: `None` when Steam is not running, else whether the variable is set globally.
/// `container_without_ok`: result of the container test without the variable, when it was run.
pub fn steam_env_check(steam_env: Option<bool>, launch: Option<&str>, appid: Option<&str>, container_without_ok: Option<bool>, line: &str) -> Check {
    let in_launch = launch.is_some_and(|l| l.contains(PV_VAR));
    let fix = match appid {
        Some(id) => format!("Steam needs the VR setting: open the game's Properties > General > Launch Options (app {id}) and paste the line below (Copy button)."),
        None => "Steam needs the VR setting: in each VR game's Properties > General > Launch Options paste the line below (Copy button), or use 'Set up Steam for VR games' to set it for all games.".into(),
    };
    if in_launch {
        return check("steam-env", Status::Pass, format!("Steam env: {PV_VAR}=1 is in the launch options of app {}", appid.unwrap_or("?")));
    }
    if steam_env == Some(true) {
        return check("steam-env", Status::Pass, format!("Steam env: {PV_VAR}=1 is set for the running Steam (all games get it)"));
    }
    if container_without_ok == Some(true) {
        return check("steam-env", Status::Pass, format!("Steam env: {PV_VAR} is not set, but the game container reaches Monado without it"));
    }
    let what = match (steam_env, launch) {
        (None, Some(l)) if l.trim().is_empty() => "Steam is not running and the game's launch options are empty".to_string(),
        (None, Some(l)) => format!("Steam is not running and the game's launch options are \"{l}\" (no {PV_VAR})"),
        (None, None) => "Steam is not running and no game was found in its settings".to_string(),
        (Some(_), Some(l)) if l.trim().is_empty() => "Steam does not have it in its environment and the game's launch options are empty".to_string(),
        (Some(_), Some(l)) => format!("Steam does not have it in its environment and the game's launch options are \"{l}\""),
        (Some(_), None) => "Steam does not have it in its environment".to_string(),
    };
    check("steam-env", Status::Fail, format!("Steam env: {PV_VAR}=1 is not set: {what}. Games run in a container without Monado's socket"))
        .fix(fix)
        .copy(line)
}

/// What the last run of a game says.
#[derive(Debug, PartialEq, Eq)]
pub enum LastRun {
    /// OpenVR could not start: the runtime was not xrizer.
    InterfaceNotFound,
    /// OpenXR runtime unavailable: the container did not reach Monado.
    RuntimeUnavailable,
    /// The session reached FOCUSED.
    Focused,
    /// xrizer started a session but it never got focus.
    SessionNoFocus,
    /// Nothing recognisable.
    Unknown,
}

/// Classify a Unity `Player.log` and/or an xrizer log. Failures win over a stale success because the Player.log is
/// the game's own, so it is the newer evidence of what the game saw.
pub fn parse_last_run(player_log: Option<&str>, xrizer_log: Option<&str>) -> LastRun {
    if let Some(p) = player_log {
        if p.contains("InterfaceNotFound") {
            return LastRun::InterfaceNotFound;
        }
        if p.contains("RUNTIME_UNAVAILABLE") {
            return LastRun::RuntimeUnavailable;
        }
    }
    if let Some(x) = xrizer_log {
        if x.contains("session state changed: FOCUSED") {
            return LastRun::Focused;
        }
        if x.contains("session state changed") {
            return LastRun::SessionNoFocus;
        }
    }
    LastRun::Unknown
}

/// The check for the last run; `age` is words like "12 min ago".
pub fn last_run_check(l: &LastRun, name: &str, age: &str, config_ok: bool) -> Check {
    let (status, text, fix) = match l {
        LastRun::InterfaceNotFound => (
            Status::Fail,
            "OpenVR failed with InterfaceNotFound: the game did not load xrizer (SteamVR was the OpenVR runtime)",
            "Fix the FAIL lines above, then start the game again.",
        ),
        LastRun::RuntimeUnavailable => (
            Status::Fail,
            "OpenXR said RUNTIME_UNAVAILABLE: the game container could not reach Monado",
            "Fix the FAIL lines above (the launch option), then start the game again.",
        ),
        LastRun::Focused => (Status::Pass, "the VR session reached FOCUSED (the game was shown in the headset)", ""),
        LastRun::SessionNoFocus => (
            Status::Warn,
            "xrizer started a VR session but it never reached FOCUSED (the headset may not have been worn, or the game did not render)",
            "Wear the headset and start the game again.",
        ),
        LastRun::Unknown => (Status::Skip, "no VR result found in the logs", ""),
    };
    let mut c = check("last-run", status, format!("Last run of {name} ({age}): {text}"));
    if status == Status::Fail && config_ok {
        // the settings above are fine now: this failure is from before the fix
        c.status = Status::Warn;
        c.title.push_str(". The settings above look right now, so this was probably before the fix");
        c.fix = "Start the game once more and run this check again.".into();
    } else {
        c.fix = fix.into();
    }
    c
}

/// The container test verdict from the two probe results (`None` = not run).
pub fn container_check(rt: &str, without: Option<&str>, with: Option<&str>) -> Check {
    let ok = |s: Option<&str>| s.is_some_and(|l| l.starts_with("XR_PROBE OK"));
    let show = |s: Option<&str>| s.map(|l| l.trim().trim_start_matches("XR_PROBE ").to_string()).unwrap_or_else(|| "not run".into());
    let head = format!("Game container ({rt}): without {PV_VAR} {}; with it {}", if ok(without) { "works" } else { "FAILS" }, if ok(with) { "works" } else { "FAILS" });
    let detail = format!(" [without: {}; with: {}]", show(without), show(with));
    let no_loader = |s: Option<&str>| s.is_some_and(|l| l.contains("no OpenXR loader in this container"));
    if no_loader(without) && no_loader(with) {
        return check("container", Status::Skip, format!("Game container ({rt}): has no OpenXR loader, so OpenXR cannot be tested in it (games that bring their own loader are not covered)"));
    }
    match (ok(without), ok(with)) {
        (true, _) => check("container", Status::Pass, format!("{head}{detail}")),
        (false, true) => check(
            "container",
            Status::Warn,
            format!("{head}: games need the variable, so it must be in Steam's environment or the game's Launch Options{detail}"),
        ),
        (false, false) => check(
            "container",
            Status::Fail,
            format!("{head}: the container cannot reach Monado even with it{detail}"),
        )
        .fix("Start VR from the app and check the service and OpenXR lines above."),
    }
}

// ---------------------------------------------------------------------------------------------------------------
// the machine

/// The probe client source: loads the system OpenXR loader and does `xrCreateInstance` + `xrGetSystem`.
pub const PROBE_C: &str = r#"#include <dlfcn.h>
#include <stdio.h>
#include <string.h>
#define XR_NO_PROTOTYPES
#include <openxr/openxr.h>
int main(void) {
  void *h = dlopen("libopenxr_loader.so.1", RTLD_NOW);
  if (!h) { printf("XR_PROBE FAIL no OpenXR loader in this container (%s)\n", dlerror()); return 2; }
  PFN_xrCreateInstance ci = (PFN_xrCreateInstance)dlsym(h, "xrCreateInstance");
  PFN_xrGetSystem gs = (PFN_xrGetSystem)dlsym(h, "xrGetSystem");
  PFN_xrDestroyInstance di = (PFN_xrDestroyInstance)dlsym(h, "xrDestroyInstance");
  if (!ci || !gs) { printf("XR_PROBE FAIL loader without xrCreateInstance\n"); return 2; }
  XrInstanceCreateInfo c; memset(&c, 0, sizeof c);
  c.type = XR_TYPE_INSTANCE_CREATE_INFO;
  strcpy(c.applicationInfo.applicationName, "pimax-game-check");
  c.applicationInfo.apiVersion = XR_MAKE_VERSION(1, 0, 0);
  XrInstance inst;
  XrResult r = ci(&c, &inst);
  if (r != XR_SUCCESS) { printf("XR_PROBE FAIL xrCreateInstance returned %d%s\n", (int)r, r == XR_ERROR_RUNTIME_UNAVAILABLE ? " (RUNTIME_UNAVAILABLE)" : ""); return 3; }
  XrSystemGetInfo s; memset(&s, 0, sizeof s);
  s.type = XR_TYPE_SYSTEM_GET_INFO; s.formFactor = XR_FORM_FACTOR_HEAD_MOUNTED_DISPLAY;
  XrSystemId id;
  r = gs(inst, &s, &id);
  if (di) di(inst);
  if (r != XR_SUCCESS) { printf("XR_PROBE FAIL xrGetSystem returned %d\n", (int)r); return 4; }
  printf("XR_PROBE OK system %llu\n", (unsigned long long)id);
  return 0;
}
"#;

/// `~/.local/share/omarchyvr/bin/xr-probe`.
pub fn probe_path(home: &Path) -> PathBuf {
    home.join(".local/share/omarchyvr/bin/xr-probe")
}

/// Build the probe once (needs `cc` and the OpenXR headers). Returns its path or why not.
pub fn ensure_probe(home: &Path) -> Result<PathBuf, String> {
    let bin = probe_path(home);
    if bin.exists() {
        return Ok(bin);
    }
    let dir = bin.parent().ok_or("no dir")?;
    std::fs::create_dir_all(dir).map_err(|e| format!("cannot create {}: {e}", dir.display()))?;
    let src = dir.join("xr-probe.c");
    std::fs::write(&src, PROBE_C).map_err(|e| e.to_string())?;
    let out = Command::new("nice")
        .args(["-n", "19", "cc", "-O1", "-o"])
        .arg(&bin)
        .arg(&src)
        .arg("-ldl")
        .output()
        .map_err(|e| format!("cc not available: {e}"))?;
    if !out.status.success() {
        return Err(format!("cc failed (OpenXR headers missing?): {}", String::from_utf8_lossy(&out.stderr).lines().next().unwrap_or("")));
    }
    Ok(bin)
}

/// Run `cmd` for at most `secs`; stdout text. A timed-out child is killed by PID.
fn run_timeout(mut cmd: Command, secs: u64) -> Result<String, String> {
    cmd.stdout(Stdio::piped()).stderr(Stdio::null()).stdin(Stdio::null());
    let mut child = cmd.spawn().map_err(|e| e.to_string())?;
    let mut out = child.stdout.take().ok_or("no stdout")?;
    let reader = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut out, &mut s);
        s
    });
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(_)) => break,
            Ok(None) if t0.elapsed() > Duration::from_secs(secs) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = reader.join();
                return Err(format!("timed out after {secs} s"));
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(100)),
            Err(e) => return Err(e.to_string()),
        }
    }
    reader.join().map_err(|_| "reader panicked".to_string())
}

/// The `XR_PROBE ...` line of a probe run inside Steam's runtime `rt_dir` (with or without the variable).
pub fn run_in_container(rt_dir: &Path, probe: &Path, with_var: bool) -> String {
    let entry = rt_dir.join("_v2-entry-point");
    let mut c = Command::new("nice");
    c.args(["-n", "19"]).arg(&entry).arg("--verb=run").arg("--").arg(probe);
    if with_var {
        c.env(PV_VAR, "1");
    } else {
        c.env_remove(PV_VAR);
    }
    match run_timeout(c, 120) {
        Ok(out) => out
            .lines()
            .find(|l| l.starts_with("XR_PROBE"))
            .map(String::from)
            .unwrap_or_else(|| "XR_PROBE FAIL the probe printed nothing (container did not start it)".into()),
        Err(e) => format!("XR_PROBE FAIL {e}"),
    }
}

/// Steam library runtimes used by Proton Experimental (`SteamLinuxRuntime_4`) and Proton 9/10 (`sniper`).
pub fn steam_runtimes(steam_root: &Path) -> Vec<(String, PathBuf)> {
    ["SteamLinuxRuntime_4", "SteamLinuxRuntime_sniper"]
        .iter()
        .map(|n| (n.to_string(), steam_root.join("steamapps/common").join(n)))
        .filter(|(_, p)| p.join("_v2-entry-point").exists())
        .collect()
}

fn pids_named(name: &str) -> Vec<u32> {
    let uid = unsafe { libc::getuid() };
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir("/proc") {
        for e in rd.flatten() {
            let Some(pid) = e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else { continue };
            use std::os::unix::fs::MetadataExt;
            if std::fs::metadata(e.path()).map(|m| m.uid()).ok() != Some(uid) {
                continue;
            }
            if std::fs::read_to_string(e.path().join("comm")).ok().as_deref().map(str::trim) == Some(name) {
                out.push(pid);
            }
        }
    }
    out.sort();
    out
}

fn age_words(p: &Path) -> String {
    let Some(secs) = std::fs::metadata(p).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).map(|d| d.as_secs()) else { return "time unknown".into() };
    match secs {
        0..=89 => format!("{secs} s ago"),
        90..=5399 => format!("{} min ago", secs / 60),
        5400..=129_599 => format!("{} h ago", secs / 3600),
        _ => format!("{} days ago", secs / 86400),
    }
}

/// Player.log of a game: Proton prefix (`compatdata/<id>/pfx/drive_c/users/steamuser/AppData/LocalLow/*/*/Player.log`).
pub fn find_player_log(steam_root: &Path, appid: &str) -> Option<PathBuf> {
    let low = steam_root.join("steamapps/compatdata").join(appid).join("pfx/drive_c/users/steamuser/AppData/LocalLow");
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for company in std::fs::read_dir(&low).ok()?.flatten() {
        for game in std::fs::read_dir(company.path()).ok()?.flatten() {
            let p = game.path().join("Player.log");
            if let Ok(t) = std::fs::metadata(&p).and_then(|m| m.modified()) {
                if best.as_ref().is_none_or(|(b, _)| t > *b) {
                    best = Some((t, p));
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Inputs of a run (overridable for tests of the app).
pub struct Options {
    /// `$HOME`.
    pub home: PathBuf,
    /// Steam root (`~/.local/share/Steam`).
    pub steam_root: PathBuf,
    /// Game to look at (`None` = the last played one).
    pub appid: Option<String>,
    /// Skip the container tests (they take a few seconds and start Steam's runtime).
    pub no_container: bool,
    /// The launch options line shown as the fix.
    pub options_line: String,
    /// "Games run with: SteamVR" is chosen: every line is judged against SteamVR instead of Monado.
    pub steamvr: bool,
}

/// Is SteamVR the saved choice (games.toml, else auto mode, else the default)?
pub fn steamvr_chosen() -> bool {
    super::steamvr::games_runtime(&|k| std::env::var(k).ok()) == super::steamvr::Games::SteamVr
}

/// The default Monado build (env override, else `~/src/monado-eye/build`).
fn service_build(_home: &Path) -> PathBuf {
    super::config::default_monado_build(&|k| std::env::var(k).ok())
}

/// All checks on this machine.
pub fn run_checks(o: &Options) -> Vec<Check> {
    let mut out = vec![];
    let home = &o.home;
    let uid = unsafe { libc::getuid() };
    let xdg = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| format!("/run/user/{uid}"));

    // monado-service
    let svc = pids_named("monado-service").first().copied();
    let sock = Path::new(&xdg).join("monado_comp_ipc");
    let sock_ok = std::fs::metadata(&sock).map(|m| std::os::unix::fs::FileTypeExt::is_socket(&m.file_type())).unwrap_or(false);
    // /proc/PID/exe is unreadable for a service with file capabilities (cap_sys_nice): use argv[0]
    let exe = svc.and_then(|p| {
        std::fs::canonicalize(format!("/proc/{p}/exe")).ok().or_else(|| {
            let raw = std::fs::read(format!("/proc/{p}/cmdline")).ok()?;
            let a0 = String::from_utf8_lossy(raw.split(|b| *b == 0).next()?).to_string();
            std::fs::canonicalize(a0).ok()
        })
    });
    let service_fix = "Press Play / Start VR in the app (it starts monado-service).";
    let vrserver = pids_named("vrserver").first().copied();
    let vrcomp = pids_named("vrcompositor").first().copied();
    let work = if o.steamvr { work_mode_check(svc, vrserver) } else { None };
    let svc_check = if o.steamvr { Some(work.clone().unwrap_or_else(|| steamvr_service_check(vrserver, vrcomp))) } else { None };
    out.push(if let Some(c) = svc_check { c } else { match (svc, sock_ok) {
        (Some(p), true) => check("service", Status::Pass, format!("monado-service is running (pid {p}) and its socket {} exists", sock.display())),
        (Some(p), false) => check("service", Status::Fail, format!("monado-service runs (pid {p}) but its socket {} does not exist (still starting, or it failed)", sock.display())).fix("Wait a few seconds and check again; if it stays, restart VR from the app."),
        (None, _) => check("service", Status::Fail, "monado-service is not running: games find no VR runtime").fix(service_fix),
    } });

    // OpenXR runtime
    let link = home.join(".config/openxr/1/active_runtime.json");
    let manifest_text = std::fs::read_to_string(&link).ok();
    let lib = manifest_text.as_deref().and_then(manifest_library).map(|l| {
        // relative library paths are relative to the manifest
        let p = PathBuf::from(&l);
        let p = if p.is_absolute() { p } else { std::fs::canonicalize(&link).ok().and_then(|m| m.parent().map(|d| d.join(&p))).unwrap_or(p) };
        std::fs::canonicalize(&p).unwrap_or(p)
    });
    let root = exe.as_deref().and_then(build_root_of_service).map(|r| std::fs::canonicalize(&r).unwrap_or(r));
    let link_name = manifest_text.is_some().then(|| link.display().to_string());
    out.push(if o.steamvr {
        openxr_check_steamvr(link_name.as_deref(), manifest_text.as_deref(), lib.as_deref(), lib.as_ref().is_some_and(|l| l.exists()))
    } else {
        openxr_check(link_name.as_deref(), lib.as_deref(), lib.as_ref().is_some_and(|l| l.exists()), root.as_deref())
    });

    // OpenVR runtime
    let vrp = home.join(".config/openvr/openvrpaths.vrpath");
    let ro = std::fs::metadata(&vrp).map(|m| m.permissions().readonly()).unwrap_or(false);
    let xrizer = omarchyvr_config::paths::src_dir(&home).join("xrizer/target/release");
    out.push(if o.steamvr {
        openvr_check_steamvr(std::fs::read_to_string(&vrp).ok().as_deref(), &super::steamvr::driver_dir(&service_build(home)).display().to_string())
    } else {
        openvr_check(std::fs::read_to_string(&vrp).ok().as_deref(), ro, !pids_named("vrserver").is_empty(), &xrizer.display().to_string())
    });

    // container tests
    let mut without_ok: Option<bool> = None;
    let rts = steam_runtimes(&o.steam_root);
    if o.no_container {
        out.push(check("container", Status::Skip, "Game container test: skipped (--no-container)"));
    } else if rts.is_empty() {
        out.push(check("container", Status::Skip, "Game container test: no SteamLinuxRuntime_4 / sniper found in the Steam library"));
    } else if work.is_some() {
        out.push(check("container", Status::Skip, "Game container test: skipped, Work is running and games run with SteamVR (it runs when SteamVR runs)"));
    } else if o.steamvr && (vrserver.is_none() || vrcomp.is_none()) {
        out.push(check("container", Status::Skip, "Game container test: skipped, SteamVR is not running (start VR first)"));
    } else if !o.steamvr && (svc.is_none() || !sock_ok) {
        out.push(check("container", Status::Skip, "Game container test: skipped, Monado is not running (start VR first)"));
    } else {
        match ensure_probe(home) {
            Err(e) => out.push(check("container", Status::Skip, format!("Game container test: the test client could not be built: {e}"))),
            Ok(probe) => {
                for (name, dir) in &rts {
                    let a = run_in_container(dir, &probe, false);
                    let b = run_in_container(dir, &probe, true);
                    if name == "SteamLinuxRuntime_4" || without_ok.is_none() {
                        without_ok = Some(a.starts_with("XR_PROBE OK"));
                    }
                    out.push(container_check(name, Some(&a), Some(&b)));
                }
            }
        }
    }
    out.push(check(
        "container-openvr",
        Status::Skip,
        "Game container test for OpenVR through xrizer: not run (the OpenXR test and the last-run log below cover it)",
    ));

    // Steam env + launch options
    let steam_env = pids_named("steam").first().and_then(|p| std::fs::read(format!("/proc/{p}/environ")).ok()).map(|e| environ_has_pv(&e));
    let (vdf, vdf_path) = std::fs::read_dir(o.steam_root.join("userdata"))
        .ok()
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|u| {
            let p = u.path().join("config/localconfig.vdf");
            let v = parse_vdf(&std::fs::read_to_string(&p).ok()?)?;
            Some((v, p))
        })
        .max_by_key(|(_, p)| std::fs::metadata(p).and_then(|m| m.modified()).ok())
        .map(|(v, p)| (Some(v), Some(p)))
        .unwrap_or((None, None));
    let _ = vdf_path;
    let appid = o.appid.clone().or_else(|| {
        let installed: Vec<String> = std::fs::read_dir(o.steam_root.join("steamapps/compatdata")).ok()?.flatten().filter_map(|e| e.file_name().into_string().ok()).collect();
        vdf.as_ref().and_then(|v| last_played(v, &installed))
    });
    let launch = appid.as_deref().and_then(|id| vdf.as_ref().and_then(|v| launch_options(v, id)));
    out.push(steam_env_check(steam_env, launch.as_deref(), appid.as_deref(), without_ok, &o.options_line));

    // last run
    let config_ok = out.iter().filter(|c| matches!(c.id, "openxr" | "openvr" | "service" | "steam-env")).all(|c| c.status != Status::Fail);
    let player = appid.as_deref().and_then(|id| find_player_log(&o.steam_root, id));
    let xlog = home.join(".local/state/xrizer/xrizer.txt");
    // only the tail: the log appends over days
    let tail = |p: &Path| std::fs::read_to_string(p).ok().map(|t| t.lines().rev().take(400).collect::<Vec<_>>().join("\n"));
    let ptext = player.as_deref().and_then(|p| std::fs::read_to_string(p).ok());
    let xtext = tail(&xlog);
    let lr = parse_last_run(ptext.as_deref(), xtext.as_deref());
    let src = player.as_deref().filter(|_| ptext.is_some()).unwrap_or(&xlog);
    let name = appid.as_deref().map(|id| format!("app {id}")).unwrap_or_else(|| "the last game".into());
    out.push(last_run_check(&lr, &name, &age_words(src), config_ok));
    out
}

/// `omarchyvr vr game-check`.
pub fn run(a: &mut crate::args::Args, line: String) -> Result<(), crate::CliError> {
    let json = a.flag("--json");
    let no_container = a.flag("--no-container");
    let appid = a.opt("--appid")?;
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let steam_root = home.join(".local/share/Steam");
    let checks = run_checks(&Options { home, steam_root, appid, no_container, options_line: line, steamvr: steamvr_chosen() });
    if json {
        println!("{}", to_json(&checks));
    } else {
        print!("{}", to_text(&checks));
    }
    if checks.iter().any(|c| c.status == Status::Fail) {
        Err(crate::CliError { msg: String::new(), code: 1 })
    } else {
        Ok(())
    }
}

/// A canned line for the app's demo mode / screenshots.
pub fn demo_check() -> Check {
    check("steam-env", Status::Fail, "Steam env: PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 is not set: the game's launch options are empty. Games run in a container without Monado's socket")
        .fix("Steam needs the VR setting: open the game's Properties > General > Launch Options and paste the line below (Copy button).")
        .copy("PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%")
}

#[cfg(test)]
mod tests {
    use super::*;

    const XRIZER_PATHS: &str = r#"{ "jsonid": "vrpathreg", "runtime": ["/home/u/src/xrizer/target/release"], "version": 1 }"#;
    const STEAMVR_PATHS: &str = r#"{ "config": ["/h/.local/share/Steam/config"], "jsonid": "vrpathreg",
        "runtime": ["/home/u/.local/share/Steam/steamapps/common/SteamVR/", "/home/u/src/xrizer/target/release"], "version": 1 }"#;
    const LOCALCONFIG: &str = r#""UserLocalConfigStore"
{
	"Software"
	{
		"Valve"
		{
			"Steam"
			{
				"apps"
				{
					"250820"
					{
						"LastPlayed"		"1700000000"
						"LaunchOptions"		"PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%"
					}
					"3189340"
					{
						"LastPlayed"		"1792052417"
						"cloud" { "last_sync_state" "synchronized" }
						"LaunchOptions"		""
					}
				}
			}
		}
	}
	"WebStorage" { "x" "{\"a\":1} \\ end" }
}"#;
    const PLAYER_IFACE: &str = "[XR] [OpenVR] [ERROR] VR_Init initError: VRInitError_Init_InterfaceNotFound\n";
    const PLAYER_RT: &str = "[XRStartup] flag: none | active OpenXR runtime: C:\\openxr\\wineopenxr64.json -> OpenVR\n[FAILURE] xrCreateInstance: XR_ERROR_RUNTIME_UNAVAILABLE (1x)\n";
    const XRIZER_OK: &str = "[INFO] OpenXR session state changed: READY\n[INFO] OpenXR session state changed: FOCUSED\n";
    const XRIZER_NOFOCUS: &str = "[INFO] OpenXR session state changed: READY\n[INFO] OpenXR session state changed: EXITING\n";

    #[test]
    fn vrpath_first_entry_is_classified() {
        assert_eq!(classify_openvr(&vrpath_first(XRIZER_PATHS).unwrap()), OpenVr::Xrizer);
        let f = vrpath_first(STEAMVR_PATHS).unwrap();
        assert_eq!(classify_openvr(&f), OpenVr::SteamVr);
        assert!(!f.ends_with('/'));
        assert_eq!(vrpath_first("not json"), None);
    }

    #[test]
    fn openvr_check_names_steamvr_as_the_one_who_changed_it() {
        let c = openvr_check(Some(STEAMVR_PATHS), true, true, "/x/xrizer");
        assert_eq!(c.status, Status::Fail);
        assert!(c.title.contains("SteamVR rewrites this file"), "{}", c.title);
        assert!(c.title.contains("running right now"));
        assert!(c.fix.contains("xrizer"));
        let ok = openvr_check(Some(XRIZER_PATHS), true, false, "/x");
        assert_eq!(ok.status, Status::Pass);
        let rw = openvr_check(Some(XRIZER_PATHS), false, false, "/x");
        assert_eq!(rw.status, Status::Pass);
        assert!(rw.title.contains("writable"));
        assert_eq!(openvr_check(None, false, false, "/x").status, Status::Fail);
        assert_eq!(openvr_check(Some("{}"), true, false, "/x").status, Status::Fail);
    }

    #[test]
    fn steamvr_mode_judges_every_line_against_steamvr() {
        // SteamVR first + our driver registered
        let ok = r#"{"runtime":["/h/.local/share/Steam/steamapps/common/SteamVR/"],"external_drivers":["/h/src/monado-eye/build/steamvr-monado"]}"#;
        assert_eq!(openvr_check_steamvr(Some(ok), "/h/src/monado-eye/build/steamvr-monado").status, Status::Pass);
        // no driver registered
        let nodrv = r#"{"runtime":["/h/.local/share/Steam/steamapps/common/SteamVR/"]}"#;
        let c = openvr_check_steamvr(Some(nodrv), "/h/b/steamvr-monado");
        assert_eq!(c.status, Status::Fail);
        assert!(c.title.contains("driver_monado"));
        // xrizer first is wrong in SteamVR mode (and SteamVR first is wrong in Monado mode: the old check is unchanged)
        assert_eq!(openvr_check_steamvr(Some(XRIZER_PATHS), "/x").status, Status::Fail);
        assert_eq!(openvr_check_steamvr(None, "/x").status, Status::Fail);
        // service line
        assert_eq!(steamvr_service_check(Some(10), Some(11)).status, Status::Pass);
        assert!(steamvr_service_check(Some(10), None).title.contains("vrcompositor"));
        let none = steamvr_service_check(None, None);
        assert_eq!(none.status, Status::Fail);
        assert!(!none.title.contains("monado-service") && none.title.contains("SteamVR"));
        // OpenXR line
        let man = r#"{"runtime":{"name":"SteamVR","VALVE_runtime_is_steamvr":true,"library_path":"bin/linux64/vrclient.so"}}"#;
        let lib = Path::new("/h/.local/share/Steam/steamapps/common/SteamVR/bin/linux64/vrclient.so");
        let m = Some("/h/.config/openxr/1/active_runtime.json");
        assert_eq!(openxr_check_steamvr(m, Some(man), Some(lib), true).status, Status::Pass);
        let mon = Path::new("/h/monado/build/src/x/libopenxr_monado.so");
        let c = openxr_check_steamvr(m, Some("{}"), Some(mon), true);
        assert_eq!(c.status, Status::Fail);
        assert!(c.title.contains("find no runtime"));
        assert_eq!(openxr_check_steamvr(None, None, None, false).status, Status::Fail);
    }

    #[test]
    fn work_mode_with_steamvr_games_is_a_warning_not_a_failure() {
        let c = work_mode_check(Some(126355), None).unwrap();
        assert_eq!((c.id, c.status), ("service", Status::Warn));
        assert!(c.title.contains("Work is running") && c.title.contains("126355"), "{}", c.title);
        assert!(c.fix.contains("Play"), "{}", c.fix);
        // SteamVR up, or no Monado: the normal SteamVR checks apply
        assert!(work_mode_check(Some(1), Some(2)).is_none());
        assert!(work_mode_check(None, None).is_none());
        // and the OpenXR check passes on SteamVR's runtime while Work runs (Work no longer relinks it)
        let lib = Path::new("/h/.local/share/Steam/steamapps/common/SteamVR/bin/linux64/vrclient.so");
        assert_eq!(openxr_check_steamvr(Some("/h/.config/openxr/1/active_runtime.json"), None, Some(lib), true).status, Status::Pass);
    }

    #[test]
    fn openxr_check_compares_with_the_running_build() {
        let root = Path::new("/h/monado/build");
        let m = Some("/h/.config/openxr/1/active_runtime.json");
        let good = openxr_check(m, Some(Path::new("/h/monado/build/src/xrt/targets/openxr/libopenxr_monado.so")), true, Some(root));
        assert_eq!(good.status, Status::Pass);
        let other = openxr_check(m, Some(Path::new("/h/monado/build-old/src/x.so")), true, Some(root));
        assert_eq!(other.status, Status::Fail);
        assert!(other.title.contains("build-old"));
        assert_eq!(openxr_check(m, Some(Path::new("/gone.so")), false, Some(root)).status, Status::Fail);
        assert_eq!(openxr_check(None, None, false, None).status, Status::Fail);
        assert_eq!(openxr_check(m, None, false, None).status, Status::Fail);
        assert_eq!(openxr_check(m, Some(Path::new("/a/b.so")), true, None).status, Status::Pass);
        // prefix must be a whole path component: build-old is not inside build
        assert!(!Path::new("/h/monado/build-old/x").starts_with(root));
    }

    #[test]
    fn manifest_and_service_paths() {
        assert_eq!(manifest_library(r#"{"runtime":{"library_path":"/a/b.so"}}"#).as_deref(), Some("/a/b.so"));
        assert_eq!(manifest_library("{}"), None);
        assert_eq!(
            build_root_of_service(Path::new("/h/monado-eye/build/src/xrt/targets/service/monado-service")),
            Some(PathBuf::from("/h/monado-eye/build"))
        );
        assert_eq!(build_root_of_service(Path::new("/usr/bin/monado-service")), None);
    }

    #[test]
    fn vdf_launch_options_and_last_played() {
        let v = parse_vdf(LOCALCONFIG).unwrap();
        assert_eq!(launch_options(&v, "3189340").as_deref(), Some(""));
        assert!(launch_options(&v, "250820").unwrap().contains(PV_VAR));
        assert_eq!(launch_options(&v, "999"), None);
        assert_eq!(last_played(&v, &[]).as_deref(), Some("3189340"));
        assert_eq!(last_played(&v, &["250820".into()]).as_deref(), Some("250820"));
        assert_eq!(last_played(&v, &["1".into()]), None);
        // escapes survive
        assert_eq!(v.path(&["UserLocalConfigStore", "WebStorage", "x"]).unwrap().val.as_deref(), Some("{\"a\":1} \\ end"));
        assert!(parse_vdf("\"a\" {").is_some()); // unterminated block is tolerated
        assert_eq!(parse_vdf("\"a\" \"b"), None); // unterminated string is not
    }

    #[test]
    fn environ_detection() {
        assert!(environ_has_pv(b"A=1\0PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1\0B=2\0"));
        assert!(!environ_has_pv(b"A=1\0PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=0\0"));
        assert!(!environ_has_pv(b"XPRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1\0"));
        assert!(!environ_has_pv(b""));
    }

    #[test]
    fn steam_env_check_needs_the_variable_somewhere() {
        let line = "PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%";
        let bad = steam_env_check(Some(false), Some(""), Some("3189340"), Some(false), line);
        assert_eq!(bad.status, Status::Fail);
        assert!(bad.fix.contains("Launch Options"));
        assert_eq!(bad.copy, line);
        assert!(bad.title.contains("launch options are empty"));
        assert_eq!(steam_env_check(Some(true), Some(""), Some("1"), None, line).status, Status::Pass);
        assert_eq!(steam_env_check(Some(false), Some("PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1 %command%"), Some("1"), None, line).status, Status::Pass);
        assert_eq!(steam_env_check(Some(false), Some(""), Some("1"), Some(true), line).status, Status::Pass);
        let none = steam_env_check(None, None, None, None, line);
        assert_eq!(none.status, Status::Fail);
        assert!(none.title.contains("Steam is not running"));
        assert!(steam_env_check(None, Some("-novr %command%"), Some("1"), None, line).title.contains("-novr"));
    }

    #[test]
    fn last_run_logs() {
        assert_eq!(parse_last_run(Some(PLAYER_IFACE), None), LastRun::InterfaceNotFound);
        assert_eq!(parse_last_run(Some(PLAYER_RT), Some(XRIZER_OK)), LastRun::RuntimeUnavailable);
        assert_eq!(parse_last_run(None, Some(XRIZER_OK)), LastRun::Focused);
        assert_eq!(parse_last_run(Some("fine\n"), Some(XRIZER_NOFOCUS)), LastRun::SessionNoFocus);
        assert_eq!(parse_last_run(None, None), LastRun::Unknown);
        let f = last_run_check(&LastRun::InterfaceNotFound, "app 1", "3 min ago", false);
        assert_eq!(f.status, Status::Fail);
        assert!(f.title.contains("InterfaceNotFound") && f.title.contains("3 min ago"));
        // settings fine now: the failure is old news
        let w = last_run_check(&LastRun::RuntimeUnavailable, "app 1", "2 days ago", true);
        assert_eq!(w.status, Status::Warn);
        assert_eq!(last_run_check(&LastRun::Focused, "x", "1 s ago", true).status, Status::Pass);
        assert_eq!(last_run_check(&LastRun::Unknown, "x", "1 s ago", true).status, Status::Skip);
    }

    #[test]
    fn container_verdicts() {
        let ok = "XR_PROBE OK system 1";
        let bad = "XR_PROBE FAIL xrCreateInstance returned -51 (RUNTIME_UNAVAILABLE)";
        assert_eq!(container_check("sniper", Some(ok), Some(ok)).status, Status::Pass);
        let nl = "XR_PROBE FAIL no OpenXR loader in this container (x)";
        assert_eq!(container_check("sniper", Some(nl), Some(nl)).status, Status::Skip);
        let need = container_check("rt4", Some(bad), Some(ok));
        assert_eq!(need.status, Status::Warn);
        assert!(need.title.contains("without PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES FAILS; with it works"), "{}", need.title);
        let dead = container_check("rt4", Some(bad), Some(bad));
        assert_eq!(dead.status, Status::Fail);
        assert!(!dead.fix.is_empty());
    }

    #[test]
    fn json_round_trip_and_text() {
        let cs = vec![
            check("openvr", Status::Fail, "t").fix("f").copy("c"),
            check("service", Status::Pass, "s"),
        ];
        let j = to_json(&cs);
        assert!(j.contains("\"ok\": false"));
        assert_eq!(from_json(&j).unwrap(), cs);
        let t = to_text(&cs);
        assert!(t.starts_with("FAIL  t\n      fix: f\n      paste: c\nPASS  s\n"), "{t}");
        assert!(to_json(&cs[1..]).contains("\"ok\": true"));
        assert_eq!(from_json("nope"), None);
    }

    #[test]
    fn steam_runtimes_need_the_entry_point() {
        let d = crate::vr::test_dir("gc-rt");
        let rt = d.join("steamapps/common/SteamLinuxRuntime_4");
        std::fs::create_dir_all(&rt).unwrap();
        assert!(steam_runtimes(&d).is_empty());
        std::fs::write(rt.join("_v2-entry-point"), "").unwrap();
        assert_eq!(steam_runtimes(&d).len(), 1);
        std::fs::remove_dir_all(&d).ok();
    }

    #[test]
    fn player_log_is_found_in_the_prefix() {
        let d = crate::vr::test_dir("gc-pl");
        let g = d.join("steamapps/compatdata/42/pfx/drive_c/users/steamuser/AppData/LocalLow/Co/Game");
        std::fs::create_dir_all(&g).unwrap();
        assert_eq!(find_player_log(&d, "42"), None);
        std::fs::write(g.join("Player.log"), PLAYER_IFACE).unwrap();
        assert_eq!(find_player_log(&d, "42"), Some(g.join("Player.log")));
        assert_eq!(find_player_log(&d, "43"), None);
        std::fs::remove_dir_all(&d).ok();
    }

    /// Needs Steam, a running Monado and the probe built: `cargo test -p omarchyvr-tools game_check -- --ignored`.
    #[test]
    #[ignore]
    fn live_container_test() {
        let home = PathBuf::from(std::env::var("HOME").unwrap());
        let probe = ensure_probe(&home).expect("probe builds");
        for (name, dir) in steam_runtimes(&home.join(".local/share/Steam")) {
            let with = run_in_container(&dir, &probe, true);
            let without = run_in_container(&dir, &probe, false);
            println!("{name}: without={without} | with={with}");
            assert!(with.starts_with("XR_PROBE OK") || with.contains("no OpenXR loader"), "{name} with the variable: {with}");
        }
    }
}
