//! The process table, read from `/proc` through a [`Machine`]: pids by name (LESSONS row 1: is a monado-service still
//! there?), process ages, environments (what mode the running service was started in) and the holders of device nodes
//! (row 2: who streams `/dev/video0`; row 3: who has the eye chip open). Read-only; signals are never sent from here.
use crate::machine::Machine;
use std::path::{Path, PathBuf};

/// Process name (`/proc/PID/comm`) of the Monado service.
pub const MONADO_COMM: &str = "monado-service";

/// All numeric entries of `/proc`.
pub fn pids(m: &dyn Machine) -> Vec<u32> {
    let mut v: Vec<u32> = m
        .list(Path::new("/proc"))
        .into_iter()
        .filter_map(|n| n.parse::<u32>().ok())
        .collect();
    v.sort_unstable();
    v
}

fn proc_path(pid: u32, f: &str) -> PathBuf {
    PathBuf::from(format!("/proc/{pid}/{f}"))
}

/// `/proc/PID/comm`.
pub fn comm(m: &dyn Machine, pid: u32) -> Option<String> {
    m.read_str(&proc_path(pid, "comm"))
}

/// `/proc/PID/cmdline` split at NUL.
pub fn cmdline(m: &dyn Machine, pid: u32) -> Vec<String> {
    m.read(&proc_path(pid, "cmdline"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter(|w| !w.is_empty())
                .map(|w| String::from_utf8_lossy(w).into_owned())
                .collect()
        })
        .unwrap_or_default()
}

