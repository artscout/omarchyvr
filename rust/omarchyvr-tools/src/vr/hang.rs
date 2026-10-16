//! Evidence for a hung `monado-service` that is about to be SIGKILLed: thread stacks (eu-stack, or gdb as fallback),
//! the state of every thread from `/proc`, and a copy of the log, so the next hang can be diagnosed.
//!
//! Capturing is best effort and bounded in time: it must never keep the kill from happening.
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Longest a stack dump may take.
pub const DUMP_TIMEOUT: Duration = Duration::from_secs(15);

/// Runs `program args...`; returns its output (stdout then stderr), or `None` if it could not run, failed or timed out.
pub type Runner<'a> = &'a dyn Fn(&str, &[String], Duration) -> Option<String>;

/// Where the debuggers are and where the files go.
pub struct HangCapture {
    pub session_dir: PathBuf,
    pub proc_root: PathBuf,
    pub eu_stack: PathBuf,
    pub gdb: String,
}

/// Real runner: the child gets `timeout` and is killed after it.
pub fn run_with_timeout(program: &str, args: &[String], timeout: Duration) -> Option<String> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let drain = |r: Option<Box<dyn Read + Send>>| {
        std::thread::spawn(move || {
            let mut s = String::new();
            if let Some(mut r) = r {
                let mut b = Vec::new();
                let _ = r.read_to_end(&mut b);
                s = String::from_utf8_lossy(&b).into_owned();
            }
            s
        })
    };
    let out = drain(child.stdout.take().map(|r| Box::new(r) as _));
    let err = drain(child.stderr.take().map(|r| Box::new(r) as _));
    let t0 = Instant::now();
    let ok = loop {
        match child.try_wait() {
            Ok(Some(st)) => break st.success(),
            Ok(None) if t0.elapsed() < timeout => std::thread::sleep(Duration::from_millis(50)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                break false;
            }
        }
    };
    let mut text = out.join().unwrap_or_default();
    text.push_str(&err.join().unwrap_or_default());
    (ok && !text.trim().is_empty()).then_some(text)
}

impl HangCapture {
    /// Write `hang-<ts>.txt` and `monado-killed-<ts>.log` into the session directory. Returns the paths written.
    pub fn capture(&self, pid: u32, ts: u64, run: Runner) -> Vec<PathBuf> {
        let mut report = format!("monado-service pid {pid}, unix time {ts}\n\n== stacks ==\n");
        let started = Instant::now();
        let mut stacks = None;
        if self.eu_stack.exists() {
            let args = ["-p".to_string(), pid.to_string()];
            stacks = run(&self.eu_stack.to_string_lossy(), &args, DUMP_TIMEOUT)
                .map(|s| format!("(eu-stack)\n{s}"));
        }
        if stacks.is_none() {
            let left = DUMP_TIMEOUT.saturating_sub(started.elapsed());
            if !left.is_zero() {
                let args = [
                    "-q".to_string(),
                    "-p".to_string(),
                    pid.to_string(),
                    "-batch".to_string(),
                    "-ex".to_string(),
                    "thread apply all bt 15".to_string(),
                ];
                stacks = run(&self.gdb, &args, left).map(|s| format!("(gdb)\n{s}"));
            }
        }
        report.push_str(stacks.as_deref().unwrap_or(
            "no stack: eu-stack and gdb are missing, failed (ptrace refused?) or timed out\n",
        ));
        report.push_str("\n== threads (tid comm state wchan) ==\n");
        report.push_str(&self.threads(pid));
        let mut written = Vec::new();
        let hang = self.session_dir.join(format!("hang-{ts}.txt"));
        if std::fs::write(&hang, report).is_ok() {
            written.push(hang);
        }
        let log = self.session_dir.join("monado.log");
        let copy = self.session_dir.join(format!("monado-killed-{ts}.log"));
        if std::fs::copy(&log, &copy).is_ok() {
            written.push(copy);
        }
        written
    }

    fn threads(&self, pid: u32) -> String {
        let dir = self.proc_root.join(pid.to_string()).join("task");
        let Ok(rd) = std::fs::read_dir(&dir) else {
            return "(no /proc task list)\n".into();
        };
        let mut tids: Vec<u32> = rd
            .flatten()
            .filter_map(|e| e.file_name().to_str()?.parse().ok())
            .collect();
        tids.sort_unstable();
        let mut s = String::new();
        for t in tids {
            let d = dir.join(t.to_string());
            let rd = |f: &str| {
                std::fs::read_to_string(d.join(f))
                    .map(|x| x.trim().to_string())
                    .unwrap_or_else(|_| "?".into())
            };
            let stat = rd("stat");
            let state = stat
                .rfind(')')
                .and_then(|i| stat[i + 1..].split_whitespace().next())
                .unwrap_or("?");
            s.push_str(&format!("{t} {} {state} {}\n", rd("comm"), rd("wchan")));
        }
        s
    }
}

