//! Read-only process facts from `/proc` (real or a fake tree): pid files, liveness, comm, state, start time,
//! argv, environment and cwd (for adoption: a crashed leaf is respawned the way it was started).
use crate::spec::{PidRoot, PidSpec};
use crate::util::Roots;
use std::path::{Path, PathBuf};

/// One process as seen in /proc.
#[derive(Debug, Clone, PartialEq)]
pub struct ProcInfo {
    pub pid: u32,
    pub comm: String,
    /// `R`, `S`, `D`, `Z`, ...
    pub state: char,
    /// Start time in clock ticks since boot (`stat` field 22): tells a reused pid apart.
    pub start_ticks: u64,
}

impl ProcInfo {
    pub fn alive(&self) -> bool {
        self.state != 'Z' && self.state != 'X'
    }
}

/// Parse `/proc/<pid>/stat`.
pub fn parse_stat(pid: u32, text: &str) -> Option<ProcInfo> {
    let open = text.find('(')?;
    let close = text.rfind(')')?;
    let comm = text[open + 1..close].to_string();
    let rest: Vec<&str> = text[close + 1..].split_whitespace().collect();
    let state = rest.first()?.chars().next()?;
    // fields after the comm start at field 3 (state); starttime is field 22 -> index 19
    let start_ticks = rest.get(19).and_then(|v| v.parse().ok()).unwrap_or(0);
    Some(ProcInfo { pid, comm, state, start_ticks })
}

pub fn info(proc_: &Path, pid: u32) -> Option<ProcInfo> {
    let text = std::fs::read_to_string(proc_.join(pid.to_string()).join("stat")).ok()?;
    parse_stat(pid, &text)
}

/// The pid in a pid file (first token).
pub fn read_pid(p: &Path) -> Option<u32> {
    let t = std::fs::read_to_string(p).ok()?;
    let first = t.lines().next()?.trim();
    let first = first.strip_prefix("pid=").unwrap_or(first);
    first.parse().ok().filter(|&p| p > 1)
}

/// Where a pid file lives.
pub fn pid_path(r: &Roots, s: &PidSpec) -> PathBuf {
    match s.root {
        PidRoot::Session => r.session_dir.join(s.file),
        PidRoot::AiState => r.ai_state.join(s.file),
    }
}

/// The live process a pid file names, if its comm matches (a reused pid of another program does not count).
pub fn from_pid_file(r: &Roots, s: &PidSpec) -> Option<ProcInfo> {
    let pid = read_pid(&pid_path(r, s))?;
    let i = info(&r.proc_, pid)?;
    (i.alive() && i.comm == s.comm).then_some(i)
}

/// argv (NUL separated).
pub fn cmdline(proc_: &Path, pid: u32) -> Vec<String> {
    let b = std::fs::read(proc_.join(pid.to_string()).join("cmdline")).unwrap_or_default();
    b.split(|&c| c == 0).filter(|s| !s.is_empty()).map(|s| String::from_utf8_lossy(s).into_owned()).collect()
}

/// Environment (readable for our own processes).
pub fn environ(proc_: &Path, pid: u32) -> Vec<(String, String)> {
    let b = std::fs::read(proc_.join(pid.to_string()).join("environ")).unwrap_or_default();
    b.split(|&c| c == 0)
        .filter_map(|s| {
            let s = String::from_utf8_lossy(s);
            let (k, v) = s.split_once('=')?;
            Some((k.to_string(), v.to_string()))
        })
        .collect()
}

pub fn cwd(proc_: &Path, pid: u32) -> Option<PathBuf> {
    std::fs::read_link(proc_.join(pid.to_string()).join("cwd")).ok()
}

/// Every process with this comm.
pub fn by_comm(proc_: &Path, comm: &str) -> Vec<ProcInfo> {
    let Ok(rd) = std::fs::read_dir(proc_) else { return vec![] };
    rd.flatten()
        .filter_map(|e| e.file_name().to_str()?.parse::<u32>().ok())
        .filter_map(|pid| info(proc_, pid))
        .filter(|i| i.alive() && i.comm == comm)
        .collect()
}

/// Is `pid` alive (and the same process: `start_ticks` matches when given)?
pub fn same_alive(proc_: &Path, pid: u32, start_ticks: Option<u64>) -> bool {
    match info(proc_, pid) {
        Some(i) => i.alive() && start_ticks.is_none_or_eq(i.start_ticks),
        None => false,
    }
}

trait OptEq {
    fn is_none_or_eq(&self, v: u64) -> bool;
}
impl OptEq for Option<u64> {
    fn is_none_or_eq(&self, v: u64) -> bool {
        match self {
            None => true,
            Some(x) => *x == v,
        }
    }
}

/// A fake `/proc/<pid>` for tests and sims.
pub fn fake_proc(proc_: &Path, pid: u32, comm: &str, state: char, argv: &[&str]) {
    let d = proc_.join(pid.to_string());
    std::fs::create_dir_all(&d).unwrap();
    let mut stat = format!("{pid} ({comm}) {state} 1 1 1 0 -1 0 0 0 0 0 0 0 0 0 20 0 1 0 ");
    stat.push_str(&format!("{} 0 0", 1000 + pid as u64));
    std::fs::write(d.join("stat"), stat).unwrap();
    std::fs::write(d.join("comm"), format!("{comm}\n")).unwrap();
    let mut cl = Vec::new();
    for a in argv {
        cl.extend_from_slice(a.as_bytes());
        cl.push(0);
    }
    std::fs::write(d.join("cmdline"), cl).unwrap();
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::{put, TempDir};

    #[test]
    fn stat_parsing_handles_spaces_and_parens_in_comm() {
        let i = parse_stat(7, "7 (a (b) c) D 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 424242 0").unwrap();
        assert_eq!(i.comm, "a (b) c");
        assert_eq!(i.state, 'D');
        assert_eq!(i.start_ticks, 424242);
    }

    #[test]
    fn pid_files_need_the_right_comm() {
        let t = TempDir::new("procs");
        let r = Roots::under(t.path());
        fake_proc(&r.proc_, 4242, "monado-service", 'S', &["/x/monado-service"]);
        fake_proc(&r.proc_, 4343, "bash", 'S', &["bash"]);
        fake_proc(&r.proc_, 4444, "wayvr", 'Z', &[]);
        let spec = crate::spec::spec("monado").unwrap().pid.unwrap();
        assert!(from_pid_file(&r, &spec).is_none(), "no pid file");
        put(&r.session_dir, "monado.pid", "4242\n");
        assert_eq!(from_pid_file(&r, &spec).unwrap().pid, 4242);
        put(&r.session_dir, "monado.pid", "4343\n");
        assert!(from_pid_file(&r, &spec).is_none(), "reused pid");
        put(&r.session_dir, "wayvr.pid", "4444\n");
        assert!(from_pid_file(&r, &crate::spec::spec("wayvr").unwrap().pid.unwrap()).is_none(), "zombie");
        assert_eq!(cmdline(&r.proc_, 4242), ["/x/monado-service"]);
        assert_eq!(by_comm(&r.proc_, "monado-service").len(), 1);
        assert!(same_alive(&r.proc_, 4242, Some(1000 + 4242)));
        assert!(!same_alive(&r.proc_, 4242, Some(1)));
        // the ai pid file format: pid=N on the first line
        put(&r.ai_state, "llama-server.pid", "pid=777\nport=8080\n");
        assert_eq!(read_pid(&r.ai_state.join("llama-server.pid")), Some(777));
    }
}
