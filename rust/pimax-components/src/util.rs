//! Paths and small file helpers. Every root is overridable so tests use temp trees, never the real system.
use std::path::{Path, PathBuf};

/// Where things are (real or fake).
#[derive(Debug, Clone, PartialEq)]
pub struct Roots {
    /// `$PIMAX_SYSFS_ROOT` or `/sys`.
    pub sys: PathBuf,
    /// `$PIMAX_PROC_ROOT` or `/proc`.
    pub proc_: PathBuf,
    /// The launcher's session dir: `$PIMAX_VR_SESSION_DIR` or `/tmp/pimax-vr-session`.
    pub session_dir: PathBuf,
    /// `$XDG_RUNTIME_DIR` (fallback `/tmp`).
    pub runtime: PathBuf,
    pub home: PathBuf,
    /// `~/.config/pimax` (`$XDG_CONFIG_HOME/pimax`).
    pub config: PathBuf,
    /// `~/.local/state/pimax/ai` (`$XDG_STATE_HOME/pimax/ai`).
    pub ai_state: PathBuf,
}

fn env_path(k: &str) -> Option<PathBuf> {
    std::env::var_os(k).filter(|v| !v.is_empty()).map(PathBuf::from)
}

impl Roots {
    pub fn from_env() -> Roots {
        let home = env_path("HOME").unwrap_or_else(|| "/".into());
        Roots {
            sys: env_path("PIMAX_SYSFS_ROOT").unwrap_or_else(|| "/sys".into()),
            proc_: env_path("PIMAX_PROC_ROOT").unwrap_or_else(|| "/proc".into()),
            session_dir: env_path("PIMAX_VR_SESSION_DIR").unwrap_or_else(|| "/tmp/pimax-vr-session".into()),
            runtime: env_path("XDG_RUNTIME_DIR").unwrap_or_else(|| "/tmp".into()),
            config: env_path("XDG_CONFIG_HOME").unwrap_or_else(|| home.join(".config")).join("pimax"),
            ai_state: env_path("XDG_STATE_HOME").unwrap_or_else(|| home.join(".local/state")).join("pimax/ai"),
            home,
        }
    }

    /// All roots under one directory (tests and sims): `<base>/{sys,proc,session,run,home}`.
    pub fn under(base: &Path) -> Roots {
        let home = base.join("home");
        Roots {
            sys: base.join("sys"),
            proc_: base.join("proc"),
            session_dir: base.join("session"),
            runtime: base.join("run"),
            config: home.join(".config/pimax"),
            ai_state: home.join(".local/state/pimax/ai"),
            home,
        }
    }

    /// `$XDG_RUNTIME_DIR/pimaxd` (`$PIMAXD_DIR` overrides it: tests and the observe-only live check).
    pub fn pimaxd_dir(&self) -> PathBuf {
        env_path("PIMAXD_DIR").unwrap_or_else(|| self.runtime.join("pimaxd"))
    }
    pub fn perf_stats(&self) -> PathBuf {
        env_path("PIMAX_PERF_FILE").unwrap_or_else(|| self.runtime.join("pimax-perf.stats"))
    }
}

/// Read a file and trim it.
pub fn read_trim(p: &Path) -> Option<String> {
    std::fs::read_to_string(p).ok().map(|s| s.trim().to_string())
}

/// Write via a temp file and rename (readers never see half a file).
pub fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension(format!("tmp{}", std::process::id()));
    std::fs::write(&tmp, data)?;
    std::fs::rename(&tmp, path)
}

/// Seconds since the file was modified (None when missing).
pub fn age_s(p: &Path) -> Option<f64> {
    let m = std::fs::metadata(p).ok()?.modified().ok()?;
    Some(std::time::SystemTime::now().duration_since(m).map(|d| d.as_secs_f64()).unwrap_or(0.0))
}

/// The last `max` bytes of a file as text.
pub fn tail(p: &Path, max: u64) -> String {
    use std::io::{Read, Seek, SeekFrom};
    let Ok(mut f) = std::fs::File::open(p) else { return String::new() };
    let len = f.metadata().map(|m| m.len()).unwrap_or(0);
    let _ = f.seek(SeekFrom::Start(len.saturating_sub(max)));
    let mut b = Vec::new();
    let _ = f.take(max).read_to_end(&mut b);
    String::from_utf8_lossy(&b).into_owned()
}

/// Unix time now.
pub fn unix_now() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// A scratch directory for tests (removed when dropped).
pub struct TempDir(pub PathBuf);

impl TempDir {
    pub fn new(name: &str) -> TempDir {
        static N: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
        let n = N.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let d = std::env::temp_dir().join(format!("pimax-comp-{}-{n}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        TempDir(d)
    }
    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// Write `text` to `base/rel`, creating directories (fake trees).
pub fn put(base: &Path, rel: &str, text: &str) {
    let p = base.join(rel);
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d).unwrap();
    }
    std::fs::write(p, text).unwrap();
}
