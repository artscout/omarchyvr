//! The app's persistent log: a `log` backend that appends to `~/.local/state/pimax/logs/pimax-setup.log`, rotated
//! (5 MB, 5 files kept), optionally mirrored to stderr, with the last lines kept in memory for crash reports.
//!
//! Before this existed, an error that eframe reports only through `log::error!` (for example a failed GL buffer swap,
//! after which the window closes) left no trace at all.
use std::cell::Cell;
use std::collections::VecDeque;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

/// Rotate when the log is larger than this.
pub const MAX_BYTES: u64 = 5 << 20;
/// Rotated files kept (`.1` .. `.4` plus the current one).
pub const KEEP: usize = 5;
/// Lines kept in memory.
pub const RING: usize = 1000;
/// One process writes at most this much into the file (a runaway loop cannot fill the disk); the supervisor
/// rotates at its start.
pub const FILE_CAP: u64 = 20 << 20;

/// `$XDG_STATE_HOME/pimax`, else `~/.local/state/pimax`.
pub fn state_dir(env: &dyn Fn(&str) -> Option<String>) -> PathBuf {
    match env("XDG_STATE_HOME").filter(|v| !v.is_empty()) {
        Some(s) => PathBuf::from(s).join("pimax"),
        None => PathBuf::from(env("HOME").unwrap_or_else(|| "/tmp".into())).join(".local/state/pimax"),
    }
}

/// The log file under a state directory.
pub fn log_file(state: &Path) -> PathBuf {
    state.join("logs/pimax-setup.log")
}

/// `2026-10-14T19:40:01.123Z` from seconds and milliseconds since the epoch.
pub fn utc(secs: u64, millis: u32) -> String {
    let s = pimax_config::store::stamp(secs); // 20261014-194001
    format!(
        "{}-{}-{}T{}:{}:{}.{millis:03}Z",
        &s[0..4],
        &s[4..6],
        &s[6..8],
        &s[9..11],
        &s[11..13],
        &s[13..15]
    )
}

/// Now, as [`utc`].
pub fn now_utc() -> String {
    let d = SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default();
    utc(d.as_secs(), d.subsec_millis())
}

/// Rotate `path` if it is larger than `max`: `log.3` -> `log.4`, ..., `log` -> `log.1`; the oldest is dropped.
pub fn rotate(path: &Path, max: u64, keep: usize) -> std::io::Result<bool> {
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len <= max {
        return Ok(false);
    }
    let name = |i: usize| -> PathBuf {
        let mut s = path.as_os_str().to_owned();
        s.push(format!(".{i}"));
        PathBuf::from(s)
    };
    let _ = std::fs::remove_file(name(keep - 1));
    for i in (1..keep - 1).rev() {
        let _ = std::fs::rename(name(i), name(i + 1));
    }
    std::fs::rename(path, name(1))?;
    Ok(true)
}

struct Logger {
    file: Mutex<Option<File>>,
    written: std::sync::atomic::AtomicU64,
    ring: Mutex<VecDeque<String>>,
    stderr: bool,
    tag: &'static str,
}

static LOGGER: OnceLock<Logger> = OnceLock::new();

thread_local! {
    static IN_LOG: Cell<bool> = const { Cell::new(false) };
}

/// `log::error!` unless this thread is inside the logger already (the panic hook: a panic while the logger holds
/// its lock must not deadlock on it).
pub fn error_unless_logging(msg: &str) {
    if !IN_LOG.with(|f| f.get()) {
        log::error!("{msg}");
    }
}

fn level_for(target: &str) -> log::LevelFilter {
    // the graphics and windowing crates are chatty: only their warnings and errors; the minidump writer warns about
    // every library without a build id
    const QUIET: [&str; 9] = ["wgpu", "naga", "winit", "sctk", "smithay", "calloop", "egui_wgpu", "egui_glow", "zbus"];
    if target.starts_with("minidump_writer") || target.starts_with("minidumper") {
        return log::LevelFilter::Error;
    }
    if QUIET.iter().any(|q| target.starts_with(q)) {
        log::LevelFilter::Warn
    } else {
        log::LevelFilter::Info
    }
}

