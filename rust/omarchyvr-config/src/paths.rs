//! Machine paths that differ between users, read from one small file: `~/.config/omarchyvr/paths.toml`.
//!
//! ```toml
//! # where the Monado, WayVR, Basalt and xrizer source and build trees live (default: ~/src)
//! src_dir = "/home/me/src"
//! # the installed layout (./install.sh); every key below is optional, the default is shown
//! lib_dir = "~/.local/lib/omarchyvr"          # Monado build, WayVR, Basalt, ONNX Runtime, sherpa-onnx, whisper.cpp
//! data_dir = "~/.local/share/omarchyvr"       # data dir; <data_dir>/data is the installed copy of the repo's docs/linux/...
//! repo_dir = "~/omarchyvr"                    # the repo the app reads docs, linux/, patches/ from
//! monado_build = "~/.local/lib/omarchyvr/monado"  # Monado CMake build dir (CMakeCache.txt)
//! wayvr_bin = "..."  wayvrctl_bin = "..."     # default <lib_dir>/wayvr/wayvr and wayvrctl
//! basalt_lib = "..."  sherpa_lib = "..."  whisper_server = "..."
//! wayvr_home = "..."  wayvr_home_perf = "..." # default <data_dir>/wayvr-home and wayvr-home-perf
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

fn key_path(home: &Path, key: &str) -> Option<PathBuf> {
    value(&file_text(home)?, key).map(|v| expand(home, &v))
}

/// A real file or directory on disk (the default `exists` predicate of the `*_with` functions).
fn on_disk(p: &Path) -> bool {
    p.exists()
}

/// The installed set (`./install.sh`): `lib_dir` in `paths.toml`, else `<home>/.local/lib/omarchyvr`.
pub fn lib_dir(home: &Path) -> PathBuf {
    key_path(home, "lib_dir").unwrap_or_else(|| home.join(".local/lib/omarchyvr"))
}

/// The data dir: `data_dir` in `paths.toml`, else `<home>/.local/share/omarchyvr`.
pub fn data_dir(home: &Path) -> PathBuf {
    key_path(home, "data_dir").unwrap_or_else(|| home.join(".local/share/omarchyvr"))
}

/// The repo: `repo_dir` in `paths.toml`, else the installed data copy (`<data_dir>/data`, when it has `docs/LESSONS.md`),
/// else `<home>/omarchyvr`.
pub fn repo_dir(home: &Path) -> PathBuf {
    repo_dir_with(home, &on_disk)
}

/// [`repo_dir`] with an injectable `exists`.
pub fn repo_dir_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(p) = key_path(home, "repo_dir") {
        return p;
    }
    let installed = data_dir(home).join("data");
    if exists(&installed.join("docs/LESSONS.md")) {
        installed
    } else {
        home.join("omarchyvr")
    }
}

/// The Monado CMake build dir: `monado_build` in `paths.toml`, else `<lib_dir>/monado` (the installed build).
pub fn monado_build(home: &Path) -> PathBuf {
    monado_build_with(home, &on_disk)
}

/// [`monado_build`] with an injectable `exists`.
pub fn monado_build_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(p) = key_path(home, "monado_build") {
        return p;
    }
    let _ = exists;
    lib_dir(home).join("monado")
}

/// `key` in `paths.toml`, else `<lib_dir>/<rel>`.
fn fixed(home: &Path, key: &str, rel: &str, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    let _ = exists;
    key_path(home, key).unwrap_or_else(|| lib_dir(home).join(rel))
}

/// `installed` (under [`lib_dir`]) when it exists, else `fallback`; `key` in `paths.toml` wins over both.
fn installed_or(home: &Path, key: &str, installed: PathBuf, legacy: PathBuf, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    if let Some(p) = key_path(home, key) {
        return p;
    }
    if exists(&installed) {
        installed
    } else {
        legacy
    }
}

/// WayVR: `wayvr_bin` in `paths.toml`, else `<lib_dir>/wayvr/wayvr` if it exists, else `<lib_dir>/wayvr/wayvr`.
pub fn wayvr_bin(home: &Path) -> PathBuf {
    wayvr_bin_with(home, &on_disk)
}

