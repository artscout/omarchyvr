//! The headset's factory calibration (cameras, IMU, lens XML): private per-device data, NOT a setting.
//!
//! It lives in `~/.config/omarchyvr/headset-calibration.xml` (mode 600), survives an uninstall (like `ai.toml`), is
//! migrated from older places on start, and, when missing, is read from the headset by `omarchyvr calib --save-config`
//! (84 allowlisted `read_calib_block` requests, nothing else; rust/pimax-device/devices/dream-air/device.toml). It is
//! never committed anywhere. Work and Play need it (Basalt/SLAM export), so a start without it says so in plain words.
use crate::cmd::Cmd;
use omarchyvr_config::paths::{headset_calibration_old, headset_calibration_target};
use std::path::{Path, PathBuf};

/// The job kind of the read.
pub const READ_KIND: &str = "room:calib";

/// What a start says when the file is missing.
pub const MISSING: &str = "Headset calibration missing: press Room setup (it reads it from the headset).";

/// The file the app keeps.
pub fn target(home: &Path) -> PathBuf {
    headset_calibration_target(home)
}

/// Is a usable calibration there (a readable, non-trivial XML document)?
pub fn present(home: &Path) -> bool {
    looks_valid(&target(home))
}

fn looks_valid(p: &Path) -> bool {
    std::fs::read(p).is_ok_and(|b| b.len() > 200 && std::str::from_utf8(&b[..b.len().min(64)]).is_ok_and(|s| s.contains('<')))
}

/// True when the lookup falls back to the shipped REFERENCE calibration: the user's own file is missing and the
/// reference is allowed (`PIMAX_NO_REFERENCE` unset; it is compiled into every reader, `pimax_cam::calib::REFERENCE_XML`).
/// Work and Play then start with the reference and the Room setup tile says so.
pub fn reference_in_use(home: &Path) -> bool {
    reference_in_use_with(home, &|k| std::env::var(k).ok())
}

/// [`reference_in_use`] with an injectable environment.
pub fn reference_in_use_with(home: &Path, env: &dyn Fn(&str) -> Option<String>) -> bool {
    !present(home) && omarchyvr_config::devicedata::reference_allowed(env)
}

/// Copy the calibration from an older place when the current file is missing (mode 600). Returns the source used.
pub fn migrate(home: &Path) -> Option<PathBuf> {
    if present(home) {
        return None;
    }
    let src = headset_calibration_old(home).into_iter().find(|p| looks_valid(p))?;
    write_private(&target(home), &std::fs::read(&src).ok()?).ok()?;
    Some(src)
}

/// Write with mode 600 through a temporary file.
pub fn write_private(path: &Path, data: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("xml.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&tmp)?;
    f.write_all(data)?;
    f.sync_all()?;
    std::fs::rename(&tmp, path)
}

/// `omarchyvr calib --save-config`: reads the document from the headset and saves it (no wake, no keepalive).
pub fn read_cmd(launcher: &Path) -> Cmd {
    Cmd::with_args(launcher.display().to_string(), vec!["calib".into(), "--save-config".into()])
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Home(PathBuf);
    impl Home {
        fn new(tag: &str) -> Home {
            let d = std::env::temp_dir().join(format!("omarchyvr-calib-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&d);
            std::fs::create_dir_all(&d).unwrap();
            Home(d)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }
    impl Drop for Home {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn doc() -> Vec<u8> {
        let mut v = b"<?xml version=\"1.0\"?><Calibration>".to_vec();
        v.extend(std::iter::repeat_n(b' ', 300));
        v.extend(b"</Calibration>");
        v
    }

    #[test]
    fn migrates_from_old_places_with_mode_600() {
        use std::os::unix::fs::PermissionsExt;
        let h = Home::new("mig");
        assert!(!present(h.path()));
        assert!(migrate(h.path()).is_none());
        let old = h.path().join(".config/pimax/headset-calibration.xml");
        std::fs::create_dir_all(old.parent().unwrap()).unwrap();
        std::fs::write(&old, doc()).unwrap();
        assert_eq!(migrate(h.path()), Some(old));
        assert!(present(h.path()));
        assert_eq!(std::fs::metadata(target(h.path())).unwrap().permissions().mode() & 0o777, 0o600);
        // a present file is left alone
        assert!(migrate(h.path()).is_none());
    }

    #[test]
    fn an_empty_or_junk_file_is_not_a_calibration() {
        let h = Home::new("junk");
        std::fs::create_dir_all(target(h.path()).parent().unwrap()).unwrap();
        std::fs::write(target(h.path()), b"").unwrap();
        assert!(!present(h.path()));
    }

    #[test]
    fn the_read_command_is_the_calib_tool_and_safe() {
        let c = read_cmd(Path::new("/b/omarchyvr"));
        assert_eq!(c.display(), "/b/omarchyvr calib --save-config");
        assert_eq!(crate::safety::check(&c), Ok(()));
    }
}