impl log::Log for Logger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= level_for(m.target())
    }

    fn log(&self, r: &log::Record) {
        if !self.enabled(r.metadata()) {
            return;
        }
        IN_LOG.with(|f| f.set(true));
        let mut line = format!("{} {} {:5} {}: {}", now_utc(), self.tag, r.level(), r.target(), r.args());
        line.push('\n');
        // one write per line: the supervisor and the window process append to the same file
        use std::sync::atomic::Ordering;
        let before = self.written.fetch_add(line.len() as u64, Ordering::Relaxed);
        if before < FILE_CAP {
            if let Ok(mut f) = self.file.lock() {
                if let Some(f) = f.as_mut() {
                    let _ = f.write_all(line.as_bytes());
                    if before + line.len() as u64 >= FILE_CAP {
                        let _ = f.write_all(b"(log file limit of this process reached: further lines only in memory)\n");
                    }
                }
            }
        }
        if self.stderr {
            let _ = std::io::stderr().write_all(line.as_bytes());
        }
        line.pop();
        if let Ok(mut ring) = self.ring.lock() {
            ring.push_back(line);
            if ring.len() > RING {
                ring.pop_front();
            }
        }
        IN_LOG.with(|f| f.set(false));
    }

    fn flush(&self) {
        if let Ok(mut f) = self.file.lock() {
            if let Some(f) = f.as_mut() {
                let _ = f.flush();
            }
        }
    }
}

/// Install the logger (once per process). `tag` names the process in each line (`ui`, `supervisor`); `rotate_now`
/// rotates first (only one process should rotate). Failing to open the file still installs the in-memory part.
pub fn init(path: &Path, tag: &'static str, stderr: bool, rotate_now: bool) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    if rotate_now {
        let _ = rotate(path, MAX_BYTES, KEEP);
    }
    let file = OpenOptions::new().create(true).append(true).open(path).ok();
    let opened = file.is_some();
    let logger = Logger {
        file: Mutex::new(file),
        written: std::sync::atomic::AtomicU64::new(0),
        ring: Mutex::new(VecDeque::with_capacity(RING)),
        stderr,
        tag,
    };
    if LOGGER.set(logger).is_err() {
        return Ok(()); // already installed
    }
    let l = LOGGER.get().expect("set above");
    log::set_logger(l).map_err(|e| e.to_string())?;
    log::set_max_level(log::LevelFilter::Info);
    if opened {
        Ok(())
    } else {
        Err(format!("cannot open {}", path.display()))
    }
}

/// The last `n` lines logged by this process. Never blocks: called from the panic hook, which may run while the
/// logger is locked.
pub fn recent(n: usize) -> Vec<String> {
    let Some(l) = LOGGER.get() else { return Vec::new() };
    let Ok(ring) = l.ring.try_lock() else { return vec!["(log busy at the time of the crash)".into()] };
    ring.iter().skip(ring.len().saturating_sub(n)).cloned().collect()
}

/// The last `n` lines of a log file on disk (any process), reading at most 1 MB from the end.
pub fn tail_file(path: &Path, n: usize) -> Vec<String> {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = File::open(path) else { return Vec::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let from = len.saturating_sub(1 << 20);
    if f.seek(SeekFrom::Start(from)).is_err() {
        return Vec::new();
    }
    let mut buf = Vec::new();
    let _ = f.take(1 << 20).read_to_end(&mut buf);
    let text = String::from_utf8_lossy(&buf);
    let lines: Vec<&str> = text.lines().collect();
    let skip = usize::from(from > 0); // the first line may be cut
    let start = lines.len().saturating_sub(n).max(skip.min(lines.len()));
    lines[start..].iter().map(|s| s.to_string()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn utc_format_and_state_dir() {
        assert_eq!(utc(1_791_968_155, 7), "2026-10-14T08:55:55.007Z");
        let env = |k: &str| (k == "HOME").then(|| "/home/u".to_string());
        assert_eq!(state_dir(&env), PathBuf::from("/home/u/.local/state/pimax"));
        let env2 = |k: &str| match k {
            "XDG_STATE_HOME" => Some("/s".to_string()),
            "HOME" => Some("/home/u".to_string()),
            _ => None,
        };
        assert_eq!(log_file(&state_dir(&env2)), PathBuf::from("/s/pimax/logs/pimax-setup.log"));
    }

    #[test]
    fn rotation_keeps_the_newest_files() {
        let d = std::env::temp_dir().join(format!("pimax-applog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        let p = d.join("a.log");
        for gen in 0..7 {
            std::fs::write(&p, format!("gen{gen} ").repeat(10)).unwrap();
            assert!(rotate(&p, 10, 4).unwrap());
        }
        assert!(!p.exists());
        assert!(std::fs::read_to_string(d.join("a.log.1")).unwrap().starts_with("gen6"));
        assert!(std::fs::read_to_string(d.join("a.log.3")).unwrap().starts_with("gen4"));
        assert!(!d.join("a.log.4").exists(), "only keep-1 rotated files");
        std::fs::write(&p, "small").unwrap();
        assert!(!rotate(&p, 10, 4).unwrap());
        std::fs::write(&p, "l1\nl2\nl3\n").unwrap();
        assert_eq!(tail_file(&p, 2), vec!["l2", "l3"]);
        let _ = std::fs::remove_dir_all(d);
    }
}
