//! `~/.local/state/pimax/fixes.log`: one tab-separated line per catalogue fix the app ran or opened
//! (`pimax_device::autofix::log_line`: time, id, outcome, commands, undo). The Setup page lists them with Undo. Read
//! and appended in the background only.
use std::io::Write;
use std::path::{Path, PathBuf};

/// The file name in the state directory.
pub const FILE: &str = "fixes.log";

/// Entries listed (the newest).
pub const SHOWN: usize = 30;

/// One logged fix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// Unix seconds.
    pub time: u64,
    /// Catalogue id.
    pub id: String,
    /// What happened ("done", "failed: ...", "undo done", ...).
    pub outcome: String,
    /// The commands, joined with ` && `.
    pub commands: Vec<String>,
    /// The undo lines.
    pub undo: Vec<String>,
}

impl Entry {
    /// The fix was undone by this entry (it ran undo lines).
    pub fn is_undo(&self) -> bool {
        self.outcome.starts_with("undo")
    }

    /// The fix as it was rendered (an undo entry flips back to the original fix).
    pub fn planned(&self) -> Option<pimax_device::check::Planned> {
        let fix = pimax_device::check::fix(&self.id)?;
        let (commands, undo) = if self.is_undo() { (self.undo.clone(), self.commands.clone()) } else { (self.commands.clone(), self.undo.clone()) };
        Some(pimax_device::check::Planned { fix, commands, undo })
    }

    /// When, for people (UTC).
    pub fn when(&self) -> String {
        crate::applog::utc(self.time, 0).split('.').next().unwrap_or("").replace('T', " ") + " UTC"
    }
}

fn split(s: &str) -> Vec<String> {
    s.split(" && ").map(str::trim).filter(|x| !x.is_empty()).map(String::from).collect()
}

/// Parse the file (bad lines are skipped), oldest first.
pub fn parse(text: &str) -> Vec<Entry> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            if f.len() < 5 {
                return None;
            }
            Some(Entry { time: f[0].parse().ok()?, id: f[1].to_string(), outcome: f[2].to_string(), commands: split(f[3]), undo: split(f[4]) })
        })
        .collect()
}

/// The newest [`SHOWN`] entries, newest first.
pub fn read(path: &Path) -> Vec<Entry> {
    let mut v = parse(&crate::bundle::tail_text(path, 256 * 1024).unwrap_or_default());
    v.reverse();
    v.truncate(SHOWN);
    v
}

/// Append one line (the directory is created; the file is private to the user).
pub fn append(path: &Path, line: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    use std::os::unix::fs::OpenOptionsExt;
    let mut f = std::fs::OpenOptions::new().create(true).append(true).mode(0o600).open(path)?;
    f.write_all(line.as_bytes())
}

/// Two entries for demo mode (newest first): the keepalive service enabled, cap_sys_nice restored on a build.
pub fn demo_entries(now: u64, monado_build: &Path) -> Vec<Entry> {
    use pimax_device::check::FixRef;
    let svc = pimax_device::checks::cpu::monado_service(monado_build).display().to_string();
    let runs = [
        (FixRef::new("user.keepalive_enable", &[]), now.saturating_sub(1500), "done"),
        (FixRef::new("cpu.setcap_monado", &[("path", &svc)]), now.saturating_sub(5400), "done"),
    ];
    runs.into_iter()
        .filter_map(|(r, t, o)| r.plan().ok().map(|planned| crate::findings::CatalogueRun { planned, undo: false }.log_line(t, o)))
        .flat_map(|l| parse(&l))
        .collect()
}

/// The log of a state directory.
pub fn path(state_dir: &Path) -> PathBuf {
    state_dir.join(FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::findings::CatalogueRun;
    use pimax_device::check::FixRef;

    #[test]
    fn append_read_and_undo_round_trip() {
        let dir = std::env::temp_dir().join(format!("pimax-fixlog-{}", std::process::id()));
        let p = path(&dir);
        let planned = FixRef::new("cpu.setcap_monado", &[("path", "/home/u/m/monado-service")]).plan().unwrap();
        let run = CatalogueRun { planned: planned.clone(), undo: false };
        append(&p, &run.log_line(100, "done")).unwrap();
        append(&p, "garbage line\n").unwrap();
        append(&p, &CatalogueRun { planned: planned.clone(), undo: true }.log_line(200, "done")).unwrap();
        let v = read(&p);
        assert_eq!(v.len(), 2);
        assert_eq!((v[0].time, v[0].outcome.as_str(), v[0].is_undo()), (200, "undo done", true));
        assert_eq!(v[1].commands, vec!["sudo setcap cap_sys_nice+ep /home/u/m/monado-service".to_string()]);
        // both entries give back the original fix
        assert_eq!(v[0].planned().unwrap(), planned);
        assert_eq!(v[1].planned().unwrap(), planned);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(dir);
    }
}
