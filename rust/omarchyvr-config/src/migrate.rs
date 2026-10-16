//! names:keep-file (this module is about the old names)
//! One-time move of the user directories and files from the names used before the rename (`pimax`) to `omarchyvr`.
//!
//! [`migrate_legacy_dirs`] is called early in `main()` of the app, the CLI and both daemons; it is cheap and idempotent.
//! Rules: a directory moves only when the new one does not exist and the old one is a real directory (never a symlink,
//! which is left alone); when both exist nothing moves and that is logged once; a rename across file systems (EXDEV)
//! leaves the old directory where it is and says so. Inside a moved directory nothing changes, file names stay.
//! Every action is appended to `~/.local/state/omarchyvr/logs/migration.log`. systemd units and the Steam desktop-entry
//! copy are not touched here (the installer and `steam-setup` handle those).
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

/// (old, new) directories below `$HOME`, as shown in the log.
/// The state dir comes first: the log lives in it, so any earlier entry would create the new dir and block the move.
pub const DIRS: [(&str, &str); 4] = [
    (".local/state/pimax", ".local/state/omarchyvr"),
    (".config/pimax", ".config/omarchyvr"),
    (".local/share/pimax", ".local/share/omarchyvr"),
    (".cache/pimax", ".cache/omarchyvr"),
];

const LOG: &str = ".local/state/omarchyvr/logs/migration.log";

/// `YYYY-MM-DDTHH:MM:SSZ` for a unix time (civil-from-days, proleptic Gregorian).
pub fn iso_utc(secs: u64) -> String {
    let days = (secs / 86_400) as i64;
    let rem = secs % 86_400;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}Z", rem / 3600, rem % 3600 / 60, rem % 60)
}

fn now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn exists(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok()
}

fn is_real_dir(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_dir())
}

fn is_real_file(p: &Path) -> bool {
    fs::symlink_metadata(p).is_ok_and(|m| m.is_file())
}

struct Log {
    path: PathBuf,
    done: String,
    lines: Vec<String>,
}

impl Log {
    fn new(home: &Path) -> Log {
        let path = home.join(LOG);
        let done = fs::read_to_string(&path).unwrap_or_default();
        Log { path, done, lines: Vec::new() }
    }
    fn seen(&self, what: &str) -> bool {
        self.done.lines().any(|l| l.ends_with(what))
    }
    fn say(&mut self, what: String) {
        let line = format!("{} {what}", iso_utc(now()));
        if let Some(d) = self.path.parent() {
            let _ = fs::create_dir_all(d);
        }
        if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(&self.path) {
            let _ = writeln!(f, "{line}");
        }
        self.done.push_str(&line);
        self.done.push('\n');
        self.lines.push(line);
    }
}

fn move_dir(home: &Path, from_rel: &str, to_rel: &str, log: &mut Log) {
    let (from, to) = (home.join(from_rel), home.join(to_rel));
    if !is_real_dir(&from) {
        return; // absent, or a symlink: never followed, never moved
    }
    if exists(&to) {
        let what = format!("both exist, kept both: ~/{from_rel} and ~/{to_rel}");
        if !log.seen(&what) {
            log.say(what);
        }
        return;
    }
    if let Some(parent) = to.parent() {
        let _ = fs::create_dir_all(parent);
    }
    match fs::rename(&from, &to) {
        Ok(()) => log.say(format!("moved ~/{from_rel} -> ~/{to_rel}")),
        Err(e) if e.raw_os_error() == Some(libc::EXDEV) => {
            let what = format!("could not move ~/{from_rel} -> ~/{to_rel} (different file system), kept the old directory");
            if !log.seen(&what) {
                log.say(what);
            }
        }
        Err(e) => {
            let what = format!("could not move ~/{from_rel} -> ~/{to_rel}: {e}");
            if !log.seen(&what) {
                log.say(what);
            }
        }
    }
}

/// The autostart entry of the app: `pimax-setup.desktop` -> `omarchyvr-app.desktop`, Exec rewritten when it pointed at
/// `~/.local/bin/pimax-setup`.
fn move_autostart(home: &Path, log: &mut Log) {
    let (old, new) = (home.join(".config/autostart/pimax-setup.desktop"), home.join(".config/autostart/omarchyvr-app.desktop"));
    if !is_real_file(&old) || exists(&new) {
        return;
    }
    let Ok(text) = fs::read_to_string(&old) else { return };
    let out: Vec<String> = text
        .lines()
        .map(|l| match l.strip_prefix("Exec=") {
            Some(rest) if rest.contains(".local/bin/pimax-setup") || rest.trim_start().starts_with("pimax-setup") => {
                format!("Exec={}", rest.replacen("pimax-setup", "omarchyvr-app", 1))
            }
            _ => l.to_string(),
        })
        .collect();
    let mut body = out.join("\n");
    body.push('\n');
    if fs::write(&new, body).is_ok() && fs::remove_file(&old).is_ok() {
        log.say("moved ~/.config/autostart/pimax-setup.desktop -> ~/.config/autostart/omarchyvr-app.desktop".into());
    }
}

