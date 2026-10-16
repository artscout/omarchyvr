//! The machine behind the checks.
use std::collections::BTreeSet;
use std::ffi::CString;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Output of a finished command.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CmdOut {
    /// Exit code (None if killed by a signal).
    pub code: Option<i32>,
    /// Standard output.
    pub stdout: String,
    /// Standard error.
    pub stderr: String,
}

/// What the checks may ask about the machine. Paths are absolute (`/sys/...`); implementations may remap them.
pub trait Host {
    /// File content.
    fn read(&self, p: &Path) -> Option<Vec<u8>>;
    /// Names of the entries of a directory, sorted (empty if missing).
    fn list(&self, p: &Path) -> Vec<String>;
    /// Whether the path exists.
    fn exists(&self, p: &Path) -> bool;
    /// Whether the calling user can read and write the path.
    fn can_rw(&self, p: &Path) -> bool;
    /// Run a read-only command; `None` if it cannot be started.
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut>;
    /// Environment variable.
    fn env(&self, key: &str) -> Option<String>;

    /// File content as trimmed text.
    fn read_str(&self, p: &Path) -> Option<String> {
        self.read(p)
            .map(|b| String::from_utf8_lossy(&b).trim().to_string())
    }
}

/// Where `/sys`, `/proc`, `/dev` live (tests point them at temp trees).
#[derive(Debug, Clone)]
pub struct Roots {
    /// Replaces `/sys`.
    pub sys: PathBuf,
    /// Replaces `/proc`.
    pub proc_: PathBuf,
    /// Replaces `/dev`.
    pub dev: PathBuf,
}

impl Default for Roots {
    fn default() -> Self {
        Self {
            sys: "/sys".into(),
            proc_: "/proc".into(),
            dev: "/dev".into(),
        }
    }
}

impl Roots {
    /// From `PIMAX_SYSFS_ROOT` / `PIMAX_PROC_ROOT` / `PIMAX_DEV_ROOT` (same names as the launcher uses for the first two).
    pub fn from_env(env: &dyn Fn(&str) -> Option<String>) -> Self {
        let mut r = Roots::default();
        if let Some(v) = env("PIMAX_SYSFS_ROOT").filter(|v| !v.is_empty()) {
            r.sys = v.into();
        }
        if let Some(v) = env("PIMAX_PROC_ROOT").filter(|v| !v.is_empty()) {
            r.proc_ = v.into();
        }
        if let Some(v) = env("PIMAX_DEV_ROOT").filter(|v| !v.is_empty()) {
            r.dev = v.into();
        }
        r
    }

    /// Map an absolute path.
    pub fn map(&self, p: &Path) -> PathBuf {
        for (prefix, root) in [
            ("/sys", &self.sys),
            ("/proc", &self.proc_),
            ("/dev", &self.dev),
        ] {
            if let Ok(rest) = p.strip_prefix(prefix) {
                return root.join(rest);
            }
        }
        p.to_path_buf()
    }
}

/// The real machine.
pub struct RealHost {
    /// Path remapping.
    pub roots: Roots,
}

impl RealHost {
    /// Real host with the roots from the process environment.
    pub fn from_env() -> Self {
        Self {
            roots: Roots::from_env(&|k| std::env::var(k).ok()),
        }
    }
}

impl Host for RealHost {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        std::fs::read(self.roots.map(p)).ok()
    }
    fn list(&self, p: &Path) -> Vec<String> {
        let s: BTreeSet<String> = std::fs::read_dir(self.roots.map(p))
            .into_iter()
            .flatten()
            .flatten()
            .filter_map(|e| e.file_name().into_string().ok())
            .collect();
        s.into_iter().collect()
    }
    fn exists(&self, p: &Path) -> bool {
        self.roots.map(p).exists()
    }
    fn can_rw(&self, p: &Path) -> bool {
        use std::os::unix::ffi::OsStrExt;
        match CString::new(self.roots.map(p).as_os_str().as_bytes()) {
            // SAFETY: valid NUL-terminated string; access(2) only reads it.
            Ok(c) => unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 },
            Err(_) => false,
        }
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        let o = Command::new(prog)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .ok()?;
        Some(CmdOut {
            code: o.status.code(),
            stdout: String::from_utf8_lossy(&o.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&o.stderr).into_owned(),
        })
    }
    fn env(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roots_map_prefixes_only() {
        let r = Roots {
            sys: "/t/sys".into(),
            proc_: "/t/proc".into(),
            dev: "/t/dev".into(),
        };
        assert_eq!(
            r.map(Path::new("/sys/class/drm")),
            Path::new("/t/sys/class/drm")
        );
        assert_eq!(
            r.map(Path::new("/proc/driver/nvidia/version")),
            Path::new("/t/proc/driver/nvidia/version")
        );
        assert_eq!(
            r.map(Path::new("/dev/hidraw3")),
            Path::new("/t/dev/hidraw3")
        );
        assert_eq!(r.map(Path::new("/system/x")), Path::new("/system/x"));
        assert_eq!(r.map(Path::new("/usr/src")), Path::new("/usr/src"));
    }

    #[test]
    fn real_host_reads_remapped_tree() {
        let d = std::env::temp_dir().join(format!("omarchyvr-doctor-host-{}", std::process::id()));
        std::fs::create_dir_all(d.join("sys/a")).unwrap();
        std::fs::write(d.join("sys/a/x"), "hi\n").unwrap();
        let h = RealHost {
            roots: Roots {
                sys: d.join("sys"),
                ..Roots::default()
            },
        };
        assert_eq!(h.read_str(Path::new("/sys/a/x")).as_deref(), Some("hi"));
        assert_eq!(h.list(Path::new("/sys")), vec!["a"]);
        assert!(h.can_rw(Path::new("/sys/a/x")));
        assert!(!h.exists(Path::new("/sys/nope")));
        let _ = std::fs::remove_dir_all(d);
    }
}
