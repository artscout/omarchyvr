//! `~/.config/openvr/openvrpaths.vrpath`: the OpenVR runtime Steam games (also Proton games) load. For Monado it must
//! list xrizer (LESSONS row 20). Starting SteamVR (Steam's SteamVR button, or a game that makes Steam start it)
//! rewrites the file to SteamVR, even when it is read-only (it is replaced, not written in place); the next OpenVR game
//! then fails with `VRInitError_Init_InterfaceNotFound` while Monado runs.
//!
//! `vr start` / `desk start` (unless `--no-set-runtime`, or `game_runtime = steamvr` in auto.toml) put xrizer back
//! with [`ensure_xrizer`]: the SteamVR version is kept as `openvrpaths.vrpath.pimax-steamvr`, the new file is
//! written atomically and made read-only (0444). Nothing happens when xrizer is not built.

use std::io;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

/// `$HOME/.config/openvr/openvrpaths.vrpath`.
pub fn vrpath(home: &Path) -> PathBuf {
    home.join(".config/openvr/openvrpaths.vrpath")
}

/// The xrizer runtime dir: `$XRIZER`, else `$HOME/src/xrizer/target/release` (as `vr_runtime_switch.sh`).
pub fn xrizer_dir(env: &dyn Fn(&str) -> Option<String>, home: &Path) -> PathBuf {
    env("XRIZER").filter(|v| !v.is_empty()).map(PathBuf::from).unwrap_or_else(|| pimax_config::paths::src_dir(&home).join("xrizer/target/release"))
}

/// True when `dir` holds an OpenVR runtime (`bin/linux64/vrclient.so`).
pub fn is_runtime(dir: &Path) -> bool {
    dir.join("bin/linux64/vrclient.so").exists()
}

/// The first `runtime` entry of a vrpath text (the one OpenVR uses).
pub fn first_runtime(text: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    v.get("runtime")?.as_array()?.first()?.as_str().map(|s| s.trim_end_matches('/').to_string())
}

/// The vrpath text that lists only `xrizer`.
pub fn xrizer_text(xrizer: &Path) -> String {
    let v = serde_json::json!({ "jsonid": "vrpathreg", "runtime": [xrizer.display().to_string()], "version": 1 });
    format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
}