/// [`wayvr_bin`] with an injectable `exists`.
pub fn wayvr_bin_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    fixed(home, "wayvr_bin", "wayvr/wayvr", exists)
}

/// `wayvrctl`: like [`wayvr_bin`] (`wayvrctl_bin` in `paths.toml`).
pub fn wayvrctl_bin(home: &Path) -> PathBuf {
    wayvrctl_bin_with(home, &on_disk)
}

/// [`wayvrctl_bin`] with an injectable `exists`.
pub fn wayvrctl_bin_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    fixed(home, "wayvrctl_bin", "wayvr/wayvrctl", exists)
}

/// Basalt (`basalt_lib`): `<lib_dir>/basalt/lib/libbasalt.so` if it exists, else `~/.local/lib/libbasalt.so`.
pub fn basalt_lib(home: &Path) -> PathBuf {
    basalt_lib_with(home, &on_disk)
}

/// [`basalt_lib`] with an injectable `exists`.
pub fn basalt_lib_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    installed_or(home, "basalt_lib", lib_dir(home).join("basalt/lib/libbasalt.so"), home.join(".local/lib/libbasalt.so"), exists)
}

/// sherpa-onnx C API (`sherpa_lib`): `<lib_dir>/sherpa-onnx/lib/libsherpa-onnx-c-api.so` (`sherpa_lib` in `paths.toml` wins).
pub fn sherpa_lib(home: &Path) -> PathBuf {
    sherpa_lib_with(home, &on_disk)
}

/// [`sherpa_lib`] with an injectable `exists`.
pub fn sherpa_lib_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    fixed(home, "sherpa_lib", "sherpa-onnx/lib/libsherpa-onnx-c-api.so", exists)
}

/// whisper.cpp's server (`whisper_server`): `<lib_dir>/whisper.cpp/bin/whisper-server` (`whisper_server` in `paths.toml` wins).
pub fn whisper_server(home: &Path) -> PathBuf {
    whisper_server_with(home, &on_disk)
}

/// [`whisper_server`] with an injectable `exists`.
pub fn whisper_server_with(home: &Path, exists: &dyn Fn(&Path) -> bool) -> PathBuf {
    fixed(home, "whisper_server", "whisper.cpp/bin/whisper-server", exists)
}

/// WayVR's `$HOME` for the desk (`wayvr_home`): `<data_dir>/wayvr-home`. No fallback.
pub fn wayvr_home(home: &Path) -> PathBuf {
    key_path(home, "wayvr_home").unwrap_or_else(|| data_dir(home).join("wayvr-home"))
}

/// WayVR's `$HOME` for the performance runs (`wayvr_home_perf`): `<data_dir>/wayvr-home-perf`. No fallback.
pub fn wayvr_home_perf(home: &Path) -> PathBuf {
    key_path(home, "wayvr_home_perf").unwrap_or_else(|| data_dir(home).join("wayvr-home-perf"))
}

