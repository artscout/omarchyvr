//! `~/.config/openxr/1/active_runtime.json`: the OpenXR runtime every OpenXR app uses when `XR_RUNTIME_JSON` is not
//! set. Steam games (pressure-vessel container) import it only with the launch option
//! `PRESSURE_VESSEL_IMPORT_OPENXR_1_RUNTIMES=1` ([`super::config::STEAM_PV_IMPORT`], LESSONS row 27).
//!
//! `vr start` and `desk start` point it at the `openxr_monado-dev.json` of the Monado build they start, so games need
//! no `XR_RUNTIME_JSON` launch option. `--no-set-runtime` skips this. `stop` leaves the link alone: it is harmless
//! while no service runs (apps then fail to connect, exactly as without it) and a second start would set it again.

use std::io;
use std::path::{Path, PathBuf};

/// The link: `$HOME/.config/openxr/1/active_runtime.json`.
pub fn link_path(home: &Path) -> PathBuf {
    home.join(".config/openxr/1/active_runtime.json")
}

/// What `active_runtime.json` is, compared with the manifest of the build being started.
#[derive(Debug, PartialEq, Eq)]
pub enum State {
    /// Symlink to the wanted manifest.
    Ok,
    /// Nothing there.
    Missing,
    /// Symlink to a manifest of another build (target given).
    Different(PathBuf),
    /// Symlink whose target does not exist (target given).
    Dangling(PathBuf),
    /// A regular file (some other runtime's own copy).
    NotSymlink,
}

/// Inspect the link against `want`.
pub fn inspect(home: &Path, want: &Path) -> State {
    let link = link_path(home);
    match std::fs::symlink_metadata(&link) {
        Err(_) => State::Missing,
        Ok(m) if !m.file_type().is_symlink() => State::NotSymlink,
        Ok(_) => {
            let target = std::fs::read_link(&link).unwrap_or_default();
            let abs = if target.is_absolute() { target.clone() } else { link.parent().unwrap().join(&target) };
            if target == want {
                if want.exists() { State::Ok } else { State::Dangling(target) }
            } else if !abs.exists() {
                State::Dangling(target)
            } else {
                State::Different(target)
            }
        }
    }
}

/// One-line description of a state that is not [`State::Ok`], for the preflight and doctor warnings.
pub fn describe(s: &State, home: &Path, want: &Path) -> Option<String> {
    let l = link_path(home);
    Some(match s {
        State::Ok => return None,
        State::Missing => format!("{} does not exist: OpenXR games find no runtime (`omarchyvr vr start` creates it, pointing at {})", l.display(), want.display()),
        State::Different(t) => format!("{} points to {} but this start uses {}", l.display(), t.display(), want.display()),
        State::Dangling(t) => format!("{} points to {}, which does not exist", l.display(), t.display()),
        State::NotSymlink => format!("{} is a regular file, not a link to {}", l.display(), want.display()),
    })
}

/// Make the link point at `want` (atomic replace). A regular file is first copied once to
/// `active_runtime.json.pimax-backup`. Returns a log line when something changed.
pub fn ensure(home: &Path, want: &Path) -> io::Result<Option<String>> {
    let state = inspect(home, want);
    if state == State::Ok {
        return Ok(None);
    }
    let link = link_path(home);
    std::fs::create_dir_all(link.parent().unwrap())?;
    let mut note = String::new();
    if state == State::NotSymlink {
        let bak = link.with_file_name("active_runtime.json.pimax-backup");
        if bak.exists() {
            note.push_str(&format!(" (backup {} already exists, not overwritten)", bak.display()));
        } else {
            std::fs::copy(&link, &bak)?;
            note.push_str(&format!(" (old file saved as {})", bak.display()));
        }
    }
    let tmp = link.with_file_name(format!(".active_runtime.json.pimax-{}", std::process::id()));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(want, &tmp)?;
    if let Err(e) = std::fs::rename(&tmp, &link) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e);
    }
    let was = match state {
        State::Missing => "was missing".to_string(),
        State::Different(t) | State::Dangling(t) => format!("was {}", t.display()),
        _ => "was a regular file".to_string(),
    };
    Ok(Some(format!("{} -> {} ({was}){note}", link.display(), want.display())))
}

/// SteamVR's OpenXR manifest (`steamxr_linux64.json` in the SteamVR folder).
pub fn steamvr_manifest(home: &Path) -> PathBuf {
    super::openvr_link::steamvr_dir(home).join("steamxr_linux64.json")
}

