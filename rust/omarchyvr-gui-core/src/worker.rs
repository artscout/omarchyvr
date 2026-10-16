//! Background tasks for everything a click or a page needs from the machine (log tails, the log list, the docs,
//! terminals, signals, profile files, saving the app state): the UI thread never waits for a file system, a process
//! or `/proc`. A task runs on its own thread and returns a closure that the UI thread applies to the
//! [`AppCore`](crate::app::AppCore) in `poll()`.
//!
//! Tests, screenshots and `--self-test` use the inline worker, which runs a task right away on the calling thread
//! (deterministic, no threads).
use crate::app::AppCore;
use std::collections::HashSet;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

/// What a task hands back to the UI thread.
pub type Apply = Box<dyn FnOnce(&mut AppCore) + Send>;

/// The text of a panic payload.
pub fn panic_text(p: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = p.downcast_ref::<&str>() {
        s.to_string()
    } else if let Some(s) = p.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic".into()
    }
}

/// Runs tasks in the background (or inline).
pub struct Worker {
    inline: bool,
    tx: Sender<Apply>,
    rx: Receiver<Apply>,
    running: Arc<Mutex<HashSet<&'static str>>>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Worker {
    /// Tasks run right away on the calling thread.
    pub fn inline() -> Worker {
        let (tx, rx) = channel();
        Worker {
            inline: true,
            tx,
            rx,
            running: Arc::default(),
            wake: Arc::new(|| {}),
        }
    }

    /// Tasks run on their own threads; `wake` asks the UI for a frame when one is done.
    pub fn threaded(wake: Arc<dyn Fn() + Send + Sync>) -> Worker {
        let (tx, rx) = channel();
        Worker {
            inline: false,
            tx,
            rx,
            running: Arc::default(),
            wake,
        }
    }

    /// Inline mode?
    pub fn is_inline(&self) -> bool {
        self.inline
    }

    /// Is a task with this key running?
    pub fn busy(&self, key: &str) -> bool {
        self.running.lock().map(|r| r.contains(key)).unwrap_or(false)
    }

    /// Run `task`. With a `key`, a task does not start while another one with the same key still runs (a hung file
    /// system cannot pile up threads); returns false then. A panicking task becomes an error message.
    pub fn run<F>(&self, key: Option<&'static str>, task: F) -> bool
    where
        F: FnOnce() -> Apply + Send + 'static,
    {
        if let Some(k) = key {
            match self.running.lock() {
                Ok(mut r) => {
                    if !r.insert(k) {
                        return false;
                    }
                }
                Err(_) => return false,
            }
        }
        let name = key.unwrap_or("task");
        let body = {
            let tx = self.tx.clone();
            let running = self.running.clone();
            let wake = self.wake.clone();
            move || {
                let apply = match catch_unwind(AssertUnwindSafe(task)) {
                    Ok(a) => a,
                    Err(p) => {
                        let msg = format!("internal error in the {name} task: {}", panic_text(p.as_ref()));
                        log::error!("{msg}");
                        Box::new(move |c: &mut AppCore| c.message = msg) as Apply
                    }
                };
                if let Some(k) = key {
                    if let Ok(mut r) = running.lock() {
                        r.remove(k);
                    }
                }
                let _ = tx.send(apply);
                wake();
            }
        };
        if self.inline {
            body();
            return true;
        }
        let spawned = std::thread::Builder::new().name(format!("pimax-io-{name}")).spawn(body);
        if let Err(e) = spawned {
            log::error!("cannot start a background task ({name}): {e}");
            if let Some(k) = key {
                if let Ok(mut r) = self.running.lock() {
                    r.remove(k);
                }
            }
            return false;
        }
        true
    }

    /// Results since the last call.
    pub fn drain(&self) -> Vec<Apply> {
        self.rx.try_iter().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn core() -> AppCore {
        let d = std::env::temp_dir().join(format!("pimax-worker-{}", std::process::id()));
        AppCore::demo(false, omarchyvr_config::Store::new(d.join("cfg"), d.join("cache")))
    }

    #[test]
    fn keyed_tasks_do_not_overlap_and_panics_become_messages() {
        let w = Worker::threaded(Arc::new(|| {}));
        let gate = Arc::new(Mutex::new(()));
        let held = gate.lock().unwrap();
        let g = gate.clone();
        assert!(w.run(Some("tail"), move || {
            let _x = g.lock();
            Box::new(|c: &mut AppCore| c.message = "tail done".into())
        }));
        assert!(w.busy("tail"));
        assert!(!w.run(Some("tail"), || Box::new(|_: &mut AppCore| {})), "second one while the first hangs");
        assert!(w.run(None, || panic!("boom")));
        drop(held);
        let mut c = core();
        let t0 = Instant::now();
        let mut got = Vec::new();
        while got.len() < 2 && t0.elapsed() < Duration::from_secs(5) {
            for a in w.drain() {
                a(&mut c);
                got.push(c.message.clone());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(got.iter().any(|m| m == "tail done"), "{got:?}");
        assert!(got.iter().any(|m| m.contains("internal error in the task task: boom")), "{got:?}");
        assert!(!w.busy("tail"));
    }

    #[test]
    fn inline_runs_now() {
        let w = Worker::inline();
        assert!(w.run(Some("x"), || Box::new(|c: &mut AppCore| c.message = "now".into())));
        let mut c = core();
        for a in w.drain() {
            a(&mut c);
        }
        assert_eq!(c.message, "now");
        assert!(!w.busy("x"));
    }
}