/// Where `headset-calibration.xml` lives: `~/.config/omarchyvr/headset-calibration.xml`, then
/// `<repo_dir>/analysis/headset-calibration.xml`.
pub fn headset_calibration_candidates(home: &Path) -> Vec<PathBuf> {
    vec![home.join(".config/omarchyvr/headset-calibration.xml"), repo_dir(home).join("analysis/headset-calibration.xml")]
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

    #[test]
    fn installed_layout_and_fallbacks() {
        let h = Path::new("/nonexistent-home-for-paths-test");
        let none = |_: &Path| false;
        let all = |_: &Path| true;
        if std::env::var_os("PIMAX_SRC_DIR").is_some() {
            return;
        }
        assert_eq!(lib_dir(h), h.join(".local/lib/omarchyvr"));
        assert_eq!(data_dir(h), h.join(".local/share/omarchyvr"));
        // repo: installed data copy, else ~/omarchyvr
        assert_eq!(repo_dir_with(h, &none), h.join("omarchyvr"));
        assert_eq!(repo_dir_with(h, &all), h.join(".local/share/omarchyvr/data"));
        // monado build: the installed build dir
        assert_eq!(monado_build_with(h, &none), h.join(".local/lib/omarchyvr/monado"));
        assert_eq!(monado_build_with(h, &all), h.join(".local/lib/omarchyvr/monado"));
        assert_eq!(wayvr_bin_with(h, &none), h.join(".local/lib/omarchyvr/wayvr/wayvr"));
        assert_eq!(wayvr_bin_with(h, &all), h.join(".local/lib/omarchyvr/wayvr/wayvr"));
        assert_eq!(wayvrctl_bin_with(h, &none), h.join(".local/lib/omarchyvr/wayvr/wayvrctl"));
        assert_eq!(wayvrctl_bin_with(h, &all), h.join(".local/lib/omarchyvr/wayvr/wayvrctl"));
        assert_eq!(basalt_lib_with(h, &none), h.join(".local/lib/libbasalt.so"));
        assert_eq!(basalt_lib_with(h, &all), h.join(".local/lib/omarchyvr/basalt/lib/libbasalt.so"));
        assert_eq!(sherpa_lib_with(h, &none), h.join(".local/lib/omarchyvr/sherpa-onnx/lib/libsherpa-onnx-c-api.so"));
        assert_eq!(sherpa_lib_with(h, &all), h.join(".local/lib/omarchyvr/sherpa-onnx/lib/libsherpa-onnx-c-api.so"));
        assert_eq!(whisper_server_with(h, &none), h.join(".local/lib/omarchyvr/whisper.cpp/bin/whisper-server"));
        assert_eq!(whisper_server_with(h, &all), h.join(".local/lib/omarchyvr/whisper.cpp/bin/whisper-server"));
        assert_eq!(wayvr_home(h), h.join(".local/share/omarchyvr/wayvr-home"));
        assert_eq!(wayvr_home_perf(h), h.join(".local/share/omarchyvr/wayvr-home-perf"));
    }

    #[test]
    fn paths_toml_overrides_every_default() {
        if std::env::var_os("PIMAX_SRC_DIR").is_some() {
            return;
        }
        let d = std::env::temp_dir().join(format!("omarchyvr-paths-keys-{}", std::process::id()));
        std::fs::create_dir_all(d.join(".config/omarchyvr")).unwrap();
        std::fs::write(
            d.join(PATHS_FILE),
            "lib_dir = \"~/L\"\ndata_dir = \"/D\"\nrepo_dir = \"~/R\"\nmonado_build = \"/M\"\nwayvr_bin = \"/w\"\nwayvrctl_bin = \"/wc\"\n\
             basalt_lib = \"/b\"\nsherpa_lib = \"/s\"\nwhisper_server = \"/ws\"\nwayvr_home = \"/wh\"\nwayvr_home_perf = \"/whp\"\n",
        )
        .unwrap();
        let none = |_: &Path| false;
        assert_eq!(lib_dir(&d), d.join("L"));
        assert_eq!(data_dir(&d), PathBuf::from("/D"));
        assert_eq!(repo_dir_with(&d, &none), d.join("R"));
        assert_eq!(monado_build_with(&d, &none), PathBuf::from("/M"));
        assert_eq!(wayvr_bin_with(&d, &none), PathBuf::from("/w"));
        assert_eq!(wayvrctl_bin_with(&d, &none), PathBuf::from("/wc"));
        assert_eq!(basalt_lib_with(&d, &none), PathBuf::from("/b"));
        assert_eq!(sherpa_lib_with(&d, &none), PathBuf::from("/s"));
        assert_eq!(whisper_server_with(&d, &none), PathBuf::from("/ws"));
        assert_eq!(wayvr_home(&d), PathBuf::from("/wh"));
        assert_eq!(wayvr_home_perf(&d), PathBuf::from("/whp"));
        let _ = std::fs::remove_dir_all(&d);
    }
}