/// `/proc/PID/environ` as pairs.
pub fn environ(m: &dyn Machine, pid: u32) -> Vec<(String, String)> {
    m.read(&proc_path(pid, "environ"))
        .map(|b| {
            b.split(|c| *c == 0)
                .filter_map(|kv| {
                    let s = String::from_utf8_lossy(kv);
                    s.split_once('=').map(|(k, v)| (k.to_string(), v.to_string()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The fields after the `(comm)` of `/proc/PID/stat` (index 0 = state).
fn stat_fields(m: &dyn Machine, pid: u32) -> Option<Vec<String>> {
    let s = m.read_str(&proc_path(pid, "stat"))?;
    let rest = &s[s.rfind(')')? + 1..];
    Some(rest.split_whitespace().map(str::to_string).collect())
}

/// Process state letter (`R`, `S`, `D`, `Z`, ...).
pub fn state(m: &dyn Machine, pid: u32) -> Option<char> {
    stat_fields(m, pid)?.first()?.chars().next()
}

/// The process exists and is not a zombie.
pub fn is_alive(m: &dyn Machine, pid: u32) -> bool {
    match state(m, pid) {
        Some(s) => s != 'Z' && s != 'X',
        // no stat (some fakes): fall back to comm
        None => comm(m, pid).is_some(),
    }
}

/// Start time of the process in clock ticks since boot (stat field 22): with the pid it identifies one process even
/// when pids are reused.
pub fn start_ticks(m: &dyn Machine, pid: u32) -> Option<u64> {
    stat_fields(m, pid)?.get(19)?.parse().ok()
}

/// Seconds since the process started (stat field 22 against `/proc/uptime`).
pub fn age_secs(m: &dyn Machine, pid: u32) -> Option<f64> {
    let f = stat_fields(m, pid)?;
    let start: f64 = f.get(19)?.parse::<u64>().ok()? as f64 / m.clk_tck() as f64;
    let up: f64 = m
        .read_str(Path::new("/proc/uptime"))?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    Some((up - start).max(0.0))
}

/// Executable of the process (`/proc/PID/exe`), if readable.
pub fn exe(m: &dyn Machine, pid: u32) -> Option<PathBuf> {
    m.read_link(&proc_path(pid, "exe")).map(|p| {
        // a replaced binary shows as "/path (deleted)"
        let s = p.to_string_lossy();
        PathBuf::from(s.trim_end_matches(" (deleted)"))
    })
}

/// Living processes whose `comm` is exactly `name` (like `pgrep -x`).
pub fn by_comm(m: &dyn Machine, name: &str) -> Vec<u32> {
    pids(m)
        .into_iter()
        .filter(|p| comm(m, *p).as_deref() == Some(name) && is_alive(m, *p))
        .collect()
}

/// Running monado-service processes (LESSONS row 1).
pub fn monado_pids(m: &dyn Machine) -> Vec<u32> {
    by_comm(m, MONADO_COMM)
}

/// A process that has one of the watched paths open.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Holder {
    /// Pid.
    pub pid: u32,
    /// `comm`.
    pub comm: String,
    /// The path it holds.
    pub path: PathBuf,
    /// Command line, for the message.
    pub cmdline: String,
}

impl Holder {
    /// `pid 123 (pimax-cam broker)`.
    pub fn describe(&self) -> String {
        let cl = if self.cmdline.is_empty() {
            self.comm.clone()
        } else {
            self.cmdline.clone()
        };
        let short: String = cl.chars().take(80).collect();
        format!("pid {} ({short}) holds {}", self.pid, self.path.display())
    }
}

/// Processes with an open file descriptor on one of `targets` (`fuser` for our own processes; others' fd tables are
/// not readable without root, which is fine: every stack process runs as this user).
pub fn holders(m: &dyn Machine, targets: &[PathBuf]) -> Vec<Holder> {
    if targets.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for pid in pids(m) {
        let fd_dir = proc_path(pid, "fd");
        let mut seen: Vec<PathBuf> = Vec::new();
        for fd in m.list(&fd_dir) {
            let Some(t) = m.read_link(&fd_dir.join(&fd)) else {
                continue;
            };
            if targets.contains(&t) && !seen.contains(&t) {
                seen.push(t.clone());
                out.push(Holder {
                    pid,
                    comm: comm(m, pid).unwrap_or_default(),
                    path: t,
                    cmdline: cmdline(m, pid).join(" "),
                });
            }
        }
    }
    out
}

/// `1h 02m 03s` style age.
pub fn fmt_age(secs: f64) -> String {
    omarchyvr_tools::vr::session::fmt_age(secs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::machine::FakeMachine;

    fn machine() -> FakeMachine {
        let mut m = FakeMachine::new();
        m.file("/proc/uptime", "1000.50 9000.00\n");
        m.process(
            372460,
            "monado-service",
            "/home/u/src/monado-eye/build/src/xrt/targets/service/monado-service",
            &[("PIMAX_FPS", "72"), ("PIMAX_EYE", "1")],
            40_050, // started at 400.5 s of uptime -> 600 s old
            &["/dev/hidraw3"],
        );
        m.process(372448, "pimax-cam", "/home/u/omarchyvr/rust/target/release/pimax-cam broker", &[], 39_000, &["/dev/video0", "anon_inode:[eventfd]"]);
        m.process(5, "kworker/0:1", "", &[], 1, &[]);
        m
    }

    #[test]
    fn finds_processes_by_exact_name_and_age() {
        let m = machine();
        assert_eq!(monado_pids(&m), vec![372460]);
        assert_eq!(by_comm(&m, "pimax-ca"), Vec::<u32>::new(), "exact names only, like pgrep -x");
        let age = age_secs(&m, 372460).unwrap();
        assert!((age - 600.0).abs() < 0.01, "{age}");
        assert_eq!(fmt_age(age), "10m 00s");
        assert_eq!(
            environ(&m, 372460),
            vec![("PIMAX_FPS".to_string(), "72".to_string()), ("PIMAX_EYE".to_string(), "1".to_string())]
        );
        assert_eq!(exe(&m, 372448), Some(PathBuf::from("/home/u/omarchyvr/rust/target/release/pimax-cam")));
    }

    #[test]
    fn zombies_do_not_count() {
        let mut m = machine();
        m.file("/proc/372460/stat", "372460 (monado-service) Z 1 0 0 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 40050 0 0\n");
        assert!(monado_pids(&m).is_empty());
    }

    #[test]
    fn device_holders_like_fuser() {
        let m = machine();
        let h = holders(&m, &[PathBuf::from("/dev/video0"), PathBuf::from("/dev/video1")]);
        assert_eq!(h.len(), 1);
        assert_eq!(h[0].pid, 372448);
        assert_eq!(h[0].comm, "pimax-cam");
        assert!(h[0].describe().contains("pid 372448 (/home/u/omarchyvr/rust/target/release/pimax-cam broker) holds /dev/video0"));
        assert!(holders(&m, &[PathBuf::from("/dev/bus/usb/001/009")]).is_empty());
        assert!(holders(&m, &[]).is_empty());
    }
}
