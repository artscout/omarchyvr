//! `--follow PROCESS`: the headset is busy only while a VR runtime runs (SteamVR's `vrserver`), as Pimax Play keeps
//! it busy only while it runs. [`Follow`] says when to wake it and when to let it sleep: active as soon as the
//! process is there, idle once it has been gone for [`IDLE_AFTER`] (SteamVR restarts its server on its own).
use std::time::{Duration, Instant};

/// How long the process must be gone before the headset goes idle.
pub const IDLE_AFTER: Duration = Duration::from_secs(5);

/// Whether a process with this name (`/proc/<pid>/comm`, at most 15 characters) runs.
pub fn process_running(name: &str) -> bool {
    process_running_in(std::path::Path::new("/proc"), name)
}

/// [`process_running`] over another proc root (tests).
pub fn process_running_in(proc_root: &std::path::Path, name: &str) -> bool {
    let want: String = name.chars().take(15).collect();
    let Ok(dir) = std::fs::read_dir(proc_root) else {
        return false;
    };
    dir.flatten().any(|e| {
        e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit()))
            && std::fs::read_to_string(e.path().join("comm")).is_ok_and(|c| c.trim_end() == want)
    })
}

/// What changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Change {
    /// The runtime started: wake the headset.
    Active,
    /// The runtime has been gone for [`IDLE_AFTER`]: let it sleep.
    Idle,
}

/// The debounced state.
#[derive(Debug)]
pub struct Follow {
    active: bool,
    gone_since: Option<Instant>,
}

impl Follow {
    /// Start in the state the process is in now (no change reported for it).
    pub fn new(running: bool) -> Follow {
        Follow { active: running, gone_since: None }
    }

    /// Whether the headset should be busy now.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Feed one observation; returns the change it causes.
    pub fn step(&mut self, now: Instant, running: bool) -> Option<Change> {
        if running {
            self.gone_since = None;
            if !self.active {
                self.active = true;
                return Some(Change::Active);
            }
            return None;
        }
        if !self.active {
            return None;
        }
        let since = *self.gone_since.get_or_insert(now);
        if now.duration_since(since) >= IDLE_AFTER {
            self.active = false;
            self.gone_since = None;
            return Some(Change::Idle);
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wakes_at_once_and_sleeps_after_the_grace() {
        let t0 = Instant::now();
        let mut f = Follow::new(false);
        assert_eq!(f.step(t0, false), None);
        assert_eq!(f.step(t0, true), Some(Change::Active));
        assert_eq!(f.step(t0 + Duration::from_secs(1), true), None);
        // a short restart of the server keeps it awake
        assert_eq!(f.step(t0 + Duration::from_secs(2), false), None);
        assert_eq!(f.step(t0 + Duration::from_secs(4), true), None);
        assert_eq!(f.step(t0 + Duration::from_secs(5), false), None);
        assert_eq!(f.step(t0 + Duration::from_secs(9), false), None);
        assert_eq!(f.step(t0 + Duration::from_secs(10), false), Some(Change::Idle));
        assert!(!f.active());
        assert_eq!(f.step(t0 + Duration::from_secs(20), false), None);
    }

    #[test]
    fn finds_a_process_by_comm() {
        let root = std::env::temp_dir().join(format!("omarchyvr-follow-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("1234")).unwrap();
        std::fs::write(root.join("1234/comm"), "vrserver\n").unwrap();
        std::fs::create_dir_all(root.join("self")).unwrap();
        std::fs::write(root.join("self/comm"), "vrserver\n").unwrap(); // not a pid: ignored
        assert!(process_running_in(&root, "vrserver"));
        assert!(!process_running_in(&root, "vrcompositor"));
        std::fs::remove_dir_all(root.join("1234")).unwrap();
        assert!(!process_running_in(&root, "vrserver"));
        std::fs::remove_dir_all(&root).unwrap();
    }
}
