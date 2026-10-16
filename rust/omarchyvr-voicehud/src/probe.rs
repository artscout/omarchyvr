//! A cheap check for "Monado is obviously not running", so that the retry loop does not call into the OpenXR runtime
//! every 2 s: Monado's client library prints a ten-line error banner each time it cannot reach `monado-service`, and
//! the loader maps the library in and out. The check only says "down" when the active runtime is Monado (named in the
//! runtime manifest) and its IPC socket `$XDG_RUNTIME_DIR/monado_comp_ipc` does not exist; in every other case the
//! runtime is simply tried.

use serde_json::Value;
use std::path::{Path, PathBuf};

/// Socket file name of a default Monado build (the CMake variable `XRT_IPC_MSG_SOCK_FILENAME`).
pub const MONADO_SOCKET: &str = "monado_comp_ipc";

/// Environment variable that turns the check off (for a Monado built with another socket name).
pub const DISABLE_ENV: &str = "PIMAX_VOICE_HUD_NO_PROBE";

/// Where the OpenXR loader looks for the active runtime manifest, in order: `XR_RUNTIME_JSON` alone if set, else the
/// user config, then the system config directories.
pub fn manifest_candidates(
    runtime_json: Option<&str>,
    xdg_config_home: Option<&str>,
    home: Option<&str>,
    xdg_config_dirs: Option<&str>,
) -> Vec<PathBuf> {
    if let Some(p) = runtime_json.filter(|p| !p.is_empty()) {
        return vec![PathBuf::from(p)];
    }
    let rel = "openxr/1/active_runtime.json";
    let mut v = Vec::new();
    match (xdg_config_home.filter(|p| !p.is_empty()), home) {
        (Some(c), _) => v.push(Path::new(c).join(rel)),
        (None, Some(h)) if !h.is_empty() => v.push(Path::new(h).join(".config").join(rel)),
        _ => {}
    }
    let dirs = xdg_config_dirs
        .filter(|d| !d.is_empty())
        .unwrap_or("/etc/xdg");
    v.extend(
        dirs.split(':')
            .filter(|d| !d.is_empty())
            .map(|d| Path::new(d).join(rel)),
    );
    v.push(Path::new("/etc").join(rel));
    v
}

/// Does the runtime manifest name Monado (runtime name or library path)?
pub fn is_monado(manifest_json: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(manifest_json) else {
        return false;
    };
    let rt = &v["runtime"];
    ["name", "library_path"].iter().any(|k| {
        rt[*k]
            .as_str()
            .is_some_and(|s| s.to_ascii_lowercase().contains("monado"))
    })
}

pub fn monado_socket(xdg_runtime_dir: &Path) -> PathBuf {
    xdg_runtime_dir.join(MONADO_SOCKET)
}

/// The decision, with the manifest text, the runtime directory and the file check passed in.
pub fn down_reason(
    manifest: Option<&str>,
    xdg_runtime_dir: Option<&Path>,
    exists: &dyn Fn(&Path) -> bool,
) -> Option<String> {
    if !is_monado(manifest?) {
        return None;
    }
    let sock = monado_socket(xdg_runtime_dir.filter(|d| !d.as_os_str().is_empty())?);
    if exists(&sock) {
        None
    } else {
        Some(format!("Monado is not running (no {})", sock.display()))
    }
}