/// Make the vrpath list only `xrizer` (read-only). Returns a log line when something changed; `None` when it already
/// did, or when `xrizer` is not a built OpenVR runtime.
pub fn ensure_xrizer(home: &Path, xrizer: &Path) -> io::Result<Option<String>> {
    if !is_runtime(xrizer) {
        return Ok(None);
    }
    let p = vrpath(home);
    let old = std::fs::read_to_string(&p).ok();
    let want = xrizer.display().to_string();
    let first = old.as_deref().and_then(first_runtime);
    let read_only = std::fs::metadata(&p).map(|m| m.permissions().mode() & 0o222 == 0).unwrap_or(false);
    let has_drivers = old.as_deref().is_some_and(|t| !external_drivers(t).is_empty());
    if first.as_deref() == Some(want.trim_end_matches('/')) && !has_drivers {
        if !read_only {
            std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o444))?;
            return Ok(Some(format!("{} made read-only", p.display())));
        }
        return Ok(None);
    }
    std::fs::create_dir_all(p.parent().unwrap())?;
    if let Some(text) = old.as_deref().filter(|t| first_runtime(t).is_some_and(|f| !f.contains("xrizer"))) {
        let bak = p.with_file_name(STEAMVR_BACKUP);
        let _ = std::fs::remove_file(&bak);
        std::fs::write(&bak, text)?;
    }
    let tmp = p.with_file_name(format!(".openvrpaths.vrpath.pimax-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, xrizer_text(xrizer))?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o444))?;
    if let Err(e) = std::fs::rename(&tmp, &p) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let was = match first {
        Some(f) => format!("was {f}; old file saved as openvrpaths.vrpath.pimax-steamvr"),
        None if old.is_some() => "had no runtime".to_string(),
        None => "was missing".to_string(),
    };
    Ok(Some(format!("{} -> xrizer {} ({was})", p.display(), xrizer.display())))
}

/// The SteamVR version of the file, kept when xrizer takes over.
pub const STEAMVR_BACKUP: &str = "openvrpaths.vrpath.pimax-steamvr";

/// `$HOME/.local/share/Steam/steamapps/common/SteamVR`.
pub fn steamvr_dir(home: &Path) -> PathBuf {
    home.join(".local/share/Steam/steamapps/common/SteamVR")
}

/// The `external_drivers` entries of a vrpath text (drivers registered with `vrpathreg adddriver`).
pub fn external_drivers(text: &str) -> Vec<String> {
    serde_json::from_str::<serde_json::Value>(text)
        .ok()
        .and_then(|v| v.get("external_drivers")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(|s| s.trim_end_matches('/').to_string()))
        .collect()
}

/// The vrpath text for SteamVR: `base` (SteamVR's own file, when we have it) with `runtime` = SteamVR first, and
/// `external_drivers` = exactly `driver` (our Monado driver for SteamVR) or none.
pub fn steamvr_text(base: Option<&str>, home: &Path, steamvr: &Path, driver: Option<&Path>) -> String {
    let mut v = base
        .and_then(|t| serde_json::from_str::<serde_json::Value>(t).ok())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| {
            let steam = home.join(".local/share/Steam");
            serde_json::json!({
                "config": [steam.join("config").display().to_string()],
                "log": [steam.join("logs").display().to_string()],
            })
        });
    let o = v.as_object_mut().unwrap();
    o.insert("jsonid".into(), "vrpathreg".into());
    o.insert("version".into(), 1.into());
    o.insert("runtime".into(), serde_json::json!([steamvr.display().to_string()]));
    o.insert(
        "external_drivers".into(),
        match driver {
            Some(d) => serde_json::json!([d.display().to_string()]),
            None => serde_json::Value::Null,
        },
    );
    format!("{}\n", serde_json::to_string_pretty(&v).unwrap())
}