/// `90-pimax-steam-vr.conf` -> `90-omarchyvr-steam-vr.conf` (contents unchanged; the old marker is still recognised).
fn move_steam_env(home: &Path, log: &mut Log) {
    let (old, new) = (home.join(".config/environment.d/90-pimax-steam-vr.conf"), home.join(".config/environment.d/90-omarchyvr-steam-vr.conf"));
    if is_real_file(&old) && !exists(&new) && fs::rename(&old, &new).is_ok() {
        log.say("moved ~/.config/environment.d/90-pimax-steam-vr.conf -> ~/.config/environment.d/90-omarchyvr-steam-vr.conf".into());
    }
}

/// Move the user data of the names before the rename to the new names. Returns the log lines written this time
/// (empty when there was nothing to do).
pub fn migrate_legacy_dirs(home: &Path) -> Vec<String> {
    let mut log = Log::new(home);
    for (from, to) in DIRS {
        move_dir(home, from, to, &mut log);
    }
    // the controller replay cache was a sibling of the cache dir
    move_dir(home, ".cache/pimax-ctrl-replay", ".cache/omarchyvr/ctrl-replay", &mut log);
    move_autostart(home, &mut log);
    move_steam_env(home, &mut log);
    log.lines
}

/// True when the migration must not touch the real home: `OMARCHYVR_NO_MIGRATE` is set (tests set it), or the
/// running binary lives under a cargo `target*/` directory (debug, test and clean builds).
pub fn migration_disabled(no_migrate: Option<&std::ffi::OsStr>, exe: Option<&Path>) -> bool {
    if no_migrate.is_some_and(|v| !v.is_empty() && v != "0") {
        return true;
    }
    exe.is_some_and(|e| {
        e.components().any(|c| {
            c.as_os_str().to_str().is_some_and(|n| n == "target" || n.starts_with("target-"))
        })
    })
}

/// For tests that spawn our binaries: point HOME and every XDG dir at `dir` (created) and switch the one-time
/// migration off, so a spawned binary can never read or move the real home. `XDG_RUNTIME_DIR` is left to the caller.
pub fn isolate_command(cmd: &mut std::process::Command, dir: &Path) {
    let home = dir.join("isolated-home");
    for sub in [".config", ".local/state", ".local/share", ".cache"] {
        let _ = fs::create_dir_all(home.join(sub));
    }
    cmd.env("HOME", &home)
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_STATE_HOME", home.join(".local/state"))
        .env("XDG_DATA_HOME", home.join(".local/share"))
        .env("XDG_CACHE_HOME", home.join(".cache"))
        .env("OMARCHYVR_NO_MIGRATE", "1");
}

