//! Machine paths that differ between users, read from one small file: `~/.config/omarchyvr/paths.toml`.
//!
//! ```toml
//! # where the Monado, WayVR, Basalt and xrizer source and build trees live (default: ~/src)
//! src_dir = "/home/me/src"
//! # optional: filesystem UUID of a shared NTFS drive for `omarchyvr mount-shared` (no default: the check is off)
//! shared_drive_uuid = "0123456789ABCDEF"
//! ```
//! Environment overrides (for tests and one-off runs): `PIMAX_SRC_DIR`, `PIMAX_SHARED_UUID`.
use std::path::{Path, PathBuf};

/// The file, relative to `$HOME`.
pub const PATHS_FILE: &str = ".config/omarchyvr/paths.toml";

/// `key = "value"` (or `key = value`) from the text of `paths.toml`; comments and blank values are ignored.
pub fn value(text: &str, key: &str) -> Option<String> {
    text.lines().find_map(|l| {
        let l = l.split('#').next().unwrap_or("").trim();
        let (k, v) = l.split_once('=')?;
        if k.trim() != key {
            return None;
        }
        let v = v.trim().trim_matches('"').trim_matches('\'').trim();
        (!v.is_empty()).then(|| v.to_string())
    })
}

fn env_or_file(home: &Path, env_key: &str, key: &str) -> Option<String> {
    if let Some(v) = std::env::var(env_key).ok().filter(|v| !v.trim().is_empty()) {
        return Some(v.trim().to_string());
    }
    value(&file_text(home)?, key)
}

/// The text of `paths.toml`, cached by path and modification time: callers on the app's UI thread only `stat` it.
fn file_text(home: &Path) -> Option<String> {
    use std::sync::Mutex;
    type Entry = (PathBuf, Option<std::time::SystemTime>, Option<String>);
    static CACHE: Mutex<Vec<Entry>> = Mutex::new(Vec::new());
    let p = home.join(PATHS_FILE);
    let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    let mut c = CACHE.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(e) = c.iter().find(|e| e.0 == p && e.1 == mtime) {
        return e.2.clone();
    }
    let text = mtime.and_then(|_| std::fs::read_to_string(&p).ok());
    c.retain(|e| e.0 != p);
    if c.len() > 64 {
        c.clear();
    }
    c.push((p, mtime, text.clone()));
    text
}

/// Expands a leading `~/` against `home`.
fn expand(home: &Path, v: &str) -> PathBuf {
    match v.strip_prefix("~/") {
        Some(rest) => home.join(rest),
        None if v == "~" => home.to_path_buf(),
        None => PathBuf::from(v),
    }
}

/// The source/build tree root: `$PIMAX_SRC_DIR`, else `src_dir` in `paths.toml`, else `<home>/src`.
pub fn src_dir(home: &Path) -> PathBuf {
    env_or_file(home, "PIMAX_SRC_DIR", "src_dir").map_or_else(|| home.join("src"), |v| expand(home, &v))
}

/// The shared drive's filesystem UUID from `$PIMAX_SHARED_UUID` or `shared_drive_uuid` in `paths.toml`. No default.
pub fn shared_drive_uuid(home: &Path) -> Option<String> {
    env_or_file(home, "PIMAX_SHARED_UUID", "shared_drive_uuid").filter(|u| valid_uuid(u))
}

/// A filesystem UUID as `/dev/disk/by-uuid` names them: hex digits and dashes only (NTFS: 16 hex digits).
pub fn valid_uuid(u: &str) -> bool {
    (8..=36).contains(&u.len()) && u.chars().all(|c| c.is_ascii_hexdigit() || c == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_from_the_file() {
        let t = "# paths\nsrc_dir = \"~/code\"  # trees\nshared_drive_uuid = 0123456789ABCDEF\nempty = \"\"\n";
        assert_eq!(value(t, "src_dir").as_deref(), Some("~/code"));
        assert_eq!(value(t, "shared_drive_uuid").as_deref(), Some("0123456789ABCDEF"));
        assert_eq!(value(t, "empty"), None);
        assert_eq!(value(t, "missing"), None);
        assert_eq!(expand(Path::new("/h"), "~/code"), PathBuf::from("/h/code"));
        assert_eq!(expand(Path::new("/h"), "/opt/src"), PathBuf::from("/opt/src"));
    }

    #[test]
    fn defaults_without_a_file() {
        let d = std::env::temp_dir().join(format!("omarchyvr-paths-test-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".config/omarchyvr")).unwrap();
        if std::env::var_os("PIMAX_SRC_DIR").is_none() {
            assert_eq!(src_dir(&d), d.join("src"));
        }
        if std::env::var_os("PIMAX_SHARED_UUID").is_none() {
            assert_eq!(shared_drive_uuid(&d), None, "no default UUID");
            std::fs::write(d.join(PATHS_FILE), "shared_drive_uuid = \"0123456789ABCDEF\"\nsrc_dir = \"~/s\"\n").unwrap();
            assert_eq!(shared_drive_uuid(&d).as_deref(), Some("0123456789ABCDEF"));
            if std::env::var_os("PIMAX_SRC_DIR").is_none() {
                assert_eq!(src_dir(&d), d.join("s"));
            }
            let d2 = d.join("other");
            std::fs::create_dir_all(d2.join(".config/omarchyvr")).unwrap();
            std::fs::write(d2.join(PATHS_FILE), "shared_drive_uuid = \"x; rm -rf /\"\n").unwrap();
            assert_eq!(shared_drive_uuid(&d2), None, "only hex UUIDs");
        }
        let _ = std::fs::remove_dir_all(&d);
    }
}
