//! The machine the app looks at. Extends the doctor's [`Host`] (files, directory listings, read-only commands,
//! environment) with what the live status needs: symlinks (`/proc/PID/fd/*`), file metadata, free disk space and
//! byte ranges (log tails, binary marker scans).
//!
//! [`RealMachine`] reads the real system (with the doctor's `/sys` `/proc` `/dev` remapping, so a fake tree on disk works
//! too); [`FakeMachine`] is in memory for tests and the demo screenshots. Nothing here writes anything.
use omarchyvr_doctor::fake::FakeHost;
use omarchyvr_doctor::{CmdOut, Host, RealHost, Roots};
use std::collections::BTreeMap;
use std::ffi::CString;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

/// What the app needs to know about a file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FileMeta {
    /// Size in bytes.
    pub len: u64,
    /// Modification time (seconds since the epoch).
    pub mtime: u64,
    /// `st_mode` (permission bits included).
    pub mode: u32,
    /// Inode number (a new log file after a restart has a new one).
    pub ino: u64,
    /// Is a directory.
    pub is_dir: bool,
}

/// Read-only view of the machine.
pub trait Machine: Host + Send + Sync {
    /// The same machine as a doctor [`Host`] (for `omarchyvr_doctor::run_checks`).
    fn host(&self) -> &dyn Host;
    /// Target of a symlink.
    fn read_link(&self, p: &Path) -> Option<PathBuf>;
    /// Metadata (follows symlinks).
    fn meta(&self, p: &Path) -> Option<FileMeta>;
    /// Bytes available to an unprivileged user on the file system that holds `p`.
    fn free_bytes(&self, p: &Path) -> Option<u64>;
    /// Wall clock, seconds since the epoch.
    fn now_unix(&self) -> u64;
    /// Real uid of this process.
    fn uid(&self) -> u32;
    /// Real gid of this process.
    fn gid(&self) -> u32 {
        self.uid()
    }
    /// Kernel clock ticks per second (`/proc/PID/stat` start times).
    fn clk_tck(&self) -> u64 {
        100
    }
    /// Up to `max` bytes of a file starting at `offset`.
    fn read_range(&self, p: &Path, offset: u64, max: usize) -> Option<Vec<u8>>;
    /// May the app signal pids it found here? Only when `/proc` is the real one (a remapped or fake tree names pids
    /// of nothing real).
    fn can_signal(&self) -> bool {
        false
    }
}

/// The read-only commands the checks may run; anything else is refused before it starts.
pub fn read_only_command(prog: &str, args: &[&str]) -> bool {
    let name = prog.rsplit('/').next().unwrap_or(prog);
    let has = |s: &str| args.iter().any(|a| a.contains(s));
    match name {
        "getcap" | "wayland-info" | "modinfo" => true,
        "dmesg" => args.is_empty(),
        "journalctl" => !(has("vacuum") || has("rotate") || has("flush") || has("sync") || has("setup-keys") || has("relinquish")),
        "systemctl" => args.first() == Some(&"--user") && args.get(1) == Some(&"is-active"),
        "patch" => args.contains(&"--dry-run"),
        "omarchyvr" => args == ["desk", "help"],
        "pactl" => matches!(args.first(), Some(&("get-default-source" | "get-default-sink" | "list" | "info"))),
        // the graphics-card check while Play runs: queries only (no GPU work, nothing changed)
        "nvidia-smi" => !args.is_empty() && args.iter().all(|a| a.starts_with("--query-") || a.starts_with("--format=")),
        _ => false,
    }
}

/// How long a read-only command may take.
pub const RUN_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20);

/// The real system.
pub struct RealMachine {
    /// Doctor host (path remapping, commands).
    pub host: RealHost,
}

impl RealMachine {
    /// Honour `PIMAX_SYSFS_ROOT` / `PIMAX_PROC_ROOT` / `PIMAX_DEV_ROOT` like the doctor.
    pub fn from_env() -> Self {
        RealMachine {
            host: RealHost::from_env(),
        }
    }

    /// With explicit roots (tests with a fake `/proc` tree on disk).
    pub fn with_roots(roots: Roots) -> Self {
        RealMachine {
            host: RealHost { roots },
        }
    }

    fn map(&self, p: &Path) -> PathBuf {
        self.host.roots.map(p)
    }
}