/// [`migrate_legacy_dirs`] for the real `$HOME`, ignoring every error: starting must never fail because of it.
/// Does nothing when [`migration_disabled`].
pub fn migrate_home() {
    let exe = std::env::current_exe().ok();
    if migration_disabled(std::env::var_os("OMARCHYVR_NO_MIGRATE").as_deref(), exe.as_deref()) {
        return;
    }
    if let Some(h) = std::env::var_os("HOME").filter(|v| !v.is_empty()) {
        let _ = migrate_legacy_dirs(Path::new(&h));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn home(name: &str) -> PathBuf {
        let h = std::env::temp_dir().join(format!("omarchyvr-migrate-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&h);
        fs::create_dir_all(&h).unwrap();
        h
    }

    fn log_text(h: &Path) -> String {
        fs::read_to_string(h.join(LOG)).unwrap_or_default()
    }

    #[test]
    fn migration_is_off_for_the_opt_out_and_for_cargo_target_dirs() {
        use std::ffi::OsStr;
        let installed = Path::new("/home/u/.local/bin/omarchyvr-app");
        assert!(!migration_disabled(None, Some(installed)));
        assert!(!migration_disabled(Some(OsStr::new("")), Some(installed)));
        assert!(!migration_disabled(Some(OsStr::new("0")), Some(installed)));
        assert!(migration_disabled(Some(OsStr::new("1")), Some(installed)));
        assert!(migration_disabled(None, Some(Path::new("/x/rust/target/debug/omarchyvr-app"))));
        assert!(migration_disabled(None, Some(Path::new("/x/rust/target-clean/release/omarchyvrd"))));
        assert!(migration_disabled(None, Some(Path::new("/x/target/debug/deps/e2e-abc"))));
    }

    #[test]
    fn iso_time_is_utc() {
        assert_eq!(iso_utc(0), "1970-01-01T00:00:00Z");
        assert_eq!(iso_utc(1_791_968_155), "2026-10-14T08:55:55Z");
        assert_eq!(iso_utc(951_782_400), "2000-02-29T00:00:00Z");
    }

    #[test]
    fn the_four_directories_move_and_the_files_inside_keep_their_names() {
        let h = home("move");
        for (old, _) in DIRS {
            fs::create_dir_all(h.join(old)).unwrap();
            fs::write(h.join(old).join("keep.txt"), old).unwrap();
        }
        let lines = migrate_legacy_dirs(&h);
        assert_eq!(lines.len(), 4, "{lines:?}");
        for (old, new) in DIRS {
            assert!(!h.join(old).exists(), "{old}");
            assert_eq!(fs::read_to_string(h.join(new).join("keep.txt")).unwrap(), old);
        }
        assert!(lines[1].ends_with("moved ~/.config/pimax -> ~/.config/omarchyvr"), "{}", lines[1]);
        assert!(log_text(&h).contains("moved ~/.cache/pimax -> ~/.cache/omarchyvr"));
        assert!(lines[0].starts_with("20") && lines[0].as_bytes()[10] == b'T');
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn running_it_again_does_nothing() {
        let h = home("idem");
        fs::create_dir_all(h.join(".config/pimax")).unwrap();
        assert_eq!(migrate_legacy_dirs(&h).len(), 1);
        let before = log_text(&h);
        assert!(migrate_legacy_dirs(&h).is_empty());
        assert_eq!(log_text(&h), before);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn when_both_exist_both_stay_and_it_is_logged_once() {
        let h = home("both");
        fs::create_dir_all(h.join(".config/pimax")).unwrap();
        fs::write(h.join(".config/pimax/a"), "old").unwrap();
        fs::create_dir_all(h.join(".config/omarchyvr")).unwrap();
        fs::write(h.join(".config/omarchyvr/a"), "new").unwrap();
        assert_eq!(migrate_legacy_dirs(&h).len(), 1);
        assert!(migrate_legacy_dirs(&h).is_empty(), "logged once");
        assert_eq!(fs::read_to_string(h.join(".config/pimax/a")).unwrap(), "old");
        assert_eq!(fs::read_to_string(h.join(".config/omarchyvr/a")).unwrap(), "new");
        assert_eq!(log_text(&h).matches("both exist, kept both").count(), 1);
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn a_symlink_is_never_followed_or_moved() {
        let h = home("link");
        let real = h.join("elsewhere");
        fs::create_dir_all(&real).unwrap();
        fs::create_dir_all(h.join(".config")).unwrap();
        std::os::unix::fs::symlink(&real, h.join(".config/pimax")).unwrap();
        assert!(migrate_legacy_dirs(&h).is_empty());
        assert!(fs::symlink_metadata(h.join(".config/pimax")).unwrap().file_type().is_symlink());
        assert!(!h.join(".config/omarchyvr").exists());
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn autostart_and_steam_env_files_are_renamed_and_the_exec_line_follows() {
        let h = home("files");
        fs::create_dir_all(h.join(".config/autostart")).unwrap();
        fs::create_dir_all(h.join(".config/environment.d")).unwrap();
        fs::write(h.join(".config/autostart/pimax-setup.desktop"), "[Desktop Entry]\nName=Pimax\nExec=/home/u/.local/bin/pimax-setup --background\n").unwrap();
        fs::write(h.join(".config/environment.d/90-pimax-steam-vr.conf"), "# X-Pimax-Steam-VR=1\nA=1\n").unwrap();
        let lines = migrate_legacy_dirs(&h);
        assert_eq!(lines.len(), 2, "{lines:?}");
        let a = fs::read_to_string(h.join(".config/autostart/omarchyvr-app.desktop")).unwrap();
        assert!(a.contains("Exec=/home/u/.local/bin/omarchyvr-app --background"), "{a}");
        assert!(!h.join(".config/autostart/pimax-setup.desktop").exists());
        assert_eq!(fs::read_to_string(h.join(".config/environment.d/90-omarchyvr-steam-vr.conf")).unwrap(), "# X-Pimax-Steam-VR=1\nA=1\n");
        assert!(migrate_legacy_dirs(&h).is_empty());
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn an_autostart_entry_pointing_elsewhere_keeps_its_exec_line() {
        let h = home("exec");
        fs::create_dir_all(h.join(".config/autostart")).unwrap();
        fs::write(h.join(".config/autostart/pimax-setup.desktop"), "[Desktop Entry]\nExec=/src/target/release/pimax-setup\n").unwrap();
        migrate_legacy_dirs(&h);
        let a = fs::read_to_string(h.join(".config/autostart/omarchyvr-app.desktop")).unwrap();
        assert!(a.contains("Exec=/src/target/release/pimax-setup\n"), "{a}");
        let _ = fs::remove_dir_all(&h);
    }

    #[test]
    fn the_replay_cache_moves_into_the_new_cache_dir() {
        let h = home("replay");
        fs::create_dir_all(h.join(".cache/pimax-ctrl-replay")).unwrap();
        fs::write(h.join(".cache/pimax-ctrl-replay/x"), "1").unwrap();
        migrate_legacy_dirs(&h);
        assert_eq!(fs::read_to_string(h.join(".cache/omarchyvr/ctrl-replay/x")).unwrap(), "1");
        let _ = fs::remove_dir_all(&h);
    }
}