/// Seconds since the epoch.
pub fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The sentence for the warning and the notification.
pub fn describe(paths: &[PathBuf]) -> Option<String> {
    if paths.is_empty() {
        return None;
    }
    let l: Vec<String> = paths
        .iter()
        .map(|p: &PathBuf| p.display().to_string())
        .collect();
    Some(format!("evidence saved: {}", l.join(", ")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    fn setup(name: &str) -> (HangCapture, PathBuf) {
        let d = crate::vr::test_dir(&format!("hang-{name}"));
        let task = d.join("proc/77/task/77");
        std::fs::create_dir_all(&task).unwrap();
        std::fs::write(task.join("comm"), "monado-service\n").unwrap();
        std::fs::write(task.join("stat"), "77 (monado (x)) D 1 2 3\n").unwrap();
        std::fs::write(task.join("wchan"), "futex_wait_queue\n").unwrap();
        std::fs::create_dir_all(d.join("sess")).unwrap();
        std::fs::write(d.join("sess/monado.log"), "Client 1 disconnected\n").unwrap();
        (
            HangCapture {
                session_dir: d.join("sess"),
                proc_root: d.join("proc"),
                eu_stack: d.join("eu-stack"),
                gdb: "gdb".into(),
            },
            d,
        )
    }

    #[test]
    fn writes_stacks_threads_and_log_copy() {
        let (h, d) = setup("ok");
        std::fs::write(&h.eu_stack, "").unwrap();
        let calls = RefCell::new(vec![]);
        let run = |p: &str, a: &[String], _t: Duration| {
            calls.borrow_mut().push((p.to_string(), a.to_vec()));
            Some("#0 futex_wait\n".to_string())
        };
        let w = h.capture(77, 1234, &run);
        assert_eq!(w.len(), 2);
        let txt = std::fs::read_to_string(d.join("sess/hang-1234.txt")).unwrap();
        assert!(txt.contains("(eu-stack)") && txt.contains("#0 futex_wait"));
        assert!(
            txt.contains("77 monado-service D futex_wait_queue"),
            "{txt}"
        );
        assert_eq!(
            std::fs::read_to_string(d.join("sess/monado-killed-1234.log")).unwrap(),
            "Client 1 disconnected\n"
        );
        assert_eq!(calls.borrow().len(), 1);
        assert_eq!(calls.borrow()[0].1, ["-p", "77"]);
        assert!(describe(&w).unwrap().contains("hang-1234.txt"));
    }

    #[test]
    fn falls_back_to_gdb_and_survives_total_failure() {
        let (h, d) = setup("fallback");
        // no eu-stack file: gdb is used
        let run = |p: &str, a: &[String], _t: Duration| {
            assert_eq!(p, "gdb");
            assert!(a.contains(&"thread apply all bt 15".to_string()));
            Some("Thread 1\n".to_string())
        };
        h.capture(77, 1, &run);
        assert!(std::fs::read_to_string(d.join("sess/hang-1.txt"))
            .unwrap()
            .contains("(gdb)"));
        // nothing works: still writes the /proc part
        let none = |_: &str, _: &[String], _: Duration| None;
        let w = h.capture(77, 2, &none);
        assert_eq!(w.len(), 2);
        assert!(std::fs::read_to_string(d.join("sess/hang-2.txt"))
            .unwrap()
            .contains("no stack"));
        // unknown pid and missing log: no panic, nothing to report for the log
        std::fs::remove_file(d.join("sess/monado.log")).unwrap();
        let w = h.capture(9999, 3, &none);
        assert_eq!(w.len(), 1);
        assert!(describe(&[]).is_none());
    }

    #[test]
    fn the_real_runner_times_out_and_collects_output() {
        let t0 = Instant::now();
        assert!(run_with_timeout("sleep", &["30".into()], Duration::from_millis(300)).is_none());
        assert!(t0.elapsed() < Duration::from_secs(5));
        let out = run_with_timeout("echo", &["hi".into()], Duration::from_secs(5)).unwrap();
        assert_eq!(out.trim(), "hi");
        assert!(run_with_timeout("/nonexistent/x", &[], Duration::from_secs(1)).is_none());
    }
}