impl Host for RealMachine {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        self.host.read(p)
    }
    fn list(&self, p: &Path) -> Vec<String> {
        self.host.list(p)
    }
    fn exists(&self, p: &Path) -> bool {
        self.host.exists(p)
    }
    fn can_rw(&self, p: &Path) -> bool {
        self.host.can_rw(p)
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        if !read_only_command(prog, args) {
            return None;
        }
        let mut child = std::process::Command::new(prog)
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let read = |p: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut v = Vec::new();
                if let Some(mut p) = p {
                    let _ = p.read_to_end(&mut v);
                }
                v
            })
        };
        let t_out = read(child.stdout.take().map(|o| Box::new(o) as Box<dyn Read + Send>));
        let t_err = read(child.stderr.take().map(|o| Box::new(o) as Box<dyn Read + Send>));
        let deadline = std::time::Instant::now() + RUN_TIMEOUT;
        loop {
            match child.try_wait() {
                Ok(Some(st)) => {
                    let so = t_out.join().unwrap_or_default();
                    let se = t_err.join().unwrap_or_default();
                    return Some(CmdOut {
                        code: st.code(),
                        stdout: String::from_utf8_lossy(&so).into_owned(),
                        stderr: String::from_utf8_lossy(&se).into_owned(),
                    });
                }
                Ok(None) if std::time::Instant::now() < deadline => std::thread::sleep(std::time::Duration::from_millis(10)),
                Ok(None) => {
                    // a hung read-only tool (journalctl): SIGTERM our own child, reap it in the background
                    log::warn!("{prog} {} did not finish in {} s: stopped", args.join(" "), RUN_TIMEOUT.as_secs());
                    // SAFETY: kill(2) with our own child's positive pid and SIGTERM.
                    unsafe {
                        libc::kill(child.id() as libc::pid_t, libc::SIGTERM);
                    }
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return None;
                }
                Err(e) => {
                    // waitpid failed (not a timeout): nothing to signal; reap it if it is still ours
                    log::warn!("{prog}: cannot wait for it: {e}");
                    std::thread::spawn(move || {
                        let _ = child.wait();
                    });
                    return None;
                }
            }
        }
    }
    fn env(&self, key: &str) -> Option<String> {
        self.host.env(key)
    }
}

impl Machine for RealMachine {
    fn can_signal(&self) -> bool {
        self.host.roots.proc_ == Path::new("/proc")
    }
    fn host(&self) -> &dyn Host {
        self
    }
    fn read_link(&self, p: &Path) -> Option<PathBuf> {
        std::fs::read_link(self.map(p)).ok()
    }
    fn meta(&self, p: &Path) -> Option<FileMeta> {
        use std::os::unix::fs::MetadataExt;
        let m = std::fs::metadata(self.map(p)).ok()?;
        Some(FileMeta {
            len: m.len(),
            mtime: m.mtime().max(0) as u64,
            mode: m.mode(),
            ino: m.ino(),
            is_dir: m.is_dir(),
        })
    }
    fn free_bytes(&self, p: &Path) -> Option<u64> {
        use std::os::unix::ffi::OsStrExt;
        let c = CString::new(self.map(p).as_os_str().as_bytes()).ok()?;
        // SAFETY: statvfs(3) fills the zeroed struct; the path is a valid NUL-terminated string.
        unsafe {
            let mut s: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut s) != 0 {
                return None;
            }
            Some((s.f_bavail as u64).saturating_mul(s.f_frsize as u64))
        }
    }
    fn now_unix(&self) -> u64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    }
    fn uid(&self) -> u32 {
        // SAFETY: getuid(2) has no arguments and cannot fail.
        unsafe { libc::getuid() }
    }
    fn gid(&self) -> u32 {
        // SAFETY: getgid(2) has no arguments and cannot fail.
        unsafe { libc::getgid() }
    }
    fn clk_tck(&self) -> u64 {
        // SAFETY: sysconf(3) only reads a constant.
        let t = unsafe { libc::sysconf(libc::_SC_CLK_TCK) };
        if t > 0 {
            t as u64
        } else {
            100
        }
    }
    fn read_range(&self, p: &Path, offset: u64, max: usize) -> Option<Vec<u8>> {
        let mut f = std::fs::File::open(self.map(p)).ok()?;
        f.seek(SeekFrom::Start(offset)).ok()?;
        let mut buf = Vec::with_capacity(max.min(1 << 20));
        f.take(max as u64).read_to_end(&mut buf).ok()?;
        Some(buf)
    }
}

