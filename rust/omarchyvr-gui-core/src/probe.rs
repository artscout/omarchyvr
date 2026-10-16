//! [`ProbeMachine`]: a [`Machine`] wrapper for responsiveness tests. Every file, `/proc`, `/sys` or command access is
//! slowed down by a fixed delay (a slow disk, a hung `journalctl`), and accesses made from a watched thread (the UI
//! thread of a test) are recorded: the UI must never touch the machine itself, so a test can assert that the list
//! stays empty and that no frame took longer than its budget.
use crate::machine::{FileMeta, Machine};
use omarchyvr_doctor::{CmdOut, Host};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::thread::ThreadId;
use std::time::Duration;

/// The wrapper.
pub struct ProbeMachine {
    inner: Arc<dyn Machine>,
    delay: Duration,
    watched: Mutex<Option<ThreadId>>,
    seen: Mutex<Vec<String>>,
}

impl ProbeMachine {
    /// Wrap `inner`; every access sleeps `delay`.
    pub fn new(inner: Arc<dyn Machine>, delay: Duration) -> ProbeMachine {
        ProbeMachine {
            inner,
            delay,
            watched: Mutex::new(None),
            seen: Mutex::new(Vec::new()),
        }
    }

    /// Record the accesses made from the calling thread from now on.
    pub fn watch_this_thread(&self) {
        if let Ok(mut w) = self.watched.lock() {
            *w = Some(std::thread::current().id());
        }
    }

    /// Accesses made from the watched thread.
    pub fn violations(&self) -> Vec<String> {
        self.seen.lock().map(|v| v.clone()).unwrap_or_default()
    }

    fn hit(&self, what: impl FnOnce() -> String) {
        let watched = self.watched.lock().ok().and_then(|w| *w);
        if watched == Some(std::thread::current().id()) {
            if let Ok(mut v) = self.seen.lock() {
                if v.len() < 1000 {
                    v.push(what());
                }
            }
        }
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
    }
}

impl Host for ProbeMachine {
    fn read(&self, p: &Path) -> Option<Vec<u8>> {
        self.hit(|| format!("read {}", p.display()));
        self.inner.read(p)
    }
    fn list(&self, p: &Path) -> Vec<String> {
        self.hit(|| format!("list {}", p.display()));
        self.inner.list(p)
    }
    fn exists(&self, p: &Path) -> bool {
        self.hit(|| format!("exists {}", p.display()));
        self.inner.exists(p)
    }
    fn can_rw(&self, p: &Path) -> bool {
        self.hit(|| format!("can_rw {}", p.display()));
        self.inner.can_rw(p)
    }
    fn run(&self, prog: &str, args: &[&str]) -> Option<CmdOut> {
        self.hit(|| format!("run {prog} {}", args.join(" ")));
        self.inner.run(prog, args)
    }
    fn env(&self, key: &str) -> Option<String> {
        // the environment is in memory: not an access
        self.inner.env(key)
    }
}

impl Machine for ProbeMachine {
    fn host(&self) -> &dyn Host {
        self
    }
    fn read_link(&self, p: &Path) -> Option<PathBuf> {
        self.hit(|| format!("read_link {}", p.display()));
        self.inner.read_link(p)
    }
    fn meta(&self, p: &Path) -> Option<FileMeta> {
        self.hit(|| format!("meta {}", p.display()));
        self.inner.meta(p)
    }
    fn free_bytes(&self, p: &Path) -> Option<u64> {
        self.hit(|| format!("free_bytes {}", p.display()));
        self.inner.free_bytes(p)
    }
    fn now_unix(&self) -> u64 {
        self.inner.now_unix()
    }
    fn uid(&self) -> u32 {
        self.inner.uid()
    }
    fn gid(&self) -> u32 {
        self.inner.gid()
    }
    fn clk_tck(&self) -> u64 {
        self.inner.clk_tck()
    }
    fn read_range(&self, p: &Path, offset: u64, max: usize) -> Option<Vec<u8>> {
        self.hit(|| format!("read_range {}", p.display()));
        self.inner.read_range(p, offset, max)
    }
    fn can_signal(&self) -> bool {
        self.inner.can_signal()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;

    #[test]
    fn records_only_the_watched_thread() {
        let mut f = FakeMachine::new();
        f.file("/a", "x");
        let p = Arc::new(ProbeMachine::new(Arc::new(f), Duration::ZERO));
        let q = p.clone();
        std::thread::spawn(move || {
            assert!(q.exists(Path::new("/a")));
        })
        .join()
        .unwrap();
        p.watch_this_thread();
        assert!(p.violations().is_empty(), "other threads are fine");
        assert_eq!(p.read_str(Path::new("/a")).as_deref(), Some("x"));
        let _ = p.env("HOME");
        assert_eq!(p.violations(), vec!["read /a".to_string()]);
    }
}
