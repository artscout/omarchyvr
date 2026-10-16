//! Which OpenXR runtime a panel would open.
//!
//! Live failure 2026-10-16: during a Work session `omarchyvr-roommap setup fast` started its panel without
//! `XR_RUNTIME_JSON`; the OpenXR loader followed `~/.config/openxr/1/active_runtime.json`, which pointed at SteamVR
//! (the games choice). SteamVR started, its Monado driver opened the Pimax headset a second time beside the running
//! monado-service (camera, controllers, the room anchor's head ring) and the room setup lost its head poses. While a
//! monado-service runs, a panel goes to that Monado or not at all.

use std::path::{Path, PathBuf};

/// Pids of the running `monado-service` processes.
pub fn monado_service_pids() -> Vec<u32> {
    let mut v: Vec<u32> = std::fs::read_dir("/proc")
        .map(|rd| {
            rd.filter_map(|e| e.ok())
                .filter_map(|e| e.file_name().to_str().and_then(|s| s.parse::<u32>().ok()))
                .filter(|pid| std::fs::read_to_string(format!("/proc/{pid}/comm")).is_ok_and(|c| c.trim() == "monado-service"))
                .collect()
        })
        .unwrap_or_default();
    v.sort_unstable();
    v
}

/// The runtime JSON of a Monado build from its monado-service path (`<build>/src/xrt/targets/service/monado-service`
/// -> `<build>/openxr_monado-dev.json`).
pub fn json_for_service(exe: &Path) -> Option<PathBuf> {
    let build = exe.parent()?.parent()?.parent()?.parent()?.parent()?;
    let tail: PathBuf = exe.strip_prefix(build).ok()?.into();
    (tail == Path::new("src/xrt/targets/service/monado-service")).then(|| build.join("openxr_monado-dev.json"))
}

/// The runtime JSON of the running monado-service (its build's `openxr_monado-dev.json`), when it exists.
pub fn running_monado_json() -> Option<PathBuf> {
    monado_service_pids().into_iter().find_map(|pid| {
        let cmd = std::fs::read(format!("/proc/{pid}/cmdline")).ok()?;
        let arg0 = cmd.split(|b| *b == 0).next()?;
        let exe = PathBuf::from(String::from_utf8_lossy(arg0).into_owned());
        json_for_service(&exe).filter(|j| j.is_file())
    })
}

/// The runtime JSON the OpenXR loader picks: `XR_RUNTIME_JSON`, else the user's then the system's active runtime.
pub fn effective_json() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("XR_RUNTIME_JSON").filter(|v| !v.is_empty()) {
        return Some(PathBuf::from(p));
    }
    let cfg = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")));
    let mut cands: Vec<PathBuf> = cfg.into_iter().map(|c| c.join("openxr/1/active_runtime.json")).collect();
    cands.push(PathBuf::from("/etc/xdg/openxr/1/active_runtime.json"));
    cands.push(PathBuf::from("/etc/openxr/1/active_runtime.json"));
    cands.into_iter().find(|p| p.exists())
}

/// Is this runtime JSON text Monado's?
pub fn is_monado_json(text: &str) -> bool {
    text.contains("libopenxr_monado") || text.contains("\"Monado\"")
}

/// Why opening a panel now would go to the wrong runtime (None: fine). `monado_running` and the JSON text are
/// passed in so tests need no processes.
pub fn problem(monado_running: bool, json: Option<&Path>, json_text: Option<&str>) -> Option<String> {
    if !monado_running {
        return None;
    }
    let text = json_text?;
    if is_monado_json(text) {
        return None;
    }
    let name = if text.contains("VALVE_runtime_is_steamvr") || text.contains("SteamVR") { "SteamVR" } else { "another OpenXR runtime" };
    Some(format!(
        "Monado runs, but this program would open {name} ({}): that would start {name} beside Monado and open the headset twice; panel not started",
        json.map(|p| p.display().to_string()).unwrap_or_default()
    ))
}

/// Point this process at the running Monado when `XR_RUNTIME_JSON` does not already name a Monado runtime.
/// Call early, before other threads start (it sets an environment variable). Returns the JSON now used.
pub fn use_running_monado() -> Option<PathBuf> {
    let cur = std::env::var_os("XR_RUNTIME_JSON").filter(|v| !v.is_empty()).map(PathBuf::from);
    if let Some(c) = &cur {
        if std::fs::read_to_string(c).is_ok_and(|t| is_monado_json(&t)) {
            return cur;
        }
    }
    let j = running_monado_json()?;
    std::env::set_var("XR_RUNTIME_JSON", &j);
    Some(j)
}

/// The check `Hud::new` runs: Err when a monado-service runs and the loader would pick another runtime.
pub fn check() -> Result<(), String> {
    let running = !monado_service_pids().is_empty();
    if !running {
        return Ok(());
    }
    let j = effective_json();
    let text = j.as_ref().and_then(|p| std::fs::read_to_string(p).ok());
    match problem(running, j.as_deref(), text.as_deref()) {
        Some(e) => Err(e),
        None => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_next_to_the_service() {
        let exe = Path::new("/home/u/.local/lib/omarchyvr/monado/src/xrt/targets/service/monado-service");
        assert_eq!(json_for_service(exe), Some(PathBuf::from("/home/u/.local/lib/omarchyvr/monado/openxr_monado-dev.json")));
        assert_eq!(json_for_service(Path::new("/usr/bin/monado-service")), None);
    }

    #[test]
    fn steamvr_refused_while_monado_runs() {
        let steam = r#"{"runtime":{"name":"SteamVR","VALVE_runtime_is_steamvr":true,"library_path":"bin/linux64/vrclient.so"}}"#;
        let monado = r#"{"runtime":{"name":"Monado","library_path":"/b/src/xrt/targets/openxr/libopenxr_monado.so"}}"#;
        let p = Path::new("/home/u/.config/openxr/1/active_runtime.json");
        let e = problem(true, Some(p), Some(steam)).unwrap();
        assert!(e.contains("SteamVR") && e.contains("beside Monado"), "{e}");
        assert_eq!(problem(true, Some(p), Some(monado)), None);
        assert_eq!(problem(false, Some(p), Some(steam)), None, "no Monado: SteamVR is the user's choice");
        assert_eq!(problem(true, None, None), None, "no runtime at all: the loader reports it");
    }
}