/// In-memory machine: the doctor's [`FakeHost`] plus symlinks, metadata, free space and a clock.
#[derive(Default, Clone)]
pub struct FakeMachine {
    /// Files, directories, commands, environment, access rights.
    pub host: FakeHost,
    /// Symlinks (`/proc/42/fd/3 -> /dev/video0`).
    pub links: BTreeMap<PathBuf, PathBuf>,
    /// Explicit metadata; files without an entry get their content length and mtime 0.
    pub metas: BTreeMap<PathBuf, FileMeta>,
    /// Free bytes by mount prefix (longest prefix wins).
    pub free: BTreeMap<PathBuf, u64>,
    /// Clock.
    pub now: u64,
    /// Uid.
    pub uid: u32,
}

impl FakeMachine {
    /// Empty machine at a fixed time.
    pub fn new() -> Self {
        FakeMachine {
            now: 1_791_968_155,
            uid: 1000,
            ..Default::default()
        }
    }

    /// Add a text or binary file.
    pub fn file(&mut self, p: &str, c: impl AsRef<[u8]>) -> &mut Self {
        self.host.file(p, c);
        self
    }

    /// Add a command result (`"prog arg arg"`).
    pub fn cmd(&mut self, line: &str, code: i32, stdout: &str) -> &mut Self {
        self.host.cmd(line, code, stdout);
        self
    }

    /// Add a symlink.
    pub fn link(&mut self, p: &str, target: &str) -> &mut Self {
        self.links.insert(p.into(), target.into());
        self
    }

    /// Set an environment variable.
    pub fn set_env(&mut self, k: &str, v: &str) -> &mut Self {
        self.host.env.insert(k.into(), v.into());
        self
    }

    /// Set metadata of a path.
    pub fn set_meta(&mut self, p: &str, m: FileMeta) -> &mut Self {
        self.metas.insert(p.into(), m);
        self
    }

    /// A process: `comm`, `cmdline` (space separated words), `environ`, a `stat` line with the start time in ticks,
    /// and `fd` symlinks.
    pub fn process(
        &mut self,
        pid: u32,
        comm: &str,
        cmdline: &str,
        environ: &[(&str, &str)],
        start_ticks: u64,
        fds: &[&str],
    ) -> &mut Self {
        let base = format!("/proc/{pid}");
        self.file(&format!("{base}/comm"), format!("{comm}\n"));
        let mut cl: Vec<u8> = Vec::new();
        for w in cmdline.split(' ').filter(|w| !w.is_empty()) {
            cl.extend_from_slice(w.as_bytes());
            cl.push(0);
        }
        self.file(&format!("{base}/cmdline"), cl);
        let mut env: Vec<u8> = Vec::new();
        for (k, v) in environ {
            env.extend_from_slice(format!("{k}={v}").as_bytes());
            env.push(0);
        }
        self.file(&format!("{base}/environ"), env);
        self.file(
            &format!("{base}/stat"),
            format!("{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194560 0 0 0 0 0 0 0 0 20 0 1 0 {start_ticks} 0 0\n"),
        );
        for (i, t) in fds.iter().enumerate() {
            self.link(&format!("{base}/fd/{}", i + 3), t);
        }
        if let Some(exe) = cmdline.split(' ').next().filter(|e| e.starts_with('/')) {
            self.link(&format!("{base}/exe"), exe);
        }
        self
    }
}

impl Host for FakeMachine {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        self.host.read(p)
    }
    fn list(&self, p: &Path) -> Vec<String> {
        let mut v = self.host.list(p);
        for l in self.links.keys().chain(self.metas.keys()) {
            if let Ok(rest) = l.strip_prefix(p) {
                if let Some(c) = rest.components().next() {
                    let n = c.as_os_str().to_string_lossy().into_owned();
                    if !v.contains(&n) {
                        v.push(n);
                    }
                }
            }
        }
        v.sort();
        v
    }
    fn exists(&self, p: &Path) -> bool {
        self.host.exists(p) || self.links.contains_key(p) || self.metas.contains_key(p)
    }
    fn can_rw(&self, p: &Path) -> bool {
        self.host.can_rw(p)
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        if !read_only_command(prog, args) {
            return None;
        }
        self.host.run(prog, args)
    }
    fn env(&self, key: &str) -> Option<String> {
        self.host.env(key)
    }
}