/// Hand the file to SteamVR: SteamVR first, writable (0644: SteamVR 2.17 exits with "Saving Path Registry failed" on a
/// read-only file), our driver registered (or none). Works without any backup; never touches the OpenXR runtime.
/// A xrizer version is kept as `openvrpaths.vrpath.pimax-xrizer`. Returns a plain-words line.
pub fn ensure_steamvr(home: &Path, steamvr: &Path, driver: Option<&Path>) -> io::Result<String> {
    if !is_runtime(steamvr) {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("SteamVR is not installed ({} has no bin/linux64/vrclient.so): install SteamVR in Steam", steamvr.display())));
    }
    let p = vrpath(home);
    std::fs::create_dir_all(p.parent().unwrap())?;
    let old = std::fs::read_to_string(&p).ok();
    let backup = std::fs::read_to_string(p.with_file_name(STEAMVR_BACKUP)).ok();
    // SteamVR's own file as the base: the current one if it is SteamVR's, else the saved one
    let is_steamvr = |t: &str| first_runtime(t).is_some_and(|f| !f.contains("xrizer"));
    let base = old.as_deref().filter(|t| is_steamvr(t)).or(backup.as_deref().filter(|t| is_steamvr(t)));
    let text = steamvr_text(base, home, steamvr, driver);
    if old.as_deref() == Some(text.as_str()) {
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o644))?;
        return Ok(format!("{} already set for SteamVR", p.display()));
    }
    if let Some(t) = old.as_deref().filter(|t| first_runtime(t).is_some_and(|f| f.contains("xrizer"))) {
        let bak = p.with_file_name("openvrpaths.vrpath.pimax-xrizer");
        let _ = std::fs::remove_file(&bak);
        std::fs::write(&bak, t)?;
    }
    let tmp = p.with_file_name(format!(".openvrpaths.vrpath.pimax-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::fs::write(&tmp, &text)?;
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o644))?;
    // rename replaces a read-only (0444) file too: the directory is ours
    if let Err(e) = std::fs::rename(&tmp, &p) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    Ok(format!(
        "{} -> SteamVR (writable){}",
        p.display(),
        match driver {
            Some(d) => format!(", Monado driver registered: {}", d.display()),
            None => String::new(),
        }
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn xrizer(home: &Path) -> PathBuf {
        let d = home.join("xrizer");
        std::fs::create_dir_all(d.join("bin/linux64")).unwrap();
        std::fs::write(d.join("bin/linux64/vrclient.so"), "").unwrap();
        d
    }

    const STEAMVR: &str = "{\n\t\"config\": [\"/h/.local/share/Steam/config\"],\n\t\"jsonid\": \"vrpathreg\",\n\t\"runtime\": [\"/h/.local/share/Steam/steamapps/common/SteamVR\"],\n\t\"version\": 1\n}";

    #[test]
    fn first_runtime_reads_the_first_entry() {
        assert_eq!(first_runtime(STEAMVR).as_deref(), Some("/h/.local/share/Steam/steamapps/common/SteamVR"));
        assert_eq!(first_runtime("{\"runtime\": [\"/x/\", \"/y\"]}").as_deref(), Some("/x"));
        assert_eq!(first_runtime("not json"), None);
        assert_eq!(first_runtime(&xrizer_text(Path::new("/a/xrizer"))).as_deref(), Some("/a/xrizer"));
    }

    #[test]
    fn steamvr_file_is_replaced_by_a_read_only_xrizer_file() {
        let h = crate::vr::test_dir("ovr-steamvr");
        let x = xrizer(&h);
        std::fs::create_dir_all(vrpath(&h).parent().unwrap()).unwrap();
        std::fs::write(vrpath(&h), STEAMVR).unwrap();
        let line = ensure_xrizer(&h, &x).unwrap().unwrap();
        assert!(line.contains("was /h/.local/share/Steam/steamapps/common/SteamVR"), "{line}");
        let text = std::fs::read_to_string(vrpath(&h)).unwrap();
        assert_eq!(first_runtime(&text).unwrap(), x.display().to_string());
        assert_eq!(std::fs::metadata(vrpath(&h)).unwrap().permissions().mode() & 0o777, 0o444);
        assert_eq!(std::fs::read_to_string(vrpath(&h).with_file_name("openvrpaths.vrpath.pimax-steamvr")).unwrap(), STEAMVR);
        assert!(ensure_xrizer(&h, &x).unwrap().is_none(), "second call changes nothing");
    }

    #[test]
    fn writable_xrizer_file_is_only_made_read_only() {
        let h = crate::vr::test_dir("ovr-writable");
        let x = xrizer(&h);
        std::fs::create_dir_all(vrpath(&h).parent().unwrap()).unwrap();
        std::fs::write(vrpath(&h), xrizer_text(&x)).unwrap();
        assert!(ensure_xrizer(&h, &x).unwrap().unwrap().contains("read-only"));
        assert_eq!(std::fs::metadata(vrpath(&h)).unwrap().permissions().mode() & 0o777, 0o444);
    }

    #[test]
    fn missing_file_is_created_and_unbuilt_xrizer_changes_nothing() {
        let h = crate::vr::test_dir("ovr-missing");
        assert!(ensure_xrizer(&h, &h.join("no-xrizer")).unwrap().is_none());
        assert!(!vrpath(&h).exists());
        let x = xrizer(&h);
        assert!(ensure_xrizer(&h, &x).unwrap().unwrap().contains("was missing"));
        assert!(vrpath(&h).exists());
    }

    fn steamvr(home: &Path) -> PathBuf {
        let d = home.join("SteamVR");
        std::fs::create_dir_all(d.join("bin/linux64")).unwrap();
        std::fs::write(d.join("bin/linux64/vrclient.so"), "").unwrap();
        d
    }

    #[test]
    fn steamvr_works_without_any_backup_on_a_read_only_xrizer_file() {
        let h = crate::vr::test_dir("ovr-to-steamvr");
        let x = xrizer(&h);
        let s = steamvr(&h);
        ensure_xrizer(&h, &x).unwrap();
        assert_eq!(std::fs::metadata(vrpath(&h)).unwrap().permissions().mode() & 0o777, 0o444);
        assert!(!vrpath(&h).with_file_name(STEAMVR_BACKUP).exists(), "no backup");
        let xr = h.join(".config/openxr/1/active_runtime.json");
        std::fs::create_dir_all(xr.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink("/monado/openxr_monado-dev.json", &xr).unwrap();
        let drv = h.join("steamvr-monado");
        let line = ensure_steamvr(&h, &s, Some(&drv)).unwrap();
        assert!(line.contains("SteamVR") && line.contains("registered"), "{line}");
        let text = std::fs::read_to_string(vrpath(&h)).unwrap();
        assert_eq!(first_runtime(&text).unwrap(), s.display().to_string());
        assert_eq!(external_drivers(&text), vec![drv.display().to_string()]);
        assert!(text.contains(".local/share/Steam/config"));
        assert_eq!(std::fs::metadata(vrpath(&h)).unwrap().permissions().mode() & 0o777, 0o644, "SteamVR must be able to save it");
        assert_eq!(std::fs::read_link(&xr).unwrap(), PathBuf::from("/monado/openxr_monado-dev.json"), "OpenXR untouched");
        assert!(vrpath(&h).with_file_name("openvrpaths.vrpath.pimax-xrizer").exists());
        // back to Monado: xrizer only, read-only, the driver unregistered, SteamVR's file saved
        let back = ensure_xrizer(&h, &x).unwrap().unwrap();
        assert!(back.contains("xrizer"), "{back}");
        let text = std::fs::read_to_string(vrpath(&h)).unwrap();
        assert!(external_drivers(&text).is_empty());
        assert_eq!(std::fs::metadata(vrpath(&h)).unwrap().permissions().mode() & 0o777, 0o444);
        assert_eq!(external_drivers(&std::fs::read_to_string(vrpath(&h).with_file_name(STEAMVR_BACKUP)).unwrap()), vec![drv.display().to_string()]);
    }

    #[test]
    fn steamvr_keeps_steamvrs_own_entries_and_unregisters_on_request() {
        let h = crate::vr::test_dir("ovr-steamvr-base");
        let s = steamvr(&h);
        std::fs::create_dir_all(vrpath(&h).parent().unwrap()).unwrap();
        std::fs::write(vrpath(&h), STEAMVR).unwrap();
        ensure_steamvr(&h, &s, Some(Path::new("/d"))).unwrap();
        let text = std::fs::read_to_string(vrpath(&h)).unwrap();
        assert!(text.contains("/h/.local/share/Steam/config"), "SteamVR's config entry kept: {text}");
        assert!(ensure_steamvr(&h, &s, Some(Path::new("/d"))).unwrap().contains("already"));
        ensure_steamvr(&h, &s, None).unwrap();
        assert!(external_drivers(&std::fs::read_to_string(vrpath(&h)).unwrap()).is_empty());
        assert!(ensure_steamvr(&h, &h.join("nope"), None).unwrap_err().to_string().contains("not installed"));
    }

    #[test]
    fn xrizer_dir_prefers_the_env() {
        let h = Path::new("/home/u");
        assert_eq!(xrizer_dir(&|_| None, h), PathBuf::from("/home/u/src/xrizer/target/release"));
        assert_eq!(xrizer_dir(&|k| (k == "XRIZER").then(|| "/opt/x".to_string()), h), PathBuf::from("/opt/x"));
    }
}