/// [`down_reason`] with the real environment and file system. `None` also when nothing is known.
pub fn runtime_down() -> Option<String> {
    if std::env::var_os(DISABLE_ENV).is_some() {
        return None;
    }
    let get = |k: &str| std::env::var(k).ok();
    let candidates = manifest_candidates(
        get("XR_RUNTIME_JSON").as_deref(),
        get("XDG_CONFIG_HOME").as_deref(),
        get("HOME").as_deref(),
        get("XDG_CONFIG_DIRS").as_deref(),
    );
    let manifest = candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok());
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from);
    down_reason(manifest.as_deref(), runtime_dir.as_deref(), &|p| p.exists())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MONADO: &str = r#"{
        "file_format_version": "1.0.0",
        "runtime": {
            "name": "Monado",
            "library_path": "/home/u/src/monado/build/src/xrt/targets/openxr/libopenxr_monado.so",
            "MND_libmonado_path": "/x/libmonado.so"
        }
    }"#;
    const STEAMVR: &str = r#"{"file_format_version":"1.0.0","runtime":{"library_path":"/home/u/.steam/steam/steamapps/common/SteamVR/bin/linux64/vrclient.so"}}"#;

    #[test]
    fn manifest_search_order() {
        // XR_RUNTIME_JSON alone
        assert_eq!(
            manifest_candidates(Some("/x/r.json"), Some("/c"), Some("/h"), None),
            [PathBuf::from("/x/r.json")]
        );
        // user config, then the config dirs, then /etc
        assert_eq!(
            manifest_candidates(None, Some("/c"), Some("/h"), Some("/d1:/d2")),
            [
                PathBuf::from("/c/openxr/1/active_runtime.json"),
                PathBuf::from("/d1/openxr/1/active_runtime.json"),
                PathBuf::from("/d2/openxr/1/active_runtime.json"),
                PathBuf::from("/etc/openxr/1/active_runtime.json"),
            ]
        );
        // no XDG_CONFIG_HOME: ~/.config; default config dir /etc/xdg; empty values count as unset
        assert_eq!(
            manifest_candidates(Some(""), Some(""), Some("/home/u"), Some("")),
            [
                PathBuf::from("/home/u/.config/openxr/1/active_runtime.json"),
                PathBuf::from("/etc/xdg/openxr/1/active_runtime.json"),
                PathBuf::from("/etc/openxr/1/active_runtime.json"),
            ]
        );
    }

    #[test]
    fn recognises_monado_manifests() {
        assert!(is_monado(MONADO));
        assert!(is_monado(
            r#"{"runtime":{"library_path":"/usr/lib/libopenxr_monado.so"}}"#
        ));
        assert!(is_monado(r#"{"runtime":{"name":"MONADO (custom)"}}"#));
        assert!(!is_monado(STEAMVR));
        assert!(!is_monado("not json"));
        assert!(!is_monado("{}"));
        assert!(!is_monado(r#"{"runtime":5}"#));
    }

    #[test]
    fn down_only_for_monado_without_its_socket() {
        let dir = Path::new("/run/user/1000");
        let absent = |_: &Path| false;
        let present = |p: &Path| p == Path::new("/run/user/1000/monado_comp_ipc");
        let why = down_reason(Some(MONADO), Some(dir), &absent).unwrap();
        assert_eq!(
            why,
            "Monado is not running (no /run/user/1000/monado_comp_ipc)"
        );
        // the socket is there: try the runtime
        assert_eq!(down_reason(Some(MONADO), Some(dir), &present), None);
        // another runtime, no manifest, no runtime dir: never "down", the runtime is tried
        assert_eq!(down_reason(Some(STEAMVR), Some(dir), &absent), None);
        assert_eq!(down_reason(None, Some(dir), &absent), None);
        assert_eq!(down_reason(Some(MONADO), None, &absent), None);
        assert_eq!(
            down_reason(Some(MONADO), Some(Path::new("")), &absent),
            None
        );
    }

    #[test]
    fn the_real_check_follows_the_environment() {
        // a manifest that does not exist: nothing known, nothing skipped (the environment of the test process is
        // not touched: the pure functions above cover the decision)
        let none = manifest_candidates(Some("/nonexistent/none.json"), None, None, None);
        assert!(none.iter().all(|p| std::fs::read_to_string(p).is_err()));
        assert_eq!(MONADO_SOCKET, "monado_comp_ipc");
        assert_eq!(
            monado_socket(Path::new("/run/user/7")),
            PathBuf::from("/run/user/7/monado_comp_ipc")
        );
    }
}