impl Machine for FakeMachine {
    fn host(&self) -> &dyn Host {
        self
    }
    fn read_link(&self, p: &Path) -> Option<PathBuf> {
        self.links.get(p).cloned()
    }
    fn meta(&self, p: &Path) -> Option<FileMeta> {
        if let Some(m) = self.metas.get(p) {
            return Some(*m);
        }
        if let Some(c) = self.host.files.get(p) {
            return Some(FileMeta {
                len: c.len() as u64,
                mode: 0o100644,
                ..FileMeta::default()
            });
        }
        if self.host.exists(p) {
            return Some(FileMeta {
                is_dir: true,
                mode: 0o40755,
                ..FileMeta::default()
            });
        }
        None
    }
    fn free_bytes(&self, p: &Path) -> Option<u64> {
        self.free
            .iter()
            .filter(|(k, _)| p.starts_with(k))
            .max_by_key(|(k, _)| k.as_os_str().len())
            .map(|(_, v)| *v)
    }
    fn now_unix(&self) -> u64 {
        self.now
    }
    fn uid(&self) -> u32 {
        self.uid
    }
    fn read_range(&self, p: &Path, offset: u64, max: usize) -> Option<Vec<u8>> {
        let c = self.host.files.get(p)?;
        let start = (offset as usize).min(c.len());
        let end = start.saturating_add(max).min(c.len());
        Some(c[start..end].to_vec())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fake_lists_links_and_files_together() {
        let mut m = FakeMachine::new();
        m.process(42, "pimax-cam", "/x/pimax-cam broker", &[], 100, &["/dev/video0", "socket:[1]"]);
        assert_eq!(m.list(Path::new("/proc/42/fd")), vec!["3", "4"]);
        assert_eq!(
            m.read_link(Path::new("/proc/42/fd/3")),
            Some(PathBuf::from("/dev/video0"))
        );
        assert_eq!(m.list(Path::new("/proc")), vec!["42"]);
        assert_eq!(m.read_str(Path::new("/proc/42/comm")).as_deref(), Some("pimax-cam"));
        assert_eq!(m.read_range(Path::new("/proc/42/comm"), 6, 3).unwrap(), b"cam");
    }

    #[test]
    fn only_read_only_commands_run() {
        for (p, a) in [
            ("getcap", &["/x"][..]),
            ("journalctl", &["-k", "-b", "--no-pager", "-q"]),
            ("dmesg", &[]),
            ("systemctl", &["--user", "is-active", "omarchyvr-keepalived"]),
            ("patch", &["-R", "-p1", "--dry-run", "-d", "/usr/src/x", "-i", "/p"]),
            ("modinfo", &["-F", "parm", "nvidia-modeset"]),
            ("/home/u/.local/bin/omarchyvr", &["desk", "help"]),
        ] {
            assert!(read_only_command(p, a), "{p} {a:?}");
        }
        for (p, a) in [
            ("dmesg", &["-c"][..]),
            ("journalctl", &["--vacuum-time=1s"]),
            ("systemctl", &["--user", "stop", "omarchyvr-keepalived"]),
            ("systemctl", &["is-active", "x"]),
            ("patch", &["-p1", "-d", "/usr/src/x", "-i", "/p"]),
            ("/home/u/.local/bin/omarchyvr", &["desk", "start"]),
            ("sh", &["-c", "true"]),
        ] {
            assert!(!read_only_command(p, a), "{p} {a:?}");
        }
        let m = RealMachine::from_env();
        assert!(m.run("sh", &["-c", "true"]).is_none(), "refused before starting");
    }

    #[test]
    fn real_machine_reads_a_fake_proc_tree_on_disk() {
        let d = std::env::temp_dir().join(format!("omarchyvr-gui-core-machine-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("proc/77/fd")).unwrap();
        std::fs::write(d.join("proc/77/comm"), "wayvr\n").unwrap();
        std::os::unix::fs::symlink("/dev/video2", d.join("proc/77/fd/5")).unwrap();
        let m = RealMachine::with_roots(Roots {
            proc_: d.join("proc"),
            ..Roots::default()
        });
        assert_eq!(m.list(Path::new("/proc/77/fd")), vec!["5"]);
        assert_eq!(
            m.read_link(Path::new("/proc/77/fd/5")),
            Some(PathBuf::from("/dev/video2"))
        );
        assert!(m.meta(Path::new("/proc/77/comm")).unwrap().len == 6);
        assert!(m.free_bytes(Path::new("/proc")).is_some());
        assert!(!m.can_signal(), "a remapped /proc names no real pids");
        assert!(RealMachine::with_roots(Roots::default()).can_signal());
        let _ = std::fs::remove_dir_all(d);
    }
}