/// "Games run with: SteamVR": point the OpenXR runtime at SteamVR's own, so native OpenXR games (and Proton games in
/// -openxr mode) find a runtime while no monado-service runs. The old target is not lost: [`ensure`] with the
/// Monado manifest (any Monado start, or `omarchyvr vr games monado`) puts it back; a regular file is saved once as
/// `active_runtime.json.pimax-backup`. Errors when SteamVR is not installed.
pub fn ensure_steamvr(home: &Path) -> io::Result<Option<String>> {
    let want = steamvr_manifest(home);
    if !want.exists() {
        return Err(io::Error::new(io::ErrorKind::NotFound, format!("{} does not exist: install SteamVR", want.display())));
    }
    ensure(home, &want)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        super::super::test_dir(&format!("rt-{name}"))
    }
    fn manifest(home: &Path, build: &str) -> PathBuf {
        let d = home.join(build);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("openxr_monado-dev.json");
        std::fs::write(&p, "{}").unwrap();
        p
    }

    #[test]
    fn missing_link_is_created() {
        let h = home("missing");
        let want = manifest(&h, "b1");
        assert_eq!(inspect(&h, &want), State::Missing);
        assert!(ensure(&h, &want).unwrap().is_some());
        assert_eq!(std::fs::read_link(link_path(&h)).unwrap(), want);
        assert_eq!(inspect(&h, &want), State::Ok);
        assert!(ensure(&h, &want).unwrap().is_none(), "second call changes nothing");
    }

    #[test]
    fn stale_link_is_replaced() {
        let h = home("stale");
        let old = manifest(&h, "old");
        let new = manifest(&h, "new");
        ensure(&h, &old).unwrap();
        assert_eq!(inspect(&h, &new), State::Different(old.clone()));
        let msg = ensure(&h, &new).unwrap().unwrap();
        assert!(msg.contains("was ") && msg.contains("old"));
        assert_eq!(std::fs::read_link(link_path(&h)).unwrap(), new);
    }

    #[test]
    fn dangling_link_is_detected_and_fixed() {
        let h = home("dangling");
        let new = manifest(&h, "new");
        std::fs::create_dir_all(link_path(&h).parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(h.join("gone/openxr_monado-dev.json"), link_path(&h)).unwrap();
        assert!(matches!(inspect(&h, &new), State::Dangling(_)));
        ensure(&h, &new).unwrap();
        assert_eq!(inspect(&h, &new), State::Ok);
    }

    #[test]
    fn regular_file_is_backed_up_once() {
        let h = home("file");
        let a = manifest(&h, "a");
        let b = manifest(&h, "b");
        std::fs::create_dir_all(link_path(&h).parent().unwrap()).unwrap();
        std::fs::write(link_path(&h), "original").unwrap();
        ensure(&h, &a).unwrap();
        let bak = link_path(&h).with_file_name("active_runtime.json.pimax-backup");
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "original");
        ensure(&h, &b).unwrap();
        assert_eq!(std::fs::read_to_string(&bak).unwrap(), "original", "backup is kept");
        assert_eq!(std::fs::read_link(link_path(&h)).unwrap(), b);
    }

    #[test]
    fn describe_is_none_only_for_ok() {
        let h = home("desc");
        let want = manifest(&h, "b");
        assert!(describe(&State::Missing, &h, &want).is_some());
        assert!(describe(&State::Ok, &h, &want).is_none());
    }

    #[test]
    fn steamvr_mode_points_at_steamvr_and_monado_mode_back() {
        let h = home("steamvr-link");
        let m = manifest(&h, "build");
        std::fs::create_dir_all(link_path(&h).parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&m, link_path(&h)).unwrap();
        // SteamVR not installed: plain error, link untouched
        assert!(ensure_steamvr(&h).unwrap_err().to_string().contains("install SteamVR"));
        assert_eq!(inspect(&h, &m), State::Ok);
        let sv = steamvr_manifest(&h);
        std::fs::create_dir_all(sv.parent().unwrap()).unwrap();
        std::fs::write(&sv, "{}").unwrap();
        assert!(ensure_steamvr(&h).unwrap().unwrap().contains("steamxr_linux64.json"));
        assert_eq!(inspect(&h, &sv), State::Ok);
        assert_eq!(ensure_steamvr(&h).unwrap(), None);
        // back to Monado
        assert!(ensure(&h, &m).unwrap().is_some());
        assert_eq!(inspect(&h, &m), State::Ok);
    }
}
